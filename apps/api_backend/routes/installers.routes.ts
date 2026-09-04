import crypto from 'crypto';
import { execFile } from 'child_process';
import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { promisify } from 'util';
import { NextFunction, Request, Response, Router } from 'express';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import { AuthedRequest, requireAuth } from '../middleware/auth';
import { attachRmmServerAuth, requireRmmServer, RmmServerRequest } from '../middleware/rmmServerKey';
import { unassignedCustomerId } from './customers.routes';
import { auditRequest, writeAuditEvent } from '../lib/audit';
import { getTrustedClientIp, getTrustedRequestOrigin } from '../lib/requestTrust';

export const installersRouter = Router();
installersRouter.use(attachRmmServerAuth);

const execFileAsync = promisify(execFile);

type InstallerScopeType = 'organization' | 'customer' | 'site';
type PrismaInstallerScopeType = 'ORGANIZATION' | 'CUSTOMER' | 'SITE';

type MembershipWithOrg = NonNullable<Awaited<ReturnType<typeof getCurrentMembership>>>;

type ScopeResolution = {
  scopeType: PrismaInstallerScopeType;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
};

const DEFAULT_INSTALLER_SFX_STUB_FILENAME = '7zSD.sfx';
const DEFAULT_INSTALLER_PAYLOAD_ARCHIVE_FILENAME = 'Talos.Agent.Setup.7z';
const DEFAULT_INSTALLER_PAYLOAD_EXE_NAME = 'Talos.Agent.Setup.exe';
const DEFAULT_VIEWER_INSTALLER_FILENAME = 'Talos.Viewer.x64.msi';
const DEFAULT_VIEWER_MACOS_INSTALLER_FILENAME = 'Talos.Viewer.macos.pkg';
const DEFAULT_ARTIFACT_MANIFEST_FILENAME = 'manifest.json';
const DEFAULT_LINUX_AGENT_BINARY_FILENAME = 'talos-rmm-agent-linux-x64';
const DEFAULT_MACOS_PACKAGE_FILENAME = 'Talos.Agent.macos-universal.pkg';
const SHORT_LINUX_INSTALL_CODE_LENGTH = 8;
const SHORT_LINUX_INSTALL_TTL_DAYS = 7;
const SHORT_LINUX_INSTALL_CODE_ALPHABET = '23456789abcdefghjkmnpqrstuvwxyz';

let cachedInstallerSfxStubPath: string | null = null;
let cachedInstallerSfxStub: Buffer | null = null;
let cachedInstallerPayloadArchivePath: string | null = null;
let cachedInstallerPayloadArchive: Buffer | null = null;
let cachedInstallerSfxStubMtimeMs: number | null = null;
let cachedInstallerSfxStubSize: number | null = null;
let cachedInstallerPayloadArchiveMtimeMs: number | null = null;
let cachedInstallerPayloadArchiveSize: number | null = null;
let cachedLinuxAgentBinaryPath: string | null = null;
let cachedLinuxAgentBinary: Buffer | null = null;
let cachedLinuxAgentBinaryMtimeMs: number | null = null;
let cachedLinuxAgentBinarySize: number | null = null;
let cachedMacosPackagePath: string | null = null;
let cachedMacosPackage: Buffer | null = null;
let cachedMacosPackageMtimeMs: number | null = null;
let cachedMacosPackageSize: number | null = null;

class HttpError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { organization: true, user: { select: { id: true, email: true } } }
  });
}

function assertUser(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== 'user') {
    res.status(403).json({ error: 'Machine tokens are not allowed' });
    return false;
  }
  return true;
}

function isAgentAdmin(role: string) {
  return role === 'AGENT_ADMIN' || role === 'SUPER_ADMIN';
}

function normalizeScopeType(value: unknown): InstallerScopeType | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  if (normalized === 'organization' || normalized === 'customer' || normalized === 'site') {
    return normalized;
  }
  return null;
}

function toPrismaScopeType(scopeType: InstallerScopeType): PrismaInstallerScopeType {
  if (scopeType === 'customer') return 'CUSTOMER';
  if (scopeType === 'site') return 'SITE';
  return 'ORGANIZATION';
}

function fromPrismaScopeType(scopeType: PrismaInstallerScopeType): InstallerScopeType {
  if (scopeType === 'CUSTOMER') return 'customer';
  if (scopeType === 'SITE') return 'site';
  return 'organization';
}

function readString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === 'string') {
      const trimmed = value.trim();
      if (trimmed) {
        return trimmed;
      }
    }
  }
  return null;
}

export function unsignedScopedInstallersEnabled(value = process.env.RMM_ENABLE_UNSIGNED_SCOPED_INSTALLERS): boolean {
  return value?.trim().toLowerCase() === 'true';
}

export function configuredInstallerBootstrapUrl(
  value = process.env.RMM_INSTALLER_BOOTSTRAP_URL,
): string | null {
  return readString(value);
}

export function requireUnsignedScopedInstallerOptIn(_req: Request, res: Response, next: NextFunction) {
  if (!unsignedScopedInstallersEnabled()) {
    return res.status(503).json({
      error: 'Runtime-assembled scoped EXEs are disabled because modifying an executable invalidates its Authenticode signature',
      code: 'UNSIGNED_SCOPED_INSTALLERS_DISABLED'
    });
  }
  return next();
}

function parseOptionalDate(value: unknown): Date | null {
  if (value === null || value === undefined || value === '') return null;
  if (typeof value !== 'string') {
    throw new HttpError(400, 'expiresAt must be an ISO datetime string');
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    throw new HttpError(400, 'expiresAt must be an ISO datetime string');
  }
  return parsed;
}

function parseOptionalMaxUses(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new HttpError(400, 'maxUses must be a positive integer');
  }
  return parsed;
}

function buildScopeName(scopeType: InstallerScopeType, scope: { customerName?: string | null; siteName?: string | null }) {
  if (scopeType === 'customer') return scope.customerName || 'Customer Installer';
  if (scopeType === 'site') return scope.siteName || 'Site Installer';
  return 'Organization Installer';
}

function generateRegistrationToken(): string {
  return crypto.randomBytes(32).toString('base64url');
}

function generateShortInstallCode(): string {
  const bytes = crypto.randomBytes(SHORT_LINUX_INSTALL_CODE_LENGTH);
  let code = '';
  for (const byte of bytes) {
    code += SHORT_LINUX_INSTALL_CODE_ALPHABET[byte % SHORT_LINUX_INSTALL_CODE_ALPHABET.length];
  }
  return code;
}

function hashToken(rawToken: string): string {
  return crypto.createHash('sha256').update(rawToken).digest('hex');
}

function getClientIp(req: Request): string | null {
  return getTrustedClientIp(req);
}

type ViewerInstallerPlatform = 'windows' | 'macos';

type ViewerInstallerManifest = {
  profile?: string | null;
  generatedAtUtc?: string | null;
  viewer?: {
    installer?: {
      fileName?: string;
      sizeBytes?: number;
      sha256?: string;
    } | null;
    macosInstaller?: {
      fileName?: string;
      sizeBytes?: number;
      sha256?: string;
    } | null;
    pkgMacos?: {
      fileName?: string;
      sizeBytes?: number;
      sha256?: string;
    } | null;
  } | null;
};

function normalizeViewerInstallerPlatform(value: unknown): ViewerInstallerPlatform {
  if (typeof value !== 'string' || !value.trim()) return 'windows';
  const normalized = value.trim().toLowerCase();
  if (normalized === 'macos' || normalized === 'mac' || normalized === 'darwin') return 'macos';
  if (normalized === 'windows' || normalized === 'win' || normalized === 'win32') return 'windows';
  throw new HttpError(400, 'viewer installer platform must be windows or macos');
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
    path.resolve(__dirname, '../../../installer/artifacts', profile, fileName)
  ]);

  return [configuredPath, ...discovered].filter((value): value is string => Boolean(value && value.trim()));
}

function resolveLinuxAgentBinaryCandidates(configuredPath: string | null, fileName: string): string[] {
  const cwd = process.cwd();
  const localBuildCandidates = [
    path.resolve(cwd, 'target', 'release', 'talos_supervisor'),
    path.resolve(cwd, 'target', 'x86_64-unknown-linux-gnu', 'release', 'talos_supervisor'),
    path.resolve(cwd, 'apps', 'target', 'release', 'talos_supervisor'),
    path.resolve(cwd, 'apps', 'target', 'x86_64-unknown-linux-gnu', 'release', 'talos_supervisor'),
    path.resolve(__dirname, '../../target/release/talos_supervisor'),
    path.resolve(__dirname, '../../target/x86_64-unknown-linux-gnu/release/talos_supervisor'),
    path.resolve(__dirname, '../../../target/release/talos_supervisor'),
    path.resolve(__dirname, '../../../target/x86_64-unknown-linux-gnu/release/talos_supervisor')
  ];

  return [...resolveArtifactCandidates(configuredPath, fileName), ...localBuildCandidates].filter(
    (value): value is string => Boolean(value && value.trim())
  );
}

function resolveMacosPackageCandidates(configuredPath: string | null, fileName: string): string[] {
  return resolveArtifactCandidates(configuredPath, fileName);
}

async function resolveArtifactFile(
  configuredPath: string | null,
  fileName: string,
  missingMessage: (attempts: string[]) => string
): Promise<string> {
  const attempts: string[] = [];
  for (const candidate of resolveArtifactCandidates(configuredPath, fileName)) {
    attempts.push(candidate);
    try {
      await fs.access(candidate);
      return candidate;
    } catch {
      // Keep trying other candidates.
    }
  }
  throw new HttpError(500, missingMessage(attempts));
}

async function resolveInstallerSfxStubPath(): Promise<string> {
  const configured = readString(process.env.RMM_INSTALLER_SFX_STUB_PATH);
  return resolveArtifactFile(
    configured,
    DEFAULT_INSTALLER_SFX_STUB_FILENAME,
    (attempts) =>
      `SFX stub not found. Set RMM_INSTALLER_SFX_STUB_PATH or place ${DEFAULT_INSTALLER_SFX_STUB_FILENAME} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
  );
}

async function resolveInstallerPayloadArchivePath(): Promise<string> {
  const configured = readString(process.env.RMM_INSTALLER_PAYLOAD_7Z_PATH);
  return resolveArtifactFile(
    configured,
    DEFAULT_INSTALLER_PAYLOAD_ARCHIVE_FILENAME,
    (attempts) =>
      `Installer payload archive not found. Set RMM_INSTALLER_PAYLOAD_7Z_PATH or place ${DEFAULT_INSTALLER_PAYLOAD_ARCHIVE_FILENAME} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
  );
}

async function resolveViewerInstallerPath(platform: ViewerInstallerPlatform = 'windows'): Promise<string> {
  const configured =
    platform === 'macos'
      ? readString(process.env.RMM_VIEWER_MACOS_INSTALLER_PATH, process.env.RMM_VIEWER_PKG_PATH)
      : readString(process.env.RMM_VIEWER_INSTALLER_PATH);
  const fileName =
    platform === 'macos'
      ? readString(process.env.RMM_VIEWER_MACOS_INSTALLER_FILENAME, process.env.RMM_VIEWER_PKG_FILENAME) ||
        DEFAULT_VIEWER_MACOS_INSTALLER_FILENAME
      : readString(process.env.RMM_VIEWER_INSTALLER_FILENAME) || DEFAULT_VIEWER_INSTALLER_FILENAME;
  return resolveArtifactFile(
    configured,
    fileName,
    (attempts) =>
      platform === 'macos'
        ? `macOS viewer installer not found. Set RMM_VIEWER_MACOS_INSTALLER_PATH or place ${fileName} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
        : `Viewer installer not found. Set RMM_VIEWER_INSTALLER_PATH or place ${fileName} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
  );
}

