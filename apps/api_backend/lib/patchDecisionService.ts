import { randomUUID } from 'crypto';
import { Prisma } from '@prisma/client';
import { prisma } from './prisma';
import { ensureDefaultPatchPolicy } from './patchPolicies';
import {
  PatchPolicyForResolution,
  PatchPolicyScopeType,
  PatchApprovalMode,
  PatchRebootBehavior,
  normalizePatchOsFamily,
  resolveEffectivePatchPolicy
} from './patchManagement';
import {
  evaluatePatchActionPlan,
  PatchActionPlan,
  PatchCategory,
  PatchDecisionDevice,
  PatchDecisionOverride,
  PatchDecisionUpdate,
  PatchDeviceType,
  PatchOverrideAction,
  PatchRing
} from './patchDecisionEngine';

type PatchPolicyRow = PatchPolicyForResolution & {
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  name: string;
  createdAt: Date;
  createdBy: string;
};

type PatchDecisionDeviceRow = {
  organizationId: string;
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  lastSeen: Date;
  collectedAt: Date | null;
  rebootRequired: boolean | null;
  deviceType: PatchDeviceType;
  patchRing: PatchRing;
  patchManaged: boolean;
  nativeWindowsUpdateControl: boolean;
  patchMaintenanceModeUntil: Date | null;
  patchTags: unknown;
};

type PatchDecisionUpdateRow = {
  updateKey: string;
  title: string;
  titleNorm: string | null;
  kbArticle: string | null;
  category: PatchCategory | string | null;
  lifecycleState: string | null;
  approvalState: string | null;
  releaseDate: Date | null;
  firstDetectedAt: Date | null;
  lastDetectedAt: Date | null;
  downloadedAt: Date | null;
  installedAt: Date | null;
  requiresReboot: boolean | null;
  superseded: boolean | null;
  failedAt: Date | null;
};

type PatchOverrideRow = PatchDecisionOverride & {
  organizationId: string;
  createdBy: string;
  createdByEmail: string | null;
  targetAgentId: string | null;
  targetHostname: string | null;
  targetOs: string | null;
  latestActionType: string | null;
  latestActionStatus: string | null;
  latestActionPhase: string | null;
  latestActionUpdatedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
};

type PatchOverviewDeviceRow = PatchDecisionDeviceRow & {
  pendingUpdates: number;
  downloadedUpdates: number;
  failedUpdates: number;
  blockedUpdates: number;
  deferredUpdates: number;
  rebootPendingUpdates: number;
  serverAdDs: unknown;
  serverDhcp: unknown;
  serverDns: unknown;
  serverIis: unknown;
  macosUpdateAccount: unknown;
};

type ServerRoleInventory = {
  evidencePresent: boolean;
  roles: string[];
  isDomainController: boolean | null;
  details: {
    domainName?: string | null;
    dhcpScopes?: number;
    dnsZones?: number;
    iisSites?: number;
    iisAppPools?: number;
  };
};

type PatchOverviewUpdateRow = {
  updateKey: string;
  title: string;
  kbArticle: string | null;
  category: PatchCategory | string;
  releaseDate: Date | null;
  releaseDateSource: string | null;
  source: string;
  affectedDevices: number;
  associatedDevices: number;
  detectedDevices: number;
  downloadedDevices: number;
  installedDevices: number;
  failedDevices: number;
  blockedDevices: number;
  deferredDevices: number;
  supersededDevices: number;
  affectedAgentIds: unknown;
  affectedHostnames: unknown;
  associatedAgentIds: unknown;
  associatedHostnames: unknown;
  customerNames: unknown;
  siteNames: unknown;
  osFamilies: unknown;
  deviceTypes: unknown;
  patchRings: unknown;
  lastSeenAt: Date;
};

type PatchDecisionLogRow = {
  id: string;
  agentId: string;
  policyId: string | null;
  operationId: string;
  action: string;
  updateKeys: unknown;
  decision: string;
  reason: string;
  actorType: string;
  actorUserId: string | null;
  actorEmail: string | null;
  actionStatus: string | null;
  actionPhase: string | null;
  actionUpdatedAt: Date | null;
  details: unknown;
  decidedAt: Date;
};

type PatchDeviceStateRow = {
  updateKey: string;
  title: string;
  kbArticle: string | null;
  category: PatchCategory | string;
  applicabilityState: string;
  approvalState: string;
  lifecycleState: string;
  releaseDate: Date | null;
  firstDetectedAt: Date;
  lastDetectedAt: Date;
  eligibleAt: Date | null;
  installDeadlineAt: Date | null;
  rebootDeadlineAt: Date | null;
  downloadedAt: Date | null;
  installedAt: Date | null;
  failedAt: Date | null;
  failureCode: string | null;
  failureHresult: number | null;
  failureMessage: string | null;
  requiresReboot: boolean | null;
};

type PatchTransactionFailureRow = {
  id: string;
  operationId: string;
  action: string;
  reason: string;
  error: string | null;
  phase: string | null;
  packageManager: string | null;
  updateKeyCount: number;
  transactionPackageCount: number | null;
  decidedAt: Date;
};

function effectivePatchLifecycleState(row: {
  lifecycleState: string | null;
  downloadedAt?: Date | string | null;
  failureMessage?: string | null;
}) {
  const lifecycleState = row.lifecycleState ?? 'detected';
  if (lifecycleState === 'failed' && row.failureMessage?.trim().toLowerCase() === 'inprogress') {
    return 'detected';
  }
  if (
    row.downloadedAt &&
    lifecycleState !== 'installed' &&
    lifecycleState !== 'failed' &&
    lifecycleState !== 'superseded' &&
    lifecycleState !== 'reboot_pending'
  ) {
    return 'downloaded';
  }
  return lifecycleState;
}

