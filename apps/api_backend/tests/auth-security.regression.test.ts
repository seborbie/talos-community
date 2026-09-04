import { beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import bcrypt from 'bcrypt';
import express from 'express';
import jwt from 'jsonwebtoken';

process.env.JWT_SECRET = 'auth-security-regression-generated-jwt-secret';
process.env.TOKEN_TTL = '2h';
process.env.MACHINE_TOKEN_TTL = '3h';
process.env.SERVICE_KEY = 'auth-security-regression-generated-service-key';

const existingPasswordHash = await bcrypt.hash('existing-password', 4);
let registeredUserCount = 0;
const prisma = {
  user: {
    count: async () => registeredUserCount,
    findUnique: async ({ where }: { where: { email: string } }) =>
      where.email === 'existing@example.test'
        ? {
            id: 'existing-user',
            email: where.email,
            password: existingPasswordHash,
            createdAt: new Date('2026-08-19T00:00:00Z'),
          }
        : null,
    create: async ({ data }: { data: { email: string; password: string } }) => {
      registeredUserCount += 1;
      return {
        id: 'registered-user',
        ...data,
        createdAt: new Date('2026-08-19T00:00:00Z'),
      };
    },
  },
  organizationMember: {
    findFirst: async ({ where }: { where: { userId: string } }) => ({
      organizationId: 'org-auth-test',
      user: { id: where.userId, email: 'existing@example.test' },
    }),
  },
  auditEvent: {
    create: async ({ data }: { data: unknown }) => ({ id: 'audit-test', data }),
  },
  $queryRaw: async () => [{ pg_advisory_xact_lock: '' }],
  $transaction: async <T>(operation: (tx: typeof prisma) => Promise<T>) => operation(prisma),
};

mock.module('../lib/prisma', () => ({ prisma }));

let makeApp: () => express.Express;

beforeAll(async () => {
  const { authRouter } = await import('../routes/auth.routes');
  makeApp = () => {
    const app = express();
    app.use(express.json());
    app.use('/auth', authRouter);
    return app;
  };
});

async function request(
  path: string,
  body: unknown,
  headers: Record<string, string> = {},
): Promise<{ status: number; body: { token?: string; error?: string } }> {
  const server = makeApp().listen(0, '127.0.0.1');
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('test server did not bind');
  }
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...headers },
      body: JSON.stringify(body),
    });
    return {
      status: response.status,
      body: (await response.json()) as { token?: string; error?: string },
    };
  } finally {
    await new Promise<void>((resolveClose, reject) => {
      server.close((error) => (error ? reject(error) : resolveClose()));
    });
  }
}

async function get(path: string): Promise<{
  status: number;
  body: { registrationOpen?: boolean; mode?: string; error?: string };
}> {
  const server = makeApp().listen(0, '127.0.0.1');
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('test server did not bind');
  }
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`);
    return {
      status: response.status,
      body: (await response.json()) as {
        registrationOpen?: boolean;
        mode?: string;
        error?: string;
      },
    };
  } finally {
    await new Promise<void>((resolveClose, reject) => {
      server.close((error) => (error ? reject(error) : resolveClose()));
    });
  }
}

function tokenLifetimeSeconds(token: string): number {
  const payload = jwt.decode(token) as { iat?: number; exp?: number } | null;
  if (!payload?.iat || !payload.exp) throw new Error('token is missing iat/exp');
  return payload.exp - payload.iat;
}

describe('authentication security configuration', () => {
  beforeEach(() => {
    registeredUserCount = 0;
  });

  test('registration and login use TOKEN_TTL instead of a route-local duration', async () => {
    const registration = await request('/auth/register', {
      email: 'new@example.test',
      password: 'registration-password',
    });
    const login = await request('/auth/login', {
      email: 'existing@example.test',
      password: 'existing-password',
    });

    expect(registration.status).toBe(201);
    expect(login.status).toBe(200);
    expect(tokenLifetimeSeconds(registration.body.token!)).toBe(2 * 60 * 60);
    expect(tokenLifetimeSeconds(login.body.token!)).toBe(2 * 60 * 60);
  });

  test('only the first user can self-register', async () => {
    const statusBefore = await get('/auth/registration-status');
    const first = await request('/auth/register', {
      email: ' First.User@Example.Test ',
      password: 'first-user-password',
    });
    const second = await request('/auth/register', {
      email: 'second@example.test',
      password: 'second-user-password',
    });
    const statusAfter = await get('/auth/registration-status');

    expect(statusBefore).toEqual({
      status: 200,
      body: { registrationOpen: true, mode: 'first_user' },
    });
    expect(first.status).toBe(201);
    expect(second).toEqual({
      status: 403,
      body: {
        error: 'Registration is closed. Ask a Talos administrator to provision your account.',
      },
    });
    expect(statusAfter).toEqual({
      status: 200,
      body: { registrationOpen: false, mode: 'closed' },
    });
  });

  test('registration validates credentials at the API boundary', async () => {
    const response = await request('/auth/register', {
      email: 'not-an-email',
      password: 'short',
    });

    expect(response).toEqual({
      status: 400,
      body: { error: 'A valid email address is required' },
    });
    expect(registeredUserCount).toBe(0);
  });

  test('user and service machine-token routes both use MACHINE_TOKEN_TTL', async () => {
    const login = await request('/auth/login', {
      email: 'existing@example.test',
      password: 'existing-password',
    });
    const userMint = await request(
      '/auth/machine-token',
      {},
      { authorization: `Bearer ${login.body.token}` },
    );
    const serviceMint = await request(
      '/auth/service/machine-token',
      { agentId: 'agent-auth-test' },
      { 'x-service-key': process.env.SERVICE_KEY! },
    );

    expect(userMint.status).toBe(200);
    expect(serviceMint.status).toBe(200);
    expect(tokenLifetimeSeconds(userMint.body.token!)).toBe(3 * 60 * 60);
    expect(tokenLifetimeSeconds(serviceMint.body.token!)).toBe(3 * 60 * 60);
  });

  test('service minting rejects any value other than the configured generated key', async () => {
    const response = await request(
      '/auth/service/machine-token',
      { agentId: 'agent-auth-test' },
      { 'x-service-key': 'replace_with_shared_service_key' },
    );

    expect(response.status).toBe(401);
    expect(response.body).toEqual({ error: 'Unauthorized' });
  });
});
