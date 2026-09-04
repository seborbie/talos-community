import { describe, expect, test } from 'bun:test';
import { resolve } from 'node:path';
import { decryptSecret, encryptSecret } from './crypto';

describe('application encryption key boundary', () => {
  test('uses the persistent application key independently of JWT rotation', () => {
    const previousEncryptionKey = process.env.APP_ENCRYPTION_KEY;
    const previousJwtSecret = process.env.JWT_SECRET;
    try {
      process.env.APP_ENCRYPTION_KEY = 'persistent-encryption-key';
      process.env.JWT_SECRET = 'jwt-signing-key-before-rotation';
      const encrypted = encryptSecret('sensitive value');
      expect(encrypted).not.toBeNull();

      process.env.JWT_SECRET = 'jwt-signing-key-after-rotation';
      expect(decryptSecret(encrypted)).toBe('sensitive value');
    } finally {
      if (previousEncryptionKey === undefined) delete process.env.APP_ENCRYPTION_KEY;
      else process.env.APP_ENCRYPTION_KEY = previousEncryptionKey;
      if (previousJwtSecret === undefined) delete process.env.JWT_SECRET;
      else process.env.JWT_SECRET = previousJwtSecret;
    }
  });

  test('does not fall back to JWT_SECRET when APP_ENCRYPTION_KEY is absent', () => {
    const result = Bun.spawnSync(
      [
        process.execPath,
        '-e',
        [
          "process.env.JWT_SECRET = 'jwt-only-must-not-encrypt';",
          "delete process.env.APP_ENCRYPTION_KEY;",
          "const { encryptSecret } = await import('./lib/crypto.ts');",
          "encryptSecret('sensitive value');",
        ].join(' '),
      ],
      {
        cwd: resolve(import.meta.dir, '..'),
        env: { ...process.env, APP_ENCRYPTION_KEY: '' },
        stdout: 'pipe',
        stderr: 'pipe',
      },
    );
    const output = `${result.stdout.toString()}${result.stderr.toString()}`;

    expect(result.exitCode).not.toBe(0);
    expect(output).toContain('Missing APP_ENCRYPTION_KEY for encryption');
    expect(output).not.toContain('jwt-only-must-not-encrypt');
  });
});