function jsonString(value: unknown): string {
  return JSON.stringify(value === undefined ? null : value);
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function booleanValue(value: unknown): boolean | null {
  return typeof value === 'boolean' ? value : null;
}

function arrayLength(value: unknown): number {
  return Array.isArray(value) ? value.length : 0;
}

function buildServerRoleInventory(device: PatchOverviewDeviceRow): ServerRoleInventory {
  const adDs = asRecord(device.serverAdDs);
  const dhcp = asRecord(device.serverDhcp);
  const dns = asRecord(device.serverDns);
  const iis = asRecord(device.serverIis);
  const roles: string[] = [];
  const isDomainController = booleanValue(adDs?.is_domain_controller) ?? booleanValue(adDs?.isDomainController);
  const dhcpInstalled = booleanValue(dhcp?.installed);
  const dnsInstalled = booleanValue(dns?.installed);
  const iisInstalled = booleanValue(iis?.installed);

  if (isDomainController === true) roles.push('Domain Controller');
  if (dnsInstalled === true) roles.push('DNS Server');
  if (dhcpInstalled === true) roles.push('DHCP Server');
  if (iisInstalled === true) roles.push('IIS');

  return {
    evidencePresent: Boolean(adDs || dhcp || dns || iis),
    roles,
    isDomainController,
    details: {
      domainName: stringValue(adDs?.domain_name) ?? stringValue(adDs?.domainName),
      dhcpScopes: arrayLength(dhcp?.scopes),
      dnsZones: arrayLength(dns?.zones),
      iisSites: arrayLength(iis?.sites),
      iisAppPools: arrayLength(iis?.app_pools) || arrayLength(iis?.appPools)
    }
  };
}

function isTransactionLevelPatchFailure(evidence: unknown): boolean {
  const record = asRecord(evidence);
  const summary = asRecord(record?.summary);
  return (
    record?.failureScope === 'transaction' ||
    summary?.transactionFailure === true
  );
}

type PatchActionResultUpdateProjection = {
  updateKey: string;
  lifecycleState: 'downloaded' | 'installed' | 'failed';
  failureMessage: string | null;
  requiresReboot: boolean | null;
};

export type PatchProgressActionType = 'scan' | 'download' | 'install' | 'reboot';

export function inferPatchProgressActionType(options: {
  eventType?: string | null;
  phase?: string | null;
  status?: string | null;
  summary?: unknown;
  existingActionType?: string | null;
}): PatchProgressActionType {
  const existing = options.existingActionType?.trim();
  if (existing === 'scan' || existing === 'download' || existing === 'install' || existing === 'reboot') {
    return existing;
  }

  const eventType = options.eventType?.trim();
  if (eventType === 'patch.scan.progress') return 'scan';

  const summary = asRecord(options.summary);
  const downloaded = typeof summary?.downloaded === 'number' ? summary.downloaded : 0;
  const installed = typeof summary?.installed === 'number' ? summary.installed : 0;
  const status = options.status?.trim().toLowerCase() ?? '';
  const phase = options.phase?.trim().toLowerCase() ?? '';
  if (phase === 'scanning') return 'scan';
  if (phase === 'downloading') return 'download';
  if (phase === 'finalizing' && status === 'completed' && downloaded > 0 && installed === 0) return 'download';
  if (phase === 'installing' || phase === 'finalizing') return 'install';

  if (installed > 0) return 'install';
  if (status === 'failed') return 'install';
  if (status === 'completed' && downloaded > 0 && installed === 0) return 'download';

  return 'install';
}

export type PatchUpdateTargetStateRow = {
  agentId: string;
  updateKey: string;
  applicabilityState: string | null;
  lifecycleState: string | null;
};

export function selectActionablePatchUpdateTargetAgentIds(
  rows: PatchUpdateTargetStateRow[],
  requestedUpdateKeys: string[]
): string[] {
  const requested = new Set(requestedUpdateKeys.map((value) => value.trim()).filter(Boolean));
  const seen = new Set<string>();
  const selected: string[] = [];
  for (const row of rows) {
    if (!requested.has(row.updateKey)) continue;
    if (row.applicabilityState !== 'applicable') continue;
    if (row.lifecycleState === 'installed' || row.lifecycleState === 'superseded') continue;
    if (seen.has(row.agentId)) continue;
    seen.add(row.agentId);
    selected.push(row.agentId);
  }
  return selected;
}

export function projectPatchActionResultUpdates(options: {
  action: string;
  status: string;
  updateKeys: string[];
  evidence: unknown;
}): { usedEvidence: boolean; updates: PatchActionResultUpdateProjection[] } {
  const evidence = asRecord(options.evidence);
  if (options.status === 'failed' && isTransactionLevelPatchFailure(evidence)) {
    return { usedEvidence: true, updates: [] };
  }
  const evidenceUpdates = Array.isArray(evidence?.updates) && evidence.updates.length > 0 ? evidence.updates : null;
  if (evidenceUpdates) {
    const projected: PatchActionResultUpdateProjection[] = [];
    const seen = new Set<string>();
    for (const item of evidenceUpdates) {
      const record = asRecord(item);
      if (!record) continue;
      const updateKey = stringValue(record.updateKey ?? record.update_key);
      if (!updateKey || seen.has(updateKey)) continue;
      const result = stringValue(record.result)?.toLowerCase() ?? '';
      if (record.matched === false || record.selected === false || result === 'skipped') continue;

      const downloaded = booleanValue(record.downloaded) === true;
      const installed = booleanValue(record.installed) === true;
      const requiresReboot = booleanValue(record.requiresReboot ?? record.requires_reboot);
      const state = stringValue(record.state)?.toLowerCase() ?? '';
      let lifecycleState: PatchActionResultUpdateProjection['lifecycleState'] | null = null;

      if (options.action === 'install' && (installed || state === 'installed')) lifecycleState = 'installed';
      else if (options.action === 'download' && (downloaded || state === 'downloaded')) lifecycleState = 'downloaded';
      else if (options.status === 'failed' && result !== 'not_found') lifecycleState = 'failed';

      if (!lifecycleState) continue;
      seen.add(updateKey);
      projected.push({
        updateKey,
        lifecycleState,
        requiresReboot,
        failureMessage:
          lifecycleState === 'failed'
            ? jsonString({ source: 'agent_action_result', status: options.status, update: record })
            : null
      });
    }
    return { usedEvidence: true, updates: projected };
  }

  let lifecycleState: PatchActionResultUpdateProjection['lifecycleState'] | null = null;
  if (options.action === 'download' && options.status === 'completed') lifecycleState = 'downloaded';
  if (options.action === 'install' && options.status === 'completed') lifecycleState = 'installed';
  if (options.status === 'failed') lifecycleState = 'failed';
  if (!lifecycleState) return { usedEvidence: false, updates: [] };

  return {
    usedEvidence: false,
    updates: options.updateKeys.map((updateKey) => ({
      updateKey,
      lifecycleState,
      requiresReboot: null,
      failureMessage: lifecycleState === 'failed' ? jsonString(options.evidence) : null
    }))
  };
}

function iso(value: Date | string | null | undefined): string | null {
  if (!value) return null;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === 'string' && item.trim().length > 0);
}

function policyForResolution(row: PatchPolicyRow): PatchPolicyForResolution {
  return {
    id: row.id,
    scopeType: row.scopeType,
    scopeKey: row.scopeKey,
    targetOsFamily: row.targetOsFamily ?? 'all',
    approvalMode: row.approvalMode,
    maintenanceWindowStart: row.maintenanceWindowStart,
    maintenanceWindowEnd: row.maintenanceWindowEnd,
    maintenanceWindowTimezone: row.maintenanceWindowTimezone,
    rebootBehavior: row.rebootBehavior,
    deferralDays: row.deferralDays,
    managedMode: row.managedMode,
    nativeWindowsUpdateControl: row.nativeWindowsUpdateControl,
    policyConfig: row.policyConfig,
    priority: row.priority,
    enabled: row.enabled,
    isDefault: row.isDefault,
    updatedAt: row.updatedAt
  };
}

