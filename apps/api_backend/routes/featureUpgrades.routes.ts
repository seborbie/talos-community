import { Router } from 'express';
import { randomUUID } from 'crypto';
import {
  BlobSASPermissions,
  BlobServiceClient,
  SASProtocol
} from '@azure/storage-blob';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import { env } from '../lib/env';
import { AuthedRequest, requireAuth } from '../middleware/auth';
import {
  attachRmmServerAuth,
  requireRmmServer,
  RmmServerRequest
} from '../middleware/rmmServerKey';
import {
  aggregateFeatureUpgradePreflightStatus,
  FEATURE_UPGRADE_PREFLIGHT_CHECKS,
  FEATURE_UPGRADE_PREFLIGHT_FACT_KEYS,
  FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES,
  evaluateFeatureUpgradePreflightChecks,
  featureUpgradeChecksForProfile,
  inferFeatureUpgradePreflightTarget,
  readFeatureUpgradeAgentIds,
  summarizeFeatureUpgradePreflightChecks
} from '../lib/featureUpgradePreflight';

export const featureUpgradesRouter = Router();
featureUpgradesRouter.use(attachRmmServerAuth);

const DEFAULT_ISO_CONTAINER = 'talos-feature-upgrade-isos';
const DEFAULT_SAS_TTL_SECONDS = 900;
const ISO_STAGE_RETENTION_SECONDS = 7 * 24 * 60 * 60;
const PREFLIGHT_STALE_AFTER_MINUTES = 15;

class HttpError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

type IsoMediaRow = {
  id: string;
  displayName: string;
  osFamily: string;
  product: string;
  version: string;
  edition: string | null;
  architecture: string;
  language: string | null;
  sha256: string | null;
  sizeBytes: bigint | null;
  containerName: string;
  blobName: string;
  active: boolean;
  createdAt: Date;
  updatedAt: Date;
};

type Membership = NonNullable<Awaited<ReturnType<typeof getCurrentMembership>>>;

type DeviceRow = {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  telemetryCollectedAt: Date | null;
  telemetryOsName: string | null;
  telemetryOsVersion: string | null;
  cpuModel: string | null;
  cpuPhysicalCores: number | null;
  cpuLogicalCores: number | null;
  cpuBaseMhz: number | null;
  memoryTotalBytes: bigint | null;
  pendingUpdatesCount: number | null;
  rebootRequired: boolean | null;
  inventoryData: unknown;
};

type FactRow = {
  agentId: string;
  factKey: string;
  factValue: unknown;
  source: string | null;
  sourceTs: Date | null;
  updatedAt: Date | null;
};

type PreflightDeviceRow = {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: string;
  phase: string;
  checkResults: unknown;
  failureSummary: unknown;
  warningSummary: unknown;
  requestedBy: string;
  claimedAt: Date | null;
  startedAt: Date | null;
  finishedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
};

type IsoStageDeviceRow = {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  isoMediaId: string;
  isoDisplayName: string | null;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: string;
  phase: string;
  progress: unknown;
  evidence: unknown;
  errorMessage: string | null;
  sizeBytes: bigint | null;
  sha256: string | null;
  requestedBy: string;
  claimedAt: Date | null;
  startedAt: Date | null;
  stagedAt: Date | null;
  expiresAt: Date | null;
  cleanedAt: Date | null;
  finishedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
};

type IsoStageClaimRow = IsoStageDeviceRow & {
  isoProduct: string;
  isoVersion: string;
  isoEdition: string | null;
  isoArchitecture: string;
  isoLanguage: string | null;
  isoContainerName: string;
  isoBlobName: string;
  isoActive: boolean;
  isoCreatedAt: Date;
  isoUpdatedAt: Date;
};

type SetupCommandMatrixRow = {
  id: string;
  isoMediaId: string;
  osFamily: string;
  product: string;
  version: string;
  edition: string | null;
  architecture: string;
  language: string | null;
  setupExecutable: string;
  arguments: unknown;
  dynamicUpdateMode: string;
  requiresEulaAccept: boolean;
  imageIndexStrategy: string;
  supported: boolean;
  notes: string | null;
  active: boolean;
  createdAt: Date;
  updatedAt: Date;
};

type FeatureUpgradeDeviceRow = {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  preflightOperationId: string;
  isoMediaId: string;
  isoDisplayName: string | null;
  setupCommandMatrixId: string;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: string;
  phase: string;
  progress: unknown;
  evidence: unknown;
  failureSummary: unknown;
  errorMessage: string | null;
  sizeBytes: bigint | null;
  sha256: string | null;
  scheduledFor: Date | null;
  requestedBy: string;
  claimedAt: Date | null;
  startedAt: Date | null;
  finalSnapshotAt: Date | null;
  setupStartedAt: Date | null;
  rebootDetectedAt: Date | null;
  verifiedAt: Date | null;
  finishedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
};

type FeatureUpgradeClaimRow = FeatureUpgradeDeviceRow & {
  isoProduct: string;
  isoVersion: string;
  isoEdition: string | null;
  isoArchitecture: string;
  isoLanguage: string | null;
  isoContainerName: string;
  isoBlobName: string;
  isoActive: boolean;
  isoCreatedAt: Date;
  isoUpdatedAt: Date;
  setupExecutable: string;
  setupArguments: unknown;
  dynamicUpdateMode: string;
  requiresEulaAccept: boolean;
  imageIndexStrategy: string;
  setupSupported: boolean;
  setupNotes: string | null;
};

type StageIsoPreviewDevice = {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  preflightStatus: string | null;
  preflightOperationId: string | null;
  canStage: boolean;
  blockingReasons: string[];
  warnings: string[];
  isoMedia: ReturnType<typeof toIsoMediaResponse> | null;
  existingStage: ReturnType<typeof isoStageDeviceResponse> | null;
  expectedSizeBytes: number | null;
  estimatedExpiresAt: string;
};

type FeatureUpgradeStartPreviewDevice = {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  preflightStatus: string | null;
  preflightOperationId: string | null;
  canStart: boolean;
  blockingReasons: string[];
  warnings: string[];
  isoMedia: ReturnType<typeof toIsoMediaResponse> | null;
  setupCommand: ReturnType<typeof setupCommandMatrixResponse> | null;
  existingStage: ReturnType<typeof isoStageDeviceResponse> | null;
  existingUpgrade: ReturnType<typeof featureUpgradeDeviceResponse> | null;
  expectedSizeBytes: number | null;
  willDownloadIso: boolean;
};

function assertUser(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== 'user') {
    res.status(403).json({ error: 'Machine tokens are not allowed' });
    return false;
  }
  return true;
}

async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { organization: true, user: { select: { id: true, email: true } } }
  });
}

async function requireMembership(req: AuthedRequest, res: any) {
  if (!assertUser(req, res)) return null;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) {
    res.status(404).json({ error: 'No organization', needsOnboarding: true });
    return null;
  }
  return membership;
}

function isAgentAdmin(role: string) {
  return role === 'AGENT_ADMIN' || role === 'SUPER_ADMIN';
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function readString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

async function loadWindowsDevicesForPreflight(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return [];
  return prisma.$queryRaw<DeviceRow[]>(Prisma.sql`
    SELECT
      d.agent_id AS "agentId",
      d.hostname,
      d.os,
      ds.os_version AS "osVersion",
      d.customer_id AS "customerId",
      c.name AS "customerName",
      d.site_id AS "siteId",
      s.name AS "siteName",
      ds.collected_at AS "telemetryCollectedAt",
      ds.os_name AS "telemetryOsName",
      ds.os_version AS "telemetryOsVersion",
      ds.cpu_model AS "cpuModel",
      ds.cpu_physical_cores AS "cpuPhysicalCores",
      ds.cpu_logical_cores AS "cpuLogicalCores",
      ds.cpu_base_mhz AS "cpuBaseMhz",
      ds.memory_total_bytes AS "memoryTotalBytes",
      ds.pending_updates_count AS "pendingUpdatesCount",
      ds.reboot_required AS "rebootRequired",
      ds.inventory_data AS "inventoryData"
    FROM public.rmm_devices d
    LEFT JOIN public.customers c ON c.id = d.customer_id
    LEFT JOIN public.rmm_sites s ON s.id = d.site_id
    LEFT JOIN rmm_telemetry.device_state ds ON ds.organization_id = d.organization_id AND ds.agent_id = d.agent_id
    WHERE d.organization_id = ${organizationId}
      AND d.agent_id IN (${Prisma.join(agentIds)})
      AND d.os ILIKE '%Windows%'
    ORDER BY d.hostname ASC
  `);
}

async function loadPreflightFacts(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return new Map<string, Map<string, FactRow>>();
  const rows = await prisma.$queryRaw<FactRow[]>(Prisma.sql`
    SELECT
      agent_id AS "agentId",
      fact_key AS "factKey",
      fact_value AS "factValue",
      source,
      source_ts AS "sourceTs",
      updated_at AS "updatedAt"
    FROM rmm_telemetry.fact_state_current
    WHERE organization_id = ${organizationId}
      AND agent_id IN (${Prisma.join(agentIds)})
      AND fact_key IN (${Prisma.join(FEATURE_UPGRADE_PREFLIGHT_FACT_KEYS)})
  `);
  const byAgent = new Map<string, Map<string, FactRow>>();
  for (const row of rows) {
    const facts = byAgent.get(row.agentId) ?? new Map<string, FactRow>();
    facts.set(row.factKey, row);
    byAgent.set(row.agentId, facts);
  }
  return byAgent;
}

function deviceTelemetryState(device: DeviceRow) {
  return {
    collectedAt: device.telemetryCollectedAt,
    osName: device.telemetryOsName,
    osVersion: device.telemetryOsVersion,
    cpuModel: device.cpuModel,
    cpuPhysicalCores: device.cpuPhysicalCores,
    cpuLogicalCores: device.cpuLogicalCores,
    cpuBaseMhz: device.cpuBaseMhz,
    memoryTotalBytes: device.memoryTotalBytes,
    pendingUpdatesCount: device.pendingUpdatesCount,
    rebootRequired: device.rebootRequired,
    inventoryData: device.inventoryData
  };
}

function toPreviewDevice(device: DeviceRow, factsByAgentId: Map<string, Map<string, FactRow>>, mode: 'preview' | 'final' = 'preview') {
  const target = inferFeatureUpgradePreflightTarget(device.os);
  if (!target) return null;
  const checks = evaluateFeatureUpgradePreflightChecks({
    device: {
      os: device.os,
      osVersion: device.osVersion,
      state: deviceTelemetryState(device),
      facts: factsByAgentId.get(device.agentId)
    },
    target,
    mode
  });
  return {
    agentId: device.agentId,
    hostname: device.hostname,
    os: device.os,
    osVersion: device.osVersion,
    snapshotCollectedAt: device.telemetryCollectedAt?.toISOString() ?? null,
    customerId: device.customerId,
    customerName: device.customerName,
    siteId: device.siteId,
    siteName: device.siteName,
    targetProduct: target.targetProduct,
    targetVersion: target.targetVersion,
    targetBuildLabel: target.targetBuildLabel,
    targetProfile: target.profile,
    checks
  };
}

function preflightDeviceResponse(row: PreflightDeviceRow) {
  return {
    operationId: row.operationId,
    runId: row.runId,
    organizationId: row.organizationId,
    agentId: row.agentId,
    hostname: row.hostname,
    sourceOs: row.sourceOs,
    targetProduct: row.targetProduct,
    targetVersion: row.targetVersion,
    targetBuildLabel: row.targetBuildLabel,
    status: row.status,
    phase: row.phase,
    checks: Array.isArray(row.checkResults) ? row.checkResults : [],
    failureSummary: Array.isArray(row.failureSummary) ? row.failureSummary : [],
    warningSummary: Array.isArray(row.warningSummary) ? row.warningSummary : [],
    requestedBy: row.requestedBy,
    claimedAt: row.claimedAt?.toISOString() ?? null,
    startedAt: row.startedAt?.toISOString() ?? null,
    finishedAt: row.finishedAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

async function failStalePreflightRuns(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return;
  const message = `Preflight timed out waiting for a completed snapshot after ${PREFLIGHT_STALE_AFTER_MINUTES} minutes`;
  const rows = await prisma.$queryRaw<Array<{ runId: string }>>(Prisma.sql`
    UPDATE public.feature_upgrade_preflight_device p
    SET status = 'failed',
        phase = 'failed',
        failure_summary_jsonb = ${JSON.stringify([{ id: 'snapshot_timeout', label: 'Fresh preflight snapshot', message }])}::jsonb,
        finished_at = NOW(),
        updated_at = NOW()
    WHERE p.organization_id = ${organizationId}
      AND p.agent_id IN (${Prisma.join(agentIds)})
      AND p.status IN ('queued', 'running')
      AND p.updated_at < NOW() - (${PREFLIGHT_STALE_AFTER_MINUTES} || ' minutes')::interval
    RETURNING p.run_id AS "runId"
  `);
  const touchedRuns = new Set(rows.map((row) => row.runId));
  for (const runId of touchedRuns) {
    await updatePreflightRunCounts(runId);
  }
}

async function updatePreflightRunCounts(runId: string) {
  await prisma.$executeRaw(Prisma.sql`
    WITH counts AS (
      SELECT
        run_id,
        COUNT(*)::int AS total,
        COUNT(*) FILTER (WHERE status = 'queued')::int AS queued,
        COUNT(*) FILTER (WHERE status = 'running')::int AS running,
        COUNT(*) FILTER (WHERE status = 'passed')::int AS passed,
        COUNT(*) FILTER (WHERE status = 'warning')::int AS warning,
        COUNT(*) FILTER (WHERE status = 'failed')::int AS failed,
        COUNT(*) FILTER (WHERE status IN ('passed', 'warning', 'failed', 'cancelled'))::int AS terminal
      FROM public.feature_upgrade_preflight_device
      WHERE run_id = ${runId}
      GROUP BY run_id
    )
    UPDATE public.feature_upgrade_preflight_run r
    SET
      total_devices = counts.total,
      queued_devices = counts.queued,
      running_devices = counts.running,
      passed_devices = counts.passed,
      warning_devices = counts.warning,
      failed_devices = counts.failed,
      status = CASE
        WHEN counts.running > 0 THEN 'running'
        WHEN counts.queued > 0 THEN 'queued'
        WHEN counts.failed > 0 THEN 'failed'
        ELSE 'completed'
      END,
      finished_at = CASE
        WHEN counts.terminal = counts.total THEN COALESCE(r.finished_at, NOW())
        ELSE NULL
      END,
      updated_at = NOW()
    FROM counts
    WHERE r.id = counts.run_id
  `);
}

async function reconcileCompletedPreflightSnapshots(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return;
  await failStalePreflightRuns(organizationId, agentIds);
  const rows = await prisma.$queryRaw<PreflightDeviceRow[]>(Prisma.sql`
    SELECT
      p.operation_id AS "operationId",
      p.run_id AS "runId",
      p.organization_id AS "organizationId",
      p.agent_id AS "agentId",
      d.hostname,
      p.source_os AS "sourceOs",
      p.target_product AS "targetProduct",
      p.target_version AS "targetVersion",
      p.target_build_label AS "targetBuildLabel",
      p.status,
      p.phase,
      p.check_results_jsonb AS "checkResults",
      p.failure_summary_jsonb AS "failureSummary",
      p.warning_summary_jsonb AS "warningSummary",
      p.requested_by AS "requestedBy",
      p.claimed_at AS "claimedAt",
      p.started_at AS "startedAt",
      p.finished_at AS "finishedAt",
      p.created_at AS "createdAt",
      p.updated_at AS "updatedAt"
    FROM public.feature_upgrade_preflight_device p
    JOIN rmm_telemetry.snapshot_request sr ON sr.agent_id = p.agent_id AND sr.request_id = p.operation_id
    LEFT JOIN public.rmm_devices d ON d.organization_id = p.organization_id AND d.agent_id = p.agent_id
    WHERE p.organization_id = ${organizationId}
      AND p.agent_id IN (${Prisma.join(agentIds)})
      AND p.status = 'running'
      AND sr.status = 'completed'
  `);
  if (rows.length === 0) return;

  const rowAgentIds = [...new Set(rows.map((row) => row.agentId))];
  const devices = await loadWindowsDevicesForPreflight(organizationId, rowAgentIds);
  const factsByAgentId = await loadPreflightFacts(organizationId, rowAgentIds);
  const devicesByAgentId = new Map(devices.map((device) => [device.agentId, device]));
  const touchedRuns = new Set<string>();

  for (const row of rows) {
    const device = devicesByAgentId.get(row.agentId);
    const target = inferFeatureUpgradePreflightTarget(row.sourceOs);
    if (!device || !target) continue;
    const checks = evaluateFeatureUpgradePreflightChecks({
      device: {
        os: row.sourceOs,
        osVersion: device.osVersion,
        state: deviceTelemetryState(device),
        facts: factsByAgentId.get(row.agentId)
      },
      target,
      mode: 'final'
    });
    const status = aggregateFeatureUpgradePreflightStatus(checks);
    const finalStatus = status === 'running' ? 'failed' : status;
    const phase = finalStatus === 'failed' ? 'failed' : 'completed';
    const failures = summarizeFeatureUpgradePreflightChecks(checks, 'failed');
    const warnings = summarizeFeatureUpgradePreflightChecks(checks, 'warning');
    const updatedRows = await prisma.$queryRaw<Array<{ runId: string }>>(Prisma.sql`
      UPDATE public.feature_upgrade_preflight_device
      SET status = ${finalStatus},
          phase = ${phase},
          check_results_jsonb = ${JSON.stringify(checks)}::jsonb,
          failure_summary_jsonb = ${JSON.stringify(failures)}::jsonb,
          warning_summary_jsonb = ${JSON.stringify(warnings)}::jsonb,
          finished_at = NOW(),
          updated_at = NOW()
      WHERE operation_id = ${row.operationId}
        AND status = 'running'
      RETURNING run_id AS "runId"
    `);
    if (updatedRows[0]) touchedRuns.add(updatedRows[0].runId);
  }

  for (const runId of touchedRuns) {
    await updatePreflightRunCounts(runId);
  }
}

async function notifyPreflightJobsAvailable(agentIds: string[], reason: string, requestedBy?: string | null) {
  const baseUrl = env.rmmServerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey || agentIds.length === 0) return;

  await fetch(`${baseUrl}/api/rmm/internal/feature-upgrades/preflight/notify`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-rmm-server-key': serverKey
    },
    body: JSON.stringify({ agentIds, reason, requestedBy })
  }).catch(() => undefined);
}

