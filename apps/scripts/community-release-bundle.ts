#!/usr/bin/env bun

import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, lstat, mkdir, opendir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { parseCommunityReleaseIdentity } from './community-release-version';

const MAX_FILE_BYTES = 4 * 1024 * 1024 * 1024;
const IMAGE_KEYS = ['api_backend', 'frontend', 'relay', 'control_server'] as const;
const COMPOSE_ASSETS = [
  'infra/compose.community.yml',
  'infra/compose.community-postgres.yml',
  'infra/compose.community-traefik.yml',
  'infra/compose.community-traefik-custom.yml',
  'infra/compose.community-traefik-local.yml',
  'infra/traefik/dynamic-acme.yml',
  'infra/traefik/dynamic-custom.yml',
] as const;
const DOCUMENT_ASSETS = [
  'docs/community-deployment.md',
  'docs/community-edge.md',
  'docs/community-release-process.md',
  'docs/release-signing.md',
  'apps/talos_appliance/README.md',
] as const;
const LEGAL_ASSETS = ['LICENSE', 'THIRD_PARTY_NOTICES.md'] as const;
const BUNDLED_POSTGRES_REFERENCE =
  'postgres:16-alpine@sha256:cf78e76683b9ca8c5733cbbdce6c9262b45b6767934dd0a95e671f9a0fc20685';

type ImageKey = (typeof IMAGE_KEYS)[number];

export type PublishedImageRecord = {
  schemaVersion: 1;
  key: ImageKey;
  releaseVersion: string;
  sourceSha: string;
  reference: string;
  digest: string;
  platforms: string[];
  ociArchiveSha256: string;
  sbomFile: string;
};

export type CommunityBundleInputs = {
  repoRoot: string;
  outputDirectory: string;
  releaseTag: string;
  releaseVersion: string;
  sourceSha: string;
  linuxLauncher: string;
  windowsLauncher: string;
  nativeArtifactsDirectory: string;
  imageRecordsFile: string;
  sbomDirectory: string;
};

type WindowsBuildProvenance = {
  source?: {
    revision?: unknown;
    trackedSourceDirty?: unknown;
  };
  builder?: {
    profile?: unknown;
  };
  trust?: {
    updaterManifestPublicKeySha256?: unknown;
    windowsAuthenticodeStatus?: unknown;
  };
};

type WindowsArtifactManifest = {
  profile?: unknown;
  signing?: {
    updaterManifests?: { publicKeySha256?: unknown };
    windowsAuthenticode?: { status?: unknown };
  };
  integrity?: {
    checksumAlgorithm?: unknown;
    checksumFile?: unknown;
  };
};

const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const DIGEST_REFERENCE_PATTERN =
  /^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*(?::[0-9]+)?\/)?[a-z0-9]+(?:[._/-][a-z0-9]+)*@sha256:[0-9a-f]{64}$/;

function assertSafeRelativePath(path: string, label: string): void {
  const components = path.split(/[\\/]/);
  if (
    path.length === 0 ||
    path.length > 512 ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.startsWith('\\') ||
    components.some((component) => component === '' || component === '.' || component === '..')
  ) {
    throw new Error(`${label} is not a safe relative path`);
  }
}