export async function loadPatchPoliciesForOrganization(organizationId: string): Promise<PatchPolicyRow[]> {
  await ensureDefaultPatchPolicy(organizationId);
  return prisma.$queryRaw<PatchPolicyRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id AS "organizationId",
      scope_type AS "scopeType",
      scope_key AS "scopeKey",
      customer_id AS "customerId",
      site_id AS "siteId",
      agent_id AS "agentId",
      name,
      target_os_family AS "targetOsFamily",
      approval_mode AS "approvalMode",
      maintenance_window_start AS "maintenanceWindowStart",
      maintenance_window_end AS "maintenanceWindowEnd",
      maintenance_window_timezone AS "maintenanceWindowTimezone",
      reboot_behavior AS "rebootBehavior",
      deferral_days AS "deferralDays",
      managed_mode AS "managedMode",
      native_windows_update_control AS "nativeWindowsUpdateControl",
      policy_config_jsonb AS "policyConfig",
      priority,
      enabled,
      is_default AS "isDefault",
      created_by AS "createdBy",
      created_at AS "createdAt",
      updated_at AS "updatedAt"
    FROM public.rmm_patch_policy
    WHERE organization_id = ${organizationId}
    ORDER BY enabled DESC, priority ASC, is_default ASC, scope_type ASC, name ASC
  `);
}

async function loadDevice(
  agentId: string,
  organizationId?: string | null,
  client: Prisma.TransactionClient = prisma
): Promise<PatchDecisionDeviceRow | null> {
  const organizationFilter = organizationId ? Prisma.sql`AND d.organization_id = ${organizationId}` : Prisma.empty;
  const rows = await client.$queryRaw<PatchDecisionDeviceRow[]>(Prisma.sql`
    SELECT
      d.organization_id AS "organizationId",
      d.agent_id AS "agentId",
      d.hostname,
      d.os,
      ds.os_version AS "osVersion",
      d.customer_id AS "customerId",
      c.name AS "customerName",
      d.site_id AS "siteId",
      s.name AS "siteName",
      d.last_seen AS "lastSeen",
      ds.collected_at AS "collectedAt",
      ds.reboot_required AS "rebootRequired",
      d.device_type AS "deviceType",
      d.patch_ring AS "patchRing",
      d.patch_managed AS "patchManaged",
      d.native_windows_update_control AS "nativeWindowsUpdateControl",
      d.patch_maintenance_mode_until AS "patchMaintenanceModeUntil",
      d.patch_tags AS "patchTags"
    FROM public.rmm_devices d
    LEFT JOIN public.customers c ON c.id = d.customer_id
    LEFT JOIN public.rmm_sites s ON s.id = d.site_id
    LEFT JOIN rmm_telemetry.device_state ds ON ds.agent_id = d.agent_id
    WHERE d.agent_id = ${agentId}
      ${organizationFilter}
    LIMIT 1
  `);
  return rows[0] ?? null;
}

function toDecisionDevice(row: PatchDecisionDeviceRow, observedState: Record<string, unknown> = {}): PatchDecisionDevice {
  const rebootRequired =
    typeof observedState.rebootRequired === 'boolean'
      ? observedState.rebootRequired
      : row.rebootRequired;
  const lastScanAt =
    typeof observedState.lastScanAt === 'string' && observedState.lastScanAt
      ? observedState.lastScanAt
      : row.collectedAt;
  const os = row.osVersion && !row.os.includes(row.osVersion)
    ? `${row.os} ${row.osVersion}`
    : row.os;
  return {
    organizationId: row.organizationId,
    agentId: row.agentId,
    hostname: row.hostname,
    os,
    customerId: row.customerId,
    siteId: row.siteId,
    deviceType: row.deviceType,
    patchRing: row.patchRing,
    patchManaged: row.patchManaged,
    nativeWindowsUpdateControl: row.nativeWindowsUpdateControl,
    patchMaintenanceModeUntil: row.patchMaintenanceModeUntil,
    patchTags: stringArray(row.patchTags),
    rebootRequired,
    lastScanAt,
    lastCheckInAt: new Date()
  };
}

async function loadUpdates(organizationId: string, agentId: string): Promise<PatchDecisionUpdate[]> {
  const rows = await prisma.$queryRaw<PatchDecisionUpdateRow[]>(Prisma.sql`
    SELECT
      update_key AS "updateKey",
      title,
      title_norm AS "titleNorm",
      kb_article AS "kbArticle",
      category,
      lifecycle_state AS "lifecycleState",
      approval_state AS "approvalState",
      release_date AS "releaseDate",
      first_detected_at AS "firstDetectedAt",
      last_detected_at AS "lastDetectedAt",
      downloaded_at AS "downloadedAt",
      installed_at AS "installedAt",
      requires_reboot AS "requiresReboot",
      (applicability_state = 'superseded' OR lifecycle_state = 'superseded') AS superseded,
      failed_at AS "failedAt"
    FROM public.rmm_patch_device_update_state
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
      AND lifecycle_state <> 'installed'
      AND (applicability_state <> 'not_applicable' OR downloaded_at IS NOT NULL)
    ORDER BY COALESCE(release_date, first_detected_at) ASC, title ASC
  `);
  return rows.map((row) => ({
    updateKey: row.updateKey,
    title: row.title,
    titleNorm: row.titleNorm,
    kbArticle: row.kbArticle,
    category: row.category,
    lifecycleState: row.lifecycleState,
    approvalState: row.approvalState,
    releaseDate: row.releaseDate,
    firstDetectedAt: row.firstDetectedAt,
    lastDetectedAt: row.lastDetectedAt,
    downloadedAt: row.downloadedAt,
    installedAt: row.installedAt,
    requiresReboot: row.requiresReboot,
    superseded: row.superseded,
    failedAt: row.failedAt
  }));
}

async function loadOverrides(organizationId: string): Promise<PatchDecisionOverride[]> {
  const rows = await prisma.$queryRaw<PatchOverrideRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id AS "organizationId",
      scope_type AS "scopeType",
      scope_key AS "scopeKey",
      action,
      operation_id AS "operationId",
      update_key AS "updateKey",
      kb_article AS "kbArticle",
      category,
      reason,
      defer_until AS "deferUntil",
      expires_at AS "expiresAt",
      enabled,
      created_by AS "createdBy",
      created_at AS "createdAt",
      updated_at AS "updatedAt"
    FROM public.rmm_patch_override
    WHERE organization_id = ${organizationId}
      AND enabled = true
      AND (expires_at IS NULL OR expires_at > NOW())
    ORDER BY created_at DESC
  `);
  return rows.map((row) => ({
    id: row.id,
    scopeType: row.scopeType,
    scopeKey: row.scopeKey,
    action: row.action,
    operationId: row.operationId,
    updateKey: row.updateKey,
    kbArticle: row.kbArticle,
    category: row.category,
    reason: row.reason,
    deferUntil: row.deferUntil,
    expiresAt: row.expiresAt,
    enabled: row.enabled
  }));
}

async function loadGroupIds(organizationId: string, agentId: string): Promise<string[]> {
  const rows = await prisma.$queryRaw<Array<{ groupId: string }>>(Prisma.sql`
    SELECT group_id AS "groupId"
    FROM public.rmm_patch_device_group_member
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
  `);
  return rows.map((row) => row.groupId);
}

function decisionLabel(action: string): string {
  if (action === 'blocked') return 'blocked';
  if (action === 'defer') return 'deferred';
  if (action === 'reportOnly') return 'report_only';
  return 'authorized';
}

function metadataRecord(item: { metadata?: Record<string, unknown> }): Record<string, unknown> {
  return item.metadata && typeof item.metadata === 'object' && !Array.isArray(item.metadata)
    ? item.metadata
    : {};
}