function isoContainerName() {
  return env.featureUpgradeIsoContainer || DEFAULT_ISO_CONTAINER;
}

function isoBlobStorageConfigured() {
  return Boolean(env.azureStorageConnectionString?.trim());
}

function connectionStringBlobEndpoint() {
  const raw = env.azureStorageConnectionString?.trim();
  if (!raw) return null;
  const part = raw
    .split(';')
    .map((item) => item.trim())
    .find((item) => item.toLowerCase().startsWith('blobendpoint='));
  return part ? part.slice(part.indexOf('=') + 1).trim() : null;
}

function isLocalOnlyBlobEndpoint(endpoint: string | null) {
  if (!endpoint) return false;
  try {
    const host = new URL(endpoint).hostname.toLowerCase();
    return host === 'azurite' || host === 'localhost' || host === '127.0.0.1' || host === '::1';
  } catch {
    return false;
  }
}

function isoBlobStorageReadinessIssue() {
  if (!isoBlobStorageConfigured()) return 'ISO blob storage is not configured on the API server';
  const internalEndpoint = connectionStringBlobEndpoint();
  if (isLocalOnlyBlobEndpoint(internalEndpoint) && !env.featureUpgradeIsoPublicBlobEndpoint?.trim()) {
    return 'ISO blob storage public endpoint is not configured for workers';
  }
  return null;
}

function sasTtlSeconds() {
  const raw = Number(env.featureUpgradeIsoSasTtlSeconds);
  if (!Number.isFinite(raw) || raw <= 0) return DEFAULT_SAS_TTL_SECONDS;
  return Math.min(Math.floor(raw), 3600);
}

function getBlobServiceClient() {
  if (!env.azureStorageConnectionString) {
    throw new HttpError(503, 'ISO blob storage is not configured');
  }
  return BlobServiceClient.fromConnectionString(env.azureStorageConnectionString);
}

function rewriteIsoDownloadUrlForWorkers(url: string) {
  const publicEndpoint = env.featureUpgradeIsoPublicBlobEndpoint?.trim().replace(/\/+$/, '');
  if (!publicEndpoint) return url;

  const internalEndpoint = connectionStringBlobEndpoint();
  if (!internalEndpoint) return url;

  try {
    const parsedUrl = new URL(url);
    const internal = new URL(internalEndpoint);
    const external = new URL(publicEndpoint);
    const internalPath = internal.pathname.replace(/\/+$/, '');
    if (
      parsedUrl.protocol !== internal.protocol ||
      parsedUrl.host !== internal.host ||
      (internalPath && !parsedUrl.pathname.startsWith(`${internalPath}/`))
    ) {
      return url;
    }

    const suffix = internalPath ? parsedUrl.pathname.slice(internalPath.length) : parsedUrl.pathname;
    parsedUrl.protocol = external.protocol;
    parsedUrl.hostname = external.hostname;
    parsedUrl.port = external.port;
    parsedUrl.pathname = `${external.pathname.replace(/\/+$/, '')}${suffix}`;
    return parsedUrl.toString();
  } catch {
    return url;
  }
}

async function generateIsoDownloadLink(media: IsoMediaRow) {
  const blobService = getBlobServiceClient();
  const containerClient = blobService.getContainerClient(media.containerName || isoContainerName());
  const blobClient = containerClient.getBlobClient(media.blobName);
  const exists = await blobClient.exists();
  if (!exists) {
    throw new HttpError(502, 'ISO blob is not available');
  }

  const ttlSeconds = sasTtlSeconds();
  const expiresAt = new Date(Date.now() + ttlSeconds * 1000);
  const startsOn = new Date(Date.now() - 60 * 1000);
  const url = await blobClient.generateSasUrl({
    permissions: BlobSASPermissions.parse('r'),
    protocol: SASProtocol.HttpsAndHttp,
    startsOn,
    expiresOn: expiresAt
  });

  return {
    mediaId: media.id,
    url: rewriteIsoDownloadUrlForWorkers(url),
    expiresAt: expiresAt.toISOString(),
    method: 'GET'
  };
}

