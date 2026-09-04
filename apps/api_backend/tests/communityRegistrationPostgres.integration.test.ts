import { expect, test } from 'bun:test';
import { randomUUID } from 'node:crypto';
import type { Server } from 'node:http';
import express from 'express';
import { prisma } from '../lib/prisma';
import { authRouter } from '../routes/auth.routes';

// Medium test: requires a migrated, disposable PostgreSQL database with no users.
const databaseUrl = process.env.COMMUNITY_REGISTRATION_TEST_DATABASE_URL?.trim();

if (!databaseUrl) {
  test.skip('Community registration PostgreSQL integration (set COMMUNITY_REGISTRATION_TEST_DATABASE_URL)', () => {});
} else {
  test('a fresh deployment registers one user, closes registration, and serializes concurrent attempts', async () => {
    if (
      process.env.DATABASE_URL !== databaseUrl ||
      !new URL(databaseUrl).pathname.endsWith('_test')
    ) {
      throw new Error(
        'Use the same disposable *_test database for DATABASE_URL and COMMUNITY_REGISTRATION_TEST_DATABASE_URL',
      );
    }
    expect(await prisma.user.count()).toBe(0);
    const suffix = randomUUID();
    const emails = ['first', 'second', 'concurrent-a', 'concurrent-b'].map(
      (name) => `${name}-${suffix}@example.test`,
    );
    const password = 'registration-integration-fixture-password';
    const app = express();
    app.use(express.json());
    app.use('/auth', authRouter);
    const server = await new Promise<Server>((resolve) => {
      const listener = app.listen(0, '127.0.0.1', () => resolve(listener));
    });

    async function removeFixtureUsers() {
      await prisma.auditEvent.deleteMany({ where: { userEmail: { in: emails } } });
      await prisma.user.deleteMany({ where: { email: { in: emails } } });
    }

    try {
      const address = server.address();
      if (!address || typeof address === 'string') throw new Error('test server did not bind');
      const base = `http://127.0.0.1:${address.port}/auth`;
      const registrationStatus = async () => {
        const response = await fetch(`${base}/registration-status`);
        expect(response.status).toBe(200);
        return response.json();
      };
      const register = (email: string) =>
        fetch(`${base}/register`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ email, password }),
        });

      expect(await registrationStatus()).toEqual({ registrationOpen: true, mode: 'first_user' });
      const first = await register(emails[0]!);
      expect(first.status).toBe(201);
      const created = (await first.json()) as {
        user: { id: string; email: string };
        token: string;
      };
      expect(created.user.email).toBe(emails[0]!);
      expect(typeof created.token).toBe('string');
      const login = await fetch(`${base}/login`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ email: emails[0], password }),
      });
      expect(login.status).toBe(200);
      expect(((await login.json()) as { user: { id: string } }).user.id).toBe(created.user.id);
      expect((await register(emails[1]!)).status).toBe(403);
      expect(await prisma.user.count()).toBe(1);
      expect(await registrationStatus()).toEqual({ registrationOpen: false, mode: 'closed' });

      // Return only this test's fixtures to the fresh-install state, then race two HTTP requests.
      await removeFixtureUsers();
      const attempts = await Promise.all([register(emails[2]!), register(emails[3]!)]);
      expect(attempts.map((response) => response.status).sort()).toEqual([201, 403]);
      expect(await prisma.user.count()).toBe(1);
      expect(await registrationStatus()).toEqual({ registrationOpen: false, mode: 'closed' });
    } finally {
      await removeFixtureUsers();
      await new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
      await prisma.$disconnect();
    }
  }, 30_000);
}