function readOverrideIds(item: { metadata?: Record<string, unknown> }): string[] {
  const metadata = metadataRecord(item);
  const raw = metadata.overrideIds ?? metadata.overrideId;
  if (Array.isArray(raw)) {
    return raw.filter((value): value is string => typeof value === 'string' && value.trim().length > 0);
  }
  return typeof raw === 'string' && raw.trim() ? [raw] : [];
}

function asPlainRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function readOverrideIdsFromDecisionDetails(details: unknown): string[] {
  const metadata = asPlainRecord(asPlainRecord(details).metadata);
  const raw = metadata.overrideIds ?? metadata.overrideId;
  if (Array.isArray(raw)) {
    return raw.filter((value): value is string => typeof value === 'string' && value.trim().length > 0);
  }
  return typeof raw === 'string' && raw.trim() ? [raw] : [];
}

async function resolveDecisionActor(organizationId: string, item: { metadata?: Record<string, unknown> }) {
  const overrideIds = readOverrideIds(item);
  if (overrideIds.length === 0) {
    return { actorType: 'system', actorUserId: null, actorEmail: null };
  }
  const rows = await prisma.$queryRaw<Array<{ createdBy: string; email: string | null }>>(Prisma.sql`
    SELECT
      o.created_by AS "createdBy",
      u.email AS email
    FROM public.rmm_patch_override o
    LEFT JOIN public."User" u ON u.id = o.created_by
    WHERE o.organization_id = ${organizationId}
      AND o.id IN (${Prisma.join(overrideIds)})
    ORDER BY o.created_at DESC
    LIMIT 1
  `);
  const row = rows[0];
  if (!row) {
    return { actorType: 'system', actorUserId: null, actorEmail: null };
  }
  return { actorType: 'user', actorUserId: row.createdBy, actorEmail: row.email };
}

async function disableCompletedOneShotOverrides(
  options: {
    organizationId: string;
    agentId: string;
    operationId?: string | null;
    status: string;
  },
  client: Prisma.TransactionClient = prisma
) {
  if (!options.operationId || (options.status !== 'completed' && options.status !== 'failed')) return;
  const rows = await client.$queryRaw<Array<{ details: unknown }>>(Prisma.sql`
    SELECT details_jsonb AS details
    FROM public.rmm_patch_decision_log
    WHERE organization_id = ${options.organizationId}
      AND agent_id = ${options.agentId}
      AND operation_id = ${options.operationId}
      AND actor_type <> 'agent'
    ORDER BY decided_at DESC
    LIMIT 1
  `);
  const overrideIds = [...new Set(readOverrideIdsFromDecisionDetails(rows[0]?.details))];
  if (overrideIds.length === 0) return;
  await client.$executeRaw(Prisma.sql`
    UPDATE public.rmm_patch_override
    SET enabled = false, updated_at = NOW()
    WHERE organization_id = ${options.organizationId}
      AND id IN (${Prisma.join(overrideIds)})
      AND action IN ('force_scan', 'force_download', 'force_install', 'force_reboot')
      AND enabled = true
  `);
}

export async function persistPatchDecisionLog(plan: PatchActionPlan): Promise<void> {
  for (const item of plan.actions) {
    const actor = await resolveDecisionActor(plan.organizationId, item);
    await prisma.$executeRaw(Prisma.sql`
      INSERT INTO public.rmm_patch_decision_log
        (
          id, organization_id, agent_id, policy_id, operation_id, action,
          update_keys_jsonb, decision, reason, actor_type, actor_user_id, actor_email,
          details_jsonb, decided_at
        )
      VALUES
        (
          ${randomUUID()},
          ${plan.organizationId},
          ${plan.agentId},
          ${plan.policyId},
          ${item.operationId},
          ${item.action},
          ${jsonString(item.updateKeys)}::jsonb,
          ${decisionLabel(item.action)},
          ${item.reason},
          ${actor.actorType},
          ${actor.actorUserId},
          ${actor.actorEmail},
          ${jsonString({
            category: item.category ?? null,
            window: item.window ?? null,
            notBefore: item.notBefore ?? null,
            deadlineAt: item.deadlineAt ?? null,
            forced: item.forced,
            metadata: item.metadata ?? {},
            managedMode: plan.managedMode,
            nativeWindowsUpdateControl: plan.nativeWindowsUpdateControl,
            generatedAt: plan.generatedAt
          })}::jsonb,
          ${new Date(plan.generatedAt)}
        )
    `);
  }
}

export async function evaluateAndPersistPatchPlan(options: {
  agentId: string;
  organizationId?: string | null;
  observedState?: Record<string, unknown>;
  now?: Date | string;
  persist?: boolean;
}): Promise<PatchActionPlan> {
  const deviceRow = await loadDevice(options.agentId, options.organizationId ?? null);
  if (!deviceRow) {
    throw Object.assign(new Error('Device not found'), { status: 404 });
  }
  const [policies, updates, overrides, groupIds] = await Promise.all([
    loadPatchPoliciesForOrganization(deviceRow.organizationId),
    loadUpdates(deviceRow.organizationId, deviceRow.agentId),
    loadOverrides(deviceRow.organizationId),
    loadGroupIds(deviceRow.organizationId, deviceRow.agentId)
  ]);
  const effectivePolicy = resolveEffectivePatchPolicy(policies.map(policyForResolution), {
    organizationId: deviceRow.organizationId,
    customerId: deviceRow.customerId,
    siteId: deviceRow.siteId,
    agentId: deviceRow.agentId,
    osFamily: normalizePatchOsFamily(deviceRow.os)
  });
  const policy = effectivePolicy
    ? {
        ...effectivePolicy,
        managedMode: effectivePolicy.managedMode ?? undefined,
        nativeWindowsUpdateControl: effectivePolicy.nativeWindowsUpdateControl ?? undefined
      }
    : null;
  const plan = evaluatePatchActionPlan({
    now: options.now,
    device: toDecisionDevice(deviceRow, options.observedState ?? {}),
    policy,
    updates,
    overrides,
    groupIds
  });
  if (options.persist !== false) {
    await persistPatchDecisionLog(plan);
  }
  return plan;
}