export function validatePublishedImageRecords(
  value: unknown,
  releaseVersion: string,
  sourceSha: string,
): Record<ImageKey, PublishedImageRecord> {
  if (!Array.isArray(value)) throw new Error('image records must be a JSON array');
  const records = new Map<ImageKey, PublishedImageRecord>();

  for (const item of value) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) {
      throw new Error('each image record must be an object');
    }
    const record = item as Partial<PublishedImageRecord>;
    if (record.schemaVersion !== 1 || !IMAGE_KEYS.includes(record.key as ImageKey)) {
      throw new Error('image record has an unsupported schema or key');
    }
    const key = record.key as ImageKey;
    if (records.has(key)) throw new Error(`duplicate image record: ${key}`);
    if (record.releaseVersion !== releaseVersion || record.sourceSha !== sourceSha) {
      throw new Error(`${key} image record does not belong to this release`);
    }
    if (
      typeof record.digest !== 'string' ||
      !/^sha256:[0-9a-f]{64}$/.test(record.digest) ||
      typeof record.reference !== 'string' ||
      !DIGEST_REFERENCE_PATTERN.test(record.reference) ||
      !record.reference.endsWith(`@${record.digest}`)
    ) {
      throw new Error(`${key} must use one matching immutable registry digest reference`);
    }
    if (
      !Array.isArray(record.platforms) ||
      JSON.stringify([...record.platforms].sort()) !==
        JSON.stringify(['linux/amd64', 'linux/arm64'])
    ) {
      throw new Error(`${key} must record the reviewed linux/amd64 and linux/arm64 images`);
    }
    if (
      typeof record.ociArchiveSha256 !== 'string' ||
      !SHA256_PATTERN.test(record.ociArchiveSha256)
    ) {
      throw new Error(`${key} must retain the candidate OCI archive SHA-256`);
    }
    if (typeof record.sbomFile !== 'string') throw new Error(`${key} must identify its SBOM`);
    assertSafeRelativePath(record.sbomFile, `${key} SBOM path`);
    records.set(key, record as PublishedImageRecord);
  }

  for (const key of IMAGE_KEYS) {
    if (!records.has(key)) throw new Error(`missing published image record: ${key}`);
  }
  return Object.fromEntries(records) as Record<ImageKey, PublishedImageRecord>;
}

async function sha256(path: string): Promise<string> {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

async function copyRegularFile(source: string, destination: string): Promise<void> {
  const metadata = await lstat(source);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`release input must be a regular file: ${source}`);
  }
  if (metadata.size > MAX_FILE_BYTES) throw new Error(`release input is too large: ${source}`);
  await mkdir(dirname(destination), { recursive: true });
  await copyFile(source, destination);
}

async function copyDirectory(source: string, destination: string): Promise<void> {
  const sourceMetadata = await lstat(source);
  if (!sourceMetadata.isDirectory() || sourceMetadata.isSymbolicLink()) {
    throw new Error(`release input must be a real directory: ${source}`);
  }
  await mkdir(destination, { recursive: true });
  const directory = await opendir(source);
  for await (const entry of directory) {
    const from = resolve(source, entry.name);
    const to = resolve(destination, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`release input contains a symlink: ${from}`);
    if (entry.isDirectory()) await copyDirectory(from, to);
    else if (entry.isFile()) await copyRegularFile(from, to);
    else throw new Error(`release input contains an unsupported filesystem entry: ${from}`);
  }
}

async function verifyNativeArtifactChecksums(directory: string): Promise<void> {
  const checksumPath = resolve(directory, 'SHA256SUMS');
  const lines = (await readFile(checksumPath, 'utf8')).split(/\r?\n/).filter(Boolean);
  if (lines.length === 0) throw new Error('native SHA256SUMS must not be empty');
  const seen = new Set<string>();
  for (const line of lines) {
    const match = /^([0-9a-f]{64})  ([^/\\]+)$/.exec(line);
    if (!match) throw new Error(`invalid native checksum line: ${line}`);
    const [, expected, fileName] = match;
    assertSafeRelativePath(fileName, 'native checksum filename');
    if (seen.has(fileName)) throw new Error(`duplicate native checksum entry: ${fileName}`);
    seen.add(fileName);
    if ((await sha256(resolve(directory, fileName))) !== expected) {
      throw new Error(`native artifact checksum mismatch: ${fileName}`);
    }
  }
  for (const required of ['UNSIGNED-BINARIES.txt', 'build-provenance.json', 'manifest.json']) {
    if (!seen.has(required)) throw new Error(`native SHA256SUMS does not cover ${required}`);
  }

  const actualEntries = await readdir(directory, { withFileTypes: true });
  const actualFiles = new Set<string>();
  for (const entry of actualEntries) {
    if (entry.isSymbolicLink() || !entry.isFile()) {
      throw new Error(`native artifact handoff must contain only regular files: ${entry.name}`);
    }
    if (entry.name !== 'SHA256SUMS') actualFiles.add(entry.name);
  }
  if (actualFiles.size !== seen.size || [...actualFiles].some((fileName) => !seen.has(fileName))) {
    throw new Error('native SHA256SUMS must cover every handed-off artifact exactly once');
  }
}

