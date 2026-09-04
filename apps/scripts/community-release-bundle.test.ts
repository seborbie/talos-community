import { afterEach, describe, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { lstat, mkdtemp, mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import {
  assembleCommunityReleaseBundle,
  validatePublishedImageRecords,
  type CommunityBundleInputs,
  type PublishedImageRecord,
} from './community-release-bundle';

const sourceSha = 'a'.repeat(40);
const digest = (character: string) => `sha256:${character.repeat(64)}`;
const archiveSha = (character: string) => character.repeat(64);
const roots: string[] = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

function record(key: PublishedImageRecord['key'], character: string): PublishedImageRecord {
  const imageDigest = digest(character);
  return {
    schemaVersion: 1,
    key,
    releaseVersion: '1.2.3-rc.1',
    sourceSha,
    reference: `ghcr.io/talos-community/talos-${key.replaceAll('_', '-')}@${imageDigest}`,
    digest: imageDigest,
    platforms: ['linux/amd64', 'linux/arm64'],
    ociArchiveSha256: archiveSha(character),
    sbomFile: `${key}.spdx.json`,
  };
}

const imageRecords = [
  record('api_backend', '1'),
  record('frontend', '2'),
  record('relay', '3'),
  record('control_server', '4'),
];

async function hash(contents: string): Promise<string> {
  return createHash('sha256').update(contents).digest('hex');
}

async function fixtureInputs(): Promise<CommunityBundleInputs> {
  const root = await mkdtemp(resolve(tmpdir(), 'talos-release-bundle-'));
  roots.push(root);
  const native = resolve(root, 'native');
  const sbom = resolve(root, 'sbom');
  await mkdir(native);
  await mkdir(sbom);

  const nativeFiles: Record<string, string> = {
    'UNSIGNED-BINARIES.txt': 'IMPORTANT: UNSIGNED COMMUNITY BINARIES\n',
    'build-provenance.json': `${JSON.stringify({
      source: { revision: sourceSha, trackedSourceDirty: false },
      builder: { profile: 'release' },
      trust: {
        updaterManifestPublicKeySha256: 'b'.repeat(64),
        windowsAuthenticodeStatus: 'unsigned',
      },
    })}\n`,
    'manifest.json': `${JSON.stringify({
      profile: 'release',
      signing: {
        updaterManifests: { publicKeySha256: 'b'.repeat(64) },
        windowsAuthenticode: { status: 'unsigned' },
      },
      integrity: { checksumAlgorithm: 'SHA-256', checksumFile: 'SHA256SUMS' },
    })}\n`,
    'Talos.Worker.x64.Update.manifest.json': '{}\n',
    'Talos.Worker.x64.Update.manifest.sig': 'signature\n',
    'Talos.Worker.x64.Update.zip': 'package\n',
  };
  for (const [name, contents] of Object.entries(nativeFiles)) {
    await writeFile(resolve(native, name), contents);
  }
  const checksumLines = await Promise.all(
    Object.entries(nativeFiles)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(async ([name, contents]) => `${await hash(contents)}  ${name}`),
  );
  await writeFile(resolve(native, 'SHA256SUMS'), `${checksumLines.join('\n')}\n`);

  for (const image of imageRecords) {
    await writeFile(resolve(sbom, image.sbomFile), '{"spdxVersion":"SPDX-2.3"}\n');
  }
  for (const name of [
    'source.spdx.json',
    'launcher-linux.spdx.json',
    'launcher-windows.spdx.json',
    'native-clients.spdx.json',
  ]) {
    await writeFile(resolve(sbom, name), '{"spdxVersion":"SPDX-2.3"}\n');
  }
  await writeFile(resolve(root, 'talos-server'), 'linux launcher');
  await writeFile(resolve(root, 'talos-server.exe'), 'windows launcher');
  await writeFile(resolve(root, 'images.json'), `${JSON.stringify(imageRecords)}\n`);

  return {
    repoRoot: resolve(import.meta.dir, '../..'),
    outputDirectory: resolve(root, 'bundle'),
    releaseTag: 'community-v1.2.3-rc.1',
    releaseVersion: '1.2.3-rc.1',
    sourceSha,
    linuxLauncher: resolve(root, 'talos-server'),
    windowsLauncher: resolve(root, 'talos-server.exe'),
    nativeArtifactsDirectory: native,
    imageRecordsFile: resolve(root, 'images.json'),
    sbomDirectory: sbom,
  };
}

describe('Community release bundle', () => {
  test('requires one immutable digest record for every appliance image', () => {
    expect(validatePublishedImageRecords(imageRecords, '1.2.3-rc.1', sourceSha)).toHaveProperty(
      'control_server.reference',
      imageRecords[3]?.reference,
    );
    expect(() =>
      validatePublishedImageRecords(imageRecords.slice(1), '1.2.3-rc.1', sourceSha),
    ).toThrow('missing published image record: api_backend');
    expect(() =>
      validatePublishedImageRecords(
        [
          { ...imageRecords[0], reference: 'ghcr.io/example/talos-api:latest' },
          ...imageRecords.slice(1),
        ],
        '1.2.3-rc.1',
        sourceSha,
      ),
    ).toThrow('immutable registry digest');
  });

  test('assembles a no-Bun appliance bundle with verified signed manifests and unsigned notices', async () => {
    const inputs = await fixtureInputs();
    await assembleCommunityReleaseBundle(inputs);

    expect(
      await Bun.file(resolve(inputs.outputDirectory, 'bin/linux-x86_64/talos-server')).exists(),
    ).toBe(true);
    expect(
      await Bun.file(
        resolve(inputs.outputDirectory, 'bin/windows-x86_64/talos-server-UNSIGNED.exe'),
      ).exists(),
    ).toBe(true);
    expect(
      await Bun.file(resolve(inputs.outputDirectory, 'compose/traefik/dynamic-acme.yml')).exists(),
    ).toBe(true);
    expect(
      (await lstat(resolve(inputs.outputDirectory, 'database/migrations'))).isDirectory(),
    ).toBe(true);
    expect(await Bun.file(resolve(inputs.outputDirectory, 'LICENSE')).exists()).toBe(true);
    expect(await Bun.file(resolve(inputs.outputDirectory, 'THIRD_PARTY_NOTICES.md')).exists()).toBe(
      true,
    );

    const install = JSON.parse(
      await readFile(resolve(inputs.outputDirectory, 'community-install.example.json'), 'utf8'),
    ) as { images: Record<string, string> };
    expect(install.images.api_backend).toEndWith(`@${digest('1')}`);
    const imageReferences = JSON.parse(
      await readFile(resolve(inputs.outputDirectory, 'image-references.json'), 'utf8'),
    ) as {
      runtimeDependencies: {
        bundledPostgres: { reference: string };
        acmeVolumeHelper: { reference: string };
      };
    };
    expect(imageReferences.runtimeDependencies.bundledPostgres.reference).toMatch(
      /^postgres:16-alpine@sha256:[0-9a-f]{64}$/,
    );
    expect(imageReferences.runtimeDependencies.acmeVolumeHelper.reference).toBe(
      imageReferences.runtimeDependencies.bundledPostgres.reference,
    );
    const releaseManifest = await readFile(
      resolve(inputs.outputDirectory, 'release-manifest.json'),
      'utf8',
    );
    expect(releaseManifest).toContain('release-bundle-metadata-not-cryptographic-attestation');
    expect(releaseManifest).toContain('"windowsAuthenticode": "unsigned"');
    expect(await readFile(resolve(inputs.outputDirectory, 'SHA256SUMS'), 'utf8')).toContain(
      'community-install.example.json',
    );
  });

  test('fails closed on a native artifact checksum mismatch or symlink', async () => {
    const checksumInputs = await fixtureInputs();
    await writeFile(
      resolve(checksumInputs.nativeArtifactsDirectory, 'Talos.Worker.x64.Update.zip'),
      'tampered',
    );
    await expect(assembleCommunityReleaseBundle(checksumInputs)).rejects.toThrow(
      'native artifact checksum mismatch',
    );

    const symlinkInputs = await fixtureInputs();
    await symlink(
      resolve(symlinkInputs.nativeArtifactsDirectory, 'manifest.json'),
      resolve(symlinkInputs.sbomDirectory, 'linked.spdx.json'),
    );
    await expect(assembleCommunityReleaseBundle(symlinkInputs)).rejects.toThrow(
      'release input contains a symlink',
    );

    const traversalInputs = await fixtureInputs();
    const nativeChecksums = await readFile(
      resolve(traversalInputs.nativeArtifactsDirectory, 'SHA256SUMS'),
      'utf8',
    );
    await writeFile(
      resolve(traversalInputs.nativeArtifactsDirectory, 'SHA256SUMS'),
      `${nativeChecksums}${'a'.repeat(64)}  ..\n`,
    );
    await expect(assembleCommunityReleaseBundle(traversalInputs)).rejects.toThrow(
      'native checksum filename is not a safe relative path',
    );
  });

  test('fails closed when required SBOM evidence is missing or malformed', async () => {
    const missingInputs = await fixtureInputs();
    await rm(resolve(missingInputs.sbomDirectory, 'launcher-windows.spdx.json'));
    await expect(assembleCommunityReleaseBundle(missingInputs)).rejects.toThrow(
      'launcher-windows.spdx.json',
    );

    const malformedInputs = await fixtureInputs();
    await writeFile(resolve(malformedInputs.sbomDirectory, 'source.spdx.json'), '{}\n');
    await expect(assembleCommunityReleaseBundle(malformedInputs)).rejects.toThrow(
      'required SBOM is not SPDX 2.3: source.spdx.json',
    );
  });
});