export async function loadPatchOverview(organizationId: string) {
  const [devices, updates, policies, overrides, decisions] = await Promise.all([
    prisma.$queryRaw<PatchOverviewDeviceRow[]>(Prisma.sql`
      WITH patch_state_counts AS (
        SELECT
          agent_id,
          COUNT(id) FILTER (
            WHERE lifecycle_state NOT IN ('installed', 'superseded')
              AND applicability_state = 'applicable'
          )::int AS pending_updates,
          COUNT(id) FILTER (
            WHERE downloaded_at IS NOT NULL
              AND lifecycle_state NOT IN ('installed', 'failed', 'superseded', 'reboot_pending')
              AND applicability_state = 'applicable'
          )::int AS downloaded_updates,
          COUNT(id) FILTER (WHERE lifecycle_state = 'failed')::int AS failed_updates,
          COUNT(id) FILTER (WHERE approval_state = 'blocked')::int AS blocked_updates,
          COUNT(id) FILTER (WHERE approval_state = 'deferred')::int AS deferred_updates,
          COUNT(id) FILTER (WHERE lifecycle_state = 'reboot_pending')::int AS reboot_pending_updates
        FROM public.rmm_patch_device_update_state
        WHERE organization_id = ${organizationId}
        GROUP BY agent_id
      )
      SELECT
        d.organization_id AS "organizationId",
        d.agent_id AS "agentId",
        d.hostname,
        d.os,
        ds.os_version AS "osVersion",
        d.customer_id AS "customerId",
        c.name AS "customerName",
        d.site_id AS "siteId",
        s.name AS "siteName",
        d.last_seen AS "lastSeen",
        ds.collected_at AS "collectedAt",
        ds.reboot_required AS "rebootRequired",
        d.device_type AS "deviceType",
        d.patch_ring AS "patchRing",
        d.patch_managed AS "patchManaged",
        d.native_windows_update_control AS "nativeWindowsUpdateControl",
        d.patch_maintenance_mode_until AS "patchMaintenanceModeUntil",
        d.patch_tags AS "patchTags",
        d.macos_update_account_status_jsonb AS "macosUpdateAccount",
        COALESCE(
          ds.inventory_data #> '{operating_system,ad_ds}',
          ds.inventory_data #> '{operatingSystem,adDs}',
          ds.inventory_data #> '{ad_ds}'
        ) AS "serverAdDs",
        COALESCE(
          ds.inventory_data #> '{operating_system,dhcp_server}',
          ds.inventory_data #> '{operatingSystem,dhcpServer}',
          ds.inventory_data #> '{dhcp_server}'
        ) AS "serverDhcp",
        COALESCE(
          ds.inventory_data #> '{operating_system,dns_server}',
          ds.inventory_data #> '{operatingSystem,dnsServer}',
          ds.inventory_data #> '{dns_server}'
        ) AS "serverDns",
        COALESCE(
          ds.inventory_data #> '{operating_system,iis}',
          ds.inventory_data #> '{operatingSystem,iis}',
          ds.inventory_data #> '{iis}'
        ) AS "serverIis",
        COALESCE(ps.pending_updates, 0)::int AS "pendingUpdates",
        COALESCE(ps.downloaded_updates, 0)::int AS "downloadedUpdates",
        COALESCE(ps.failed_updates, 0)::int AS "failedUpdates",
        COALESCE(ps.blocked_updates, 0)::int AS "blockedUpdates",
        COALESCE(ps.deferred_updates, 0)::int AS "deferredUpdates",
        COALESCE(ps.reboot_pending_updates, 0)::int AS "rebootPendingUpdates"
      FROM public.rmm_devices d
      LEFT JOIN public.customers c ON c.id = d.customer_id
      LEFT JOIN public.rmm_sites s ON s.id = d.site_id
      LEFT JOIN rmm_telemetry.device_state ds ON ds.organization_id = d.organization_id AND ds.agent_id = d.agent_id
      LEFT JOIN patch_state_counts ps ON ps.agent_id = d.agent_id
      WHERE d.organization_id = ${organizationId}
      ORDER BY d.hostname ASC
    `),
    prisma.$queryRaw<PatchOverviewUpdateRow[]>(Prisma.sql`
      SELECT
        c.update_key AS "updateKey",
        c.title,
        c.kb_article AS "kbArticle",
        c.category,
        c.release_date AS "releaseDate",
        c.release_date_source AS "releaseDateSource",
        c.source,
        COUNT(DISTINCT us.agent_id) FILTER (
          WHERE us.lifecycle_state NOT IN ('installed', 'superseded')
            AND us.applicability_state = 'applicable'
        )::int AS "affectedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (
          WHERE us.agent_id IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        )::int AS "associatedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.lifecycle_state = 'detected')::int AS "detectedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (
          WHERE us.downloaded_at IS NOT NULL
            AND us.lifecycle_state NOT IN ('installed', 'failed', 'superseded', 'reboot_pending')
            AND us.applicability_state = 'applicable'
        )::int AS "downloadedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.lifecycle_state = 'installed')::int AS "installedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.lifecycle_state = 'failed')::int AS "failedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.approval_state = 'blocked')::int AS "blockedDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.approval_state = 'deferred')::int AS "deferredDevices",
        COUNT(DISTINCT us.agent_id) FILTER (WHERE us.lifecycle_state = 'superseded')::int AS "supersededDevices",
        COALESCE(array_agg(DISTINCT d.agent_id) FILTER (
          WHERE d.agent_id IS NOT NULL
            AND us.lifecycle_state NOT IN ('installed', 'superseded')
            AND us.applicability_state = 'applicable'
        ), ARRAY[]::text[]) AS "affectedAgentIds",
        COALESCE(array_agg(DISTINCT d.hostname) FILTER (
          WHERE d.hostname IS NOT NULL
            AND us.lifecycle_state NOT IN ('installed', 'superseded')
            AND us.applicability_state = 'applicable'
        ), ARRAY[]::text[]) AS "affectedHostnames",
        COALESCE(array_agg(DISTINCT d.agent_id) FILTER (
          WHERE d.agent_id IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "associatedAgentIds",
        COALESCE(array_agg(DISTINCT d.hostname) FILTER (
          WHERE d.hostname IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "associatedHostnames",
        COALESCE(array_agg(DISTINCT customer.name) FILTER (
          WHERE customer.name IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "customerNames",
        COALESCE(array_agg(DISTINCT site.name) FILTER (
          WHERE site.name IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "siteNames",
        COALESCE(array_agg(DISTINCT CASE
          WHEN lower(COALESCE(d.os, '')) LIKE '%windows%' THEN 'windows'
          WHEN lower(COALESCE(d.os, '')) LIKE '%linux%' OR lower(COALESCE(d.os, '')) LIKE '%ubuntu%' OR lower(COALESCE(d.os, '')) LIKE '%alma%' OR lower(COALESCE(d.os, '')) LIKE '%debian%' THEN 'linux'
          WHEN lower(COALESCE(d.os, '')) LIKE '%mac%' OR lower(COALESCE(d.os, '')) LIKE '%darwin%' THEN 'macos'
          ELSE 'unknown'
        END) FILTER (
          WHERE d.agent_id IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "osFamilies",
        COALESCE(array_agg(DISTINCT d.device_type) FILTER (
          WHERE d.device_type IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "deviceTypes",
        COALESCE(array_agg(DISTINCT d.patch_ring) FILTER (
          WHERE d.patch_ring IS NOT NULL
            AND us.lifecycle_state <> 'superseded'
            AND us.applicability_state <> 'not_applicable'
        ), ARRAY[]::text[]) AS "patchRings",
        c.last_seen_at AS "lastSeenAt"
      FROM public.rmm_patch_update_catalog c
      LEFT JOIN public.rmm_patch_device_update_state us
        ON us.organization_id = c.organization_id
        AND us.update_key = c.update_key
      LEFT JOIN public.rmm_devices d
        ON d.organization_id = c.organization_id
        AND d.agent_id = us.agent_id
      LEFT JOIN public.customers customer ON customer.id = d.customer_id
      LEFT JOIN public.rmm_sites site ON site.id = d.site_id
      WHERE c.organization_id = ${organizationId}
      GROUP BY c.id
      ORDER BY "affectedDevices" DESC, "associatedDevices" DESC, "lastSeenAt" DESC
      LIMIT 500
    `),
    loadPatchPoliciesForOrganization(organizationId),
    prisma.$queryRaw<PatchOverrideRow[]>(Prisma.sql`
      SELECT
        o.id,
        o.organization_id AS "organizationId",
        o.scope_type AS "scopeType",
        o.scope_key AS "scopeKey",
        o.action,
        o.operation_id AS "operationId",
        o.update_key AS "updateKey",
        o.kb_article AS "kbArticle",
        o.category,
        o.reason,
        o.defer_until AS "deferUntil",
        o.expires_at AS "expiresAt",
        o.enabled,
        o.created_by AS "createdBy",
        u.email AS "createdByEmail",
        d.agent_id AS "targetAgentId",
        d.hostname AS "targetHostname",
        d.os AS "targetOs",
        latest_action.action_type AS "latestActionType",
        latest_action.status AS "latestActionStatus",
        latest_action.phase AS "latestActionPhase",
        latest_action.updated_at AS "latestActionUpdatedAt",
        o.created_at AS "createdAt",
        o.updated_at AS "updatedAt"
      FROM public.rmm_patch_override o
      LEFT JOIN public."User" u ON u.id = o.created_by
      LEFT JOIN public.rmm_devices d
        ON o.scope_type = 'device'
        AND d.organization_id = o.organization_id
        AND d.agent_id = o.scope_key
      LEFT JOIN LATERAL (
        SELECT action_type, status, phase, updated_at
        FROM public.rmm_patch_action a
        WHERE a.organization_id = o.organization_id
          AND o.operation_id IS NOT NULL
          AND a.operation_id = o.operation_id
          AND (o.scope_type <> 'device' OR a.agent_id = o.scope_key)
        ORDER BY a.updated_at DESC
        LIMIT 1
      ) latest_action ON TRUE
      WHERE o.organization_id = ${organizationId}
        AND o.enabled = true
        AND (o.expires_at IS NULL OR o.expires_at > NOW())
      ORDER BY o.created_at DESC
      LIMIT 200
    `),
    prisma.$queryRaw<PatchDecisionLogRow[]>(Prisma.sql`
      SELECT
        l.id,
        l.agent_id AS "agentId",
        l.policy_id AS "policyId",
        l.operation_id AS "operationId",
        l.action,
        l.update_keys_jsonb AS "updateKeys",
        l.decision,
        l.reason,
        l.actor_type AS "actorType",
        l.actor_user_id AS "actorUserId",
        l.actor_email AS "actorEmail",
        a.status AS "actionStatus",
        a.phase AS "actionPhase",
        a.updated_at AS "actionUpdatedAt",
        l.details_jsonb AS details,
        l.decided_at AS "decidedAt"
      FROM public.rmm_patch_decision_log l
      LEFT JOIN LATERAL (
        SELECT status, phase, updated_at
        FROM public.rmm_patch_action a
        WHERE a.organization_id = l.organization_id
          AND a.agent_id = l.agent_id
          AND a.operation_id = l.operation_id
        ORDER BY a.updated_at DESC
        LIMIT 1
      ) a ON TRUE
      WHERE l.organization_id = ${organizationId}
      ORDER BY l.decided_at DESC
      LIMIT 200
    `)
  ]);

  return {
    generatedAt: new Date().toISOString(),
    summary: {
      devices: devices.length,
      managed: devices.filter((device) => device.patchManaged).length,
      pending: devices.reduce((sum, device) => sum + Number(device.pendingUpdates ?? 0), 0),
      downloaded: devices.reduce((sum, device) => sum + Number(device.downloadedUpdates ?? 0), 0),
      failed: devices.reduce((sum, device) => sum + Number(device.failedUpdates ?? 0), 0),
      reboot: devices.filter((device) => device.rebootRequired === true || Number(device.rebootPendingUpdates ?? 0) > 0).length
    },
    devices: devices.map((device) => ({
      agentId: device.agentId,
      hostname: device.hostname,
      os: device.os,
      osVersion: device.osVersion,
      customerId: device.customerId,
      customerName: device.customerName,
      siteId: device.siteId,
      siteName: device.siteName,
      lastSeen: device.lastSeen.toISOString(),
      lastScanAt: iso(device.collectedAt),
      rebootRequired: device.rebootRequired === true,
      deviceType: device.deviceType,
      patchRing: device.patchRing,
      patchManaged: device.patchManaged,
      nativeWindowsUpdateControl: device.nativeWindowsUpdateControl,
      patchMaintenanceModeUntil: iso(device.patchMaintenanceModeUntil),
      patchTags: stringArray(device.patchTags),
      macosUpdateAccount: device.macosUpdateAccount ?? null,
      serverRoleInventory: buildServerRoleInventory(device),
      pendingUpdates: Number(device.pendingUpdates ?? 0),
      downloadedUpdates: Number(device.downloadedUpdates ?? 0),
      failedUpdates: Number(device.failedUpdates ?? 0),
      blockedUpdates: Number(device.blockedUpdates ?? 0),
      deferredUpdates: Number(device.deferredUpdates ?? 0),
      rebootPendingUpdates: Number(device.rebootPendingUpdates ?? 0)
    })),
    updates: updates.map((update) => ({
      updateKey: update.updateKey,
      title: update.title,
      kbArticle: update.kbArticle,
      category: update.category,
      releaseDate: iso(update.releaseDate),
      releaseDateSource: update.releaseDateSource,
      source: update.source,
      affectedDevices: Number(update.affectedDevices ?? 0),
      associatedDevices: Number(update.associatedDevices ?? 0),
      detectedDevices: Number(update.detectedDevices ?? 0),
      downloadedDevices: Number(update.downloadedDevices ?? 0),
      installedDevices: Number(update.installedDevices ?? 0),
      failedDevices: Number(update.failedDevices ?? 0),
      blockedDevices: Number(update.blockedDevices ?? 0),
      deferredDevices: Number(update.deferredDevices ?? 0),
      supersededDevices: Number(update.supersededDevices ?? 0),
      affectedAgentIds: stringArray(update.affectedAgentIds),
      affectedHostnames: stringArray(update.affectedHostnames),
      associatedAgentIds: stringArray(update.associatedAgentIds),
      associatedHostnames: stringArray(update.associatedHostnames),
      customerNames: stringArray(update.customerNames),
      siteNames: stringArray(update.siteNames),
      osFamilies: stringArray(update.osFamilies),
      deviceTypes: stringArray(update.deviceTypes),
      patchRings: stringArray(update.patchRings),
      lastSeenAt: update.lastSeenAt.toISOString()
    })),
    policies: policies.map((policy) => ({
      ...policy,
      createdAt: policy.createdAt.toISOString(),
      updatedAt: policy.updatedAt instanceof Date ? policy.updatedAt.toISOString() : iso(policy.updatedAt)
    })),
    overrides: overrides.map((override) => ({
      ...override,
      deferUntil: iso(override.deferUntil),
      expiresAt: iso(override.expiresAt),
      latestActionUpdatedAt: iso(override.latestActionUpdatedAt),
      createdAt: override.createdAt.toISOString(),
      updatedAt: override.updatedAt.toISOString()
    })),
    decisions: decisions.map((decision) => ({
      ...decision,
      actorType: decision.actorType ?? 'system',
      actorUserId: decision.actorUserId,
      actorEmail: decision.actorEmail,
      actionUpdatedAt: iso(decision.actionUpdatedAt),
      decidedAt: decision.decidedAt.toISOString()
    }))
  };
}