async function inspectWindowsTrust(directory: string, sourceSha: string): Promise<string> {
  await verifyNativeArtifactChecksums(directory);
  const notice = await readFile(resolve(directory, 'UNSIGNED-BINARIES.txt'), 'utf8');
  if (!notice.includes('UNSIGNED COMMUNITY BINARIES')) {
    throw new Error('native artifact set is missing the explicit unsigned-binary warning');
  }
  const provenance = JSON.parse(
    await readFile(resolve(directory, 'build-provenance.json'), 'utf8'),
  ) as WindowsBuildProvenance;
  if (
    provenance.source?.revision !== sourceSha ||
    provenance.source.trackedSourceDirty !== false ||
    provenance.builder?.profile !== 'release'
  ) {
    throw new Error('native provenance must identify this clean release source and profile');
  }
  if (provenance.trust?.windowsAuthenticodeStatus !== 'unsigned') {
    throw new Error('initial Community Windows artifacts must explicitly record unsigned status');
  }
  const fingerprint = provenance.trust?.updaterManifestPublicKeySha256;
  if (typeof fingerprint !== 'string' || !SHA256_PATTERN.test(fingerprint)) {
    throw new Error('native provenance must record the updater-manifest public-key SHA-256');
  }

  const artifactManifest = JSON.parse(
    await readFile(resolve(directory, 'manifest.json'), 'utf8'),
  ) as WindowsArtifactManifest;
  if (
    artifactManifest.profile !== 'release' ||
    artifactManifest.signing?.updaterManifests?.publicKeySha256 !== fingerprint ||
    artifactManifest.signing.windowsAuthenticode?.status !== 'unsigned' ||
    artifactManifest.integrity?.checksumAlgorithm !== 'SHA-256' ||
    artifactManifest.integrity.checksumFile !== 'SHA256SUMS'
  ) {
    throw new Error('native artifact manifest trust metadata conflicts with provenance');
  }

  const files = await readdir(directory);
  const manifests = files.filter((name) => name.endsWith('.Update.manifest.json'));
  if (manifests.length === 0) throw new Error('native artifacts contain no updater manifests');
  for (const manifest of manifests) {
    const signature = manifest.replace(/\.json$/, '.sig');
    if (!files.includes(signature)) throw new Error(`missing updater signature for ${manifest}`);
  }
  return fingerprint;
}

async function verifySboms(
  directory: string,
  imageRecords: Record<ImageKey, PublishedImageRecord>,
): Promise<void> {
  const required = new Set([
    'source.spdx.json',
    'launcher-linux.spdx.json',
    'launcher-windows.spdx.json',
    'native-clients.spdx.json',
    ...IMAGE_KEYS.map((key) => imageRecords[key].sbomFile),
  ]);
  for (const relativePath of required) {
    assertSafeRelativePath(relativePath, 'SBOM path');
    const path = resolve(directory, relativePath);
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new Error(`required SBOM must be a regular file: ${relativePath}`);
    }
    let value: unknown;
    try {
      value = JSON.parse(await readFile(path, 'utf8'));
    } catch {
      throw new Error(`required SBOM is not valid JSON: ${relativePath}`);
    }
    if (
      !value ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      (value as { spdxVersion?: unknown }).spdxVersion !== 'SPDX-2.3'
    ) {
      throw new Error(`required SBOM is not SPDX 2.3: ${relativePath}`);
    }
  }
}

async function verifyRuntimeDependencyContract(repoRoot: string): Promise<void> {
  const [postgresCompose, launcherCompose] = await Promise.all([
    readFile(resolve(repoRoot, 'infra/compose.community-postgres.yml'), 'utf8'),
    readFile(resolve(repoRoot, 'apps/talos_appliance/src/compose.rs'), 'utf8'),
  ]);
  if (
    !postgresCompose.includes(BUNDLED_POSTGRES_REFERENCE) ||
    !launcherCompose.includes(BUNDLED_POSTGRES_REFERENCE)
  ) {
    throw new Error('bundled PostgreSQL runtime digest differs between Compose and launcher');
  }
}

