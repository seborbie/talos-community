import crypto from 'crypto';
import fs from 'fs/promises';
import path from 'path';
import { Router } from 'express';
import { attachRmmServerAuth } from '../middleware/rmmServerKey';

export const updatesRouter = Router();
updatesRouter.use(attachRmmServerAuth);

export type UpdateProduct = 'agent' | 'viewer' | 'worker' | 'supervisor';
export type UpdateArch =
  | 'x64'
  | 'x86'
  | 'x64-v1'
  | 'x64-v2'
  | 'x64-v3'
  | 'x64-v4'
  | 'linux-x64'
  | 'linux-x86'
  | 'linux-arm64'
  | 'linux-arm'
  | 'macos-arm64'
  | 'macos-x64';

export type UpdateManifest = {
  product: UpdateProduct;
  platform: string;
  arch: UpdateArch;
  channel: string;
  ring?: string | null;
  version: string;
  minimumSupportedVersion?: string | null;
  severity: string;
  publishedAtUtc: string;
  rolloutPercentage?: number | null;
  package: {
    fileName: string;
    sizeBytes: number;
    sha256: string;
  };
  contents: string[];
  requiresRestart: boolean;
  installMode: string;
};

class HttpError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

const DEFAULT_ARTIFACT_MANIFEST_FILENAME = 'manifest.json';
function readString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value !== 'string') continue;
    const trimmed = value.trim();
    if (trimmed) {
      return trimmed;
    }
  }
  return null;
}

function parseProduct(value: unknown): UpdateProduct {
  const normalized = readString(value)?.toLowerCase();
  if (
    normalized === 'agent' ||
    normalized === 'viewer' ||
    normalized === 'worker' ||
    normalized === 'supervisor'
  ) {
    return normalized;
  }
  throw new HttpError(400, 'product must be agent, worker, supervisor, or viewer');
}

function parseArch(product: UpdateProduct, value: unknown): UpdateArch {
  const normalized = readString(value)?.toLowerCase() || defaultArchForProduct(product);
  if (product === 'viewer') {
    if (normalized === 'x64' || normalized === 'macos-arm64' || normalized === 'macos-x64') {
      return normalized;
    }
    throw new HttpError(400, 'viewer updates support x64, macos-arm64, or macos-x64');
  }
  if (product === 'agent') {
    if (normalized === 'x64' || normalized === 'x86') return normalized;
    throw new HttpError(400, 'agent updates support x64 or x86');
  }
  if (product === 'supervisor') {
    if (isSupervisorArch(normalized)) return normalized;
    throw new HttpError(
      400,
      'supervisor updates support x86, x64, linux-x64, linux-x86, linux-arm64, linux-arm, macos-arm64, or macos-x64',
    );
  }
  if (isWorkerArch(normalized)) {
    return normalized;
  }
  throw new HttpError(
    400,
    'worker updates support x86, x64, x64-v1, x64-v2, x64-v3, x64-v4, linux-x64, linux-x86, linux-arm64, linux-arm, macos-arm64, or macos-x64',
  );
}

function isSupervisorArch(value: string): value is UpdateArch {
  return (
    value === 'x86' ||
    value === 'x64' ||
    value === 'linux-x64' ||
    value === 'linux-x86' ||
    value === 'linux-arm64' ||
    value === 'linux-arm' ||
    value === 'macos-arm64' ||
    value === 'macos-x64'
  );
}

function isWorkerArch(value: string): value is UpdateArch {
  return (
    value === 'x86' ||
    value === 'x64' ||
    value === 'x64-v1' ||
    value === 'x64-v2' ||
    value === 'x64-v3' ||
    value === 'x64-v4' ||
    value === 'linux-x64' ||
    value === 'linux-x86' ||
    value === 'linux-arm64' ||
    value === 'linux-arm' ||
    value === 'macos-arm64' ||
    value === 'macos-x64'
  );
}

function defaultArchForProduct(product: UpdateProduct): UpdateArch {
  if (product === 'supervisor') return 'x86';
  if (product === 'worker') return 'x64-v1';
  return 'x64';
}

function normalizeVersion(value: string): number[] | null {
  const trimmed = value.trim().replace(/^v/i, '');
  if (!trimmed) return null;
  const parts = trimmed.split('.');
  const parsed = parts.map((part) => Number(part));
  if (parsed.some((part) => !Number.isInteger(part) || part < 0)) {
    return null;
  }
  return parsed;
}

function compareVersions(left: string, right: string): number | null {
  const a = normalizeVersion(left);
  const b = normalizeVersion(right);
  if (!a || !b) return null;
  const max = Math.max(a.length, b.length);
  for (let index = 0; index < max; index += 1) {
    const av = a[index] ?? 0;
    const bv = b[index] ?? 0;
    if (av > bv) return 1;
    if (av < bv) return -1;
  }
  return 0;
}