export async function loadPatchDeviceState(organizationId: string, agentId: string) {
  const deviceRows = await prisma.$queryRaw<Array<{ agentId: string }>>(Prisma.sql`
    SELECT agent_id AS "agentId"
    FROM public.rmm_devices
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
    LIMIT 1
  `);
  if (!deviceRows[0]) {
    throw Object.assign(new Error('Device not found'), { status: 404 });
  }

  const rows = await prisma.$queryRaw<PatchDeviceStateRow[]>(Prisma.sql`
    SELECT
      update_key AS "updateKey",
      title,
      kb_article AS "kbArticle",
      category,
      applicability_state AS "applicabilityState",
      approval_state AS "approvalState",
      lifecycle_state AS "lifecycleState",
      release_date AS "releaseDate",
      first_detected_at AS "firstDetectedAt",
      last_detected_at AS "lastDetectedAt",
      eligible_at AS "eligibleAt",
      install_deadline_at AS "installDeadlineAt",
      reboot_deadline_at AS "rebootDeadlineAt",
      downloaded_at AS "downloadedAt",
      installed_at AS "installedAt",
      failed_at AS "failedAt",
      failure_code AS "failureCode",
      failure_hresult AS "failureHresult",
      failure_message AS "failureMessage",
      requires_reboot AS "requiresReboot"
    FROM public.rmm_patch_device_update_state
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
    ORDER BY
      CASE lifecycle_state
        WHEN 'failed' THEN 0
        WHEN 'downloaded' THEN 1
        WHEN 'reboot_pending' THEN 2
        WHEN 'detected' THEN 3
        WHEN 'installed' THEN 4
        WHEN 'superseded' THEN 5
        ELSE 6
      END ASC,
      COALESCE(failed_at, downloaded_at, installed_at, last_detected_at, first_detected_at) DESC,
      title ASC
  `);

  const transactionFailures = await prisma.$queryRaw<PatchTransactionFailureRow[]>(Prisma.sql`
    SELECT
      id,
      operation_id AS "operationId",
      action,
      reason,
      details_jsonb #>> '{evidence,error}' AS error,
      details_jsonb #>> '{evidence,phase}' AS phase,
      details_jsonb #>> '{evidence,packageManager}' AS "packageManager",
      COALESCE(jsonb_array_length(update_keys_jsonb), 0)::int AS "updateKeyCount",
      CASE
        WHEN jsonb_typeof(details_jsonb #> '{evidence,transactionPackageSpecs}') = 'array'
          THEN jsonb_array_length(details_jsonb #> '{evidence,transactionPackageSpecs}')::int
        WHEN jsonb_typeof(details_jsonb #> '{evidence,summary,transactionPackageSpecs}') = 'array'
          THEN jsonb_array_length(details_jsonb #> '{evidence,summary,transactionPackageSpecs}')::int
        ELSE NULL
      END AS "transactionPackageCount",
      decided_at AS "decidedAt"
    FROM public.rmm_patch_decision_log
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
      AND action IN ('download', 'install')
      AND decision = 'failed'
      AND (
        details_jsonb #>> '{evidence,failureScope}' = 'transaction'
        OR details_jsonb #>> '{evidence,summary,transactionFailure}' = 'true'
      )
    ORDER BY decided_at DESC
    LIMIT 10
  `);

  const summary = rows.reduce(
    (acc, row) => {
      const applicable = row.applicabilityState === 'applicable';
      const lifecycleState = effectivePatchLifecycleState(row);
      if (applicable && lifecycleState !== 'installed' && lifecycleState !== 'superseded') acc.pending += 1;
      if (applicable && lifecycleState === 'downloaded') acc.downloaded += 1;
      if (lifecycleState === 'failed') acc.failed += 1;
      if (lifecycleState === 'installed') acc.installed += 1;
      if (row.approvalState === 'blocked') acc.blocked += 1;
      if (row.approvalState === 'deferred') acc.deferred += 1;
      if (lifecycleState === 'reboot_pending') acc.rebootPending += 1;
      return acc;
    },
    { pending: 0, downloaded: 0, failed: 0, installed: 0, blocked: 0, deferred: 0, rebootPending: 0, transactionFailures: transactionFailures.length }
  );

  return {
    agentId,
    generatedAt: new Date().toISOString(),
    summary,
    updates: rows.map((row) => ({
      updateKey: row.updateKey,
      title: row.title,
      kbArticle: row.kbArticle,
      category: row.category,
      applicabilityState: row.applicabilityState,
      approvalState: row.approvalState,
      lifecycleState: effectivePatchLifecycleState(row),
      releaseDate: iso(row.releaseDate),
      firstDetectedAt: row.firstDetectedAt.toISOString(),
      lastDetectedAt: row.lastDetectedAt.toISOString(),
      eligibleAt: iso(row.eligibleAt),
      installDeadlineAt: iso(row.installDeadlineAt),
      rebootDeadlineAt: iso(row.rebootDeadlineAt),
      downloadedAt: iso(row.downloadedAt),
      installedAt: iso(row.installedAt),
      failedAt: iso(row.failedAt),
      failureCode: row.failureCode,
      failureHresult: row.failureHresult,
      failureMessage: row.failureMessage,
      requiresReboot: row.requiresReboot
    })),
    transactionFailures: transactionFailures.map((failure) => ({
      id: failure.id,
      operationId: failure.operationId,
      action: failure.action,
      reason: failure.reason,
      error: failure.error,
      phase: failure.phase,
      packageManager: failure.packageManager,
      updateKeyCount: Number(failure.updateKeyCount ?? 0),
      transactionPackageCount:
        failure.transactionPackageCount === null ? null : Number(failure.transactionPackageCount),
      decidedAt: failure.decidedAt.toISOString()
    }))
  };
}

