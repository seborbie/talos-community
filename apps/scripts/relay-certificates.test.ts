import { describe, expect, test } from 'bun:test';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import {
  DEFAULT_RELAY_CERTIFICATE_HOST_SOURCE,
  DEFAULT_RELAY_CERTIFICATE_PATH,
  DEFAULT_RELAY_KEY_HOST_SOURCE,
  DEFAULT_RELAY_KEY_PATH,
  requireRelayCertificates,
  resolveRelayCertificateFiles,
} from './relay-certificates';

describe('relay certificate preflight', () => {
  test('uses the certificate filenames mounted by the shared Compose model', () => {
    const repoRoot = resolve('test-repository');

    expect(resolveRelayCertificateFiles({}, repoRoot)).toEqual({
      certificateFile: resolve(repoRoot, 'apps', 'certs', 'local-dev-relay-fullchain.pem'),
      keyFile: resolve(repoRoot, 'apps', 'certs', 'local-dev-relay-key.pem'),
    });
    expect(DEFAULT_RELAY_CERTIFICATE_PATH).toBe('/.certs/local-dev-relay-fullchain.pem');
    expect(DEFAULT_RELAY_KEY_PATH).toBe('/.certs/local-dev-relay-key.pem');
    expect(DEFAULT_RELAY_CERTIFICATE_HOST_SOURCE).toBe(
      '../apps/certs/local-dev-relay-fullchain.pem',
    );
    expect(DEFAULT_RELAY_KEY_HOST_SOURCE).toBe('../apps/certs/local-dev-relay-key.pem');
  });

  test('maps exact relative host files from the Compose file directory', () => {
    const repoRoot = resolve('test-repository');

    expect(
      resolveRelayCertificateFiles(
        {
          RMM_RELAY_TLS_CERT_HOST_PATH: '../private/relay/chain.pem',
          RMM_RELAY_TLS_KEY_HOST_PATH: '../private/relay/key.pem',
          RMM_RELAY_TLS_CERT_PATH: '/.certs/nested/chain.pem',
          RMM_RELAY_TLS_KEY_PATH: '/.certs/nested/key.pem',
        },
        repoRoot,
      ),
    ).toEqual({
      certificateFile: resolve(repoRoot, 'private', 'relay', 'chain.pem'),
      keyFile: resolve(repoRoot, 'private', 'relay', 'key.pem'),
    });
  });

  test('rejects container paths outside the mounted certificate directory', () => {
    expect(() =>
      resolveRelayCertificateFiles(
        { RMM_RELAY_TLS_CERT_PATH: '/tmp/chain.pem' },
        resolve('test-repository'),
      ),
    ).toThrow('below /.certs/');
  });

  test('fails before startup and names every missing host file', async () => {
    const repoRoot = resolve('test-repository');

    await expect(requireRelayCertificates({}, repoRoot, () => false)).rejects.toThrow(
      'local-dev-relay-fullchain.pem',
    );
    await expect(requireRelayCertificates({}, repoRoot, () => false)).rejects.toThrow(
      'local-dev-relay-key.pem',
    );
  });

  test('returns the resolved files only when both are readable', async () => {
    const repoRoot = resolve('test-repository');
    const expected = resolveRelayCertificateFiles({}, repoRoot);
    const present = new Set([expected.certificateFile, expected.keyFile]);

    await expect(
      requireRelayCertificates({}, repoRoot, (path) => present.has(path)),
    ).resolves.toEqual(expected);
  });

  test('rejects a directory where an exact regular-file mount is required', async () => {
    const fixture = mkdtempSync(join(tmpdir(), 'talos-relay-files-'));
    const certificateFile = join(fixture, 'chain.pem');
    const keyDirectory = join(fixture, 'key.pem');
    writeFileSync(certificateFile, 'fake certificate fixture');
    mkdirSync(keyDirectory);

    try {
      await expect(
        requireRelayCertificates(
          {
            RMM_RELAY_TLS_CERT_HOST_PATH: certificateFile,
            RMM_RELAY_TLS_KEY_HOST_PATH: keyDirectory,
          },
          resolve('test-repository'),
        ),
      ).rejects.toThrow(keyDirectory);
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  });
});
