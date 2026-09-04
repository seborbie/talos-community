import { describe, expect, test } from 'bun:test';
import express from 'express';
import { resolve } from 'node:path';
import {
  API_REQUIRED_ENVIRONMENT_VARIABLES,
  assertSecureEnvironment,
  findUnsafeCredentialVariables,
  isKnownPublicExampleCredential,
  parseApiTrustedProxies,
} from './environmentPolicy';

const safeApiEnvironment = {
  JWT_SECRET: 'generated-jwt-secret-for-this-installation',
  APP_ENCRYPTION_KEY: 'generated-encryption-key-for-this-installation',
  TOKEN_TTL: '1h',
  MACHINE_TOKEN_TTL: '30d',
};

describe('startup credential policy', () => {
  test('recognizes every published marker and historical fixed development credential', () => {
    for (const value of [
      'replace_with_long_random_string',
      'replace_with_shared_service_key',
      'replace-with-enrollment-token',
      'your-secret-key',
      'talos_dev_local_service_key_01',
      'talos_dev_jwt_secret_minimum_32_chars_long_',
      'talos_dev_agent_token_01',
    ]) {
      expect(isKnownPublicExampleCredential(value)).toBe(true);
    }
    expect(isKnownPublicExampleCredential('generated-service-key-for-one-install')).toBe(false);
  });

  test('reports variable names without including credential values', () => {
    const source = {
      ...safeApiEnvironment,
      SERVICE_KEY: 'replace_with_shared_service_key',
      RMM_SERVER_API_KEY: 'replace_with_shared_rmm_server_key',
    };

    expect(findUnsafeCredentialVariables(source)).toEqual(['RMM_SERVER_API_KEY', 'SERVICE_KEY']);
    try {
      assertSecureEnvironment(source, API_REQUIRED_ENVIRONMENT_VARIABLES, 'API');
      throw new Error('expected unsafe credentials to be rejected');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      expect(message).toContain('RMM_SERVER_API_KEY, SERVICE_KEY');
      expect(message).not.toContain('replace_with_shared_service_key');
      expect(message).not.toContain('replace_with_shared_rmm_server_key');
    }
  });

  test('the actual API environment module exits non-zero for an example service key', () => {
    const result = Bun.spawnSync([process.execPath, '-e', "import './lib/env.ts'"], {
      cwd: resolve(import.meta.dir, '..'),
      env: {
        ...process.env,
        ...safeApiEnvironment,
        SERVICE_KEY: 'replace_with_shared_service_key',
      },
      stdout: 'pipe',
      stderr: 'pipe',
    });
    const output = `${result.stdout.toString()}${result.stderr.toString()}`;

    expect(result.exitCode).not.toBe(0);
    expect(output).toContain('SERVICE_KEY');
    expect(output).not.toContain('replace_with_shared_service_key');
  });

  test('requires an independent application encryption key', () => {
    expect(() =>
      assertSecureEnvironment(
        { ...safeApiEnvironment, APP_ENCRYPTION_KEY: '' },
        API_REQUIRED_ENVIRONMENT_VARIABLES,
        'API',
      ),
    ).toThrow('missing required variables: APP_ENCRYPTION_KEY');

    expect(() =>
      assertSecureEnvironment(
        {
          ...safeApiEnvironment,
          APP_ENCRYPTION_KEY: safeApiEnvironment.JWT_SECRET,
        },
        API_REQUIRED_ENVIRONMENT_VARIABLES,
        'API',
      ),
    ).toThrow('APP_ENCRYPTION_KEY must be independent from JWT_SECRET');
  });

  test('the actual API environment module rejects a missing encryption key', () => {
    const result = Bun.spawnSync([process.execPath, '-e', "import './lib/env.ts'"], {
      cwd: resolve(import.meta.dir, '..'),
      env: {
        ...process.env,
        ...safeApiEnvironment,
        APP_ENCRYPTION_KEY: '',
      },
      stdout: 'pipe',
      stderr: 'pipe',
    });
    const output = `${result.stdout.toString()}${result.stderr.toString()}`;

    expect(result.exitCode).not.toBe(0);
    expect(output).toContain('APP_ENCRYPTION_KEY');
    expect(output).not.toContain(safeApiEnvironment.JWT_SECRET);
  });
});

describe('trusted proxy policy', () => {
  test('trusts no proxy by default and accepts only explicit address allowlists', () => {
    expect(parseApiTrustedProxies(undefined)).toBe(false);
    expect(parseApiTrustedProxies('false')).toBe(false);
    expect(parseApiTrustedProxies('127.0.0.1, 10.42.0.0/16, ::1/128')).toEqual([
      '127.0.0.1',
      '10.42.0.0/16',
      '::1/128',
    ]);
    expect(parseApiTrustedProxies('Loopback')).toEqual(['loopback']);
    expect(() => parseApiTrustedProxies('true')).toThrow('allowlist');
    expect(() => parseApiTrustedProxies('1')).toThrow('allowlist');
    expect(() => parseApiTrustedProxies('proxy.example.test')).toThrow('allowlist');
    expect(() => parseApiTrustedProxies('10.0.0.0/99')).toThrow('allowlist');
  });

  test('ignores a spoofed forwarded address when no proxy is trusted', async () => {
    const app = express();
    app.set('trust proxy', parseApiTrustedProxies(undefined));
    app.get('/ip', (req, res) => res.json({ ip: req.ip }));
    const server = app.listen(0, '127.0.0.1');
    const address = server.address();
    if (!address || typeof address === 'string') {
      throw new Error('test server did not bind');
    }

    try {
      const response = await fetch(`http://127.0.0.1:${address.port}/ip`, {
        headers: { 'x-forwarded-for': '198.51.100.42' },
      });
      const body = (await response.json()) as { ip: string };
      expect(body.ip).not.toBe('198.51.100.42');
      expect(body.ip).toContain('127.0.0.1');
    } finally {
      await new Promise<void>((resolveClose, reject) => {
        server.close((error) => (error ? reject(error) : resolveClose()));
      });
    }
  });
});
