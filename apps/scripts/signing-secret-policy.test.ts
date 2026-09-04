import { describe, expect, test } from 'bun:test';
import { checkTrackedSigningSecrets, trackedSigningSecretFailures } from './signing-secret-policy';

const bytes = (contents: string) => new TextEncoder().encode(contents);
const syntheticPrivateKey = [
  '-----BEGIN PRIVATE ',
  'KEY-----\nsecret\n-----END PRIVATE ',
  'KEY-----\n',
].join('');

describe('tracked signing-secret policy', () => {
  test('rejects key containers and PEM private keys', () => {
    const failures = trackedSigningSecretFailures([
      { path: 'release/publisher.pfx', bytes: bytes('binary') },
      {
        path: 'release/publisher.pem',
        bytes: bytes(syntheticPrivateKey),
      },
    ]);

    expect(failures).toEqual([
      'tracked signing-key container is forbidden: release/publisher.pfx',
      'tracked PEM private key is forbidden: release/publisher.pem',
    ]);
  });

  test('allows public certificates and synthetic inline protocol examples', () => {
    expect(
      trackedSigningSecretFailures([
        {
          path: 'fixtures/public.pem',
          bytes: bytes('-----BEGIN PUBLIC KEY-----\npublic\n-----END PUBLIC KEY-----\n'),
        },
        {
          path: 'fixtures/protocol.json',
          bytes: bytes('{"key":"-----BEGIN PRIVATE KEY-----\\nMIIB\\n"}'),
        },
      ]),
    ).toEqual([]);
  });

  test('the tracked repository contains no signing private key', async () => {
    expect((await checkTrackedSigningSecrets()).failures).toEqual([]);
  });
});
