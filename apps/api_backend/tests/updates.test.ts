import { describe, expect, test } from 'bun:test';
import {
  manifestMatchesAudience,
  validateUpdateManifestArtifact,
  type UpdateManifest,
} from '../routes/updates.routes';

function workerManifest(): UpdateManifest {
  return {
    product: 'worker',
    platform: 'linux',
    arch: 'linux-x64',
    channel: 'stable',
    ring: 'pilot',
    version: '1.2.3',
    minimumSupportedVersion: '1.0.0',
    severity: 'normal',
    publishedAtUtc: '2026-08-28T00:00:00Z',
    rolloutPercentage: 100,
    package: {
      fileName: 'Talos.Worker.linux-x64.Update.zip',
      sizeBytes: 123,
      sha256: 'a'.repeat(64),
    },
    contents: ['talos_worker'],
    requiresRestart: true,
    installMode: 'silent',
  };
}

describe('signed update artifact binding', () => {
  test('accepts an exact product, platform, architecture, mode, name, and size match', () => {
    expect(validateUpdateManifestArtifact(workerManifest(), 'worker', 'linux-x64', 123)).toEqual(
      workerManifest(),
    );
  });

  test('rejects every cross-slot field and package size drift', () => {
    const mutations: Array<[string, (manifest: UpdateManifest) => void]> = [
      ['product', (manifest) => (manifest.product = 'supervisor')],
      ['platform', (manifest) => (manifest.platform = 'windows')],
      ['arch', (manifest) => (manifest.arch = 'linux-arm64')],
      ['installMode', (manifest) => (manifest.installMode = 'zip')],
      [
        'fileName',
        (manifest) => (manifest.package.fileName = 'Talos.Supervisor.linux-x64.Update.zip'),
      ],
      ['sizeBytes', (manifest) => (manifest.package.sizeBytes = 0)],
    ];

    for (const [field, mutate] of mutations) {
      const manifest = workerManifest();
      mutate(manifest);
      expect(
        () => validateUpdateManifestArtifact(manifest, 'worker', 'linux-x64', 123),
        field,
      ).toThrow();
    }
    expect(() =>
      validateUpdateManifestArtifact(workerManifest(), 'worker', 'linux-x64', 124),
    ).toThrow('does not match the selected package');
  });

  test('requires exact channel and ring audience matching', () => {
    const manifest = workerManifest();
    expect(manifestMatchesAudience(manifest, 'stable', 'pilot')).toBe(true);
    expect(manifestMatchesAudience(manifest, 'preview', 'pilot')).toBe(false);
    expect(manifestMatchesAudience(manifest, 'stable', null)).toBe(false);

    manifest.ring = null;
    expect(manifestMatchesAudience(manifest, 'stable', null)).toBe(true);
    expect(manifestMatchesAudience(manifest, 'stable', 'pilot')).toBe(false);
  });

  test('derives macOS viewer package identity and install mode exactly', () => {
    const manifest: UpdateManifest = {
      ...workerManifest(),
      product: 'viewer',
      platform: 'macos',
      arch: 'macos-arm64',
      ring: null,
      package: {
        fileName: 'Talos.Viewer.macos.pkg',
        sizeBytes: 456,
        sha256: 'b'.repeat(64),
      },
      installMode: 'pkg',
    };
    expect(validateUpdateManifestArtifact(manifest, 'viewer', 'macos-arm64', 456)).toEqual(
      manifest,
    );
  });
});