export async function createPatchOverride(options: {
  organizationId: string;
  scopeType: PatchDecisionOverride['scopeType'];
  scopeKey: string;
  action: PatchOverrideAction;
  operationId?: string | null;
  createdBy: string;
  updateKey?: string | null;
  kbArticle?: string | null;
  category?: string | null;
  reason?: string | null;
  deferUntil?: Date | null;
  expiresAt?: Date | null;
}) {
  const id = randomUUID();
  await prisma.$executeRaw(Prisma.sql`
    INSERT INTO public.rmm_patch_override
      (
        id, organization_id, scope_type, scope_key, action, operation_id, update_key, kb_article,
        category, reason, defer_until, expires_at, enabled, created_by, created_at, updated_at
      )
    VALUES
      (
        ${id}, ${options.organizationId}, ${options.scopeType}, ${options.scopeKey}, ${options.action},
        ${options.operationId ?? null}, ${options.updateKey ?? null}, ${options.kbArticle ?? null}, ${options.category ?? null},
        ${options.reason ?? null}, ${options.deferUntil ?? null}, ${options.expiresAt ?? null},
        true, ${options.createdBy}, NOW(), NOW()
      )
  `);
  return id;
}

export async function updateDevicePatchControl(options: {
  organizationId: string;
  agentIds: string[];
  patchManaged?: boolean;
  nativeWindowsUpdateControl?: boolean;
  patchRing?: PatchRing;
  maintenanceModeUntil?: Date | null;
}) {
  if (options.agentIds.length === 0) return 0;
  const rows = await prisma.$queryRaw<Array<{ agentId: string }>>(Prisma.sql`
    UPDATE public.rmm_devices
    SET
      patch_managed = COALESCE(${options.patchManaged ?? null}, patch_managed),
      native_windows_update_control = COALESCE(${options.nativeWindowsUpdateControl ?? null}, native_windows_update_control),
      patch_ring = COALESCE(${options.patchRing ?? null}, patch_ring),
      patch_maintenance_mode_until = CASE
        WHEN ${options.maintenanceModeUntil === undefined} THEN patch_maintenance_mode_until
        ELSE ${options.maintenanceModeUntil ?? null}
      END,
      updated_at = NOW()
    WHERE organization_id = ${options.organizationId}
      AND agent_id IN (${Prisma.join(options.agentIds)})
    RETURNING agent_id AS "agentId"
  `);
  return rows.length;
}