async function resolveLinuxAgentBinaryPath(): Promise<string> {
  const configured = readString(process.env.RMM_LINUX_AGENT_BINARY_PATH);
  const fileName = readString(process.env.RMM_LINUX_AGENT_BINARY_FILENAME) || DEFAULT_LINUX_AGENT_BINARY_FILENAME;
  const attempts: string[] = [];
  for (const candidate of resolveLinuxAgentBinaryCandidates(configured, fileName)) {
    attempts.push(candidate);
    try {
      await fs.access(candidate);
      return candidate;
    } catch {
      // Keep trying other candidates.
    }
  }
  throw new HttpError(
    500,
    `Linux agent binary not found. Set RMM_LINUX_AGENT_BINARY_PATH or place ${fileName} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
  );
}

async function resolveMacosPackagePath(): Promise<string> {
  const configured = readString(process.env.RMM_MACOS_PACKAGE_PATH);
  const fileName = readString(process.env.RMM_MACOS_PACKAGE_FILENAME) || DEFAULT_MACOS_PACKAGE_FILENAME;
  const attempts: string[] = [];
  for (const candidate of resolveMacosPackageCandidates(configured, fileName)) {
    attempts.push(candidate);
    try {
      await fs.access(candidate);
      return candidate;
    } catch {
      // Keep trying other candidates.
    }
  }
  throw new HttpError(
    500,
    `macOS package not found. Set RMM_MACOS_PACKAGE_PATH or place ${fileName} in apps/installer/artifacts/<profile>. Attempted: ${attempts.join(', ')}`
  );
}

function getDownloadContentType(fileName: string): string {
  const extension = path.extname(fileName).toLowerCase();
  if (extension === '.msi') {
    return 'application/x-msi';
  }
  if (extension === '.exe') {
    return 'application/vnd.microsoft.portable-executable';
  }
  if (extension === '.pkg') {
    return 'application/octet-stream';
  }
  return 'application/octet-stream';
}

async function tryLoadViewerInstallerManifest(): Promise<ViewerInstallerManifest | null> {
  const configured = readString(process.env.RMM_VIEWER_INSTALLER_MANIFEST_PATH);
  for (const candidate of resolveArtifactCandidates(configured, DEFAULT_ARTIFACT_MANIFEST_FILENAME)) {
    try {
      const raw = await fs.readFile(candidate, 'utf8');
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object') {
        return parsed as ViewerInstallerManifest;
      }
    } catch {
      // Ignore unreadable manifests and fall back to file stat metadata.
    }
  }
  return null;
}

async function loadInstallerSfxStubBuffer(): Promise<Buffer> {
  const stubPath = await resolveInstallerSfxStubPath();
  const stat = await fs.stat(stubPath);
  if (
    !cachedInstallerSfxStub ||
    cachedInstallerSfxStubPath !== stubPath ||
    cachedInstallerSfxStubMtimeMs !== stat.mtimeMs ||
    cachedInstallerSfxStubSize !== stat.size
  ) {
    cachedInstallerSfxStub = await fs.readFile(stubPath);
    cachedInstallerSfxStubPath = stubPath;
    cachedInstallerSfxStubMtimeMs = stat.mtimeMs;
    cachedInstallerSfxStubSize = stat.size;
  }
  return Buffer.from(cachedInstallerSfxStub);
}

async function loadInstallerPayloadArchiveBuffer(): Promise<Buffer> {
  const archivePath = await resolveInstallerPayloadArchivePath();
  const stat = await fs.stat(archivePath);
  if (
    !cachedInstallerPayloadArchive ||
    cachedInstallerPayloadArchivePath !== archivePath ||
    cachedInstallerPayloadArchiveMtimeMs !== stat.mtimeMs ||
    cachedInstallerPayloadArchiveSize !== stat.size
  ) {
    cachedInstallerPayloadArchive = await fs.readFile(archivePath);
    cachedInstallerPayloadArchivePath = archivePath;
    cachedInstallerPayloadArchiveMtimeMs = stat.mtimeMs;
    cachedInstallerPayloadArchiveSize = stat.size;
  }
  return Buffer.from(cachedInstallerPayloadArchive);
}

async function loadLinuxAgentBinaryBuffer(): Promise<{ binaryPath: string; buffer: Buffer }> {
  const binaryPath = await resolveLinuxAgentBinaryPath();
  const stat = await fs.stat(binaryPath);
  if (
    !cachedLinuxAgentBinary ||
    cachedLinuxAgentBinaryPath !== binaryPath ||
    cachedLinuxAgentBinaryMtimeMs !== stat.mtimeMs ||
    cachedLinuxAgentBinarySize !== stat.size
  ) {
    cachedLinuxAgentBinary = await fs.readFile(binaryPath);
    cachedLinuxAgentBinaryPath = binaryPath;
    cachedLinuxAgentBinaryMtimeMs = stat.mtimeMs;
    cachedLinuxAgentBinarySize = stat.size;
  }
  return { binaryPath, buffer: Buffer.from(cachedLinuxAgentBinary) };
}

async function loadMacosPackageBuffer(): Promise<{ packagePath: string; buffer: Buffer }> {
  const packagePath = await resolveMacosPackagePath();
  const stat = await fs.stat(packagePath);
  if (
    !cachedMacosPackage ||
    cachedMacosPackagePath !== packagePath ||
    cachedMacosPackageMtimeMs !== stat.mtimeMs ||
    cachedMacosPackageSize !== stat.size
  ) {
    cachedMacosPackage = await fs.readFile(packagePath);
    cachedMacosPackagePath = packagePath;
    cachedMacosPackageMtimeMs = stat.mtimeMs;
    cachedMacosPackageSize = stat.size;
  }
  return { packagePath, buffer: Buffer.from(cachedMacosPackage) };
}

function assertInstallerConfigValue(value: string, fieldName: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new HttpError(500, `${fieldName} must not be empty`);
  }
  if (/[\x00-\x1F\x7F]/.test(trimmed)) {
    throw new HttpError(500, `${fieldName} must not contain control characters`);
  }
  return trimmed;
}

function escapeSfxQuotedValue(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function resolveInstallerPayloadExeName(): string {
  const configured = readString(process.env.RMM_INSTALLER_PAYLOAD_EXE_NAME) || DEFAULT_INSTALLER_PAYLOAD_EXE_NAME;
  const normalized = assertInstallerConfigValue(configured, 'Installer payload executable name');
  if (normalized.includes('/') || normalized.includes('\\')) {
    throw new HttpError(500, 'RMM_INSTALLER_PAYLOAD_EXE_NAME must be a file name, not a path');
  }
  return normalized;
}

function buildSfxConfig(token: string, serverUrl: string, payloadExeName: string): Buffer {
  const safeToken = escapeSfxQuotedValue(assertInstallerConfigValue(token, 'Enrollment token'));
  const safeServerUrl = escapeSfxQuotedValue(assertInstallerConfigValue(serverUrl, 'RMM server URL'));
  const safePayloadExe = escapeSfxQuotedValue(assertInstallerConfigValue(payloadExeName, 'Installer payload executable name'));
  const executeParameters = `EnrollmentToken=\\"${safeToken}\\" RmmServerUrl=\\"${safeServerUrl}\\"`;
  const lines = [
    ';!@Install@!UTF-8!',
    `ExecuteFile="${safePayloadExe}"`,
    `ExecuteParameters="${executeParameters}"`,
    `AutoInstall="${safePayloadExe} ${executeParameters}"`,
    ';!@InstallEnd@!',
    ''
  ];
  return Buffer.from(lines.join('\n'), 'utf8');
}

function buildScopedInstallerExe(stub: Buffer, config: Buffer, payloadArchive: Buffer): Buffer {
  return Buffer.concat([stub, config, payloadArchive]);
}

async function findFileByName(root: string, fileName: string): Promise<string | null> {
  const entries = await fs.readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const entryPath = path.join(root, entry.name);
    if (entry.isFile() && entry.name === fileName) {
      return entryPath;
    }
    if (entry.isDirectory()) {
      const found = await findFileByName(entryPath, fileName);
      if (found) return found;
    }
  }
  return null;
}

function buildScopedMacosAgentEnvBlock(token: string, serverUrl: string): string {
  const agentTokenLiteral = shellSingleQuote(assertInstallerConfigValue(token, 'Enrollment token'));
  const serverUrlLiteral = shellSingleQuote(normalizeAgentWebSocketUrl(serverUrl));

  return `OLD_AGENT_TOKEN=""
OLD_AGENT_ID_PATH="$STATE_DIR/talos_worker_id.txt"
if [ -f "$AGENT_ENV_PATH" ]; then
  OLD_AGENT_TOKEN="$(sed -n 's/^RMM_AGENT_TOKEN=//p' "$AGENT_ENV_PATH" | tail -n 1 | sed "s/^'//;s/'$//")"
  OLD_AGENT_ID_PATH="$(sed -n 's/^RMM_AGENT_ID_PATH=//p' "$AGENT_ENV_PATH" | tail -n 1 | sed "s/^'//;s/'$//")"
  if [ -z "$OLD_AGENT_ID_PATH" ]; then
    OLD_AGENT_ID_PATH="$STATE_DIR/talos_worker_id.txt"
  fi
fi
if [ -n "$OLD_AGENT_TOKEN" ] && [ "$OLD_AGENT_TOKEN" != ${agentTokenLiteral} ]; then
  rm -f "$OLD_AGENT_ID_PATH"
fi

cat > "$AGENT_ENV_PATH" <<'EOF_AGENT_ENV'
RMM_SERVER_URL=${serverUrlLiteral}
RMM_AGENT_TOKEN=${agentTokenLiteral}
RMM_AGENT_ID_PATH='/Library/Application Support/Talos/talos_worker_id.txt'
RMM_INVENTORY_INTERVAL_SECS=30
RMM_RECONNECT_MAX_SECS=30
RMM_COMMAND_TIMEOUT_SECS=120
RUST_LOG=info
EOF_AGENT_ENV
chmod 0600 "$AGENT_ENV_PATH"`;
}

function buildScopedMacosSupervisorEnvBlock(updateBaseUrl: string): string {
  const updateBaseUrlLiteral = shellSingleQuote(assertInstallerConfigValue(updateBaseUrl, 'RMM update base URL'));

  return `cat > "$SUPERVISOR_ENV_PATH" <<'EOF_SUPERVISOR_ENV'
RMM_UPDATE_BASE_URL=${updateBaseUrlLiteral}
RMM_UPDATE_CHANNEL=stable
RMM_WORKER_INSTALL_DIR='/Library/Talos/Worker'
RMM_WORKER_ENV_FILE='/Library/Preferences/Talos/rmm-agent.env'
RMM_WORKER_VERSION_PATH='/Library/Application Support/Talos/worker.version'
RMM_WORKER_SERVICE_NAME=com.talos.talos-worker
RMM_SUPERVISOR_SERVICE_NAME=com.talos.talos-supervisor
RMM_SUPERVISOR_UPDATE_INTERVAL_SECS=86400
RMM_SUPERVISOR_MONITOR_INTERVAL_SECS=60
RUST_LOG=info
EOF_SUPERVISOR_ENV
chmod 0600 "$SUPERVISOR_ENV_PATH"`;
}

export function stampMacosPackagePostinstall(
  script: string,
  params: { token: string; serverUrl: string; updateBaseUrl: string }
): string {
  const agentEnvBlockPattern =
    /if \[ ! -f "\$AGENT_ENV_PATH" \]; then\n[\s\S]*?\nfi\n(?=\nif \[ ! -f "\$SUPERVISOR_ENV_PATH" \]; then)/;
  const supervisorEnvBlockPattern =
    /if \[ ! -f "\$SUPERVISOR_ENV_PATH" \]; then\n[\s\S]*?\nfi\n/;

  if (!agentEnvBlockPattern.test(script) || !supervisorEnvBlockPattern.test(script)) {
    throw new HttpError(500, 'macOS package postinstall script does not contain expected Talos env blocks');
  }

  return script
    .replace(agentEnvBlockPattern, `${buildScopedMacosAgentEnvBlock(params.token, params.serverUrl)}\n`)
    .replace(supervisorEnvBlockPattern, `${buildScopedMacosSupervisorEnvBlock(params.updateBaseUrl)}\n`);
}

async function buildScopedMacosPackage(
  packagePath: string,
  params: { token: string; serverUrl: string; updateBaseUrl: string }
): Promise<Buffer> {
  const tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'talos-macos-pkg-'));
  const expandedDir = path.join(tmpRoot, 'expanded');
  const outputPath = path.join(tmpRoot, 'Talos.Agent.scoped.pkg');

  try {
    await execFileAsync('pkgutil', ['--expand', packagePath, expandedDir]);
    const postinstallPath = await findFileByName(expandedDir, 'postinstall');
    if (!postinstallPath) {
      throw new HttpError(500, 'macOS package postinstall script not found');
    }

    const originalPostinstall = await fs.readFile(postinstallPath, 'utf8');
    const stampedPostinstall = stampMacosPackagePostinstall(originalPostinstall, params);
    await fs.writeFile(postinstallPath, stampedPostinstall, { mode: 0o755 });
    await execFileAsync('pkgutil', ['--flatten', expandedDir, outputPath]);
    return await fs.readFile(outputPath);
  } finally {
    await fs.rm(tmpRoot, { recursive: true, force: true });
  }
}

function resolveInstallerServerUrl(): string {
  const value = readString(process.env.RMM_INSTALLER_SERVER_URL, process.env.RMM_SERVER_URL);
  if (!value) {
    throw new HttpError(500, 'RMM_INSTALLER_SERVER_URL or RMM_SERVER_URL must be configured');
  }
  return assertInstallerConfigValue(value, 'RMM server URL');
}

function buildRequestOrigin(req: Request): string {
  const configured = readString(
    process.env.RMM_INSTALLER_PUBLIC_API_URL,
    process.env.API_PUBLIC_URL,
    process.env.PUBLIC_API_URL
  );
  return getTrustedRequestOrigin(
    req,
    configured,
    'RMM_INSTALLER_PUBLIC_API_URL/API_PUBLIC_URL/PUBLIC_API_URL',
  );
}

function buildFrontendOrigin(req: Request): string {
  const configured = readString(
    process.env.RMM_INSTALLER_PUBLIC_FRONTEND_URL,
    process.env.FRONTEND_PUBLIC_URL,
    process.env.PUBLIC_APP_URL,
    process.env.APP_PUBLIC_URL
  );
  if (configured) {
    return getTrustedRequestOrigin(
      req,
      configured,
      'RMM_INSTALLER_PUBLIC_FRONTEND_URL/FRONTEND_PUBLIC_URL/PUBLIC_APP_URL/APP_PUBLIC_URL',
    );
  }

  const apiOrigin = buildRequestOrigin(req);
  return apiOrigin.replace('://api.', '://');
}

function buildAbsoluteUrl(req: Request, pathnameWithQuery: string): string {
  if (/^https?:\/\//i.test(pathnameWithQuery)) {
    return pathnameWithQuery;
  }
  const normalizedPath = pathnameWithQuery.startsWith('/') ? pathnameWithQuery : `/${pathnameWithQuery}`;
  return `${buildRequestOrigin(req)}${normalizedPath}`;
}

function shellSingleQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function buildLinuxInstallScriptPath(token: string, serverUrl: string): string {
  const params = new URLSearchParams({
    token,
    serverUrl
  });
  return `/rmm/installers/linux/install.sh?${params.toString()}`;
}

function buildLinuxInstallCommand(scriptUrl: string): string {
  return `curl -fsSL ${shellSingleQuote(scriptUrl)} | sudo sh`;
}

function buildMacosInstallCommand(scriptUrl: string): string {
  return `curl -fsSL ${shellSingleQuote(scriptUrl)} | sudo bash`;
}

function buildShortLinuxInstallPath(code: string): string {
  return `/${code}`;
}

function buildShortLinuxInstallUrl(req: any, code: string): string {
  return `${buildFrontendOrigin(req)}${buildShortLinuxInstallPath(code)}`;
}

function buildMacosInstallScriptPath(token: string, serverUrl: string): string {
  const params = new URLSearchParams({ token, serverUrl });
  return `/rmm/installers/macos/install.sh?${params.toString()}`;
}

function buildShortMacosInstallPath(code: string): string {
  return `/macos/${code}`;
}

function buildShortMacosInstallUrl(req: any, code: string): string {
  return `${buildFrontendOrigin(req)}${buildShortMacosInstallPath(code)}`;
}

function buildLinuxScriptFilename(profile: InstallerProfileWithNames): string {
  const scopeType = fromPrismaScopeType(profile.scopeType);
  const scopeId = profile.siteId || profile.customerId || profile.organizationId;
  return `talos-${scopeType}-${sanitizeFilenamePart(scopeId)}-linux-install.sh`;
}

function buildMacosScriptFilename(profile: InstallerProfileWithNames): string {
  const scopeType = fromPrismaScopeType(profile.scopeType);
  const scopeId = profile.siteId || profile.customerId || profile.organizationId;
  return `talos-${scopeType}-${sanitizeFilenamePart(scopeId)}-macos-install.sh`;
}

function normalizeAgentWebSocketUrl(value: string): string {
  const trimmed = assertInstallerConfigValue(value, 'RMM server URL');
  const parsed = new URL(trimmed);
  if (parsed.protocol === 'https:') parsed.protocol = 'wss:';
  if (parsed.protocol === 'http:') parsed.protocol = 'ws:';
  if (parsed.protocol !== 'wss:' && parsed.protocol !== 'ws:') {
    throw new HttpError(500, 'RMM server URL must use https, http, wss, or ws');
  }
  if (!parsed.pathname || parsed.pathname === '/') {
    parsed.pathname = '/agent/ws';
  }
  return parsed.toString();
}

async function buildMacosPackageInfo(req?: any) {
  const packagePath = await resolveMacosPackagePath();
  const packageStat = await fs.stat(packagePath);
  const packageBuffer = await fs.readFile(packagePath);
  const sha256 = crypto.createHash('sha256').update(packageBuffer).digest('hex');
  const fileName = readString(process.env.RMM_MACOS_PACKAGE_FILENAME) || DEFAULT_MACOS_PACKAGE_FILENAME;
  const downloadPath = '/rmm/installers/macos/package/download';

  return {
    available: true,
    downloadPath,
    downloadUrl: req ? buildAbsoluteUrl(req, downloadPath) : null,
    package: { fileName, sizeBytes: packageStat.size, sha256 },
    packagePath
  };
}

async function resolveMacosPackageInstallUrl(req: any): Promise<string> {
  const configuredPackageUrl = readString(process.env.RMM_MACOS_PACKAGE_URL);
  if (configuredPackageUrl) {
    return configuredPackageUrl;
  }
  await buildMacosPackageInfo(req);
  return buildAbsoluteUrl(req, '/rmm/installers/macos/package/download');
}

function sanitizeFilenamePart(value: string): string {
  const normalized = value.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');
  return normalized || 'scope';
}

async function getOrCreateUnassignedTx(tx: Prisma.TransactionClient, organizationId: string) {
  const id = unassignedCustomerId(organizationId);
  const existing = await tx.customer.findUnique({ where: { id } });
  if (existing) return existing;

  return tx.customer.create({
    data: {
      id,
      organizationId,
      name: 'Unassigned',
      description: 'Default holding customer for unassigned devices.',
      isUnassigned: true
    }
  });
}

async function resolveScope(
  membership: MembershipWithOrg,
  scopeType: InstallerScopeType,
  customerIdRaw: string | null,
  siteIdRaw: string | null
): Promise<ScopeResolution> {
  if (scopeType === 'organization') {
    return {
      scopeType: 'ORGANIZATION',
      organizationId: membership.organizationId,
      customerId: null,
      siteId: null
    };
  }

  if (scopeType === 'customer') {
    const customerId = readString(customerIdRaw);
    if (!customerId) {
      throw new HttpError(400, 'customerId is required for customer scope');
    }
    const customer = await prisma.customer.findFirst({
      where: {
        id: customerId,
        organizationId: membership.organizationId
      },
      select: { id: true }
    });
    if (!customer) {
      throw new HttpError(404, 'Customer not found');
    }
    return {
      scopeType: 'CUSTOMER',
      organizationId: membership.organizationId,
      customerId: customer.id,
      siteId: null
    };
  }

  const siteId = readString(siteIdRaw);
  if (!siteId) {
    throw new HttpError(400, 'siteId is required for site scope');
  }
  const site = await prisma.rmmSite.findFirst({
    where: {
      id: siteId,
      customer: { organizationId: membership.organizationId }
    },
    select: {
      id: true,
      customerId: true
    }
  });
  if (!site) {
    throw new HttpError(404, 'Site not found');
  }
  return {
    scopeType: 'SITE',
    organizationId: membership.organizationId,
    customerId: site.customerId,
    siteId: site.id
  };
}

async function issueEnrollmentToken(
  tx: Prisma.TransactionClient,
  params: {
    profileId: string;
    organizationId: string;
    customerId: string | null;
    siteId: string | null;
    issuedBy: string;
    expiresAt: Date | null;
    maxUses: number | null;
  }
) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const rawToken = generateRegistrationToken();
    const tokenHash = hashToken(rawToken);
    const tokenPrefix = rawToken.slice(0, 12);

    try {
      const token = await tx.rmmInstallerEnrollmentToken.create({
        data: {
          profileId: params.profileId,
          organizationId: params.organizationId,
          customerId: params.customerId,
          siteId: params.siteId,
          tokenHash,
          tokenPrefix,
          expiresAt: params.expiresAt,
          maxUses: params.maxUses,
          issuedBy: params.issuedBy
        }
      });
      return { rawToken, token };
    } catch (error) {
      if (error instanceof Prisma.PrismaClientKnownRequestError && error.code === 'P2002') {
        continue;
      }
      throw error;
    }
  }

  throw new Error('failed to generate unique enrollment token');
}

function profileToResponse(profile: {
  id: string;
  name: string;
  scopeType: PrismaInstallerScopeType;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  expiresAt: Date | null;
  maxUses: number | null;
  revokedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
  customer?: { id: string; name: string } | null;
  site?: { id: string; name: string } | null;
}) {
  return {
    id: profile.id,
    name: profile.name,
    scopeType: fromPrismaScopeType(profile.scopeType),
    organizationId: profile.organizationId,
    customerId: profile.customerId,
    siteId: profile.siteId,
    customerName: profile.customer?.name ?? null,
    siteName: profile.site?.name ?? null,
    expiresAt: profile.expiresAt ? profile.expiresAt.toISOString() : null,
    maxUses: profile.maxUses,
    revokedAt: profile.revokedAt ? profile.revokedAt.toISOString() : null,
    createdAt: profile.createdAt.toISOString(),
    updatedAt: profile.updatedAt.toISOString()
  };
}

type InstallerProfileWithNames = {
  id: string;
  name: string;
  scopeType: PrismaInstallerScopeType;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  expiresAt: Date | null;
  maxUses: number | null;
  revokedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
  customer: { id: string; name: string } | null;
  site: { id: string; name: string } | null;
};

type IssuedInstallerDownload = {
  profile: InstallerProfileWithNames;
  issued: Awaited<ReturnType<typeof issueEnrollmentToken>>;
};

function buildInstallerExeFilename(profile: InstallerProfileWithNames): string {
  const scopeType = fromPrismaScopeType(profile.scopeType);
  const scopeId = profile.siteId || profile.customerId || profile.organizationId;
  return `talos-${scopeType}-${sanitizeFilenamePart(scopeId)}-installer.exe`;
}

function buildInstallerMacosPackageFilename(profile: InstallerProfileWithNames): string {
  const scopeType = fromPrismaScopeType(profile.scopeType);
  const scopeId = profile.siteId || profile.customerId || profile.organizationId;
  return `talos-${scopeType}-${sanitizeFilenamePart(scopeId)}-agent.pkg`;
}

async function buildViewerInstallerInfo(platform: ViewerInstallerPlatform = 'windows') {
  const installerPath = await resolveViewerInstallerPath(platform);
  const installerStat = await fs.stat(installerPath);
  const manifest = await tryLoadViewerInstallerManifest();
  const manifestInstaller =
    platform === 'macos'
      ? manifest?.viewer?.macosInstaller || manifest?.viewer?.pkgMacos
      : manifest?.viewer?.installer;
  const installerBuffer = manifestInstaller?.sha256 ? null : await fs.readFile(installerPath);
  const sha256 =
    manifestInstaller?.sha256 ||
    crypto.createHash('sha256').update(installerBuffer as Buffer).digest('hex');
  const downloadPath =
    platform === 'macos'
      ? '/rmm/installers/viewer/download?platform=macos'
      : '/rmm/installers/viewer/download?platform=windows';

  return {
    available: true,
    platform,
    profile: manifest?.profile ?? null,
    generatedAtUtc: manifest?.generatedAtUtc ?? null,
    downloadPath,
    installer: {
      fileName: manifestInstaller?.fileName || path.basename(installerPath),
      sizeBytes:
        typeof manifestInstaller?.sizeBytes === 'number' ? manifestInstaller.sizeBytes : installerStat.size,
      sha256
    },
    installerPath,
  };
}

async function buildLinuxAgentInfo(req?: any) {
  const binaryPath = await resolveLinuxAgentBinaryPath();
  const binaryStat = await fs.stat(binaryPath);
  const binaryBuffer = await fs.readFile(binaryPath);
  const sha256 = crypto.createHash('sha256').update(binaryBuffer).digest('hex');
  const configuredFileName = readString(process.env.RMM_LINUX_AGENT_BINARY_FILENAME);
  const fileName = configuredFileName || DEFAULT_LINUX_AGENT_BINARY_FILENAME;
  const downloadPath = '/rmm/installers/linux/agent/download';

  return {
    available: true,
    downloadPath,
    downloadUrl: req ? buildAbsoluteUrl(req, downloadPath) : null,
    binary: {
      fileName,
      sizeBytes: binaryStat.size,
      sha256
    },
    binaryPath
  };
}

async function assertLinuxInstallTokenCanRender(rawToken: string): Promise<void> {
  const tokenHash = hashToken(rawToken);
  const token = await prisma.rmmInstallerEnrollmentToken.findUnique({
    where: { tokenHash },
    include: {
      profile: { select: { revokedAt: true } }
    }
  });

  if (!token) {
    throw new HttpError(401, 'Invalid enrollment token');
  }
  if (token.revokedAt || token.profile.revokedAt) {
    throw new HttpError(403, 'Enrollment token has been revoked');
  }
  if (token.expiresAt && token.expiresAt.getTime() <= Date.now()) {
    throw new HttpError(403, 'Enrollment token has expired');
  }
  if (token.maxUses !== null && token.usedCount >= token.maxUses) {
    throw new HttpError(403, 'Enrollment token maximum uses reached');
  }
}

type ShortLinuxInstallLink = {
  code: string;
  expiresAt: Date;
};

async function createShortLinuxInstallLink(
  tx: Prisma.TransactionClient,
  params: {
    tokenId: string;
    profileId: string;
    organizationId: string;
    customerId: string | null;
    siteId: string | null;
    serverUrl: string;
    issuedBy: string;
    rawToken: string;
  }
): Promise<ShortLinuxInstallLink> {
  const expiresAt = new Date(Date.now() + SHORT_LINUX_INSTALL_TTL_DAYS * 24 * 60 * 60 * 1000);
  await tx.$executeRaw`DELETE FROM public.rmm_installer_short_link WHERE expires_at <= NOW()`;

  for (let attempt = 0; attempt < 8; attempt += 1) {
    const code = generateShortInstallCode();
    const inserted = await tx.$executeRaw`
        INSERT INTO public.rmm_installer_short_link (
          code,
          token_id,
          profile_id,
          organization_id,
          customer_id,
          site_id,
          registration_token,
          server_url,
          issued_by,
          expires_at
        )
        VALUES (
          ${code},
          ${params.tokenId},
          ${params.profileId},
          ${params.organizationId},
          ${params.customerId},
          ${params.siteId},
          ${params.rawToken},
          ${params.serverUrl},
          ${params.issuedBy},
          ${expiresAt}
        )
        ON CONFLICT (code) DO NOTHING
      `;
    if (inserted === 1) {
      return { code, expiresAt };
    }
  }

  throw new Error('failed to generate unique short Linux install code');
}

async function resolveShortLinuxInstallLink(code: string): Promise<{ token: string; serverUrl: string }> {
  const normalizedCode = code.trim().toLowerCase();
  if (!/^[a-z0-9]{8}$/.test(normalizedCode)) {
    throw new HttpError(404, 'Short installer link not found');
  }

  const rows = await prisma.$queryRaw<
    Array<{
      tokenHash: string;
      registrationToken: string;
      serverUrl: string;
      tokenRevokedAt: Date | null;
      profileRevokedAt: Date | null;
      tokenExpiresAt: Date | null;
      shortExpiresAt: Date;
      maxUses: number | null;
      usedCount: number;
    }>
  >`
    SELECT
      t.token_hash AS "tokenHash",
      s.registration_token AS "registrationToken",
      s.server_url AS "serverUrl",
      t.revoked_at AS "tokenRevokedAt",
      p.revoked_at AS "profileRevokedAt",
      t.expires_at AS "tokenExpiresAt",
      s.expires_at AS "shortExpiresAt",
      t.max_uses AS "maxUses",
      t.used_count AS "usedCount"
    FROM public.rmm_installer_short_link s
    JOIN public.rmm_installer_enrollment_token t ON t.id = s.token_id
    JOIN public.rmm_installer_profile p ON p.id = s.profile_id
    WHERE s.code = ${normalizedCode}
    LIMIT 1
  `;

  const link = rows[0];
  if (!link) {
    throw new HttpError(404, 'Short installer link not found');
  }
  if (link.shortExpiresAt.getTime() <= Date.now()) {
    throw new HttpError(410, 'Short installer link has expired');
  }
  if (link.tokenRevokedAt || link.profileRevokedAt) {
    throw new HttpError(403, 'Enrollment token has been revoked');
  }
  if (link.tokenExpiresAt && link.tokenExpiresAt.getTime() <= Date.now()) {
    throw new HttpError(403, 'Enrollment token has expired');
  }
  if (link.maxUses !== null && link.usedCount >= link.maxUses) {
    throw new HttpError(403, 'Enrollment token maximum uses reached');
  }

  if (hashToken(link.registrationToken) !== link.tokenHash) {
    throw new HttpError(500, 'Short installer link token mismatch');
  }

  return { token: link.registrationToken, serverUrl: link.serverUrl };
}

function buildLinuxInstallScript(params: {
  token: string;
  serverUrl: string;
  binaryUrl: string;
  updateBaseUrl: string;
}): string {
  const tokenLiteral = shellSingleQuote(assertInstallerConfigValue(params.token, 'Enrollment token'));
  const serverUrlLiteral = shellSingleQuote(assertInstallerConfigValue(params.serverUrl, 'RMM server URL'));
  const binaryUrlLiteral = shellSingleQuote(assertInstallerConfigValue(params.binaryUrl, 'Linux supervisor binary URL'));
  const updateBaseUrlLiteral = shellSingleQuote(assertInstallerConfigValue(params.updateBaseUrl, 'RMM update base URL'));

  return `#!/usr/bin/env sh
set -eu

AGENT_TOKEN=${tokenLiteral}
SERVER_URL=${serverUrlLiteral}
SUPERVISOR_BINARY_URL=${binaryUrlLiteral}
UPDATE_BASE_URL=${updateBaseUrlLiteral}
LEGACY_INSTALL_ROOT="/usr/local/bin"
SUPERVISOR_INSTALL_DIR="/opt/talos/supervisor"
WORKER_INSTALL_DIR="/opt/talos/worker"
CONFIG_DIR="/etc/talos"
LOG_DIR="/var/log/talos"
STATE_DIR="/var/lib/talos"
UPDATE_DIR="$STATE_DIR/updates"
SYSTEMD_DIR="/etc/systemd/system"
LEGACY_SERVICE_NAME="talos-rmm-agent.service"
WORKER_SERVICE_NAME="talos-worker.service"
SUPERVISOR_SERVICE_NAME="talos-supervisor.service"
LEGACY_BIN_PATH="$LEGACY_INSTALL_ROOT/talos-rmm-agent"
SUPERVISOR_BIN_PATH="$SUPERVISOR_INSTALL_DIR/talos_supervisor"
ENV_PATH="$CONFIG_DIR/rmm-agent.env"
SUPERVISOR_ENV_PATH="$CONFIG_DIR/talos-supervisor.env"

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this installer as root, for example: curl -fsSL <url> | sudo sh" >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "systemd is required for the Talos Linux agent service." >&2
  exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ;;
  *)
    echo "Unsupported architecture: $ARCH. The current Linux MVP installer supports x86_64 only." >&2
    exit 1
    ;;
esac

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$SUPERVISOR_BINARY_URL" -o "$TMP_DIR/talos_supervisor"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$TMP_DIR/talos_supervisor" "$SUPERVISOR_BINARY_URL"
else
  echo "curl or wget is required to download the Talos Linux supervisor." >&2
  exit 1
fi

chmod 0755 "$TMP_DIR/talos_supervisor"

for unit in "$SUPERVISOR_SERVICE_NAME" "$WORKER_SERVICE_NAME" "$LEGACY_SERVICE_NAME"; do
  if systemctl list-unit-files "$unit" >/dev/null 2>&1 || systemctl status "$unit" >/dev/null 2>&1; then
    systemctl disable --now "$unit" >/dev/null 2>&1 || true
  fi
  rm -f "$SYSTEMD_DIR/$unit"
  systemctl reset-failed "$unit" >/dev/null 2>&1 || true
done

rm -f "$LEGACY_BIN_PATH" "$SUPERVISOR_BIN_PATH"
install -d -m 0755 "$LEGACY_INSTALL_ROOT" "$SUPERVISOR_INSTALL_DIR" "$WORKER_INSTALL_DIR" "$CONFIG_DIR" "$LOG_DIR" "$SYSTEMD_DIR"
install -d -m 0700 "$STATE_DIR" "$UPDATE_DIR"
install -m 0755 "$TMP_DIR/talos_supervisor" "$SUPERVISOR_BIN_PATH"

OLD_AGENT_TOKEN=""
OLD_AGENT_ID_PATH="/etc/talos/rmm_agent_id.txt"
if [ -f "$ENV_PATH" ]; then
  OLD_AGENT_TOKEN="$(sed -n 's/^RMM_AGENT_TOKEN=//p' "$ENV_PATH" | tail -n 1)"
  OLD_AGENT_ID_PATH="$(sed -n 's/^RMM_AGENT_ID_PATH=//p' "$ENV_PATH" | tail -n 1)"
  if [ -z "$OLD_AGENT_ID_PATH" ]; then
    OLD_AGENT_ID_PATH="/etc/talos/rmm_agent_id.txt"
  fi
fi
if [ -n "$OLD_AGENT_TOKEN" ] && [ "$OLD_AGENT_TOKEN" != "$AGENT_TOKEN" ]; then
  rm -f "$OLD_AGENT_ID_PATH"
fi

{
  printf 'RMM_SERVER_URL=%s\\n' "$SERVER_URL"
  printf 'RMM_AGENT_TOKEN=%s\\n' "$AGENT_TOKEN"
  printf 'RMM_AGENT_ID_PATH=/etc/talos/rmm_agent_id.txt\\n'
  printf 'RMM_INVENTORY_INTERVAL_SECS=30\\n'
  printf 'RMM_RECONNECT_MAX_SECS=30\\n'
  printf 'RMM_COMMAND_TIMEOUT_SECS=120\\n'
  printf 'RMM_SHELL_USER=talos\\n'
  printf 'RUST_LOG=info\\n'
} > "$ENV_PATH"
chmod 0600 "$ENV_PATH"

{
  printf 'RMM_UPDATE_BASE_URL=%s\\n' "$UPDATE_BASE_URL"
  printf 'RMM_UPDATE_CHANNEL=stable\\n'
  printf 'RMM_WORKER_INSTALL_DIR=%s\\n' "$WORKER_INSTALL_DIR"
  printf 'RMM_WORKER_ENV_FILE=%s\\n' "$ENV_PATH"
  printf 'RMM_WORKER_VERSION_PATH=%s\\n' "$STATE_DIR/worker.version"
  printf 'RMM_WORKER_SERVICE_NAME=talos-worker\\n'
  printf 'RMM_SUPERVISOR_SERVICE_NAME=talos-supervisor\\n'
  printf 'RMM_SUPERVISOR_STARTUP_JITTER_SECS=0\\n'
  printf 'RMM_SUPERVISOR_UPDATE_INTERVAL_SECS=86400\\n'
  printf 'RMM_SUPERVISOR_MONITOR_INTERVAL_SECS=60\\n'
  printf 'RUST_LOG=info\\n'
} > "$SUPERVISOR_ENV_PATH"
chmod 0600 "$SUPERVISOR_ENV_PATH"

cat > "$SYSTEMD_DIR/$SUPERVISOR_SERVICE_NAME" <<'EOF_SERVICE'
[Unit]
Description=Talos Supervisor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/talos/talos-supervisor.env
ExecStart=/opt/talos/supervisor/talos_supervisor
Restart=always
RestartSec=10
User=root
Group=root
KillMode=process
NoNewPrivileges=false
PrivateTmp=true
ProtectSystem=full
ProtectHome=read-only
ReadWritePaths=/opt/talos /etc/talos /etc/systemd/system /var/lib/talos /var/log/talos /tmp

[Install]
WantedBy=multi-user.target
EOF_SERVICE

systemctl daemon-reload
systemctl enable --now "$SUPERVISOR_SERVICE_NAME"

echo "Talos Linux supervisor installed and started."
echo "The supervisor will install and monitor talos-worker.service."
systemctl --no-pager --full status "$SUPERVISOR_SERVICE_NAME" || true
`;
}

function buildMacosInstallScript(params: {
  token: string;
  serverUrl: string;
  packageUrl: string;
  updateBaseUrl: string;
}): string {
  const tokenLiteral = shellSingleQuote(assertInstallerConfigValue(params.token, 'Enrollment token'));
  const serverUrlLiteral = shellSingleQuote(normalizeAgentWebSocketUrl(params.serverUrl));
  const packageUrlLiteral = shellSingleQuote(assertInstallerConfigValue(params.packageUrl, 'macOS package URL'));
  const updateBaseUrlLiteral = shellSingleQuote(assertInstallerConfigValue(params.updateBaseUrl, 'RMM update base URL'));

  return `#!/bin/bash
set -euo pipefail

AGENT_TOKEN=${tokenLiteral}
SERVER_URL=${serverUrlLiteral}
PACKAGE_URL=${packageUrlLiteral}
UPDATE_BASE_URL=${updateBaseUrlLiteral}
WORKER_INSTALL_DIR="/Library/Talos/Worker"
CONFIG_DIR="/Library/Preferences/Talos"
STATE_DIR="/Library/Application Support/Talos"
LOG_DIR="/Library/Logs/Talos"
UPDATE_DIR="$STATE_DIR/updates"
ENV_PATH="$CONFIG_DIR/rmm-agent.env"
SUPERVISOR_ENV_PATH="$CONFIG_DIR/talos-supervisor.env"
WORKER_VERSION_PATH="$STATE_DIR/worker.version"
AGENT_ID_PATH="$STATE_DIR/talos_worker_id.txt"
SUPERVISOR_SERVICE_LABEL="com.talos.talos-supervisor"
WORKER_SERVICE_LABEL="com.talos.talos-worker"
SUPERVISOR_PLIST_PATH="/Library/LaunchDaemons/$SUPERVISOR_SERVICE_LABEL.plist"
WORKER_PLIST_PATH="/Library/LaunchDaemons/$WORKER_SERVICE_LABEL.plist"

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this installer as root, for example: curl -fsSL <url> | sudo bash" >&2
  exit 1
fi

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This installer is for macOS only." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64|x86_64) ;;
  *)
    echo "Unsupported macOS architecture: $(uname -m)." >&2
    exit 1
    ;;