async function listRegularFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  async function visit(directory: string): Promise<void> {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const path = resolve(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`bundle contains an unexpected symlink: ${path}`);
      if (entry.isDirectory()) await visit(path);
      else if (entry.isFile()) files.push(path);
      else throw new Error(`bundle contains an unsupported filesystem entry: ${path}`);
    }
  }
  await visit(root);
  return files.sort((left, right) => left.localeCompare(right));
}

function portableRelative(root: string, path: string): string {
  return relative(root, path).split(sep).join('/');
}

async function writeChecksums(root: string): Promise<void> {
  const paths = (await listRegularFiles(root)).filter(
    (path) => portableRelative(root, path) !== 'SHA256SUMS',
  );
  const lines: string[] = [];
  for (const path of paths) lines.push(`${await sha256(path)}  ${portableRelative(root, path)}`);
  await writeFile(resolve(root, 'SHA256SUMS'), `${lines.join('\n')}\n`, 'utf8');
}

export async function assembleCommunityReleaseBundle(inputs: CommunityBundleInputs): Promise<void> {
  const identity = parseCommunityReleaseIdentity({
    tag: inputs.releaseTag,
    sourceSha: inputs.sourceSha,
    refType: 'tag',
    tagObjectType: 'tag',
  });
  if (identity.version !== inputs.releaseVersion) {
    throw new Error('release version must be derived from the release tag');
  }
  try {
    await lstat(inputs.outputDirectory);
    throw new Error('release bundle output directory must not already exist');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }

  const imageRecords = validatePublishedImageRecords(
    JSON.parse(await readFile(inputs.imageRecordsFile, 'utf8')),
    inputs.releaseVersion,
    inputs.sourceSha,
  );
  const manifestFingerprint = await inspectWindowsTrust(
    inputs.nativeArtifactsDirectory,
    inputs.sourceSha,
  );
  await verifySboms(inputs.sbomDirectory, imageRecords);
  await verifyRuntimeDependencyContract(inputs.repoRoot);
  await mkdir(inputs.outputDirectory, { recursive: true });

  await copyRegularFile(
    inputs.linuxLauncher,
    resolve(inputs.outputDirectory, 'bin/linux-x86_64/talos-server'),
  );
  await copyRegularFile(
    inputs.windowsLauncher,
    resolve(inputs.outputDirectory, 'bin/windows-x86_64/talos-server-UNSIGNED.exe'),
  );
  await copyDirectory(
    inputs.nativeArtifactsDirectory,
    resolve(inputs.outputDirectory, 'clients/UNSIGNED-WINDOWS'),
  );
  await copyDirectory(inputs.sbomDirectory, resolve(inputs.outputDirectory, 'sbom'));

  for (const asset of LEGAL_ASSETS) {
    await copyRegularFile(resolve(inputs.repoRoot, asset), resolve(inputs.outputDirectory, asset));
  }

  for (const asset of COMPOSE_ASSETS) {
    const relativeAsset = asset.replace(/^infra\//, '');
    await copyRegularFile(
      resolve(inputs.repoRoot, asset),
      resolve(inputs.outputDirectory, 'compose', relativeAsset),
    );
  }
  for (const asset of DOCUMENT_ASSETS) {
    await copyRegularFile(
      resolve(inputs.repoRoot, asset),
      resolve(inputs.outputDirectory, 'notices-and-guides', asset),
    );
  }
  await copyRegularFile(
    resolve(inputs.repoRoot, 'apps/api_backend/prisma/schema.prisma'),
    resolve(inputs.outputDirectory, 'database/schema.prisma'),
  );
  await copyDirectory(
    resolve(inputs.repoRoot, 'apps/api_backend/prisma/migrations'),
    resolve(inputs.outputDirectory, 'database/migrations'),
  );

  const imageReferences = Object.fromEntries(
    IMAGE_KEYS.map((key) => [key, imageRecords[key].reference]),
  ) as Record<ImageKey, string>;
  const runtimeDependencies = {
    bundledPostgres: {
      reference: BUNDLED_POSTGRES_REFERENCE,
      policy: 'reviewed-digest-embedded-in-compose-and-launcher',
    },
    acmeVolumeHelper: {
      reference: BUNDLED_POSTGRES_REFERENCE,
      policy: 'reviewed-third-party-helper-digest-embedded-in-launcher',
    },
    traefik: {
      requestedReference: 'docker.io/library/traefik:latest',
      policy: 'resolve-official-latest-on-install-and-explicit-update-then-persist-digest',
    },
  };
  await writeFile(
    resolve(inputs.outputDirectory, 'image-references.json'),
    `${JSON.stringify(
      { schemaVersion: 1, ...identity, images: imageRecords, runtimeDependencies },
      null,
      2,
    )}\n`,
    'utf8',
  );
  await writeFile(
    resolve(inputs.outputDirectory, 'community-install.example.json'),
    `${JSON.stringify(
      {
        schema_version: 1,
        release_version: inputs.releaseVersion,
        update_channel: 'stable',
        images: imageReferences,
        database: { mode: 'bundled', user: 'talos', database: 'talos' },
        edge: {
          mode: 'public_acme',
          frontend_domain: 'talos.example.invalid',
          api_domain: 'api.talos.example.invalid',
          control_domain: 'control.talos.example.invalid',
          relay_domain: 'relay.talos.example.invalid',
          acme_email: 'talos-operator@example.invalid',
          http_port: 80,
          https_port: 443,
          subnet: '172.31.240.0/24',
          proxy_ipv4: '172.31.240.2',
        },
        paths: {},
      },
      null,
      2,
    )}\n`,
    'utf8',
  );
  await writeFile(
    resolve(inputs.outputDirectory, 'README.txt'),
    `Talos Community ${inputs.releaseVersion}\n\n` +
      '1. Verify SHA256SUMS before using any file.\n' +
      '2. Edit community-install.example.json and replace every example.invalid value.\n' +
      '3. Run the launcher for your platform: talos-server install --config <absolute-path>.\n' +
      '4. Windows launchers and client artifacts in this release are intentionally unsigned.\n' +
      '5. The updater manifests remain cryptographically signed; their public-key fingerprint is\n' +
      `   ${manifestFingerprint}.\n\n` +
      'See notices-and-guides/docs/community-deployment.md and docs/release-signing.md.\n',
    'utf8',
  );

  const artifactFiles = await listRegularFiles(inputs.outputDirectory);
  const inventory = [];
  for (const path of artifactFiles) {
    inventory.push({
      path: portableRelative(inputs.outputDirectory, path),
      sha256: await sha256(path),
    });
  }
  await writeFile(
    resolve(inputs.outputDirectory, 'release-manifest.json'),
    `${JSON.stringify(
      {
        schemaVersion: 1,
        release: identity,
        generatedBy: 'apps/scripts/community-release-bundle.ts',
        provenanceKind: 'release-bundle-metadata-not-cryptographic-attestation',
        signing: {
          windowsAuthenticode: 'unsigned',
          updaterManifestPublicKeySha256: manifestFingerprint,
        },
        runtimeDependencies,
        artifacts: inventory,
      },
      null,
      2,
    )}\n`,
    'utf8',
  );
  await writeChecksums(inputs.outputDirectory);
}

function option(name: string): string {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  if (!value || value.startsWith('--')) throw new Error(`${name} is required`);
  return resolve(value);
}

function rawOption(name: string): string {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  if (!value || value.startsWith('--')) throw new Error(`${name} is required`);
  return value;
}

if (import.meta.main) {
  await assembleCommunityReleaseBundle({
    repoRoot: option('--repo-root'),
    outputDirectory: option('--output'),
    releaseTag: rawOption('--release-tag'),
    releaseVersion: rawOption('--release-version'),
    sourceSha: rawOption('--source-sha'),
    linuxLauncher: option('--linux-launcher'),
    windowsLauncher: option('--windows-launcher'),
    nativeArtifactsDirectory: option('--native-artifacts'),
    imageRecordsFile: option('--image-records'),
    sbomDirectory: option('--sbom-directory'),
  });
}