type PatchActionResultOptions = {
  organizationId?: string | null;
  agentId: string;
  operationId?: string | null;
  action: string;
  status: string;
  updateKeys?: string[];
  evidence?: unknown;
  logDecision?: boolean;
};

async function recordPatchActionResultWithClient(
  client: Prisma.TransactionClient,
  options: PatchActionResultOptions,
  actionProjection: 'full' | 'evidence_only'
) {
  const device = await loadDevice(options.agentId, options.organizationId ?? null, client);
  if (!device) throw Object.assign(new Error('Device not found'), { status: 404 });
  const now = new Date();
  const updateKeys = options.updateKeys ?? [];
  const projection = projectPatchActionResultUpdates({
    action: options.action,
    status: options.status,
    updateKeys,
    evidence: options.evidence
  });
  if (projection.usedEvidence) {
    for (const update of projection.updates) {
      await client.$executeRaw(Prisma.sql`
        UPDATE public.rmm_patch_device_update_state
        SET
          lifecycle_state = ${update.lifecycleState},
          downloaded_at = CASE WHEN ${update.lifecycleState === 'downloaded' || update.lifecycleState === 'installed'} THEN COALESCE(downloaded_at, ${now}) ELSE downloaded_at END,
          installed_at = CASE WHEN ${update.lifecycleState === 'installed'} THEN ${now} ELSE installed_at END,
          failed_at = CASE WHEN ${update.lifecycleState === 'failed'} THEN ${now} ELSE failed_at END,
          requires_reboot = CASE WHEN ${update.requiresReboot === true} THEN TRUE ELSE requires_reboot END,
          failure_message = CASE WHEN ${update.lifecycleState === 'failed'} THEN ${update.failureMessage ?? jsonString(options.evidence)} ELSE failure_message END,
          metadata_jsonb = COALESCE(metadata_jsonb, '{}'::jsonb) || ${jsonString({ lastActionResult: options })}::jsonb,
          updated_at = NOW()
        WHERE organization_id = ${device.organizationId}
          AND agent_id = ${device.agentId}
          AND update_key = ${update.updateKey}
      `);
    }
  } else if (projection.updates.length > 0) {
    const lifecycleState = projection.updates[0].lifecycleState;
    await client.$executeRaw(Prisma.sql`
      UPDATE public.rmm_patch_device_update_state
      SET
        lifecycle_state = ${lifecycleState},
        downloaded_at = CASE WHEN ${lifecycleState === 'downloaded' || lifecycleState === 'installed'} THEN COALESCE(downloaded_at, ${now}) ELSE downloaded_at END,
        installed_at = CASE WHEN ${lifecycleState === 'installed'} THEN ${now} ELSE installed_at END,
        failed_at = CASE WHEN ${lifecycleState === 'failed'} THEN ${now} ELSE failed_at END,
        failure_message = CASE WHEN ${lifecycleState === 'failed'} THEN ${projection.updates[0].failureMessage ?? jsonString(options.evidence)} ELSE failure_message END,
        metadata_jsonb = COALESCE(metadata_jsonb, '{}'::jsonb) || ${jsonString({ lastActionResult: options })}::jsonb,
        updated_at = NOW()
      WHERE organization_id = ${device.organizationId}
        AND agent_id = ${device.agentId}
        AND update_key IN (${Prisma.join(updateKeys)})
    `);
  }
  if (options.operationId && actionProjection === 'full') {
    const finished = options.status === 'completed' || options.status === 'failed' || options.status === 'cancelled';
    await client.$executeRaw(Prisma.sql`
      UPDATE public.rmm_patch_action
      SET
        status = ${options.status},
        phase = CASE
          WHEN ${finished} THEN COALESCE(phase, 'finalizing')
          ELSE phase
        END,
        evidence_jsonb = COALESCE(evidence_jsonb, '{}'::jsonb) || ${jsonString({ actionResult: options.evidence ?? null })}::jsonb,
        error_message = CASE WHEN ${options.status === 'failed'} THEN ${jsonString(options.evidence)} ELSE error_message END,
        finished_at = CASE WHEN ${finished} THEN COALESCE(finished_at, ${now}) ELSE finished_at END,
        updated_at = NOW()
      WHERE organization_id = ${device.organizationId}
        AND agent_id = ${device.agentId}
        AND operation_id = ${options.operationId}
    `);
  } else if (options.operationId) {
    await client.$executeRaw(Prisma.sql`
      UPDATE public.rmm_patch_action
      SET evidence_jsonb = COALESCE(evidence_jsonb, '{}'::jsonb)
            || ${jsonString({ actionResult: options.evidence ?? null })}::jsonb
      WHERE organization_id = ${device.organizationId}
        AND agent_id = ${device.agentId}
        AND operation_id = ${options.operationId}
    `);
  }
  await disableCompletedOneShotOverrides(
    {
      organizationId: device.organizationId,
      agentId: device.agentId,
      operationId: options.operationId,
      status: options.status
    },
    client
  );
  if (options.logDecision !== false) {
    await client.$executeRaw(Prisma.sql`
      INSERT INTO public.rmm_patch_decision_log
        (
          id, organization_id, agent_id, policy_id, operation_id, action,
          update_keys_jsonb, decision, reason, actor_type, actor_user_id, actor_email,
          details_jsonb, decided_at
        )
      VALUES
        (
          ${randomUUID()}, ${device.organizationId}, ${device.agentId}, NULL,
          ${options.operationId ?? randomUUID()}, ${options.action},
          ${jsonString(updateKeys)}::jsonb, ${options.status},
          ${`Agent reported ${options.action} ${options.status}.`},
          ${'agent'},
          ${null},
          ${null},
          ${jsonString({ source: 'agent_action_result', evidence: options.evidence ?? null })}::jsonb,
          NOW()
        )
    `);
  }
}

export async function recordPatchActionResult(options: PatchActionResultOptions) {
  return recordPatchActionResultWithClient(
    prisma,
    options,
    'full'
  );
}

export async function recordPatchActionResultInTransaction(
  transaction: Prisma.TransactionClient,
  options: Omit<PatchActionResultOptions, 'logDecision'>
) {
  return recordPatchActionResultWithClient(
    transaction,
    { ...options, logDecision: false },
    'evidence_only'
  );
}

export type { PatchPolicyRow, PatchOverviewDeviceRow, PatchOverrideRow, PatchDecisionLogRow };