function toIsoMediaResponse(row: IsoMediaRow) {
  return {
    id: row.id,
    displayName: row.displayName,
    osFamily: row.osFamily,
    product: row.product,
    version: row.version,
    edition: row.edition,
    architecture: row.architecture,
    language: row.language,
    sha256: row.sha256,
    sizeBytes: row.sizeBytes === null ? null : Number(row.sizeBytes),
    containerName: row.containerName,
    blobName: row.blobName,
    active: row.active,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

function readNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'bigint') return Number(value);
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function readIsoDate(value: unknown): Date | null {
  if (value instanceof Date && !Number.isNaN(value.getTime())) return value;
  if (typeof value !== 'string' || !value.trim()) return null;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function stageExpiryFromNow() {
  return new Date(Date.now() + ISO_STAGE_RETENTION_SECONDS * 1000);
}

function normalizeText(value: string | null | undefined) {
  return (value ?? '').trim().toLowerCase().replace(/\s+/g, ' ');
}

function isX64MediaArchitecture(value: string) {
  const normalized = normalizeText(value);
  return normalized.includes('x64') || normalized.includes('amd64') || normalized.includes('64');
}

function mediaProductMatches(targetProduct: string, mediaProduct: string) {
  const target = normalizeText(targetProduct);
  const media = normalizeText(mediaProduct);
  return target === media || target.includes(media) || media.includes(target);
}

function mediaVersionMatches(targetVersion: string, mediaVersion: string) {
  return normalizeText(targetVersion) === normalizeText(mediaVersion);
}

function languageMatches(mediaLanguage: string | null, deviceLanguage: string | null) {
  if (!mediaLanguage) return true;
  if (!deviceLanguage) return false;
  const media = normalizeText(mediaLanguage).replace(/_/g, '-');
  const device = normalizeText(deviceLanguage).replace(/_/g, '-');
  return media === device;
}

function editionMatches(mediaEdition: string | null, deviceEdition: string | null) {
  if (!mediaEdition) return true;
  if (!deviceEdition) return false;
  const media = normalizeText(mediaEdition);
  const device = normalizeText(deviceEdition);
  return media === device || media.includes(device) || device.includes(media);
}

function isWindowsClientProduct(value: string | null | undefined) {
  const normalized = normalizeText(value);
  return normalized.includes('windows 10') || normalized.includes('windows 11') || normalized === 'windows';
}

function editionMatchesForTarget(targetProduct: string, mediaEdition: string | null, deviceEdition: string | null) {
  if (editionMatches(mediaEdition, deviceEdition)) return true;
  // Windows client ISO media is commonly multi-edition even when blob names include one edition label.
  // Keep server media strict because Standard/Datacenter media selection matters operationally.
  return isWindowsClientProduct(targetProduct);
}

function preflightEditionLanguage(row: PreflightDeviceRow | undefined) {
  const checks = Array.isArray(row?.checkResults) ? row?.checkResults : [];
  const editionCheck = checks.find((item) => asRecord(item).id === 'edition_language');
  const details = asRecord(asRecord(editionCheck).details);
  return {
    edition: readString(details.edition),
    language: readString(details.locale ?? details.language)
  };
}

function selectIsoMediaForPreflight(row: PreflightDeviceRow | undefined, media: IsoMediaRow[]) {
  if (!row) return null;
  const { edition, language } = preflightEditionLanguage(row);
  const candidates = media
    .filter((item) =>
      item.active &&
      item.osFamily === 'windows' &&
      mediaProductMatches(row.targetProduct, item.product) &&
      mediaVersionMatches(row.targetVersion, item.version) &&
      isX64MediaArchitecture(item.architecture) &&
      editionMatchesForTarget(row.targetProduct, item.edition, edition) &&
      languageMatches(item.language, language)
    )
    .map((item) => {
      let score = 0;
      if (normalizeText(item.product) === normalizeText(row.targetProduct)) score += 20;
      if (normalizeText(item.version) === normalizeText(row.targetVersion)) score += 20;
      score += editionMatches(item.edition, edition) ? (item.edition ? 10 : 4) : 1;
      score += item.language ? 10 : 4;
      if (item.sizeBytes !== null) score += 1;
      return { item, score };
    })
    .sort((left, right) => right.score - left.score || left.item.displayName.localeCompare(right.item.displayName));
  return candidates[0]?.item ?? null;
}

type InferredIsoMedia = {
  displayName: string;
  osFamily: string;
  product: string;
  version: string;
  edition: string | null;
  architecture: string;
  language: string | null;
  sha256: string | null;
  sizeBytes: number | null;
  containerName: string;
  blobName: string;
};

function metadataValue(metadata: Record<string, string | undefined> | undefined, ...keys: string[]) {
  if (!metadata) return null;
  const normalized = new Map(Object.entries(metadata).map(([key, value]) => [key.toLowerCase(), value]));
  for (const key of keys) {
    const value = normalized.get(key.toLowerCase());
    if (value && value.trim()) return value.trim();
  }
  return null;
}

function titleFromBlobName(blobName: string) {
  const basename = blobName.split(/[\\/]/).pop() ?? blobName;
  return basename
    .replace(/\.iso$/i, '')
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function inferIsoMediaFromBlob(
  containerName: string,
  blobName: string,
  metadata: Record<string, string | undefined> | undefined,
  sizeBytes: number | null | undefined
): InferredIsoMedia | null {
  if (!/\.iso$/i.test(blobName)) return null;

  const searchable = normalizeText(`${blobName} ${Object.values(metadata ?? {}).join(' ')}`);
  const product =
    metadataValue(metadata, 'product', 'targetProduct') ??
    (searchable.includes('server 2025') || searchable.includes('windows server 2025')
      ? 'Windows Server 2025'
      : searchable.includes('windows 11') || searchable.includes('win11') || searchable.includes('25h2')
        ? 'Windows 11'
        : null);
  const version =
    metadataValue(metadata, 'version', 'targetVersion') ??
    (searchable.includes('25h2')
      ? '25H2'
      : searchable.includes('server 2025') || searchable.includes('windows server 2025')
        ? '2025'
        : null);
  const architecture =
    metadataValue(metadata, 'architecture', 'arch') ??
    (/\b(x64|amd64|64-bit|64bit)\b/i.test(searchable) ? 'x64' : null);

  if (!product || !version || !architecture) return null;

  const explicitEdition = metadataValue(metadata, 'edition', 'sku');
  const edition =
    explicitEdition ??
    (isWindowsClientProduct(product)
      ? null
      : searchable.includes('datacenter')
        ? 'Datacenter'
        : searchable.includes('standard')
          ? 'Standard'
          : null);
  const language =
    metadataValue(metadata, 'language', 'locale', 'culture') ??
    (/\ben[-_ ]?us\b/i.test(searchable)
      ? 'en-US'
      : /\ben[-_ ]?gb\b/i.test(searchable)
        ? 'en-GB'
        : null);
  const displayName =
    metadataValue(metadata, 'displayName', 'display_name', 'name') ??
    titleFromBlobName(blobName) ??
    `${product} ${version} ${architecture}`;
  const sha256 = metadataValue(metadata, 'sha256', 'sha256Hash', 'hash');

  return {
    displayName,
    osFamily: 'windows',
    product,
    version,
    edition,
    architecture,
    language,
    sha256,
    sizeBytes: typeof sizeBytes === 'number' && Number.isFinite(sizeBytes) ? sizeBytes : null,
    containerName,
    blobName
  };
}

async function syncIsoMediaFromBlobStorage() {
  if (!env.azureStorageConnectionString) return 0;

  const containerName = isoContainerName();
  const containerClient = getBlobServiceClient().getContainerClient(containerName);
  const exists = await containerClient.exists();
  if (!exists) return 0;

  let synced = 0;
  for await (const blob of containerClient.listBlobsFlat({ includeMetadata: true })) {
    const inferred = inferIsoMediaFromBlob(containerName, blob.name, blob.metadata, blob.properties.contentLength ?? null);
    if (!inferred) continue;

    await prisma.$executeRaw(Prisma.sql`
      INSERT INTO public.feature_upgrade_iso_media
        (id, display_name, os_family, product, version, edition, architecture, language, sha256, size_bytes, container_name, blob_name, active, created_at, updated_at)
      VALUES
        (
          ${randomUUID()},
          ${inferred.displayName},
          ${inferred.osFamily},
          ${inferred.product},
          ${inferred.version},
          ${inferred.edition},
          ${inferred.architecture},
          ${inferred.language},
          ${inferred.sha256},
          ${inferred.sizeBytes},
          ${inferred.containerName},
          ${inferred.blobName},
          true,
          NOW(),
          NOW()
        )
      ON CONFLICT (container_name, blob_name)
      DO UPDATE SET
        display_name = EXCLUDED.display_name,
        os_family = EXCLUDED.os_family,
        product = EXCLUDED.product,
        version = EXCLUDED.version,
        edition = EXCLUDED.edition,
        architecture = EXCLUDED.architecture,
        language = EXCLUDED.language,
        sha256 = COALESCE(EXCLUDED.sha256, public.feature_upgrade_iso_media.sha256),
        size_bytes = EXCLUDED.size_bytes,
        active = true,
        updated_at = NOW()
    `);
    synced += 1;
  }

  return synced;
}

async function syncSetupCommandMatrixFromIsoMedia() {
  await prisma.$executeRaw(Prisma.sql`
    INSERT INTO public.feature_upgrade_setup_command_matrix
      (
        id, iso_media_id, os_family, product, version, edition, architecture, language,
        setup_executable, arguments_jsonb, dynamic_update_mode, requires_eula_accept,
        image_index_strategy, supported, notes, active, created_at, updated_at
      )
    SELECT
      CONCAT('matrix-', media.id),
      media.id,
      media.os_family,
      media.product,
      media.version,
      media.edition,
      media.architecture,
      media.language,
      '{mount_drive}\\setup.exe',
      CASE
        WHEN media.product ILIKE '%windows server%' AND media.version ILIKE '%2008%' THEN '[]'::jsonb
        WHEN media.product ILIKE '%windows server%' AND media.version = '2025' THEN
          '["/auto","upgrade","/quiet","/eula","accept","/pkey","{target_server_gvlk}","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
        WHEN media.product ILIKE '%windows 11%' THEN
          '["/auto","upgrade","/quiet","/eula","accept","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
        ELSE
          '["/auto","upgrade","/quiet","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
      END,
      'disable',
      CASE
        WHEN media.product ILIKE '%windows 11%' OR (media.product ILIKE '%windows server%' AND media.version = '2025') THEN true
        ELSE false
      END,
      'auto_match_current_edition',
      CASE
        WHEN media.product ILIKE '%windows server%' AND media.version ILIKE '%2008%' THEN false
        ELSE true
      END,
      CASE
        WHEN media.product ILIKE '%windows server%' AND media.version ILIKE '%2008%' THEN 'Unsupported by the v1 automated in-place feature upgrade flow.'
        ELSE 'Seeded from Microsoft Windows Setup command-line options for silent in-place upgrades.'
      END,
      true,
      NOW(),
      NOW()
    FROM public.feature_upgrade_iso_media media
    WHERE media.os_family = 'windows'
    ON CONFLICT (iso_media_id) DO NOTHING
  `);
}

async function loadActiveIsoMedia() {
  await syncIsoMediaFromBlobStorage().catch(() => 0);
  await syncSetupCommandMatrixFromIsoMedia().catch(() => undefined);

  const rows = await prisma.$queryRaw<IsoMediaRow[]>(Prisma.sql`
    SELECT
      id,
      display_name AS "displayName",
      os_family AS "osFamily",
      product,
      version,
      edition,
      architecture,
      language,
      sha256,
      size_bytes AS "sizeBytes",
      container_name AS "containerName",
      blob_name AS "blobName",
      active,
      created_at AS "createdAt",
      updated_at AS "updatedAt"
    FROM public.feature_upgrade_iso_media
    WHERE active = true
      AND os_family = 'windows'
    ORDER BY product ASC, version DESC, edition ASC, architecture ASC
  `);

  return filterExistingIsoMedia(rows);
}

async function filterExistingIsoMedia(rows: IsoMediaRow[]) {
  if (!isoBlobStorageConfigured() || rows.length === 0) return rows;

  const blobService = getBlobServiceClient();
  const available: IsoMediaRow[] = [];
  for (const row of rows) {
    try {
      const exists = await blobService
        .getContainerClient(row.containerName || isoContainerName())
        .getBlobClient(row.blobName)
        .exists();
      if (exists) available.push(row);
    } catch {
      // Treat storage lookup failures as unavailable so stale DB rows are not queued for workers.
    }
  }
  return available;
}

async function loadLatestPreflightRows(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return new Map<string, PreflightDeviceRow>();
  const rows = await prisma.$queryRaw<PreflightDeviceRow[]>(Prisma.sql`
    SELECT DISTINCT ON (p.agent_id)
      p.operation_id AS "operationId",
      p.run_id AS "runId",
      p.organization_id AS "organizationId",
      p.agent_id AS "agentId",
      d.hostname,
      p.source_os AS "sourceOs",
      p.target_product AS "targetProduct",
      p.target_version AS "targetVersion",
      p.target_build_label AS "targetBuildLabel",
      p.status,
      p.phase,
      p.check_results_jsonb AS "checkResults",
      p.failure_summary_jsonb AS "failureSummary",
      p.warning_summary_jsonb AS "warningSummary",
      p.requested_by AS "requestedBy",
      p.claimed_at AS "claimedAt",
      p.started_at AS "startedAt",
      p.finished_at AS "finishedAt",
      p.created_at AS "createdAt",
      p.updated_at AS "updatedAt"
    FROM public.feature_upgrade_preflight_device p
    LEFT JOIN public.rmm_devices d ON d.organization_id = p.organization_id AND d.agent_id = p.agent_id
    WHERE p.organization_id = ${organizationId}
      AND p.agent_id IN (${Prisma.join(agentIds)})
    ORDER BY p.agent_id, p.updated_at DESC, p.created_at DESC
  `);
  return new Map(rows.map((row) => [row.agentId, row]));
}

async function loadLatestIsoStageRows(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return new Map<string, IsoStageDeviceRow>();
  const rows = await prisma.$queryRaw<IsoStageDeviceRow[]>(Prisma.sql`
    SELECT DISTINCT ON (s.agent_id)
      s.operation_id AS "operationId",
      s.run_id AS "runId",
      s.organization_id AS "organizationId",
      s.agent_id AS "agentId",
      d.hostname,
      s.iso_media_id AS "isoMediaId",
      m.display_name AS "isoDisplayName",
      s.source_os AS "sourceOs",
      s.target_product AS "targetProduct",
      s.target_version AS "targetVersion",
      s.target_build_label AS "targetBuildLabel",
      s.status,
      s.phase,
      s.progress_jsonb AS progress,
      s.evidence_jsonb AS evidence,
      s.error_message AS "errorMessage",
      s.size_bytes AS "sizeBytes",
      s.sha256,
      s.requested_by AS "requestedBy",
      s.claimed_at AS "claimedAt",
      s.started_at AS "startedAt",
      s.staged_at AS "stagedAt",
      s.expires_at AS "expiresAt",
      s.cleaned_at AS "cleanedAt",
      s.finished_at AS "finishedAt",
      s.created_at AS "createdAt",
      s.updated_at AS "updatedAt"
    FROM public.feature_upgrade_iso_stage_device s
    LEFT JOIN public.rmm_devices d ON d.organization_id = s.organization_id AND d.agent_id = s.agent_id
    LEFT JOIN public.feature_upgrade_iso_media m ON m.id = s.iso_media_id
    WHERE s.organization_id = ${organizationId}
      AND s.agent_id IN (${Prisma.join(agentIds)})
    ORDER BY s.agent_id,
      CASE
        WHEN s.status = 'staged' AND (s.expires_at IS NULL OR s.expires_at > NOW()) THEN 0
        WHEN s.status IN ('queued', 'running') THEN 1
        ELSE 2
      END,
      s.updated_at DESC,
      s.created_at DESC
  `);
  return new Map(rows.map((row) => [row.agentId, row]));
}

async function loadSetupCommandMatrixRows(mediaIds: string[]) {
  if (mediaIds.length === 0) return new Map<string, SetupCommandMatrixRow>();
  const rows = await prisma.$queryRaw<SetupCommandMatrixRow[]>(Prisma.sql`
    SELECT
      id,
      iso_media_id AS "isoMediaId",
      os_family AS "osFamily",
      product,
      version,
      edition,
      architecture,
      language,
      setup_executable AS "setupExecutable",
      arguments_jsonb AS arguments,
      dynamic_update_mode AS "dynamicUpdateMode",
      requires_eula_accept AS "requiresEulaAccept",
      image_index_strategy AS "imageIndexStrategy",
      supported,
      notes,
      active,
      created_at AS "createdAt",
      updated_at AS "updatedAt"
    FROM public.feature_upgrade_setup_command_matrix
    WHERE iso_media_id IN (${Prisma.join(mediaIds)})
      AND active = true
  `);
  return new Map(rows.map((row) => [row.isoMediaId, row]));
}

async function loadLatestFeatureUpgradeRows(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return new Map<string, FeatureUpgradeDeviceRow>();
  const rows = await prisma.$queryRaw<FeatureUpgradeDeviceRow[]>(Prisma.sql`
    SELECT DISTINCT ON (u.agent_id)
      u.operation_id AS "operationId",
      u.run_id AS "runId",
      u.organization_id AS "organizationId",
      u.agent_id AS "agentId",
      d.hostname,
      u.preflight_operation_id AS "preflightOperationId",
      u.iso_media_id AS "isoMediaId",
      m.display_name AS "isoDisplayName",
      u.setup_command_matrix_id AS "setupCommandMatrixId",
      u.source_os AS "sourceOs",
      u.target_product AS "targetProduct",
      u.target_version AS "targetVersion",
      u.target_build_label AS "targetBuildLabel",
      u.status,
      u.phase,
      u.progress_jsonb AS progress,
      u.evidence_jsonb AS evidence,
      u.failure_summary_jsonb AS "failureSummary",
      u.error_message AS "errorMessage",
      u.size_bytes AS "sizeBytes",
      u.sha256,
      u.scheduled_for AS "scheduledFor",
      u.requested_by AS "requestedBy",
      u.claimed_at AS "claimedAt",
      u.started_at AS "startedAt",
      u.final_snapshot_at AS "finalSnapshotAt",
      u.setup_started_at AS "setupStartedAt",
      u.reboot_detected_at AS "rebootDetectedAt",
      u.verified_at AS "verifiedAt",
      u.finished_at AS "finishedAt",
      u.created_at AS "createdAt",
      u.updated_at AS "updatedAt"
    FROM public.feature_upgrade_device u
    LEFT JOIN public.rmm_devices d ON d.organization_id = u.organization_id AND d.agent_id = u.agent_id
    LEFT JOIN public.feature_upgrade_iso_media m ON m.id = u.iso_media_id
    WHERE u.organization_id = ${organizationId}
      AND u.agent_id IN (${Prisma.join(agentIds)})
    ORDER BY u.agent_id, u.updated_at DESC, u.created_at DESC
  `);
  return new Map(rows.map((row) => [row.agentId, row]));
}

function setupCommandMatrixResponse(row: SetupCommandMatrixRow) {
  return {
    id: row.id,
    isoMediaId: row.isoMediaId,
    osFamily: row.osFamily,
    product: row.product,
    version: row.version,
    edition: row.edition,
    architecture: row.architecture,
    language: row.language,
    setupExecutable: row.setupExecutable,
    arguments: Array.isArray(row.arguments) ? row.arguments : [],
    dynamicUpdateMode: row.dynamicUpdateMode,
    requiresEulaAccept: row.requiresEulaAccept,
    imageIndexStrategy: row.imageIndexStrategy,
    supported: row.supported,
    notes: row.notes,
    active: row.active,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

function featureUpgradeProgressPayloadFromRow(row: FeatureUpgradeDeviceRow) {
  const progress = asRecord(row.progress);
  const defaultPercent =
    row.status === 'succeeded' ? 100 :
    row.status === 'failed' || row.status === 'cancelled' ? 100 :
    row.status === 'awaiting_reboot' ? 80 :
    row.status === 'verifying' ? 90 :
    row.status === 'running' ? 45 :
    0;
  return {
    schemaVersion: readNumber(progress.schemaVersion) ?? 1,
    eventType: 'feature_upgrade.start.progress',
    organizationId: row.organizationId,
    agentId: row.agentId,
    operationId: row.operationId,
    runId: row.runId,
    jobId: row.operationId,
    commandId: row.operationId,
    isoMedia: {
      id: row.isoMediaId,
      displayName: row.isoDisplayName,
      sizeBytes: row.sizeBytes === null ? null : Number(row.sizeBytes),
      sha256: row.sha256
    },
    status: row.status,
    phase: row.phase,
    reportedAt: readString(progress.reportedAt ?? progress.reported_at) ?? row.updatedAt.toISOString(),
    receivedAt: row.updatedAt.toISOString(),
    overallPercent: readNumber(progress.overallPercent ?? progress.overall_percent) ?? defaultPercent,
    phasePercent: readNumber(progress.phasePercent ?? progress.phase_percent) ?? defaultPercent,
    scheduledFor: row.scheduledFor?.toISOString() ?? null,
    finalSnapshotAt: row.finalSnapshotAt?.toISOString() ?? null,
    setupStartedAt: row.setupStartedAt?.toISOString() ?? null,
    rebootDetectedAt: row.rebootDetectedAt?.toISOString() ?? null,
    verifiedAt: row.verifiedAt?.toISOString() ?? null,
    error: readString(progress.error) ?? row.errorMessage
  };
}

function featureUpgradeDeviceResponse(row: FeatureUpgradeDeviceRow) {
  return {
    operationId: row.operationId,
    runId: row.runId,
    organizationId: row.organizationId,
    agentId: row.agentId,
    hostname: row.hostname,
    preflightOperationId: row.preflightOperationId,
    isoMediaId: row.isoMediaId,
    isoDisplayName: row.isoDisplayName,
    setupCommandMatrixId: row.setupCommandMatrixId,
    sourceOs: row.sourceOs,
    targetProduct: row.targetProduct,
    targetVersion: row.targetVersion,
    targetBuildLabel: row.targetBuildLabel,
    status: row.status,
    phase: row.phase,
    progress: featureUpgradeProgressPayloadFromRow(row),
    evidence: asRecord(row.evidence),
    failureSummary: Array.isArray(row.failureSummary) ? row.failureSummary : [],
    errorMessage: row.errorMessage,
    sizeBytes: row.sizeBytes === null ? null : Number(row.sizeBytes),
    sha256: row.sha256,
    scheduledFor: row.scheduledFor?.toISOString() ?? null,
    requestedBy: row.requestedBy,
    claimedAt: row.claimedAt?.toISOString() ?? null,
    startedAt: row.startedAt?.toISOString() ?? null,
    finalSnapshotAt: row.finalSnapshotAt?.toISOString() ?? null,
    setupStartedAt: row.setupStartedAt?.toISOString() ?? null,
    rebootDetectedAt: row.rebootDetectedAt?.toISOString() ?? null,
    verifiedAt: row.verifiedAt?.toISOString() ?? null,
    finishedAt: row.finishedAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

async function updateFeatureUpgradeRunCounts(runId: string) {
  await prisma.$executeRaw(Prisma.sql`
    WITH counts AS (
      SELECT
        run_id,
        COUNT(*)::int AS total,
        COUNT(*) FILTER (WHERE status = 'scheduled')::int AS scheduled,
        COUNT(*) FILTER (WHERE status = 'queued')::int AS queued,
        COUNT(*) FILTER (WHERE status = 'running')::int AS running,
        COUNT(*) FILTER (WHERE status = 'awaiting_reboot')::int AS awaiting,
        COUNT(*) FILTER (WHERE status = 'verifying')::int AS verifying,
        COUNT(*) FILTER (WHERE status = 'succeeded')::int AS succeeded,
        COUNT(*) FILTER (WHERE status = 'failed')::int AS failed,
        COUNT(*) FILTER (WHERE status IN ('succeeded', 'failed', 'cancelled'))::int AS terminal
      FROM public.feature_upgrade_device
      WHERE run_id = ${runId}
      GROUP BY run_id
    )
    UPDATE public.feature_upgrade_run r
    SET
      total_devices = counts.total,
      scheduled_devices = counts.scheduled,
      queued_devices = counts.queued,
      running_devices = counts.running,
      awaiting_devices = counts.awaiting,
      verifying_devices = counts.verifying,
      succeeded_devices = counts.succeeded,
      failed_devices = counts.failed,
      status = CASE
        WHEN counts.running > 0 OR counts.awaiting > 0 OR counts.verifying > 0 THEN 'running'
        WHEN counts.queued > 0 THEN 'queued'
        WHEN counts.scheduled > 0 THEN 'scheduled'
        WHEN counts.failed > 0 THEN 'failed'
        ELSE 'completed'
      END,
      finished_at = CASE
        WHEN counts.terminal = counts.total THEN COALESCE(r.finished_at, NOW())
        ELSE NULL
      END,
      updated_at = NOW()
    FROM counts
    WHERE r.id = counts.run_id
  `);
}

async function notifyFeatureUpgradeStartJobsAvailable(agentIds: string[], reason: string, requestedBy?: string | null) {
  const baseUrl = env.rmmServerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey || agentIds.length === 0) return;

  await fetch(`${baseUrl}/api/rmm/internal/feature-upgrades/start/notify`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-rmm-server-key': serverKey
    },
    body: JSON.stringify({ agentIds, reason, requestedBy })
  }).catch(() => undefined);
}

function isoStageProgressPayloadFromRow(row: IsoStageDeviceRow) {
  const progress = asRecord(row.progress);
  const sizeBytes = row.sizeBytes === null ? null : Number(row.sizeBytes);
  const bytesTotal = readNumber(progress.bytesTotal ?? progress.bytes_total) ?? sizeBytes;
  const bytesDownloaded = readNumber(progress.bytesDownloaded ?? progress.bytes_downloaded) ?? (
    row.status === 'staged' || row.status === 'deleted' || row.status === 'expired' ? bytesTotal : 0
  );
  const defaultPercent = row.status === 'queued' || row.status === 'running' ? 0 : 100;
  return {
    schemaVersion: readNumber(progress.schemaVersion) ?? 1,
    eventType: 'feature_upgrade.iso.stage.progress',
    organizationId: row.organizationId,
    agentId: row.agentId,
    operationId: row.operationId,
    runId: row.runId,
    jobId: row.operationId,
    commandId: row.operationId,
    isoMedia: {
      id: row.isoMediaId,
      displayName: row.isoDisplayName,
      sizeBytes,
      sha256: row.sha256
    },
    status: row.status,
    phase: row.phase,
    reportedAt: readString(progress.reportedAt ?? progress.reported_at) ?? row.updatedAt.toISOString(),
    receivedAt: row.updatedAt.toISOString(),
    overallPercent: readNumber(progress.overallPercent ?? progress.overall_percent) ?? defaultPercent,
    phasePercent: readNumber(progress.phasePercent ?? progress.phase_percent) ?? defaultPercent,
    bytesDownloaded,
    bytesTotal,
    bytesPerSecond: readNumber(progress.bytesPerSecond ?? progress.bytes_per_second),
    stagedAt: row.stagedAt?.toISOString() ?? null,
    expiresAt: row.expiresAt?.toISOString() ?? null,
    cleanedAt: row.cleanedAt?.toISOString() ?? null,
    error: readString(progress.error) ?? row.errorMessage
  };
}

function isoStageDeviceResponse(row: IsoStageDeviceRow) {
  return {
    operationId: row.operationId,
    runId: row.runId,
    organizationId: row.organizationId,
    agentId: row.agentId,
    hostname: row.hostname,
    isoMediaId: row.isoMediaId,
    isoDisplayName: row.isoDisplayName,
    sourceOs: row.sourceOs,
    targetProduct: row.targetProduct,
    targetVersion: row.targetVersion,
    targetBuildLabel: row.targetBuildLabel,
    status: row.status,
    phase: row.phase,
    progress: isoStageProgressPayloadFromRow(row),
    evidence: asRecord(row.evidence),
    errorMessage: row.errorMessage,
    sizeBytes: row.sizeBytes === null ? null : Number(row.sizeBytes),
    sha256: row.sha256,
    requestedBy: row.requestedBy,
    claimedAt: row.claimedAt?.toISOString() ?? null,
    startedAt: row.startedAt?.toISOString() ?? null,
    stagedAt: row.stagedAt?.toISOString() ?? null,
    expiresAt: row.expiresAt?.toISOString() ?? null,
    cleanedAt: row.cleanedAt?.toISOString() ?? null,
    finishedAt: row.finishedAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

async function updateIsoStageRunCounts(runId: string) {
  await prisma.$executeRaw(Prisma.sql`
    WITH counts AS (
      SELECT
        run_id,
        COUNT(*)::int AS total,
        COUNT(*) FILTER (WHERE status = 'queued')::int AS queued,
        COUNT(*) FILTER (WHERE status = 'running')::int AS running,
        COUNT(*) FILTER (WHERE status = 'staged')::int AS staged,
        COUNT(*) FILTER (WHERE status = 'failed')::int AS failed,
        COUNT(*) FILTER (WHERE status IN ('deleted', 'expired'))::int AS deleted,
        COUNT(*) FILTER (WHERE status IN ('staged', 'failed', 'cancelled', 'deleted', 'expired'))::int AS terminal
      FROM public.feature_upgrade_iso_stage_device
      WHERE run_id = ${runId}
      GROUP BY run_id
    )
    UPDATE public.feature_upgrade_iso_stage_run r
    SET
      total_devices = counts.total,
      queued_devices = counts.queued,
      running_devices = counts.running,
      staged_devices = counts.staged,
      failed_devices = counts.failed,
      deleted_devices = counts.deleted,
      status = CASE
        WHEN counts.running > 0 THEN 'running'
        WHEN counts.queued > 0 THEN 'queued'
        WHEN counts.failed > 0 THEN 'failed'
        ELSE 'completed'
      END,
      finished_at = CASE
        WHEN counts.terminal = counts.total THEN COALESCE(r.finished_at, NOW())
        ELSE NULL
      END,
      updated_at = NOW()
    FROM counts
    WHERE r.id = counts.run_id
  `);
}

async function notifyStageIsoJobsAvailable(agentIds: string[], reason: string, requestedBy?: string | null) {
  const baseUrl = env.rmmServerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey || agentIds.length === 0) return;

  await fetch(`${baseUrl}/api/rmm/internal/feature-upgrades/stage-iso/notify`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-rmm-server-key': serverKey
    },
    body: JSON.stringify({ agentIds, reason, requestedBy })
  }).catch(() => undefined);
}

function buildStagePreviewDevice(
  device: DeviceRow,
  preflight: PreflightDeviceRow | undefined,
  stage: IsoStageDeviceRow | undefined,
  mediaRows: IsoMediaRow[]
): StageIsoPreviewDevice | null {
  const fallbackTarget = inferFeatureUpgradePreflightTarget(device.os);
  const target = preflight
    ? {
        targetProduct: preflight.targetProduct,
        targetVersion: preflight.targetVersion,
        targetBuildLabel: preflight.targetBuildLabel
      }
    : fallbackTarget;
  if (!target) return null;

  const stagedMedia = stage && stage.status === 'staged' && (!stage.expiresAt || stage.expiresAt.getTime() > Date.now())
    ? mediaRows.find((media) => media.id === stage.isoMediaId && media.active) ?? null
    : null;
  const stagedMediaMatchesPreflight = Boolean(
    stagedMedia &&
    preflight &&
    mediaProductMatches(preflight.targetProduct, stagedMedia.product) &&
    mediaVersionMatches(preflight.targetVersion, stagedMedia.version) &&
    isX64MediaArchitecture(stagedMedia.architecture) &&
    editionMatchesForTarget(preflight.targetProduct, stagedMedia.edition, preflightEditionLanguage(preflight).edition) &&
    languageMatches(stagedMedia.language, preflightEditionLanguage(preflight).language)
  );
  const validStagedMedia = stagedMediaMatchesPreflight ? stagedMedia : null;
  const isoMedia = validStagedMedia ?? selectIsoMediaForPreflight(preflight, mediaRows);
  const blockingReasons: string[] = [];
  const warnings: string[] = [];
  const activeStage = stage && ['queued', 'running'].includes(stage.status) ? stage : null;
  const alreadyStaged = stage && stage.status === 'staged' && (!stage.expiresAt || stage.expiresAt.getTime() > Date.now()) ? stage : null;

  if (!preflight) {
    blockingReasons.push('Run feature upgrade preflight before staging ISO');
  } else if (!['passed', 'warning'].includes(preflight.status)) {
    blockingReasons.push(preflight.status === 'failed' ? 'Latest preflight failed' : 'Latest preflight has not completed');
  }
  if (preflight?.status === 'warning') {
    warnings.push('Preflight passed with warnings; review before staging');
  }
  const storageReadinessIssue = isoBlobStorageReadinessIssue();
  if (storageReadinessIssue) {
    blockingReasons.push(storageReadinessIssue);
  }
  if (isoMedia && isWindowsClientProduct(preflight?.targetProduct) && !editionMatches(isoMedia.edition, preflightEditionLanguage(preflight).edition)) {
    warnings.push('Selected Windows client ISO may be multi-edition; verify it includes this device edition before staging');
  }
  if (!isoMedia) {
    blockingReasons.push('No active ISO media matches this device target, edition, and language');
  }
  if (activeStage) {
    blockingReasons.push(activeStage.status === 'queued' ? 'ISO staging is already queued' : 'ISO staging is already running');
  }
  if (alreadyStaged && alreadyStaged.isoMediaId === isoMedia?.id) {
    blockingReasons.push(`ISO already staged until ${alreadyStaged.expiresAt?.toISOString() ?? 'the recorded expiry time'}`);
  }

  return {
    agentId: device.agentId,
    hostname: device.hostname,
    os: device.os,
    osVersion: device.osVersion,
    customerId: device.customerId,
    customerName: device.customerName,
    siteId: device.siteId,
    siteName: device.siteName,
    targetProduct: target.targetProduct,
    targetVersion: target.targetVersion,
    targetBuildLabel: target.targetBuildLabel,
    preflightStatus: preflight?.status ?? null,
    preflightOperationId: preflight?.operationId ?? null,
    canStage: blockingReasons.length === 0,
    blockingReasons,
    warnings,
    isoMedia: isoMedia ? toIsoMediaResponse(isoMedia) : null,
    existingStage: stage ? isoStageDeviceResponse(stage) : null,
    expectedSizeBytes: isoMedia?.sizeBytes === null || isoMedia?.sizeBytes === undefined ? null : Number(isoMedia.sizeBytes),
    estimatedExpiresAt: stageExpiryFromNow().toISOString()
  };
}

async function buildStageIsoPreview(organizationId: string, agentIds: string[]) {
  const devices = await loadWindowsDevicesForPreflight(organizationId, agentIds);
  await reconcileCompletedPreflightSnapshots(organizationId, devices.map((device) => device.agentId));
  const [preflightByAgentId, stageByAgentId, mediaRows] = await Promise.all([
    loadLatestPreflightRows(organizationId, devices.map((device) => device.agentId)),
    loadLatestIsoStageRows(organizationId, devices.map((device) => device.agentId)),
    loadActiveIsoMedia()
  ]);
  const previewDevices = devices
    .map((device) => buildStagePreviewDevice(device, preflightByAgentId.get(device.agentId), stageByAgentId.get(device.agentId), mediaRows))
    .filter(Boolean) as StageIsoPreviewDevice[];
  const foundIds = new Set(devices.map((device) => device.agentId));
  const skipped = agentIds
    .filter((agentId) => !foundIds.has(agentId))
    .map((agentId) => ({ agentId, reason: 'not_found_or_not_windows' }));
  return { devices: previewDevices, skipped };
}

function buildFeatureUpgradeStartPreviewDevice(
  device: DeviceRow,
  preflight: PreflightDeviceRow | undefined,
  stage: IsoStageDeviceRow | undefined,
  existingUpgrade: FeatureUpgradeDeviceRow | undefined,
  mediaRows: IsoMediaRow[],
  setupRowsByMediaId: Map<string, SetupCommandMatrixRow>
): FeatureUpgradeStartPreviewDevice | null {
  const fallbackTarget = inferFeatureUpgradePreflightTarget(device.os);
  const target = preflight
    ? {
        targetProduct: preflight.targetProduct,
        targetVersion: preflight.targetVersion,
        targetBuildLabel: preflight.targetBuildLabel
      }
    : fallbackTarget;
  if (!target) return null;

  const stagedMedia = stage && stage.status === 'staged' && (!stage.expiresAt || stage.expiresAt.getTime() > Date.now())
    ? mediaRows.find((media) => media.id === stage.isoMediaId && media.active) ?? null
    : null;
  const stagedMediaMatchesPreflight = Boolean(
    stagedMedia &&
    preflight &&
    mediaProductMatches(preflight.targetProduct, stagedMedia.product) &&
    mediaVersionMatches(preflight.targetVersion, stagedMedia.version) &&
    isX64MediaArchitecture(stagedMedia.architecture) &&
    editionMatchesForTarget(preflight.targetProduct, stagedMedia.edition, preflightEditionLanguage(preflight).edition) &&
    languageMatches(stagedMedia.language, preflightEditionLanguage(preflight).language)
  );
  const validStagedMedia = stagedMediaMatchesPreflight ? stagedMedia : null;
  const isoMedia = validStagedMedia ?? selectIsoMediaForPreflight(preflight, mediaRows);
  const setupCommand = isoMedia ? setupRowsByMediaId.get(isoMedia.id) ?? null : null;
  const activeUpgrade = existingUpgrade && ['scheduled', 'queued', 'running', 'awaiting_reboot', 'verifying'].includes(existingUpgrade.status)
    ? existingUpgrade
    : null;
  const alreadyStaged = stage && stage.status === 'staged' && (!stage.expiresAt || stage.expiresAt.getTime() > Date.now()) && stage.isoMediaId === isoMedia?.id
    ? stage
    : null;
  const willDownloadIso = Boolean(isoMedia && !alreadyStaged);
  const blockingReasons: string[] = [];
  const warnings: string[] = [];

  if (!preflight) {
    blockingReasons.push('Run feature upgrade preflight before starting the upgrade');
  } else if (preflight.status !== 'passed') {
    blockingReasons.push(preflight.status === 'warning'
      ? 'Latest preflight has warnings; strict start gate requires a passed preflight'
      : preflight.status === 'failed'
        ? 'Latest preflight failed'
        : 'Latest preflight has not completed');
  }
  if (!isoMedia) {
    blockingReasons.push('No active ISO media matches this device target, edition, and language');
  }
  if (stagedMedia && !validStagedMedia) {
    warnings.push('Existing staged ISO does not match the latest target media requirements and will not be reused');
  }
  if (!setupCommand) {
    blockingReasons.push('No setup command matrix row is available for the selected ISO');
  } else if (!setupCommand.supported) {
    blockingReasons.push(setupCommand.notes ?? 'The selected ISO is not supported by automated start upgrade');
  }
  if (activeUpgrade) {
    blockingReasons.push(activeUpgrade.status === 'scheduled'
      ? `Upgrade is already scheduled for ${activeUpgrade.scheduledFor?.toISOString() ?? 'a recorded time'}`
      : 'Feature upgrade is already queued or running');
  }
  const storageReadinessIssue = isoBlobStorageReadinessIssue();
  if (willDownloadIso && storageReadinessIssue) {
    blockingReasons.push(storageReadinessIssue);
  }
  if (isoMedia && isWindowsClientProduct(preflight?.targetProduct) && !editionMatches(isoMedia.edition, preflightEditionLanguage(preflight).edition)) {
    warnings.push('Selected Windows client ISO may be multi-edition; setup will use the existing edition during upgrade');
  }
  if (willDownloadIso) {
    warnings.push('ISO is not currently staged; the worker will download it before starting Windows Setup');
  }

  return {
    agentId: device.agentId,
    hostname: device.hostname,
    os: device.os,
    osVersion: device.osVersion,
    customerId: device.customerId,
    customerName: device.customerName,
    siteId: device.siteId,
    siteName: device.siteName,
    targetProduct: target.targetProduct,
    targetVersion: target.targetVersion,
    targetBuildLabel: target.targetBuildLabel,
    preflightStatus: preflight?.status ?? null,
    preflightOperationId: preflight?.operationId ?? null,
    canStart: blockingReasons.length === 0,
    blockingReasons,
    warnings,
    isoMedia: isoMedia ? toIsoMediaResponse(isoMedia) : null,
    setupCommand: setupCommand ? setupCommandMatrixResponse(setupCommand) : null,
    existingStage: stage ? isoStageDeviceResponse(stage) : null,
    existingUpgrade: existingUpgrade ? featureUpgradeDeviceResponse(existingUpgrade) : null,
    expectedSizeBytes: isoMedia?.sizeBytes === null || isoMedia?.sizeBytes === undefined ? null : Number(isoMedia.sizeBytes),
    willDownloadIso
  };
}

async function buildFeatureUpgradeStartPreview(organizationId: string, agentIds: string[]) {
  const devices = await loadWindowsDevicesForPreflight(organizationId, agentIds);
  await reconcileCompletedPreflightSnapshots(organizationId, devices.map((device) => device.agentId));
  const deviceAgentIds = devices.map((device) => device.agentId);
  const [preflightByAgentId, stageByAgentId, upgradeByAgentId, mediaRows] = await Promise.all([
    loadLatestPreflightRows(organizationId, deviceAgentIds),
    loadLatestIsoStageRows(organizationId, deviceAgentIds),
    loadLatestFeatureUpgradeRows(organizationId, deviceAgentIds),
    loadActiveIsoMedia()
  ]);
  const setupRowsByMediaId = await loadSetupCommandMatrixRows(mediaRows.map((media) => media.id));
  const previewDevices = devices
    .map((device) => buildFeatureUpgradeStartPreviewDevice(
      device,
      preflightByAgentId.get(device.agentId),
      stageByAgentId.get(device.agentId),
      upgradeByAgentId.get(device.agentId),
      mediaRows,
      setupRowsByMediaId
    ))
    .filter(Boolean) as FeatureUpgradeStartPreviewDevice[];
  const foundIds = new Set(devices.map((device) => device.agentId));
  const skipped = agentIds
    .filter((agentId) => !foundIds.has(agentId))
    .map((agentId) => ({ agentId, reason: 'not_found_or_not_windows' }));
  return { devices: previewDevices, skipped };
}

featureUpgradesRouter.post('/preflight/preview', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;

    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    const devices = await loadWindowsDevicesForPreflight(membership.organizationId, agentIds);
    const factsByAgentId = await loadPreflightFacts(membership.organizationId, devices.map((device) => device.agentId));
    const previewDevices = devices.map((device) => toPreviewDevice(device, factsByAgentId, 'preview')).filter(Boolean);
    const foundIds = new Set(devices.map((device) => device.agentId));
    const skipped = agentIds
      .filter((agentId) => !foundIds.has(agentId))
      .map((agentId) => ({ agentId, reason: 'not_found_or_not_windows' }));

    return res.json({
      diskFreeBytesRequired: FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES,
      devices: previewDevices,
      skipped,
      checks: FEATURE_UPGRADE_PREFLIGHT_CHECKS
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/preflight-runs', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) {
      return res.status(403).json({ error: 'Only admins can run feature upgrade preflight checks' });
    }

    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    const devices = await loadWindowsDevicesForPreflight(membership.organizationId, agentIds);
    const factsByAgentId = await loadPreflightFacts(membership.organizationId, devices.map((device) => device.agentId));
    const previewDevices = devices
      .map((device) => toPreviewDevice(device, factsByAgentId, 'preview'))
      .filter(Boolean) as Array<NonNullable<ReturnType<typeof toPreviewDevice>>>;
    if (previewDevices.length === 0) {
      return res.status(400).json({ error: 'No Windows devices found for preflight' });
    }

    const runId = randomUUID();
    const requestedBy = req.jwt!.sub;
    await prisma.$transaction(async (tx) => {
      await tx.$executeRaw(Prisma.sql`
        INSERT INTO public.feature_upgrade_preflight_run
          (id, organization_id, requested_by, status, total_devices, queued_devices, created_at, updated_at)
        VALUES
          (${runId}, ${membership.organizationId}, ${requestedBy}, 'queued', ${previewDevices.length}, ${previewDevices.length}, NOW(), NOW())
      `);

      for (const device of previewDevices) {
        const operationId = randomUUID();
        await tx.$executeRaw(Prisma.sql`
          INSERT INTO public.feature_upgrade_preflight_device
            (
              operation_id, run_id, organization_id, agent_id, source_os,
              target_product, target_version, target_build_label,
              status, phase, check_results_jsonb, failure_summary_jsonb, warning_summary_jsonb,
              requested_by, created_at, updated_at
            )
          VALUES
            (
              ${operationId}, ${runId}, ${membership.organizationId}, ${device.agentId}, ${device.os},
              ${device.targetProduct}, ${device.targetVersion}, ${device.targetBuildLabel},
              'queued', 'queued', ${JSON.stringify(device.checks)}::jsonb, '[]'::jsonb, '[]'::jsonb,
              ${requestedBy}, NOW(), NOW()
            )
        `);
        await tx.rmmTelemetrySnapshotRequest.upsert({
          where: {
            agentId_requestId: {
              agentId: device.agentId,
              requestId: operationId
            }
          },
          create: {
            organizationId: membership.organizationId,
            agentId: device.agentId,
            requestId: operationId,
            status: 'pending'
          },
          update: {
            organizationId: membership.organizationId,
            status: 'pending',
            updatedAt: new Date()
          }
        });
      }
    });

    const rows = await prisma.$queryRaw<PreflightDeviceRow[]>(Prisma.sql`
      SELECT
        p.operation_id AS "operationId",
        p.run_id AS "runId",
        p.organization_id AS "organizationId",
        p.agent_id AS "agentId",
        d.hostname,
        p.source_os AS "sourceOs",
        p.target_product AS "targetProduct",
        p.target_version AS "targetVersion",
        p.target_build_label AS "targetBuildLabel",
        p.status,
        p.phase,
        p.check_results_jsonb AS "checkResults",
        p.failure_summary_jsonb AS "failureSummary",
        p.warning_summary_jsonb AS "warningSummary",
        p.requested_by AS "requestedBy",
        p.claimed_at AS "claimedAt",
        p.started_at AS "startedAt",
        p.finished_at AS "finishedAt",
        p.created_at AS "createdAt",
        p.updated_at AS "updatedAt"
      FROM public.feature_upgrade_preflight_device p
      LEFT JOIN public.rmm_devices d ON d.organization_id = p.organization_id AND d.agent_id = p.agent_id
      WHERE p.run_id = ${runId}
      ORDER BY d.hostname ASC NULLS LAST, p.agent_id ASC
    `);

    await notifyPreflightJobsAvailable(previewDevices.map((device) => device.agentId), 'feature_upgrade_preflight_queued', requestedBy);

    return res.status(202).json({
      runId,
      targetedDevices: previewDevices.length,
      devices: rows.map(preflightDeviceResponse)
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/preflight/progress/query', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.json({ items: [] });

    const allowedDevices = await prisma.rmmDevice.findMany({
      where: { organizationId: membership.organizationId, agentId: { in: agentIds } },
      select: { agentId: true }
    });
    const allowedAgentIds = allowedDevices.map((device) => device.agentId);
    if (allowedAgentIds.length === 0) return res.json({ items: [] });

    await reconcileCompletedPreflightSnapshots(membership.organizationId, allowedAgentIds);

    const rows = await prisma.$queryRaw<PreflightDeviceRow[]>(Prisma.sql`
      SELECT DISTINCT ON (p.agent_id)
        p.operation_id AS "operationId",
        p.run_id AS "runId",
        p.organization_id AS "organizationId",
        p.agent_id AS "agentId",
        d.hostname,
        p.source_os AS "sourceOs",
        p.target_product AS "targetProduct",
        p.target_version AS "targetVersion",
        p.target_build_label AS "targetBuildLabel",
        p.status,
        p.phase,
        p.check_results_jsonb AS "checkResults",
        p.failure_summary_jsonb AS "failureSummary",
        p.warning_summary_jsonb AS "warningSummary",
        p.requested_by AS "requestedBy",
        p.claimed_at AS "claimedAt",
        p.started_at AS "startedAt",
        p.finished_at AS "finishedAt",
        p.created_at AS "createdAt",
        p.updated_at AS "updatedAt"
      FROM public.feature_upgrade_preflight_device p
      LEFT JOIN public.rmm_devices d ON d.organization_id = p.organization_id AND d.agent_id = p.agent_id
      WHERE p.organization_id = ${membership.organizationId}
        AND p.agent_id IN (${Prisma.join(allowedAgentIds)})
      ORDER BY p.agent_id, p.updated_at DESC, p.created_at DESC
    `);

    return res.json({ items: rows.map(preflightDeviceResponse) });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/stage-iso/preview', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;

    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    const preview = await buildStageIsoPreview(membership.organizationId, agentIds);
    const stageable = preview.devices.filter((device) => device.canStage);
    const totalBytes = stageable.reduce((sum, device) => sum + (device.expectedSizeBytes ?? 0), 0);
    return res.json({
      retentionSeconds: ISO_STAGE_RETENTION_SECONDS,
      retentionDays: 7,
      estimatedExpiresAt: stageExpiryFromNow().toISOString(),
      totalSizeBytes: totalBytes,
      devices: preview.devices,
      skipped: preview.skipped
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/stage-iso-runs', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) {
      return res.status(403).json({ error: 'Only admins can stage feature upgrade ISO media' });
    }

    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    const preview = await buildStageIsoPreview(membership.organizationId, agentIds);
    const devicesToStage = preview.devices.filter((device) => device.canStage && device.isoMedia);
    if (devicesToStage.length === 0) {
      return res.status(400).json({
        error: 'No selected devices are ready to stage ISO media',
        devices: preview.devices,
        skipped: preview.skipped
      });
    }

    const runId = randomUUID();
    const requestedBy = req.jwt!.sub;
    const nowIso = new Date().toISOString();
    await prisma.$transaction(async (tx) => {
      await tx.$executeRaw(Prisma.sql`
        INSERT INTO public.feature_upgrade_iso_stage_run
          (id, organization_id, requested_by, status, total_devices, queued_devices, created_at, updated_at)
        VALUES
          (${runId}, ${membership.organizationId}, ${requestedBy}, 'queued', ${devicesToStage.length}, ${devicesToStage.length}, NOW(), NOW())
      `);

      for (const device of devicesToStage) {
        const operationId = randomUUID();
        const media = device.isoMedia!;
        const progress = {
          schemaVersion: 1,
          eventType: 'feature_upgrade.iso.stage.progress',
          organizationId: membership.organizationId,
          agentId: device.agentId,
          operationId,
          runId,
          jobId: operationId,
          commandId: operationId,
          status: 'queued',
          phase: 'queued',
          reportedAt: nowIso,
          overallPercent: 0,
          phasePercent: 0,
          bytesDownloaded: 0,
          bytesTotal: media.sizeBytes,
          bytesPerSecond: null,
          isoMedia: {
            id: media.id,
            displayName: media.displayName,
            sizeBytes: media.sizeBytes,
            sha256: media.sha256
          },
          retentionSeconds: ISO_STAGE_RETENTION_SECONDS
        };
        await tx.$executeRaw(Prisma.sql`
          INSERT INTO public.feature_upgrade_iso_stage_device
            (
              operation_id, run_id, organization_id, agent_id, iso_media_id,
              source_os, target_product, target_version, target_build_label,
              status, phase, progress_jsonb, evidence_jsonb, error_message,
              size_bytes, sha256, requested_by, created_at, updated_at
            )
          VALUES
            (
              ${operationId}, ${runId}, ${membership.organizationId}, ${device.agentId}, ${media.id},
              ${device.os}, ${device.targetProduct}, ${device.targetVersion}, ${device.targetBuildLabel},
              'queued', 'queued', ${JSON.stringify(progress)}::jsonb, '{}'::jsonb, NULL,
              ${media.sizeBytes}, ${media.sha256}, ${requestedBy}, NOW(), NOW()
            )
        `);
      }
    });

    const rows = await prisma.$queryRaw<IsoStageDeviceRow[]>(Prisma.sql`
      SELECT
        s.operation_id AS "operationId",
        s.run_id AS "runId",
        s.organization_id AS "organizationId",
        s.agent_id AS "agentId",
        d.hostname,
        s.iso_media_id AS "isoMediaId",
        m.display_name AS "isoDisplayName",
        s.source_os AS "sourceOs",
        s.target_product AS "targetProduct",
        s.target_version AS "targetVersion",
        s.target_build_label AS "targetBuildLabel",
        s.status,
        s.phase,
        s.progress_jsonb AS progress,
        s.evidence_jsonb AS evidence,
        s.error_message AS "errorMessage",
        s.size_bytes AS "sizeBytes",
        s.sha256,
        s.requested_by AS "requestedBy",
        s.claimed_at AS "claimedAt",
        s.started_at AS "startedAt",
        s.staged_at AS "stagedAt",
        s.expires_at AS "expiresAt",
        s.cleaned_at AS "cleanedAt",
        s.finished_at AS "finishedAt",
        s.created_at AS "createdAt",
        s.updated_at AS "updatedAt"
      FROM public.feature_upgrade_iso_stage_device s
      LEFT JOIN public.rmm_devices d ON d.organization_id = s.organization_id AND d.agent_id = s.agent_id
      LEFT JOIN public.feature_upgrade_iso_media m ON m.id = s.iso_media_id
      WHERE s.run_id = ${runId}
      ORDER BY d.hostname ASC NULLS LAST, s.agent_id ASC
    `);

    await notifyStageIsoJobsAvailable(devicesToStage.map((device) => device.agentId), 'feature_upgrade_stage_iso_queued', requestedBy);

    return res.status(202).json({
      runId,
      retentionSeconds: ISO_STAGE_RETENTION_SECONDS,
      retentionDays: 7,
      targetedDevices: devicesToStage.length,
      skipped: [
        ...preview.skipped,
        ...preview.devices
          .filter((device) => !device.canStage)
          .map((device) => ({ agentId: device.agentId, reason: device.blockingReasons[0] ?? 'not_ready' }))
      ],
      devices: rows.map(isoStageDeviceResponse)
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/stage-iso/progress/query', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.json({ items: [] });

    const allowedDevices = await prisma.rmmDevice.findMany({
      where: { organizationId: membership.organizationId, agentId: { in: agentIds } },
      select: { agentId: true }
    });
    const allowedAgentIds = allowedDevices.map((device) => device.agentId);
    if (allowedAgentIds.length === 0) return res.json({ items: [] });

    const stageByAgent = await loadLatestIsoStageRows(membership.organizationId, allowedAgentIds);
    const rows = [...stageByAgent.values()];
    return res.json({ items: rows.map(isoStageDeviceResponse) });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/start/preview', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;

    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    const preview = await buildFeatureUpgradeStartPreview(membership.organizationId, agentIds);
    const ready = preview.devices.filter((device) => device.canStart);
    const downloadBytes = ready.reduce((sum, device) => sum + (device.willDownloadIso ? (device.expectedSizeBytes ?? 0) : 0), 0);
    return res.json({
      diskFreeBytesRequired: FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES,
      totalDownloadBytes: downloadBytes,
      devices: preview.devices,
      skipped: preview.skipped
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/start-runs', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) {
      return res.status(403).json({ error: 'Only admins can start feature upgrades' });
    }

    const body = asRecord(req.body);
    const agentIds = readFeatureUpgradeAgentIds(body.agentIds);
    if (agentIds.length === 0) return res.status(400).json({ error: 'agentIds are required' });

    let scheduledFor = readIsoDate(body.scheduledFor ?? body.scheduled_for);
    if ((body.scheduledFor ?? body.scheduled_for) && !scheduledFor) {
      return res.status(400).json({ error: 'scheduledFor must be a valid ISO date/time' });
    }
    if (scheduledFor && scheduledFor.getTime() <= Date.now()) {
      scheduledFor = null;
    }

    const preview = await buildFeatureUpgradeStartPreview(membership.organizationId, agentIds);
    const devicesToStart = preview.devices.filter((device) => device.canStart && device.isoMedia && device.setupCommand && device.preflightOperationId);
    if (devicesToStart.length === 0) {
      return res.status(400).json({
        error: 'No selected devices are ready to start feature upgrade',
        devices: preview.devices,
        skipped: preview.skipped
      });
    }

    const runId = randomUUID();
    const requestedBy = req.jwt!.sub;
    const nowIso = new Date().toISOString();
    const initialStatus = scheduledFor ? 'scheduled' : 'queued';
    const initialPhase = scheduledFor ? 'scheduled' : 'queued';
    await prisma.$transaction(async (tx) => {
      await tx.$executeRaw(Prisma.sql`
        INSERT INTO public.feature_upgrade_run
          (
            id, organization_id, requested_by, status, scheduled_for, total_devices,
            scheduled_devices, queued_devices, created_at, updated_at
          )
        VALUES
          (
            ${runId}, ${membership.organizationId}, ${requestedBy}, ${initialStatus}, ${scheduledFor}, ${devicesToStart.length},
            ${scheduledFor ? devicesToStart.length : 0}, ${scheduledFor ? 0 : devicesToStart.length}, NOW(), NOW()
          )
      `);

      for (const device of devicesToStart) {
        const operationId = randomUUID();
        const media = device.isoMedia!;
        const setupCommand = device.setupCommand!;
        const progress = {
          schemaVersion: 1,
          eventType: 'feature_upgrade.start.progress',
          organizationId: membership.organizationId,
          agentId: device.agentId,
          operationId,
          runId,
          jobId: operationId,
          commandId: operationId,
          status: initialStatus,
          phase: initialPhase,
          reportedAt: nowIso,
          overallPercent: 0,
          phasePercent: 0,
          scheduledFor: scheduledFor?.toISOString() ?? null,
          willDownloadIso: device.willDownloadIso,
          isoMedia: {
            id: media.id,
            displayName: media.displayName,
            sizeBytes: media.sizeBytes,
            sha256: media.sha256
          }
        };
        await tx.$executeRaw(Prisma.sql`
          INSERT INTO public.feature_upgrade_device
            (
              operation_id, run_id, organization_id, agent_id, preflight_operation_id,
              iso_media_id, setup_command_matrix_id, source_os,
              target_product, target_version, target_build_label,
              status, phase, progress_jsonb, evidence_jsonb, failure_summary_jsonb, error_message,
              size_bytes, sha256, scheduled_for, requested_by, created_at, updated_at
            )
          VALUES
            (
              ${operationId}, ${runId}, ${membership.organizationId}, ${device.agentId}, ${device.preflightOperationId},
              ${media.id}, ${setupCommand.id}, ${device.os},
              ${device.targetProduct}, ${device.targetVersion}, ${device.targetBuildLabel},
              ${initialStatus}, ${initialPhase}, ${JSON.stringify(progress)}::jsonb, '{}'::jsonb, '[]'::jsonb, NULL,
              ${media.sizeBytes}, ${media.sha256}, ${scheduledFor}, ${requestedBy}, NOW(), NOW()
            )
        `);
      }
    });

    const rows = await prisma.$queryRaw<FeatureUpgradeDeviceRow[]>(Prisma.sql`
      SELECT
        u.operation_id AS "operationId",
        u.run_id AS "runId",
        u.organization_id AS "organizationId",
        u.agent_id AS "agentId",
        d.hostname,
        u.preflight_operation_id AS "preflightOperationId",
        u.iso_media_id AS "isoMediaId",
        m.display_name AS "isoDisplayName",
        u.setup_command_matrix_id AS "setupCommandMatrixId",
        u.source_os AS "sourceOs",
        u.target_product AS "targetProduct",
        u.target_version AS "targetVersion",
        u.target_build_label AS "targetBuildLabel",
        u.status,
        u.phase,
        u.progress_jsonb AS progress,
        u.evidence_jsonb AS evidence,
        u.failure_summary_jsonb AS "failureSummary",
        u.error_message AS "errorMessage",
        u.size_bytes AS "sizeBytes",
        u.sha256,
        u.scheduled_for AS "scheduledFor",
        u.requested_by AS "requestedBy",
        u.claimed_at AS "claimedAt",
        u.started_at AS "startedAt",
        u.final_snapshot_at AS "finalSnapshotAt",
        u.setup_started_at AS "setupStartedAt",
        u.reboot_detected_at AS "rebootDetectedAt",
        u.verified_at AS "verifiedAt",
        u.finished_at AS "finishedAt",
        u.created_at AS "createdAt",
        u.updated_at AS "updatedAt"
      FROM public.feature_upgrade_device u
      LEFT JOIN public.rmm_devices d ON d.organization_id = u.organization_id AND d.agent_id = u.agent_id
      LEFT JOIN public.feature_upgrade_iso_media m ON m.id = u.iso_media_id
      WHERE u.run_id = ${runId}
      ORDER BY d.hostname ASC NULLS LAST, u.agent_id ASC
    `);

    await notifyFeatureUpgradeStartJobsAvailable(
      devicesToStart.map((device) => device.agentId),
      scheduledFor ? 'feature_upgrade_start_scheduled' : 'feature_upgrade_start_queued',
      requestedBy
    );

    return res.status(202).json({
      runId,
      scheduledFor: scheduledFor?.toISOString() ?? null,
      targetedDevices: devicesToStart.length,
      skipped: [
        ...preview.skipped,
        ...preview.devices
          .filter((device) => !device.canStart)
          .map((device) => ({ agentId: device.agentId, reason: device.blockingReasons[0] ?? 'not_ready' }))
      ],
      devices: rows.map(featureUpgradeDeviceResponse)
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/start/progress/query', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const agentIds = readFeatureUpgradeAgentIds(asRecord(req.body).agentIds);
    if (agentIds.length === 0) return res.json({ items: [] });

    const allowedDevices = await prisma.rmmDevice.findMany({
      where: { organizationId: membership.organizationId, agentId: { in: agentIds } },
      select: { agentId: true }
    });
    const allowedAgentIds = allowedDevices.map((device) => device.agentId);
    if (allowedAgentIds.length === 0) return res.json({ items: [] });

    const upgradeByAgent = await loadLatestFeatureUpgradeRows(membership.organizationId, allowedAgentIds);
    const rows = [...upgradeByAgent.values()];
    return res.json({ items: rows.map(featureUpgradeDeviceResponse) });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/preflight/jobs/claim', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const agentId = readString(body.agentId ?? body.agent_id);
    if (!agentId) return res.status(400).json({ error: 'agentId is required' });
    const limitRaw = Number(body.limit ?? 1);
    const limit = Number.isInteger(limitRaw) ? Math.max(1, Math.min(3, limitRaw)) : 1;

    const rows = await prisma.$queryRaw<PreflightDeviceRow[]>(Prisma.sql`
      UPDATE public.feature_upgrade_preflight_device p
      SET status = 'running',
          phase = 'checking',
          claimed_at = NOW(),
          started_at = COALESCE(started_at, NOW()),
          updated_at = NOW()
      FROM (
        SELECT operation_id
        FROM public.feature_upgrade_preflight_device
        WHERE agent_id = ${agentId}
          AND status = 'queued'
        ORDER BY created_at ASC
        LIMIT ${limit}
        FOR UPDATE SKIP LOCKED
      ) claim
      WHERE p.operation_id = claim.operation_id
      RETURNING
        p.operation_id AS "operationId",
        p.run_id AS "runId",
        p.organization_id AS "organizationId",
        p.agent_id AS "agentId",
        (
          SELECT d.hostname
          FROM public.rmm_devices d
          WHERE d.organization_id = p.organization_id
            AND d.agent_id = p.agent_id
          LIMIT 1
        ) AS hostname,
        p.source_os AS "sourceOs",
        p.target_product AS "targetProduct",
        p.target_version AS "targetVersion",
        p.target_build_label AS "targetBuildLabel",
        p.status,
        p.phase,
        p.check_results_jsonb AS "checkResults",
        p.failure_summary_jsonb AS "failureSummary",
        p.warning_summary_jsonb AS "warningSummary",
        p.requested_by AS "requestedBy",
        p.claimed_at AS "claimedAt",
        p.started_at AS "startedAt",
        p.finished_at AS "finishedAt",
        p.created_at AS "createdAt",
        p.updated_at AS "updatedAt"
    `);

    for (const row of rows) {
      await updatePreflightRunCounts(row.runId);
    }

    return res.json({
      jobs: rows.map((row) => ({
        operationId: row.operationId,
        runId: row.runId,
        organizationId: row.organizationId,
        agentId: row.agentId,
        sourceOs: row.sourceOs,
        targetProduct: row.targetProduct,
        targetVersion: row.targetVersion,
        targetBuildLabel: row.targetBuildLabel,
        snapshotRequestId: row.operationId,
        checks: Array.isArray(row.checkResults) && row.checkResults.length > 0
          ? row.checkResults
          : featureUpgradeChecksForProfile(inferFeatureUpgradePreflightTarget(row.sourceOs)?.profile ?? 'windows11_feature')
      }))
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/preflight/progress', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const items = Array.isArray(body.items) ? body.items : [body];
    let updated = 0;
    const touchedRuns = new Set<string>();

    for (const item of items) {
      const record = asRecord(item);
      const operationId = readString(record.operationId ?? record.operation_id);
      const runId = readString(record.runId ?? record.run_id);
      const organizationId = readString(record.organizationId ?? record.organization_id);
      const agentId = readString(record.agentId ?? record.agent_id);
      const rawStatus = readString(record.status) ?? 'running';
      const status = ['running', 'passed', 'warning', 'failed', 'cancelled'].includes(rawStatus) ? rawStatus : 'running';
      const rawPhase = readString(record.phase);
      const phase = rawPhase && ['queued', 'checking', 'completed', 'failed', 'cancelled'].includes(rawPhase)
        ? rawPhase
        : status === 'running'
          ? 'checking'
          : status === 'failed'
            ? 'failed'
            : status === 'cancelled'
              ? 'cancelled'
              : 'completed';
      if (!operationId || !runId || !organizationId || !agentId) continue;

      const checks = Array.isArray(record.checks) ? record.checks : [];
      const shouldUpdateChecks = checks.length > 0;
      const failures = summarizeFeatureUpgradePreflightChecks(checks, 'failed');
      const warnings = summarizeFeatureUpgradePreflightChecks(checks, 'warning');
      const finished = ['passed', 'warning', 'failed', 'cancelled'].includes(status);
      const rows = await prisma.$queryRaw<Array<{ runId: string }>>(Prisma.sql`
        UPDATE public.feature_upgrade_preflight_device
        SET status = ${status},
            phase = ${phase},
            check_results_jsonb = CASE WHEN ${shouldUpdateChecks} THEN ${JSON.stringify(checks)}::jsonb ELSE check_results_jsonb END,
            failure_summary_jsonb = CASE WHEN ${shouldUpdateChecks} THEN ${JSON.stringify(failures)}::jsonb ELSE failure_summary_jsonb END,
            warning_summary_jsonb = CASE WHEN ${shouldUpdateChecks} THEN ${JSON.stringify(warnings)}::jsonb ELSE warning_summary_jsonb END,
            started_at = COALESCE(started_at, NOW()),
            finished_at = CASE WHEN ${finished} THEN NOW() ELSE finished_at END,
            updated_at = NOW()
        WHERE operation_id = ${operationId}
          AND run_id = ${runId}
          AND organization_id = ${organizationId}
          AND agent_id = ${agentId}
          AND status NOT IN ('succeeded', 'failed', 'cancelled')
        RETURNING run_id AS "runId"
      `);
      if (rows[0]) {
        touchedRuns.add(rows[0].runId);
        updated += 1;
      }
    }

    for (const touchedRunId of touchedRuns) {
      await updatePreflightRunCounts(touchedRunId);
    }

    return res.status(202).json({ accepted: true, updated });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/stage-iso/jobs/claim', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const agentId = readString(body.agentId ?? body.agent_id);
    if (!agentId) return res.status(400).json({ error: 'agentId is required' });
    const limitRaw = Number(body.limit ?? 1);
    const limit = Number.isInteger(limitRaw) ? Math.max(1, Math.min(3, limitRaw)) : 1;

    const rows = await prisma.$queryRaw<IsoStageClaimRow[]>(Prisma.sql`
      UPDATE public.feature_upgrade_iso_stage_device s
      SET status = 'running',
          phase = 'requesting_link',
          claimed_at = NOW(),
          started_at = COALESCE(started_at, NOW()),
          progress_jsonb = jsonb_set(
            jsonb_set(COALESCE(progress_jsonb, '{}'::jsonb), '{status}', '"running"'::jsonb, true),
            '{phase}', '"requesting_link"'::jsonb,
            true
          ),
          updated_at = NOW()
      FROM (
        SELECT operation_id
        FROM public.feature_upgrade_iso_stage_device
        WHERE agent_id = ${agentId}
          AND status = 'queued'
        ORDER BY created_at ASC
        LIMIT ${limit}
        FOR UPDATE SKIP LOCKED
      ) claim
      JOIN public.feature_upgrade_iso_media m ON m.id = (
        SELECT iso_media_id
        FROM public.feature_upgrade_iso_stage_device s2
        WHERE s2.operation_id = claim.operation_id
      )
      WHERE s.operation_id = claim.operation_id
      RETURNING
        s.operation_id AS "operationId",
        s.run_id AS "runId",
        s.organization_id AS "organizationId",
        s.agent_id AS "agentId",
        (
          SELECT d.hostname
          FROM public.rmm_devices d
          WHERE d.organization_id = s.organization_id
            AND d.agent_id = s.agent_id
          LIMIT 1
        ) AS hostname,
        s.iso_media_id AS "isoMediaId",
        m.display_name AS "isoDisplayName",
        s.source_os AS "sourceOs",
        s.target_product AS "targetProduct",
        s.target_version AS "targetVersion",
        s.target_build_label AS "targetBuildLabel",
        s.status,
        s.phase,
        s.progress_jsonb AS progress,
        s.evidence_jsonb AS evidence,
        s.error_message AS "errorMessage",
        s.size_bytes AS "sizeBytes",
        s.sha256,
        s.requested_by AS "requestedBy",
        s.claimed_at AS "claimedAt",
        s.started_at AS "startedAt",
        s.staged_at AS "stagedAt",
        s.expires_at AS "expiresAt",
        s.cleaned_at AS "cleanedAt",
        s.finished_at AS "finishedAt",
        s.created_at AS "createdAt",
        s.updated_at AS "updatedAt",
        m.product AS "isoProduct",
        m.version AS "isoVersion",
        m.edition AS "isoEdition",
        m.architecture AS "isoArchitecture",
        m.language AS "isoLanguage",
        m.container_name AS "isoContainerName",
        m.blob_name AS "isoBlobName",
        m.active AS "isoActive",
        m.created_at AS "isoCreatedAt",
        m.updated_at AS "isoUpdatedAt"
    `);

    const jobs: unknown[] = [];
    for (const row of rows) {
      await updateIsoStageRunCounts(row.runId);
      const media: IsoMediaRow = {
        id: row.isoMediaId,
        displayName: row.isoDisplayName ?? row.isoMediaId,
        osFamily: 'windows',
        product: row.isoProduct,
        version: row.isoVersion,
        edition: row.isoEdition,
        architecture: row.isoArchitecture,
        language: row.isoLanguage,
        sha256: row.sha256,
        sizeBytes: row.sizeBytes,
        containerName: row.isoContainerName,
        blobName: row.isoBlobName,
        active: row.isoActive,
        createdAt: row.isoCreatedAt,
        updatedAt: row.isoUpdatedAt
      };

      try {
        const download = await generateIsoDownloadLink(media);
        jobs.push({
          operationId: row.operationId,
          runId: row.runId,
          organizationId: row.organizationId,
          agentId: row.agentId,
          sourceOs: row.sourceOs,
          targetProduct: row.targetProduct,
          targetVersion: row.targetVersion,
          targetBuildLabel: row.targetBuildLabel,
          retentionSeconds: ISO_STAGE_RETENTION_SECONDS,
          isoMedia: toIsoMediaResponse(media),
          download
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Unable to create ISO download link';
        const progress = {
          ...isoStageProgressPayloadFromRow(row),
          status: 'failed',
          phase: 'failed',
          error: message,
          reportedAt: new Date().toISOString()
        };
        await prisma.$executeRaw(Prisma.sql`
          UPDATE public.feature_upgrade_iso_stage_device
          SET status = 'failed',
              phase = 'failed',
              progress_jsonb = ${JSON.stringify(progress)}::jsonb,
              error_message = ${message},
              finished_at = NOW(),
              updated_at = NOW()
          WHERE operation_id = ${row.operationId}
        `);
        await updateIsoStageRunCounts(row.runId);
      }
    }

    return res.json({ jobs });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/stage-iso/progress', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const items = Array.isArray(body.items) ? body.items : [body];
    let updated = 0;
    const touchedRuns = new Set<string>();

    for (const item of items) {
      const record = asRecord(item);
      const operationId = readString(record.operationId ?? record.operation_id);
      const runId = readString(record.runId ?? record.run_id);
      const organizationId = readString(record.organizationId ?? record.organization_id);
      const agentId = readString(record.agentId ?? record.agent_id);
      const isoMediaId = readString(record.isoMediaId ?? record.iso_media_id ?? asRecord(record.isoMedia).id);
      if (!operationId || !runId || !organizationId || !agentId) continue;

      const rawStatus = readString(record.status) ?? 'running';
      const status = ['queued', 'running', 'staged', 'failed', 'cancelled', 'deleted', 'expired'].includes(rawStatus)
        ? rawStatus
        : 'running';
      const rawPhase = readString(record.phase);
      const phase = rawPhase && ['queued', 'requesting_link', 'downloading', 'verifying', 'staged', 'failed', 'cleanup_pending', 'deleted', 'cancelled'].includes(rawPhase)
        ? rawPhase
        : status === 'running'
          ? 'downloading'
          : status === 'staged'
            ? 'staged'
            : status === 'deleted' || status === 'expired'
              ? 'deleted'
              : status === 'cancelled'
                ? 'cancelled'
                : 'failed';
      const stagedAt = readIsoDate(record.stagedAt ?? record.staged_at);
      const expiresAt = readIsoDate(record.expiresAt ?? record.expires_at) ?? (status === 'staged' ? stageExpiryFromNow() : null);
      const cleanedAt = readIsoDate(record.cleanedAt ?? record.cleaned_at);
      const terminal = ['staged', 'failed', 'cancelled', 'deleted', 'expired'].includes(status);
      const errorMessage = readString(record.error ?? record.errorMessage ?? record.error_message);
      const evidence = asRecord(record.evidence);
      const progress = {
        ...record,
        schemaVersion: readNumber(record.schemaVersion) ?? 1,
        eventType: 'feature_upgrade.iso.stage.progress',
        operationId,
        runId,
        organizationId,
        agentId,
        isoMediaId: isoMediaId ?? undefined,
        status,
        phase,
        reportedAt: readString(record.reportedAt ?? record.reported_at) ?? new Date().toISOString(),
        error: errorMessage
      };

      const rows = await prisma.$queryRaw<Array<{ runId: string }>>(Prisma.sql`
        UPDATE public.feature_upgrade_iso_stage_device
        SET status = ${status},
            phase = ${phase},
            progress_jsonb = ${JSON.stringify(progress)}::jsonb,
            evidence_jsonb = ${JSON.stringify(evidence)}::jsonb,
            error_message = ${errorMessage},
            started_at = COALESCE(started_at, NOW()),
            staged_at = CASE WHEN ${status === 'staged'} THEN COALESCE(${stagedAt}, NOW()) ELSE staged_at END,
            expires_at = CASE WHEN ${status === 'staged'} THEN ${expiresAt} ELSE expires_at END,
            cleaned_at = CASE WHEN ${status === 'deleted' || status === 'expired'} THEN COALESCE(${cleanedAt}, NOW()) ELSE cleaned_at END,
            finished_at = CASE WHEN ${terminal} THEN NOW() ELSE finished_at END,
            updated_at = NOW()
        WHERE operation_id = ${operationId}
          AND run_id = ${runId}
          AND organization_id = ${organizationId}
          AND agent_id = ${agentId}
          AND status NOT IN ('succeeded', 'failed', 'cancelled')
        RETURNING run_id AS "runId"
      `);
      if (rows[0]) {
        touchedRuns.add(rows[0].runId);
        updated += 1;
      }
    }

    for (const runId of touchedRuns) {
      await updateIsoStageRunCounts(runId);
    }

    return res.status(202).json({ accepted: true, updated });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/start/jobs/claim', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const agentId = readString(body.agentId ?? body.agent_id);
    if (!agentId) return res.status(400).json({ error: 'agentId is required' });
    const limitRaw = Number(body.limit ?? 1);
    const limit = Number.isInteger(limitRaw) ? Math.max(1, Math.min(3, limitRaw)) : 1;

    const rows = await prisma.$queryRaw<FeatureUpgradeClaimRow[]>(Prisma.sql`
      UPDATE public.feature_upgrade_device u
      SET status = 'running',
          phase = 'final_checks',
          claimed_at = NOW(),
          started_at = COALESCE(started_at, NOW()),
          progress_jsonb = jsonb_set(
            jsonb_set(COALESCE(progress_jsonb, '{}'::jsonb), '{status}', '"running"'::jsonb, true),
            '{phase}', '"final_checks"'::jsonb,
            true
          ),
          updated_at = NOW()
      FROM (
        SELECT operation_id
        FROM public.feature_upgrade_device
        WHERE agent_id = ${agentId}
          AND status IN ('queued', 'scheduled')
          AND (scheduled_for IS NULL OR scheduled_for <= NOW())
        ORDER BY COALESCE(scheduled_for, created_at) ASC, created_at ASC
        LIMIT ${limit}
        FOR UPDATE SKIP LOCKED
      ) claim
      JOIN public.feature_upgrade_iso_media m ON m.id = (
        SELECT iso_media_id
        FROM public.feature_upgrade_device u2
        WHERE u2.operation_id = claim.operation_id
      )
      JOIN public.feature_upgrade_setup_command_matrix cm ON cm.id = (
        SELECT setup_command_matrix_id
        FROM public.feature_upgrade_device u3
        WHERE u3.operation_id = claim.operation_id
      )
      WHERE u.operation_id = claim.operation_id
      RETURNING
        u.operation_id AS "operationId",
        u.run_id AS "runId",
        u.organization_id AS "organizationId",
        u.agent_id AS "agentId",
        (
          SELECT d.hostname
          FROM public.rmm_devices d
          WHERE d.organization_id = u.organization_id
            AND d.agent_id = u.agent_id
          LIMIT 1
        ) AS hostname,
        u.preflight_operation_id AS "preflightOperationId",
        u.iso_media_id AS "isoMediaId",
        m.display_name AS "isoDisplayName",
        u.setup_command_matrix_id AS "setupCommandMatrixId",
        u.source_os AS "sourceOs",
        u.target_product AS "targetProduct",
        u.target_version AS "targetVersion",
        u.target_build_label AS "targetBuildLabel",
        u.status,
        u.phase,
        u.progress_jsonb AS progress,
        u.evidence_jsonb AS evidence,
        u.failure_summary_jsonb AS "failureSummary",
        u.error_message AS "errorMessage",
        u.size_bytes AS "sizeBytes",
        u.sha256,
        u.scheduled_for AS "scheduledFor",
        u.requested_by AS "requestedBy",
        u.claimed_at AS "claimedAt",
        u.started_at AS "startedAt",
        u.final_snapshot_at AS "finalSnapshotAt",
        u.setup_started_at AS "setupStartedAt",
        u.reboot_detected_at AS "rebootDetectedAt",
        u.verified_at AS "verifiedAt",
        u.finished_at AS "finishedAt",
        u.created_at AS "createdAt",
        u.updated_at AS "updatedAt",
        m.product AS "isoProduct",
        m.version AS "isoVersion",
        m.edition AS "isoEdition",
        m.architecture AS "isoArchitecture",
        m.language AS "isoLanguage",
        m.container_name AS "isoContainerName",
        m.blob_name AS "isoBlobName",
        m.active AS "isoActive",
        m.created_at AS "isoCreatedAt",
        m.updated_at AS "isoUpdatedAt",
        cm.setup_executable AS "setupExecutable",
        cm.arguments_jsonb AS "setupArguments",
        cm.dynamic_update_mode AS "dynamicUpdateMode",
        cm.requires_eula_accept AS "requiresEulaAccept",
        cm.image_index_strategy AS "imageIndexStrategy",
        cm.supported AS "setupSupported",
        cm.notes AS "setupNotes"
    `);

    const jobs: unknown[] = [];
    for (const row of rows) {
      await updateFeatureUpgradeRunCounts(row.runId);
      if (!row.setupSupported) {
        const message = row.setupNotes ?? 'Selected ISO is not supported by automated start upgrade';
        const progress = {
          ...featureUpgradeProgressPayloadFromRow(row),
          status: 'failed',
          phase: 'failed',
          error: message,
          reportedAt: new Date().toISOString()
        };
        await prisma.$executeRaw(Prisma.sql`
          UPDATE public.feature_upgrade_device
          SET status = 'failed',
              phase = 'failed',
              progress_jsonb = ${JSON.stringify(progress)}::jsonb,
              failure_summary_jsonb = ${JSON.stringify([{ id: 'setup_command_matrix', label: 'Setup command matrix', message }])}::jsonb,
              error_message = ${message},
              finished_at = NOW(),
              updated_at = NOW()
          WHERE operation_id = ${row.operationId}
        `);
        await updateFeatureUpgradeRunCounts(row.runId);
        continue;
      }

      const media: IsoMediaRow = {
        id: row.isoMediaId,
        displayName: row.isoDisplayName ?? row.isoMediaId,
        osFamily: 'windows',
        product: row.isoProduct,
        version: row.isoVersion,
        edition: row.isoEdition,
        architecture: row.isoArchitecture,
        language: row.isoLanguage,
        sha256: row.sha256,
        sizeBytes: row.sizeBytes,
        containerName: row.isoContainerName,
        blobName: row.isoBlobName,
        active: row.isoActive,
        createdAt: row.isoCreatedAt,
        updatedAt: row.isoUpdatedAt
      };

      try {
        const download = await generateIsoDownloadLink(media);
        jobs.push({
          operationId: row.operationId,
          runId: row.runId,
          organizationId: row.organizationId,
          agentId: row.agentId,
          sourceOs: row.sourceOs,
          targetProduct: row.targetProduct,
          targetVersion: row.targetVersion,
          targetBuildLabel: row.targetBuildLabel,
          scheduledFor: row.scheduledFor?.toISOString() ?? null,
          snapshotRequestId: row.operationId,
          diskFreeBytesRequired: FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES,
          retentionSeconds: ISO_STAGE_RETENTION_SECONDS,
          isoMedia: toIsoMediaResponse(media),
          download,
          setupCommand: {
            id: row.setupCommandMatrixId,
            setupExecutable: row.setupExecutable,
            arguments: Array.isArray(row.setupArguments) ? row.setupArguments : [],
            dynamicUpdateMode: row.dynamicUpdateMode,
            requiresEulaAccept: row.requiresEulaAccept,
            imageIndexStrategy: row.imageIndexStrategy,
            notes: row.setupNotes
          }
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Unable to create ISO download link';
        const progress = {
          ...featureUpgradeProgressPayloadFromRow(row),
          status: 'failed',
          phase: 'failed',
          error: message,
          reportedAt: new Date().toISOString()
        };
        await prisma.$executeRaw(Prisma.sql`
          UPDATE public.feature_upgrade_device
          SET status = 'failed',
              phase = 'failed',
              progress_jsonb = ${JSON.stringify(progress)}::jsonb,
              failure_summary_jsonb = ${JSON.stringify([{ id: 'download_link', label: 'ISO download link', message }])}::jsonb,
              error_message = ${message},
              finished_at = NOW(),
              updated_at = NOW()
          WHERE operation_id = ${row.operationId}
        `);
        await updateFeatureUpgradeRunCounts(row.runId);
      }
    }

    return res.json({ jobs });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post('/internal/start/progress', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = asRecord(req.body);
    const items = Array.isArray(body.items) ? body.items : [body];
    let updated = 0;
    const touchedRuns = new Set<string>();

    for (const item of items) {
      const record = asRecord(item);
      const operationId = readString(record.operationId ?? record.operation_id);
      const runId = readString(record.runId ?? record.run_id);
      const organizationId = readString(record.organizationId ?? record.organization_id);
      const agentId = readString(record.agentId ?? record.agent_id);
      const isoMediaId = readString(record.isoMediaId ?? record.iso_media_id ?? asRecord(record.isoMedia).id);
      if (!operationId || !runId || !organizationId || !agentId) continue;

      const rawStatus = readString(record.status) ?? 'running';
      const status = ['scheduled', 'queued', 'running', 'awaiting_reboot', 'verifying', 'succeeded', 'failed', 'cancelled'].includes(rawStatus)
        ? rawStatus
        : 'running';
      const rawPhase = readString(record.phase);
      const phase = rawPhase && [
        'scheduled',
        'queued',
        'final_checks',
        'resolving_iso',
        'downloading_iso',
        'verifying_iso',
        'mounting_iso',
        'launching_setup',
        'setup_running',
        'awaiting_reboot',
        'post_reboot_verifying',
        'completed',
        'failed',
        'cancelled'
      ].includes(rawPhase)
        ? rawPhase
        : status === 'awaiting_reboot'
          ? 'awaiting_reboot'
          : status === 'verifying'
            ? 'post_reboot_verifying'
            : status === 'succeeded'
              ? 'completed'
              : status === 'failed'
                ? 'failed'
                : status === 'cancelled'
                  ? 'cancelled'
                  : 'setup_running';
      const finalSnapshotAt = readIsoDate(record.finalSnapshotAt ?? record.final_snapshot_at);
      const setupStartedAt = readIsoDate(record.setupStartedAt ?? record.setup_started_at);
      const rebootDetectedAt = readIsoDate(record.rebootDetectedAt ?? record.reboot_detected_at);
      const verifiedAt = readIsoDate(record.verifiedAt ?? record.verified_at);
      const terminal = ['succeeded', 'failed', 'cancelled'].includes(status);
      const errorMessage = readString(record.error ?? record.errorMessage ?? record.error_message);
      const evidence = asRecord(record.evidence);
      const failureSummary = Array.isArray(record.failureSummary)
        ? record.failureSummary
        : errorMessage
          ? [{ id: 'feature_upgrade_start', label: 'Feature upgrade start', message: errorMessage }]
          : [];
      const progress = {
        ...record,
        schemaVersion: readNumber(record.schemaVersion) ?? 1,
        eventType: 'feature_upgrade.start.progress',
        operationId,
        runId,
        organizationId,
        agentId,
        isoMediaId: isoMediaId ?? undefined,
        status,
        phase,
        reportedAt: readString(record.reportedAt ?? record.reported_at) ?? new Date().toISOString(),
        error: errorMessage
      };

      const rows = await prisma.$queryRaw<Array<{ runId: string }>>(Prisma.sql`
        UPDATE public.feature_upgrade_device
        SET status = ${status},
            phase = ${phase},
            progress_jsonb = ${JSON.stringify(progress)}::jsonb,
            evidence_jsonb = ${JSON.stringify(evidence)}::jsonb,
            failure_summary_jsonb = CASE WHEN ${failureSummary.length > 0} THEN ${JSON.stringify(failureSummary)}::jsonb ELSE failure_summary_jsonb END,
            error_message = ${errorMessage},
            started_at = COALESCE(started_at, NOW()),
            final_snapshot_at = CASE
              WHEN ${finalSnapshotAt}::timestamptz IS NOT NULL THEN ${finalSnapshotAt}
              WHEN ${phase === 'final_checks'} THEN COALESCE(final_snapshot_at, NOW())
              ELSE final_snapshot_at
            END,
            setup_started_at = CASE
              WHEN ${setupStartedAt}::timestamptz IS NOT NULL THEN ${setupStartedAt}
              WHEN ${phase === 'launching_setup' || phase === 'setup_running'} THEN COALESCE(setup_started_at, NOW())
              ELSE setup_started_at
            END,
            reboot_detected_at = CASE
              WHEN ${rebootDetectedAt}::timestamptz IS NOT NULL THEN ${rebootDetectedAt}
              WHEN ${status === 'verifying' || status === 'succeeded'} THEN COALESCE(reboot_detected_at, NOW())
              ELSE reboot_detected_at
            END,
            verified_at = CASE
              WHEN ${verifiedAt}::timestamptz IS NOT NULL THEN ${verifiedAt}
              WHEN ${status === 'succeeded'} THEN COALESCE(verified_at, NOW())
              ELSE verified_at
            END,
            finished_at = CASE WHEN ${terminal} THEN NOW() ELSE finished_at END,
            updated_at = NOW()
        WHERE operation_id = ${operationId}
          AND run_id = ${runId}
          AND organization_id = ${organizationId}
          AND agent_id = ${agentId}
          AND status NOT IN ('succeeded', 'failed', 'cancelled')
        RETURNING run_id AS "runId"
      `);
      if (rows[0]) {
        touchedRuns.add(rows[0].runId);
        updated += 1;
      }
    }

    for (const runId of touchedRuns) {
      await updateFeatureUpgradeRunCounts(runId);
    }

    return res.status(202).json({ accepted: true, updated });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.get('/iso-media', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;

    const items = await loadActiveIsoMedia();

    return res.json({
      items: items.map(toIsoMediaResponse)
    });
  } catch (error) {
    return next(error);
  }
});

featureUpgradesRouter.post(
  '/iso-media/:id/download-link',
  requireRmmServer,
  async (req: RmmServerRequest, res, next) => {
    try {
      const rows = await prisma.$queryRaw<IsoMediaRow[]>(Prisma.sql`
        SELECT
          id,
          display_name AS "displayName",
          os_family AS "osFamily",
          product,
          version,
          edition,
          architecture,
          language,
          sha256,
          size_bytes AS "sizeBytes",
          container_name AS "containerName",
          blob_name AS "blobName",
          active,
          created_at AS "createdAt",
          updated_at AS "updatedAt"
        FROM public.feature_upgrade_iso_media
        WHERE id = ${req.params.id}
          AND active = true
          AND os_family = 'windows'
        LIMIT 1
      `);
      const media = rows[0] ?? null;
      if (!media) {
        throw new HttpError(404, 'ISO media not found');
      }

      return res.json(await generateIsoDownloadLink(media));
    } catch (error) {
      return next(error);
    }
  }
);