esac

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$PACKAGE_URL" -o "$TMP_DIR/Talos.Agent.pkg"
elif command -v python3 >/dev/null 2>&1; then
  python3 - "$PACKAGE_URL" "$TMP_DIR/Talos.Agent.pkg" <<'PY_DOWNLOAD'
import sys
import urllib.request
urllib.request.urlretrieve(sys.argv[1], sys.argv[2])
PY_DOWNLOAD
else
  echo "curl or python3 is required to download the Talos macOS package." >&2
  exit 1
fi

launchctl bootout "system/$SUPERVISOR_SERVICE_LABEL" >/dev/null 2>&1 || true
launchctl bootout "system/$WORKER_SERVICE_LABEL" >/dev/null 2>&1 || true
installer -pkg "$TMP_DIR/Talos.Agent.pkg" -target /

install -d -m 0755 "$WORKER_INSTALL_DIR" "$CONFIG_DIR" "$LOG_DIR"
install -d -m 0700 "$STATE_DIR" "$UPDATE_DIR"

OLD_AGENT_TOKEN=""
OLD_AGENT_ID_PATH="$AGENT_ID_PATH"
if [ -f "$ENV_PATH" ]; then
  OLD_AGENT_TOKEN="$(sed -n 's/^RMM_AGENT_TOKEN=//p' "$ENV_PATH" | tail -n 1 | sed "s/^'//;s/'$//")"
  OLD_AGENT_ID_PATH="$(sed -n 's/^RMM_AGENT_ID_PATH=//p' "$ENV_PATH" | tail -n 1 | sed "s/^'//;s/'$//")"
  if [ -z "$OLD_AGENT_ID_PATH" ]; then
    OLD_AGENT_ID_PATH="$AGENT_ID_PATH"
  fi
