import { afterEach, describe, expect, test } from 'bun:test';
import express, { type Express } from 'express';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Server } from 'node:http';
import { getAuditRequestMetadata } from './audit';
import { getTrustedClientIp, getTrustedRequestOrigin } from './requestTrust';

const openServers: Server[] = [];

afterEach(async () => {
  await Promise.all(
    openServers.splice(0).map(
      (server) =>
        new Promise<void>((resolveClose, reject) => {
          server.close((error) => (error ? reject(error) : resolveClose()));
        }),
    ),
  );
});

async function listen(app: Express): Promise<string> {
  const server = app.listen(0, '127.0.0.1');
  openServers.push(server);
  await new Promise<void>((resolveListening) => server.once('listening', resolveListening));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('test server did not bind');
  return `http://127.0.0.1:${address.port}`;
}

describe('trusted request metadata', () => {
  test('ignores spoofed forwarding headers when the peer is not trusted', async () => {
    const app = express();
    app.set('trust proxy', false);
    app.get('/metadata', (req, res) => {
      res.json({
        ip: getTrustedClientIp(req),
        auditIp: getAuditRequestMetadata(req).clientIp,
        origin: getTrustedRequestOrigin(req),
      });
    });
    const baseUrl = await listen(app);

    const response = await fetch(`${baseUrl}/metadata`, {
      headers: {
        'x-forwarded-for': '198.51.100.42',
        'x-forwarded-proto': 'https',
        'x-forwarded-host': 'attacker.example.test',
      },
    });
    const body = (await response.json()) as {
      ip: string;
      auditIp: string;
      origin: string;
    };

    expect(body.ip).not.toBe('198.51.100.42');
    expect(body.auditIp).toBe(body.ip);
    expect(body.origin).toBe(baseUrl);
  });

  test('uses forwarding metadata only through Express trust-proxy policy', async () => {
    const app = express();
    app.set('trust proxy', ['loopback']);
    app.get('/metadata', (req, res) => {
      res.json({
        ip: getTrustedClientIp(req),
        origin: getTrustedRequestOrigin(req),
      });
    });
    const baseUrl = await listen(app);

    const response = await fetch(`${baseUrl}/metadata`, {
      headers: {
        'x-forwarded-for': '198.51.100.42',
        'x-forwarded-proto': 'https',
        'x-forwarded-host': 'api.example.test',
      },
    });
    const body = (await response.json()) as { ip: string; origin: string };

    expect(body.ip).toBe('198.51.100.42');
    expect(body.origin).toBe('https://api.example.test');
  });

  test('prefers and validates explicit deployment public URLs', () => {
    const request = {
      protocol: 'http',
      hostname: '127.0.0.1',
      socket: { localPort: 3001 },
    } as never;

    expect(
      getTrustedRequestOrigin(request, 'https://api.example.test/control/', 'PUBLIC_API_URL'),
    ).toBe('https://api.example.test/control');
    expect(() => getTrustedRequestOrigin(request, 'javascript:alert(1)', 'PUBLIC_API_URL')).toThrow(
      'absolute HTTP(S) URL',
    );
    expect(() =>
      getTrustedRequestOrigin(request, 'https://user:secret@example.test', 'PUBLIC_API_URL'),
    ).toThrow('without credentials');
  });

  test('audit, installer, and update code never reads forwarding headers directly', () => {
    const apiRoot = resolve(import.meta.dir, '..');
    for (const relativePath of [
      'lib/audit.ts',
      'routes/installers.routes.ts',
      'routes/updates.routes.ts',
    ]) {
      const source = readFileSync(resolve(apiRoot, relativePath), 'utf8').toLowerCase();
      expect(source).not.toContain('x-forwarded-for');
      expect(source).not.toContain('x-forwarded-proto');
      expect(source).not.toContain('x-forwarded-host');
    }
  });
});