function computeRolloutBucket(seed: string): number {
  const digest = crypto.createHash('sha256').update(seed).digest();
  return digest.readUInt32BE(0) % 100;
}

function resolveArtifactCandidates(configuredPath: string | null, fileName: string): string[] {
  const cwd = process.cwd();
  const profiles = ['dev', 'perfdev', 'release', 'release-native'];
  const discovered = profiles.flatMap((profile) => [
    path.resolve('/installer-artifacts', profile, fileName),
    path.resolve(cwd, 'installer', 'artifacts', profile, fileName),
    path.resolve(cwd, 'apps', 'installer', 'artifacts', profile, fileName),
    path.resolve(cwd, '..', 'installer', 'artifacts', profile, fileName),
    path.resolve(__dirname, '../../installer/artifacts', profile, fileName),
    path.resolve(__dirname, '../../../installer/artifacts', profile, fileName),
  ]);

  return [configuredPath, ...discovered].filter((value): value is string =>
    Boolean(value && value.trim()),
  );
}

async function resolveArtifactFile(
  configuredPath: string | null,
  fileName: string,
  missingMessage: (attempts: string[]) => string,
): Promise<string> {
  const attempts: string[] = [];
  for (const candidate of resolveArtifactCandidates(configuredPath, fileName)) {
    attempts.push(candidate);
    try {
      await fs.access(candidate);
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }
  throw new HttpError(500, missingMessage(attempts));
}

function updateArtifactNames(product: UpdateProduct, arch: UpdateArch) {
  const prefixByProduct: Record<UpdateProduct, string> = {
    agent: `Talos.Agent.${arch}`,
    worker: `Talos.Worker.${arch}`,
    supervisor: `Talos.Supervisor.${arch}`,
    viewer:
      arch === 'macos-arm64' || arch === 'macos-x64' ? `Talos.Viewer.${arch}` : 'Talos.Viewer.x64',
  };
  const prefix = prefixByProduct[product];
  return {
    packageFileName:
      product === 'viewer' && (arch === 'macos-arm64' || arch === 'macos-x64')
        ? 'Talos.Viewer.macos.pkg'
        : `${prefix}.Update.zip`,
    manifestFileName: `${prefix}.Update.manifest.json`,
    signatureFileName: `${prefix}.Update.manifest.sig`,
  };
}

function artifactEnvKeys(product: UpdateProduct, arch: UpdateArch) {
  const productKey = product.toUpperCase();
  const archKey = arch.toUpperCase().replace(/-/g, '_');
  return {
    manifestPath: readString(process.env[`RMM_${productKey}_UPDATE_MANIFEST_PATH_${archKey}`]),
    signaturePath: readString(process.env[`RMM_${productKey}_UPDATE_SIGNATURE_PATH_${archKey}`]),
    packagePath: readString(process.env[`RMM_${productKey}_UPDATE_PACKAGE_PATH_${archKey}`]),
  };
}

async function resolveUpdateArtifacts(product: UpdateProduct, arch: UpdateArch) {
  const names = updateArtifactNames(product, arch);
  const envKeys = artifactEnvKeys(product, arch);
  const manifestPath = await resolveArtifactFile(
    envKeys.manifestPath,
    names.manifestFileName,
    (attempts) => `Update manifest not found. Attempted: ${attempts.join(', ')}`,
  );
  const signaturePath = await resolveArtifactFile(
    envKeys.signaturePath,
    names.signatureFileName,
    (attempts) => `Update manifest signature not found. Attempted: ${attempts.join(', ')}`,
  );
  const packagePath = await resolveArtifactFile(
    envKeys.packagePath,
    names.packageFileName,
    (attempts) => `Update package not found. Attempted: ${attempts.join(', ')}`,
  );
  return {
    names,
    manifestPath,
    signaturePath,
    packagePath,
  };
}

async function loadSignedManifest(product: UpdateProduct, arch: UpdateArch) {
  const artifacts = await resolveUpdateArtifacts(product, arch);
  const manifestBytes = await fs.readFile(artifacts.manifestPath);
  const signature = (await fs.readFile(artifacts.signaturePath, 'utf8')).trim();
  if (!signature) {
    throw new HttpError(500, 'update manifest signature file is empty');
  }
  let parsedManifest: unknown;
  try {
    parsedManifest = JSON.parse(manifestBytes.toString('utf8'));
  } catch {
    throw new HttpError(500, 'update manifest is not valid JSON');
  }
  const packageStat = await fs.stat(artifacts.packagePath);
  if (!packageStat.isFile()) {
    throw new HttpError(500, 'update package is not a regular file');
  }
  const manifest = validateUpdateManifestArtifact(parsedManifest, product, arch, packageStat.size);
  const stat = await fs.stat(artifacts.manifestPath);
  return {
    artifacts,
    manifest,
    manifestBytes,
    signature,
    etag: `W/"${stat.size}-${Math.floor(stat.mtimeMs)}"`,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function requiredManifestString(record: Record<string, unknown>, field: string): string {
  const value = record[field];
  if (typeof value !== 'string' || !value.trim()) {
    throw new HttpError(500, `update manifest ${field} must be a non-empty string`);
  }
  return value;
}

function expectedPlatformForArch(arch: UpdateArch): string {
  if (arch.startsWith('linux-')) return 'linux';
  if (arch.startsWith('macos-')) return 'macos';
  return 'windows';
}

function expectedInstallMode(product: UpdateProduct, platform: string): string {
  if (product === 'viewer' && platform === 'windows') return 'restart';
  if (product === 'viewer' && platform === 'macos') return 'pkg';
  if ((product === 'worker' || product === 'supervisor') && platform === 'macos') return 'zip';
  return 'silent';
}

export function validateUpdateManifestArtifact(
  value: unknown,
  expectedProduct: UpdateProduct,
  expectedArch: UpdateArch,
  actualPackageSize: number,
): UpdateManifest {
  if (!isRecord(value) || !isRecord(value.package)) {
    throw new HttpError(500, 'update manifest must be an object with package metadata');
  }
  const platform = expectedPlatformForArch(expectedArch);
  const names = updateArtifactNames(expectedProduct, expectedArch);
  const expectedContext: Array<[string, string, string]> = [
    ['product', requiredManifestString(value, 'product'), expectedProduct],
    ['platform', requiredManifestString(value, 'platform'), platform],
    ['arch', requiredManifestString(value, 'arch'), expectedArch],
    [
      'installMode',
      requiredManifestString(value, 'installMode'),
      expectedInstallMode(expectedProduct, platform),
    ],
    ['package.fileName', requiredManifestString(value.package, 'fileName'), names.packageFileName],
  ];
  for (const [field, actual, expected] of expectedContext) {
    if (actual !== expected) {
      throw new HttpError(500, `update manifest ${field} does not match its artifact slot`);
    }
  }

  const channel = requiredManifestString(value, 'channel');
  const version = requiredManifestString(value, 'version');
  const severity = requiredManifestString(value, 'severity');
  const publishedAtUtc = requiredManifestString(value, 'publishedAtUtc');
  const ringValue = value.ring;
  if (
    ringValue !== undefined &&
    ringValue !== null &&
    (typeof ringValue !== 'string' || !ringValue.trim())
  ) {
    throw new HttpError(500, 'update manifest ring must be a non-empty string when present');
  }
  const minimumSupportedVersion = value.minimumSupportedVersion;
  if (
    minimumSupportedVersion !== undefined &&
    minimumSupportedVersion !== null &&
    (typeof minimumSupportedVersion !== 'string' || !minimumSupportedVersion.trim())
  ) {
    throw new HttpError(
      500,
      'update manifest minimumSupportedVersion must be a non-empty string when present',
    );
  }
  const rolloutPercentage = value.rolloutPercentage;
  if (
    rolloutPercentage !== undefined &&
    rolloutPercentage !== null &&
    (typeof rolloutPercentage !== 'number' ||
      !Number.isInteger(rolloutPercentage) ||
      rolloutPercentage < 0 ||
      rolloutPercentage > 100)
  ) {
    throw new HttpError(
      500,
      'update manifest rolloutPercentage must be an integer from 0 through 100',
    );
  }
  if (!Array.isArray(value.contents) || value.contents.some((entry) => typeof entry !== 'string')) {
    throw new HttpError(500, 'update manifest contents must be an array of strings');
  }
  if (typeof value.requiresRestart !== 'boolean') {
    throw new HttpError(500, 'update manifest requiresRestart must be a boolean');
  }

  const sizeBytes = value.package.sizeBytes;
  if (typeof sizeBytes !== 'number' || !Number.isSafeInteger(sizeBytes) || sizeBytes <= 0) {
    throw new HttpError(500, 'update manifest package.sizeBytes must be a positive safe integer');
  }
  if (
    !Number.isSafeInteger(actualPackageSize) ||
    actualPackageSize <= 0 ||
    sizeBytes !== actualPackageSize
  ) {
    throw new HttpError(
      500,
      'update manifest package.sizeBytes does not match the selected package',
    );
  }
  const sha256 = requiredManifestString(value.package, 'sha256');
  if (!/^[0-9a-f]{64}$/i.test(sha256)) {
    throw new HttpError(
      500,
      'update manifest package.sha256 must be a 64-character hexadecimal digest',
    );
  }

  return {
    product: expectedProduct,
    platform,
    arch: expectedArch,
    channel,
    ring: ringValue as string | null | undefined,
    version,
    minimumSupportedVersion: minimumSupportedVersion as string | null | undefined,
    severity,
    publishedAtUtc,
    rolloutPercentage: rolloutPercentage as number | null | undefined,
    package: {
      fileName: names.packageFileName,
      sizeBytes: sizeBytes as number,
      sha256,
    },
    contents: value.contents as string[],
    requiresRestart: value.requiresRestart,
    installMode: expectedInstallMode(expectedProduct, platform),
  };
}

async function tryLoadInstallerArtifactManifest(): Promise<any | null> {
  const configured = readString(process.env.RMM_INSTALLER_ARTIFACT_MANIFEST_PATH);
  for (const candidate of resolveArtifactCandidates(
    configured,
    DEFAULT_ARTIFACT_MANIFEST_FILENAME,
  )) {
    try {
      const raw = await fs.readFile(candidate, 'utf8');
      return JSON.parse(raw);
    } catch {
      // Ignore missing or invalid manifest files.
    }
  }
  return null;
}

function shouldServeUpdate(
  manifest: UpdateManifest,
  currentVersion: string | null,
  requestedChannel: string | null,
  requestedRing: string | null,
  rolloutSeed: string | null,
): boolean {
  if (!manifestMatchesAudience(manifest, requestedChannel, requestedRing)) {
    return false;
  }
  if (currentVersion) {
    const comparison = compareVersions(currentVersion, manifest.version);
    if (comparison !== null && comparison >= 0) {
      return false;
    }
  }
  const rolloutPercentage = manifest.rolloutPercentage ?? 100;
  if (rolloutPercentage < 100) {
    if (!rolloutSeed) {
      return false;
    }
    const bucket = computeRolloutBucket(rolloutSeed);
    if (bucket >= rolloutPercentage) {
      return false;
    }
  }
  return true;
}

export function manifestMatchesAudience(
  manifest: UpdateManifest,
  requestedChannel: string | null,
  requestedRing: string | null,
): boolean {
  if (requestedChannel && requestedChannel !== manifest.channel) {
    return false;
  }
  return requestedRing === (manifest.ring ?? null);
}

updatesRouter.get('/:product/manifest', async (req, res, next) => {
  try {
    const product = parseProduct(req.params.product);
    const arch = parseArch(product, req.query.arch);
    const requestedChannel = readString(req.query.channel) || 'stable';
    const requestedRing = readString(req.query.ring);
    const currentVersion = readString(req.query.currentVersion);
    const rolloutSeed = readString(req.query.rolloutSeed, req.query.agentId, req.query.viewerId);
    const signedManifest = await loadSignedManifest(product, arch);
    const manifest = signedManifest.manifest;
    if (
      !shouldServeUpdate(manifest, currentVersion, requestedChannel, requestedRing, rolloutSeed)
    ) {
      return res.status(204).send();
    }
    if (readString(req.header('if-none-match')) === signedManifest.etag) {
      return res.status(304).send();
    }

    res.setHeader('Content-Type', 'application/json');
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('ETag', signedManifest.etag);
    res.setHeader('X-Talos-Manifest-Signature', signedManifest.signature);
    res.setHeader('X-Talos-Manifest-Key-Id', `${product}-${arch}`);
    return res.send(signedManifest.manifestBytes);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    return next(error);
  }
});

updatesRouter.get('/:product/package', async (req, res, next) => {
  try {
    const product = parseProduct(req.params.product);
    const arch = parseArch(product, req.query.arch);
    const requestedChannel = readString(req.query.channel) || 'stable';
    const requestedRing = readString(req.query.ring);
    const signedManifest = await loadSignedManifest(product, arch);
    if (!manifestMatchesAudience(signedManifest.manifest, requestedChannel, requestedRing)) {
      return res.status(204).send();
    }
    const packagePath = signedManifest.artifacts.packagePath;
    const packageStat = await fs.stat(packagePath);
    if (packageStat.size !== signedManifest.manifest.package.sizeBytes) {
      throw new HttpError(500, 'update package changed after manifest validation');
    }
    const fileName = signedManifest.manifest.package.fileName;
    res.setHeader(
      'Content-Type',
      fileName.endsWith('.pkg') ? 'application/octet-stream' : 'application/zip',
    );
    res.setHeader('Content-Disposition', `attachment; filename="${fileName}"`);
    res.setHeader('Content-Length', String(packageStat.size));
    res.setHeader('Cache-Control', 'no-store');
    return res.sendFile(packagePath);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    return next(error);
  }
});

updatesRouter.get('/meta/installers', async (_req, res, next) => {
  try {
    const installerManifest = await tryLoadInstallerArtifactManifest();
    return res.json({
      manifest: installerManifest,
    });
  } catch (error) {
    return next(error);
  }
});