fi
if [ -n "$OLD_AGENT_TOKEN" ] && [ "$OLD_AGENT_TOKEN" != "$AGENT_TOKEN" ]; then
  rm -f "$OLD_AGENT_ID_PATH"
fi

{
  printf 'RMM_SERVER_URL=%q\\n' "$SERVER_URL"
  printf 'RMM_AGENT_TOKEN=%q\\n' "$AGENT_TOKEN"
  printf 'RMM_AGENT_ID_PATH=%q\\n' "$AGENT_ID_PATH"
  printf 'RMM_INVENTORY_INTERVAL_SECS=30\\n'
  printf 'RMM_RECONNECT_MAX_SECS=30\\n'
  printf 'RMM_COMMAND_TIMEOUT_SECS=120\\n'
  printf 'RUST_LOG=info\\n'
} > "$ENV_PATH"
chmod 0600 "$ENV_PATH"

{
  printf 'RMM_UPDATE_BASE_URL=%q\\n' "$UPDATE_BASE_URL"
  printf 'RMM_UPDATE_CHANNEL=stable\\n'
  printf 'RMM_WORKER_INSTALL_DIR=%q\\n' "$WORKER_INSTALL_DIR"
  printf 'RMM_WORKER_ENV_FILE=%q\\n' "$ENV_PATH"
  printf 'RMM_WORKER_VERSION_PATH=%q\\n' "$WORKER_VERSION_PATH"
  printf 'RMM_WORKER_SERVICE_NAME=%q\\n' "$WORKER_SERVICE_LABEL"
  printf 'RMM_SUPERVISOR_SERVICE_NAME=%q\\n' "$SUPERVISOR_SERVICE_LABEL"
  printf 'RMM_SUPERVISOR_UPDATE_INTERVAL_SECS=86400\\n'
  printf 'RMM_SUPERVISOR_MONITOR_INTERVAL_SECS=60\\n'
  printf 'RUST_LOG=info\\n'
} > "$SUPERVISOR_ENV_PATH"
chmod 0600 "$SUPERVISOR_ENV_PATH"

chown root:wheel "$SUPERVISOR_PLIST_PATH"
chmod 0644 "$SUPERVISOR_PLIST_PATH"

launchctl bootout "system/$WORKER_SERVICE_LABEL" >/dev/null 2>&1 || true
launchctl bootout "system/$SUPERVISOR_SERVICE_LABEL" >/dev/null 2>&1 || true
launchctl bootstrap system "$SUPERVISOR_PLIST_PATH"
launchctl enable "system/$SUPERVISOR_SERVICE_LABEL"
launchctl kickstart -k "system/$SUPERVISOR_SERVICE_LABEL"

echo "Talos macOS package installed and supervisor started."
echo "The supervisor will install and monitor $WORKER_SERVICE_LABEL."
launchctl print "system/$SUPERVISOR_SERVICE_LABEL" >/dev/null 2>&1 || true
`;
}

async function issueInstallerDownloadForProfile(
  req: any,
  membership: MembershipWithOrg,
  profileId: string,
  expiresAtOverride: Date | null,
  maxUsesOverride: number | null,
  deliveryKind: 'json' | 'exe' | 'linux-script' | 'macos-script' | 'macos-pkg'
): Promise<IssuedInstallerDownload> {
  return prisma.$transaction(async (tx) => {
    const profile = await tx.rmmInstallerProfile.findFirst({
      where: {
        id: profileId,
        organizationId: membership.organizationId
      },
      include: {
        customer: { select: { id: true, name: true } },
        site: { select: { id: true, name: true } }
      }
    });

    if (!profile) {
      throw new HttpError(404, 'Installer profile not found');
    }
    if (profile.revokedAt) {
      throw new HttpError(409, 'Installer profile is revoked');
    }

    const issued = await issueEnrollmentToken(tx, {
      profileId: profile.id,
      organizationId: profile.organizationId,
      customerId: profile.customerId,
      siteId: profile.siteId,
      issuedBy: membership.userId,
      expiresAt: expiresAtOverride ?? profile.expiresAt,
      maxUses: maxUsesOverride ?? profile.maxUses
    });

    await tx.rmmInstallerDownloadAudit.create({
      data: {
        profileId: profile.id,
        tokenId: issued.token.id,
        organizationId: profile.organizationId,
        customerId: profile.customerId,
        siteId: profile.siteId,
        userId: membership.userId,
        userEmail: membership.user.email,
        clientIp: getClientIp(req),
        userAgent: readString(req.header('user-agent'))
      }
    });

    await writeAuditEvent(auditRequest(req, {
      organizationId: profile.organizationId,
      customerId: profile.customerId,
      siteId: profile.siteId,
      actorType: 'user',
      userId: membership.userId,
      userEmail: membership.user.email,
      actionType: 'installer.token.issue',
      targetType: 'installer_profile',
      targetId: profile.id,
      targetName: profile.name,
      result: 'success',
      metadata: {
        tokenId: issued.token.id,
        tokenPrefix: issued.token.tokenPrefix,
        deliveryKind,
        expiresAt: issued.token.expiresAt ? issued.token.expiresAt.toISOString() : null,
        maxUses: issued.token.maxUses
      }
    }), tx);

    return { profile, issued };
  });
}

installersRouter.get('/profiles', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const scopeTypeInput = normalizeScopeType(req.query.scopeType);
  const customerIdInput = readString(req.query.customerId);
  const siteIdInput = readString(req.query.siteId);

  const where: Prisma.RmmInstallerProfileWhereInput = {
    organizationId: membership.organizationId,
    ...(scopeTypeInput ? { scopeType: toPrismaScopeType(scopeTypeInput) } : {}),
    ...(customerIdInput ? { customerId: customerIdInput } : {}),
    ...(siteIdInput ? { siteId: siteIdInput } : {})
  };

  const profiles = await prisma.rmmInstallerProfile.findMany({
    where,
    include: {
      customer: { select: { id: true, name: true } },
      site: { select: { id: true, name: true } },
      tokens: {
        where: { revokedAt: null },
        orderBy: { createdAt: 'desc' },
        take: 1
      }
    },
    orderBy: [{ revokedAt: 'asc' }, { createdAt: 'desc' }]
  });

  return res.json(
    profiles.map((profile) => {
      const latestToken = profile.tokens[0] || null;
      return {
        ...profileToResponse(profile),
        latestToken: latestToken
          ? {
              id: latestToken.id,
              tokenPrefix: latestToken.tokenPrefix,
              expiresAt: latestToken.expiresAt ? latestToken.expiresAt.toISOString() : null,
              maxUses: latestToken.maxUses,
              usedCount: latestToken.usedCount,
              revokedAt: latestToken.revokedAt ? latestToken.revokedAt.toISOString() : null,
              createdAt: latestToken.createdAt.toISOString(),
              lastUsedAt: latestToken.lastUsedAt ? latestToken.lastUsedAt.toISOString() : null
            }
          : null
      };
    })
  );
});

installersRouter.get('/viewer', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can view installer downloads' });
  }

  let platform: ViewerInstallerPlatform = 'windows';
  try {
    platform = normalizeViewerInstallerPlatform(req.query.platform);
    const info = await buildViewerInstallerInfo(platform);
    return res.json({
      available: true,
      platform: info.platform,
      profile: info.profile,
      generatedAtUtc: info.generatedAtUtc,
      downloadPath: info.downloadPath,
      installer: info.installer,
      error: null
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.json({
        available: false,
        platform,
        profile: null,
        generatedAtUtc: null,
        downloadPath:
          platform === 'macos'
            ? '/rmm/installers/viewer/download?platform=macos'
            : '/rmm/installers/viewer/download?platform=windows',
        installer: null,
        error: error.message
      });
    }
    throw error;
  }
});

installersRouter.get('/viewer/download', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can download viewer installers' });
  }

  try {
    const platform = normalizeViewerInstallerPlatform(req.query.platform);
    const info = await buildViewerInstallerInfo(platform);
    const buffer = await fs.readFile(info.installerPath);

    await writeAuditEvent(auditRequest(req, {
      organizationId: membership.organizationId,
      actorType: 'user',
      userId: membership.userId,
      userEmail: membership.user.email,
      actionType: 'installer.viewer.download',
      targetType: 'viewer_installer',
      targetName: info.installer.fileName,
      result: 'success',
      metadata: {
        profile: info.profile,
        sizeBytes: info.installer.sizeBytes,
        sha256: info.installer.sha256
      }
    }));

    res.status(200);
    res.setHeader('Content-Type', getDownloadContentType(info.installer.fileName));
    res.setHeader('Content-Disposition', `attachment; filename="${info.installer.fileName}"`);
    res.setHeader('X-Installer-Filename', info.installer.fileName);
    res.setHeader('Cache-Control', 'no-store');
    return res.send(buffer);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/linux/agent', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can view installer downloads' });
  }

  try {
    const info = await buildLinuxAgentInfo(req);
    return res.json({
      available: true,
      downloadPath: info.downloadPath,
      downloadUrl: info.downloadUrl,
      binary: info.binary,
      error: null
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.json({
        available: false,
        downloadPath: '/rmm/installers/linux/agent/download',
        downloadUrl: null,
        binary: null,
        error: error.message
      });
    }
    throw error;
  }
});

installersRouter.get('/linux/agent/download', async (_req, res) => {
  try {
    const { buffer } = await loadLinuxAgentBinaryBuffer();
    const fileName = readString(process.env.RMM_LINUX_AGENT_BINARY_FILENAME) || DEFAULT_LINUX_AGENT_BINARY_FILENAME;

    res.status(200);
    res.setHeader('Content-Type', 'application/octet-stream');
    res.setHeader('Content-Disposition', `attachment; filename="${fileName}"`);
    res.setHeader('X-Installer-Filename', fileName);
    res.setHeader('Cache-Control', 'no-store');
    return res.send(buffer);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/linux/install.sh', async (req, res) => {
  try {
    const token = readString(req.query.token);
    if (!token) {
      throw new HttpError(400, 'token is required');
    }

    await assertLinuxInstallTokenCanRender(token);

    const serverUrl = readString(req.query.serverUrl) || resolveInstallerServerUrl();
    const configuredBinaryUrl = readString(process.env.RMM_LINUX_AGENT_BINARY_URL);
    const binaryUrl = configuredBinaryUrl || buildAbsoluteUrl(req, '/rmm/installers/linux/agent/download');
    const updateBaseUrl = readString(process.env.RMM_LINUX_UPDATE_BASE_URL, process.env.RMM_UPDATE_BASE_URL) || buildAbsoluteUrl(req, '/rmm/updates');
    const script = buildLinuxInstallScript({ token, serverUrl, binaryUrl, updateBaseUrl });

    res.status(200);
    res.setHeader('Content-Type', 'text/x-shellscript; charset=utf-8');
    res.setHeader('Content-Disposition', 'inline; filename="talos-linux-install.sh"');
    res.setHeader('Cache-Control', 'no-store');
    return res.send(script);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/linux/short/:code/install.sh', async (req, res) => {
  try {
    const code = readString(req.params.code);
    if (!code) {
      throw new HttpError(404, 'Short installer link not found');
    }

    const resolved = await resolveShortLinuxInstallLink(code);
    const configuredBinaryUrl = readString(process.env.RMM_LINUX_AGENT_BINARY_URL);
    const binaryUrl = configuredBinaryUrl || buildAbsoluteUrl(req, '/rmm/installers/linux/agent/download');
    const updateBaseUrl = readString(process.env.RMM_LINUX_UPDATE_BASE_URL, process.env.RMM_UPDATE_BASE_URL) || buildAbsoluteUrl(req, '/rmm/updates');
    const script = buildLinuxInstallScript({
      token: resolved.token,
      serverUrl: resolved.serverUrl,
      binaryUrl,
      updateBaseUrl
    });

    res.status(200);
    res.setHeader('Content-Type', 'text/x-shellscript; charset=utf-8');
    res.setHeader('Content-Disposition', 'inline; filename="talos-linux-install.sh"');
    res.setHeader('Cache-Control', 'no-store');
    return res.send(script);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/macos/package', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can view installer downloads' });
  }

  try {
    const info = await buildMacosPackageInfo(req);
    return res.json({
      available: true,
      downloadPath: info.downloadPath,
      downloadUrl: info.downloadUrl,
      package: info.package,
      error: null
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.json({
        available: false,
        downloadPath: '/rmm/installers/macos/package/download',
        downloadUrl: null,
        package: null,
        error: error.message
      });
    }
    throw error;
  }
});

installersRouter.get('/macos/package/download', async (_req, res) => {
  try {
    const { buffer } = await loadMacosPackageBuffer();
    const fileName = readString(process.env.RMM_MACOS_PACKAGE_FILENAME) || DEFAULT_MACOS_PACKAGE_FILENAME;

    res.status(200);
    res.setHeader('Content-Type', getDownloadContentType(fileName));
    res.setHeader('Content-Disposition', `attachment; filename="${fileName}"`);
    res.setHeader('X-Installer-Filename', fileName);
    res.setHeader('Cache-Control', 'no-store');
    return res.send(buffer);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/macos/install.sh', async (req, res) => {
  try {
    const token = readString(req.query.token);
    if (!token) {
      throw new HttpError(400, 'token is required');
    }

    await assertLinuxInstallTokenCanRender(token);

    const serverUrl = readString(req.query.serverUrl) || resolveInstallerServerUrl();
    const packageUrl = await resolveMacosPackageInstallUrl(req);
    const updateBaseUrl = readString(process.env.RMM_MACOS_UPDATE_BASE_URL, process.env.RMM_UPDATE_BASE_URL) || buildAbsoluteUrl(req, '/rmm/updates');
    const script = buildMacosInstallScript({ token, serverUrl, packageUrl, updateBaseUrl });

    res.status(200);
    res.setHeader('Content-Type', 'text/x-shellscript; charset=utf-8');
    res.setHeader('Content-Disposition', 'inline; filename="talos-macos-install.sh"');
    res.setHeader('Cache-Control', 'no-store');
    return res.send(script);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.get('/macos/short/:code/install.sh', async (req, res) => {
  try {
    const code = readString(req.params.code);
    if (!code) {
      throw new HttpError(404, 'Short installer link not found');
    }

    const resolved = await resolveShortLinuxInstallLink(code);
    const packageUrl = await resolveMacosPackageInstallUrl(req);
    const updateBaseUrl = readString(process.env.RMM_MACOS_UPDATE_BASE_URL, process.env.RMM_UPDATE_BASE_URL) || buildAbsoluteUrl(req, '/rmm/updates');
    const script = buildMacosInstallScript({
      token: resolved.token,
      serverUrl: resolved.serverUrl,
      packageUrl,
      updateBaseUrl
    });

    res.status(200);
    res.setHeader('Content-Type', 'text/x-shellscript; charset=utf-8');
    res.setHeader('Content-Disposition', 'inline; filename="talos-macos-install.sh"');
    res.setHeader('Cache-Control', 'no-store');
    return res.send(script);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post('/profiles', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can create installer profiles' });
  }

  try {
    const scopeType = normalizeScopeType(req.body?.scopeType) || 'organization';
    const customerIdInput = readString(req.body?.customerId);
    const siteIdInput = readString(req.body?.siteId);
    const expiresAt = parseOptionalDate(req.body?.expiresAt);
    const maxUses = parseOptionalMaxUses(req.body?.maxUses);

    if (expiresAt && expiresAt.getTime() <= Date.now()) {
      throw new HttpError(400, 'expiresAt must be in the future');
    }

    const scope = await resolveScope(membership, scopeType, customerIdInput, siteIdInput);
    const profileName = readString(req.body?.name) || buildScopeName(scopeType, {
      customerName: null,
      siteName: null
    });

    const result = await prisma.$transaction(async (tx) => {
      const profile = await tx.rmmInstallerProfile.create({
        data: {
          organizationId: scope.organizationId,
          customerId: scope.customerId,
          siteId: scope.siteId,
          scopeType: scope.scopeType,
          name: profileName,
          expiresAt,
          maxUses,
          createdBy: membership.userId
        },
        include: {
          customer: { select: { id: true, name: true } },
          site: { select: { id: true, name: true } }
        }
      });

      const issued = await issueEnrollmentToken(tx, {
        profileId: profile.id,
        organizationId: profile.organizationId,
        customerId: profile.customerId,
        siteId: profile.siteId,
        issuedBy: membership.userId,
        expiresAt: profile.expiresAt,
        maxUses: profile.maxUses
      });

      await writeAuditEvent(auditRequest(req, {
        organizationId: profile.organizationId,
        customerId: profile.customerId,
        siteId: profile.siteId,
        actorType: 'user',
        userId: membership.userId,
        userEmail: membership.user.email,
        actionType: 'installer.profile.create',
        targetType: 'installer_profile',
        targetId: profile.id,
        targetName: profile.name,
        result: 'success',
        metadata: {
          scopeType: profile.scopeType,
          tokenId: issued.token.id,
          tokenPrefix: issued.token.tokenPrefix,
          expiresAt: profile.expiresAt ? profile.expiresAt.toISOString() : null,
          maxUses: profile.maxUses
        }
      }), tx);

      return { profile, issued };
    });

    const bootstrapUrl = configuredInstallerBootstrapUrl();

    return res.status(201).json({
      profile: profileToResponse(result.profile),
      issuedToken: {
        id: result.issued.token.id,
        tokenPrefix: result.issued.token.tokenPrefix,
        token: result.issued.rawToken,
        expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
        maxUses: result.issued.token.maxUses,
        usedCount: result.issued.token.usedCount,
        createdAt: result.issued.token.createdAt.toISOString()
      },
      bootstrapUrl,
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post('/profiles/:id/download', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can issue installer downloads' });
  }

  try {
    const expiresAtOverride = parseOptionalDate(req.body?.expiresAt);
    const maxUsesOverride = parseOptionalMaxUses(req.body?.maxUses);
    if (expiresAtOverride && expiresAtOverride.getTime() <= Date.now()) {
      throw new HttpError(400, 'expiresAt must be in the future');
    }
    const result = await issueInstallerDownloadForProfile(
      req,
      membership,
      req.params.id,
      expiresAtOverride,
      maxUsesOverride,
      'json'
    );

    const bootstrapUrl = configuredInstallerBootstrapUrl();
    const payload = {
      version: 1,
      registrationToken: result.issued.rawToken,
      organizationId: result.profile.organizationId,
      customerId: result.profile.customerId,
      siteId: result.profile.siteId,
      expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
      maxUses: result.issued.token.maxUses,
      tokenId: result.issued.token.id,
      issuedAt: result.issued.token.createdAt.toISOString()
    };

    const scopeType = fromPrismaScopeType(result.profile.scopeType);
    const scopeId = result.profile.siteId || result.profile.customerId || result.profile.organizationId;
    const filename = `talos-${scopeType}-${sanitizeFilenamePart(scopeId)}-installer.json`;

    return res.status(201).json({
      profile: profileToResponse(result.profile),
      issuedToken: {
        id: result.issued.token.id,
        tokenPrefix: result.issued.token.tokenPrefix,
        token: result.issued.rawToken,
        expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
        maxUses: result.issued.token.maxUses,
        usedCount: result.issued.token.usedCount,
        createdAt: result.issued.token.createdAt.toISOString()
      },
      bootstrapUrl,
      filename,
      enrollmentBlob: Buffer.from(JSON.stringify(payload)).toString('base64url'),
      payload
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post('/profiles/:id/linux-install', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can issue Linux installer downloads' });
  }

  try {
    const expiresAtOverride = parseOptionalDate(req.body?.expiresAt);
    const maxUsesOverride = parseOptionalMaxUses(req.body?.maxUses);
    if (expiresAtOverride && expiresAtOverride.getTime() <= Date.now()) {
      throw new HttpError(400, 'expiresAt must be in the future');
    }

    await buildLinuxAgentInfo(req);

    const result = await issueInstallerDownloadForProfile(
      req,
      membership,
      req.params.id,
      expiresAtOverride,
      maxUsesOverride,
      'linux-script'
    );
    const serverUrl = resolveInstallerServerUrl();
    const scriptPath = buildLinuxInstallScriptPath(result.issued.rawToken, serverUrl);
    const scriptUrl = buildAbsoluteUrl(req, scriptPath);
    const shortLink = await prisma.$transaction((tx) =>
      createShortLinuxInstallLink(tx, {
        tokenId: result.issued.token.id,
        profileId: result.profile.id,
        organizationId: result.profile.organizationId,
        customerId: result.profile.customerId,
        siteId: result.profile.siteId,
        serverUrl,
        issuedBy: membership.userId,
        rawToken: result.issued.rawToken
      })
    );
    const shortScriptPath = buildShortLinuxInstallPath(shortLink.code);
    const shortScriptUrl = buildShortLinuxInstallUrl(req, shortLink.code);
    const installCommand = buildLinuxInstallCommand(shortScriptUrl);
    const payload = {
      version: 1,
      registrationToken: result.issued.rawToken,
      organizationId: result.profile.organizationId,
      customerId: result.profile.customerId,
      siteId: result.profile.siteId,
      expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
      maxUses: result.issued.token.maxUses,
      tokenId: result.issued.token.id,
      issuedAt: result.issued.token.createdAt.toISOString()
    };

    return res.status(201).json({
      profile: profileToResponse(result.profile),
      issuedToken: {
        id: result.issued.token.id,
        tokenPrefix: result.issued.token.tokenPrefix,
        token: result.issued.rawToken,
        expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
        maxUses: result.issued.token.maxUses,
        usedCount: result.issued.token.usedCount,
        createdAt: result.issued.token.createdAt.toISOString()
      },
      bootstrapUrl: scriptUrl,
      linuxScriptPath: scriptPath,
      linuxScriptUrl: scriptUrl,
      linuxShortCode: shortLink.code,
      linuxShortScriptPath: shortScriptPath,
      linuxShortScriptUrl: shortScriptUrl,
      linuxShortScriptExpiresAt: shortLink.expiresAt.toISOString(),
      linuxInstallCommand: installCommand,
      linuxScriptFilename: buildLinuxScriptFilename(result.profile),
      filename: buildLinuxScriptFilename(result.profile),
      enrollmentBlob: Buffer.from(JSON.stringify(payload)).toString('base64url'),
      payload
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post('/profiles/:id/macos-install', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can issue macOS installer downloads' });
  }

  try {
    const expiresAtOverride = parseOptionalDate(req.body?.expiresAt);
    const maxUsesOverride = parseOptionalMaxUses(req.body?.maxUses);
    if (expiresAtOverride && expiresAtOverride.getTime() <= Date.now()) {
      throw new HttpError(400, 'expiresAt must be in the future');
    }

    await buildMacosPackageInfo(req);

    const result = await issueInstallerDownloadForProfile(
      req,
      membership,
      req.params.id,
      expiresAtOverride,
      maxUsesOverride,
      'macos-script'
    );
    const serverUrl = resolveInstallerServerUrl();
    const scriptPath = buildMacosInstallScriptPath(result.issued.rawToken, serverUrl);
    const scriptUrl = buildAbsoluteUrl(req, scriptPath);
    const shortLink = await prisma.$transaction((tx) =>
      createShortLinuxInstallLink(tx, {
        tokenId: result.issued.token.id,
        profileId: result.profile.id,
        organizationId: result.profile.organizationId,
        customerId: result.profile.customerId,
        siteId: result.profile.siteId,
        serverUrl,
        issuedBy: membership.userId,
        rawToken: result.issued.rawToken
      })
    );
    const shortScriptPath = buildShortMacosInstallPath(shortLink.code);
    const shortScriptUrl = buildShortMacosInstallUrl(req, shortLink.code);
    const installCommand = buildMacosInstallCommand(shortScriptUrl);
    const payload = {
      version: 1,
      registrationToken: result.issued.rawToken,
      organizationId: result.profile.organizationId,
      customerId: result.profile.customerId,
      siteId: result.profile.siteId,
      expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
      maxUses: result.issued.token.maxUses,
      tokenId: result.issued.token.id,
      issuedAt: result.issued.token.createdAt.toISOString()
    };

    return res.status(201).json({
      profile: profileToResponse(result.profile),
      issuedToken: {
        id: result.issued.token.id,
        tokenPrefix: result.issued.token.tokenPrefix,
        token: result.issued.rawToken,
        expiresAt: result.issued.token.expiresAt ? result.issued.token.expiresAt.toISOString() : null,
        maxUses: result.issued.token.maxUses,
        usedCount: result.issued.token.usedCount,
        createdAt: result.issued.token.createdAt.toISOString()
      },
      bootstrapUrl: scriptUrl,
      macosScriptPath: scriptPath,
      macosScriptUrl: scriptUrl,
      macosShortCode: shortLink.code,
      macosShortScriptPath: shortScriptPath,
      macosShortScriptUrl: shortScriptUrl,
      macosShortScriptExpiresAt: shortLink.expiresAt.toISOString(),
      macosInstallCommand: installCommand,
      macosScriptFilename: buildMacosScriptFilename(result.profile),
      filename: buildMacosScriptFilename(result.profile),
      enrollmentBlob: Buffer.from(JSON.stringify(payload)).toString('base64url'),
      payload
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post(
  '/profiles/:id/download-exe',
  requireAuth,
  requireUnsignedScopedInstallerOptIn,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
    if (!isAgentAdmin(membership.role)) {
      return res.status(403).json({ error: 'Only admins can issue installer downloads' });
    }

    try {
      const expiresAtOverride = parseOptionalDate(req.body?.expiresAt);
      const maxUsesOverride = parseOptionalMaxUses(req.body?.maxUses);
      if (expiresAtOverride && expiresAtOverride.getTime() <= Date.now()) {
        throw new HttpError(400, 'expiresAt must be in the future');
      }

      const result = await issueInstallerDownloadForProfile(
        req,
        membership,
        req.params.id,
        expiresAtOverride,
        maxUsesOverride,
        'exe'
      );
      const sfxStub = await loadInstallerSfxStubBuffer();
      const payloadArchive = await loadInstallerPayloadArchiveBuffer();
      const payloadExeName = resolveInstallerPayloadExeName();
      const serverUrl = resolveInstallerServerUrl();
      const sfxConfig = buildSfxConfig(result.issued.rawToken, serverUrl, payloadExeName);
      const stampedInstaller = buildScopedInstallerExe(sfxStub, sfxConfig, payloadArchive);
      const filename = buildInstallerExeFilename(result.profile);

      res.status(201);
      res.setHeader('Content-Type', 'application/vnd.microsoft.portable-executable');
      res.setHeader('Content-Disposition', `attachment; filename="${filename}"`);
      res.setHeader('X-Installer-Filename', filename);
      res.setHeader('Cache-Control', 'no-store');
      return res.send(stampedInstaller);
    } catch (error) {
      if (error instanceof HttpError) {
        return res.status(error.status).json({ error: error.message });
      }
      throw error;
    }
  }
);

installersRouter.post('/profiles/:id/download-macos-pkg', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can issue macOS package downloads' });
  }

  try {
    const expiresAtOverride = parseOptionalDate(req.body?.expiresAt);
    const maxUsesOverride = parseOptionalMaxUses(req.body?.maxUses);
    if (expiresAtOverride && expiresAtOverride.getTime() <= Date.now()) {
      throw new HttpError(400, 'expiresAt must be in the future');
    }

    const result = await issueInstallerDownloadForProfile(
      req,
      membership,
      req.params.id,
      expiresAtOverride,
      maxUsesOverride,
      'macos-pkg'
    );
    const packagePath = await resolveMacosPackagePath();
    const serverUrl = resolveInstallerServerUrl();
    const updateBaseUrl =
      readString(process.env.RMM_MACOS_UPDATE_BASE_URL, process.env.RMM_UPDATE_BASE_URL) ||
      buildAbsoluteUrl(req, '/rmm/updates');
    const stampedPackage = await buildScopedMacosPackage(packagePath, {
      token: result.issued.rawToken,
      serverUrl,
      updateBaseUrl
    });
    const filename = buildInstallerMacosPackageFilename(result.profile);

    res.status(201);
    res.setHeader('Content-Type', 'application/octet-stream');
    res.setHeader('Content-Disposition', `attachment; filename="${filename}"`);
    res.setHeader('X-Installer-Filename', filename);
    res.setHeader('Cache-Control', 'no-store');
    return res.send(stampedPackage);
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});

installersRouter.post('/profiles/:id/revoke', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) {
    return res.status(403).json({ error: 'Only admins can revoke installer profiles' });
  }

  const now = new Date();
  const profile = await prisma.rmmInstallerProfile.findFirst({
    where: {
      id: req.params.id,
      organizationId: membership.organizationId
    },
    include: {
      customer: { select: { id: true, name: true } },
      site: { select: { id: true, name: true } }
    }
  });

  if (!profile) {
    return res.status(404).json({ error: 'Installer profile not found' });
  }

  await prisma.$transaction(async (tx) => {
    await tx.rmmInstallerProfile.update({
      where: { id: profile.id },
      data: { revokedAt: now }
    });
    await tx.rmmInstallerEnrollmentToken.updateMany({
      where: {
        profileId: profile.id,
        revokedAt: null
      },
      data: { revokedAt: now }
    });
    await writeAuditEvent(auditRequest(req, {
      organizationId: profile.organizationId,
      customerId: profile.customerId,
      siteId: profile.siteId,
      actorType: 'user',
      userId: membership.userId,
      userEmail: membership.user.email,
      actionType: 'installer.profile.revoke',
      targetType: 'installer_profile',
      targetId: profile.id,
      targetName: profile.name,
      result: 'success',
      metadata: {
        scopeType: profile.scopeType,
        revokedAt: now.toISOString()
      }
    }), tx);
  });

  return res.json({
    ...profileToResponse({
      ...profile,
      revokedAt: now
    }),
    revoked: true
  });
});

installersRouter.post('/enroll', requireRmmServer, async (req: RmmServerRequest, res) => {
  try {
    const token = readString(req.body?.token, req.body?.registrationToken, req.body?.registration_token);
    const agentId = readString(req.body?.agentId, req.body?.agent_id);
    const hostname = readString(req.body?.hostname) || 'unknown';
    const os = readString(req.body?.os) || 'unknown';
    const ip = readString(req.body?.ip) || '0.0.0.0';
    const version = readString(req.body?.version);

    if (!token || !agentId) {
      throw new HttpError(400, 'token and agentId are required');
    }

    const now = new Date();
    const tokenHash = hashToken(token);

    const enrollment = await prisma.$transaction(async (tx) => {
      const enrollmentToken = await tx.rmmInstallerEnrollmentToken.findUnique({
        where: { tokenHash },
        include: {
          profile: true
        }
      });

      if (!enrollmentToken) {
        throw new HttpError(401, 'Invalid enrollment token');
      }

      const organizationId = enrollmentToken.organizationId;
      const existingUse = await tx.rmmInstallerTokenUse.findUnique({
        where: {
          tokenId_agentId: {
            tokenId: enrollmentToken.id,
            agentId
          }
        }
      });

      if (!existingUse) {
        if (enrollmentToken.revokedAt || enrollmentToken.profile.revokedAt) {
          throw new HttpError(403, 'Enrollment token has been revoked');
        }
        if (enrollmentToken.expiresAt && enrollmentToken.expiresAt.getTime() < now.getTime()) {
          throw new HttpError(403, 'Enrollment token has expired');
        }
      }

      let targetCustomerId = enrollmentToken.customerId;
      let targetSiteId = enrollmentToken.siteId;

      if (targetSiteId) {
        const site = await tx.rmmSite.findFirst({
          where: {
            id: targetSiteId,
            customer: { organizationId }
          },
          select: { id: true, customerId: true }
        });
        if (!site) {
          throw new HttpError(409, 'Enrollment token site is invalid');
        }
        targetSiteId = site.id;
        targetCustomerId = site.customerId;
      }

      if (targetCustomerId && !targetSiteId) {
        const customer = await tx.customer.findFirst({
          where: {
            id: targetCustomerId,
            organizationId
          },
          select: { id: true }
        });
        if (!customer) {
          throw new HttpError(409, 'Enrollment token customer is invalid');
        }
      }

      if (!targetCustomerId) {
        const unassigned = await getOrCreateUnassignedTx(tx, organizationId);
        targetCustomerId = unassigned.id;
        targetSiteId = null;
      }

      const existingDevice = await tx.rmmDevice.findUnique({
        where: { agentId },
        select: {
          agentId: true,
          organizationId: true,
          customerId: true,
          siteId: true
        }
      });

      if (existingDevice && existingDevice.organizationId !== organizationId) {
        throw new HttpError(409, 'agentId is already bound to another organization');
      }

      if (!existingUse) {
        if (enrollmentToken.maxUses !== null) {
          const consumed = await tx.rmmInstallerEnrollmentToken.updateMany({
            where: {
              id: enrollmentToken.id,
              revokedAt: null,
              usedCount: { lt: enrollmentToken.maxUses }
            },
            data: {
              usedCount: { increment: 1 },
              lastUsedAt: now
            }
          });
          if (consumed.count === 0) {
            throw new HttpError(403, 'Enrollment token maximum uses reached');
          }
        } else {
          await tx.rmmInstallerEnrollmentToken.update({
            where: { id: enrollmentToken.id },
            data: {
              usedCount: { increment: 1 },
              lastUsedAt: now
            }
          });
        }

        await tx.rmmInstallerTokenUse.create({
          data: {
            tokenId: enrollmentToken.id,
            profileId: enrollmentToken.profileId,
            organizationId,
            agentId,
            firstSeenAt: now,
            lastSeenAt: now
          }
        });
      } else {
        await tx.rmmInstallerTokenUse.update({
          where: {
            tokenId_agentId: {
              tokenId: enrollmentToken.id,
              agentId
            }
          },
          data: { lastSeenAt: now }
        });
        await tx.rmmInstallerEnrollmentToken.update({
          where: { id: enrollmentToken.id },
          data: { lastUsedAt: now }
        });
      }

      if (existingDevice) {
        const updateData: Prisma.RmmDeviceUpdateInput = {
          lastSeen: now,
          hostname,
          os,
          ip,
          version: version || null
        };

        if (!existingDevice.customerId && targetCustomerId) {
          updateData.customer = { connect: { id: targetCustomerId } };
        }
        if (!existingDevice.siteId && targetSiteId) {
          updateData.site = { connect: { id: targetSiteId } };
        }

        await tx.rmmDevice.update({
          where: { agentId },
          data: updateData
        });
      } else {
        await tx.rmmDevice.create({
          data: {
            agentId,
            organizationId,
            hostname,
            os,
            ip,
            version: version || null,
            lastSeen: now,
            customerId: targetCustomerId,
            siteId: targetSiteId
          }
        });
      }

      await writeAuditEvent(auditRequest(req, {
        organizationId,
        customerId: targetCustomerId,
        siteId: targetSiteId,
        agentId,
        actorType: 'agent',
        actionType: 'installer.token.enroll',
        targetType: 'rmm_device',
        targetId: agentId,
        targetName: hostname,
        result: 'success',
        metadata: {
          tokenId: enrollmentToken.id,
          tokenPrefix: enrollmentToken.tokenPrefix,
          profileId: enrollmentToken.profileId,
          existingDevice: Boolean(existingDevice),
          usedCount: enrollmentToken.usedCount + (existingUse ? 0 : 1)
        }
      }), tx);

      return {
        organizationId,
        customerId: targetCustomerId,
        siteId: targetSiteId,
        tokenId: enrollmentToken.id,
        profileId: enrollmentToken.profileId,
        existingDevice: Boolean(existingDevice)
      };
    });

    return res.json({
      enrolled: true,
      organizationId: enrollment.organizationId,
      customerId: enrollment.customerId,
      siteId: enrollment.siteId,
      profileId: enrollment.profileId,
      tokenId: enrollment.tokenId,
      existingDevice: enrollment.existingDevice
    });
  } catch (error) {
    if (error instanceof HttpError) {
      return res.status(error.status).json({ error: error.message });
    }
    throw error;
  }
});
