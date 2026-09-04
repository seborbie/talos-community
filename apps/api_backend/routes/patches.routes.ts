import { randomUUID } from 'crypto';
import { Prisma } from '@prisma/client';
import { Router } from 'express';
import { env } from '../lib/env';
import { createLogger } from '../lib/logger';
import { DEFAULT_PATCH_POLICY_SCOPE_KEY, ensureDefaultPatchPolicy } from '../lib/patchPolicies';
import { prisma } from '../lib/prisma';
import {
  buildPatchUpdateKey,
  calculatePatchComplianceSummary,
  CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
  DEFAULT_PATCH_POLICY_PRIORITY,
  PatchApprovalMode,
  PatchDecision,
  PatchDeviceComplianceInput,
  PatchPolicyForResolution,
  PatchPolicyScopeType,
  PatchPolicyTargetOsFamily,
  PatchRebootBehavior
} from '../lib/patchManagement';
import {
  coercePatchPolicyConfig,
  defaultPatchPolicyConfig,
  evaluatePatchActionPlan,
  PatchActionPlan,
  PatchCategory,
  PatchDecisionDevice,
  PatchDecisionOverride,
  PatchDecisionUpdate,
  PatchDeviceType,
  PatchOverrideAction,
  PatchRing
} from '../lib/patchDecisionEngine';
import {
  createPatchOverride,
  evaluateAndPersistPatchPlan,
  loadPatchDeviceState,
  loadPatchOverview,
  selectActionablePatchUpdateTargetAgentIds,
  updateDevicePatchControl
} from '../lib/patchDecisionService';
import { requireAuth, AuthedRequest } from '../middleware/auth';

export const patchesRouter = Router();
patchesRouter.use(requireAuth);

const log = createLogger('api_backend::patches');

type Membership = NonNullable<Awaited<ReturnType<typeof getCurrentMembership>>>;

type PatchPolicyRow = {
  id: string;
  organizationId: string;
  scopeType: PatchPolicyScopeType;
  scopeKey: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  name: string;
  targetOsFamily: PatchPolicyTargetOsFamily;
  approvalMode: PatchApprovalMode;
  maintenanceWindowStart: string | null;
  maintenanceWindowEnd: string | null;
  maintenanceWindowTimezone: string | null;
  rebootBehavior: PatchRebootBehavior;
  deferralDays: number;
  managedMode: boolean;
  nativeWindowsUpdateControl: boolean;
  policyConfig: unknown;
  priority: number;
  enabled: boolean;
  isDefault: boolean;
  createdBy: string;
  createdAt: Date;
  updatedAt: Date;
};

type ComplianceFlatRow = {
  agentId: string;
  hostname: string;
  os: string;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  collectedAt: Date | null;
  rebootRequired: boolean | null;
  title: string | null;
  titleNorm: string | null;
  description: string | null;
  kbArticle: string | null;
  isMandatory: boolean | null;
  sizeBytes: bigint | null;
  requiresReboot: boolean | null;
};

type PatchActionRow = {
  agentId: string;
  actionType: string;
  status: string;
  remediationJobId: bigint | null;
  createdAt: Date;
};

type ApprovalRow = {
  agentId: string;
  updateKey: string;
  decision: PatchDecision;
  deferUntil: Date | null;
};

type PatchOverviewDeviceRow = {
  agentId: string;
  hostname: string;
  os: string;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  lastSeen: Date;
  collectedAt: Date | null;
  rebootRequired: boolean | null;
  deviceType: PatchDeviceType;
  deviceTypeSource: string;
  patchRing: PatchRing;
  criticalityTier: string | null;
  patchManaged: boolean;
  nativeWindowsUpdateControl: boolean;
  patchMaintenanceModeUntil: Date | null;
  patchTags: unknown;
  pendingUpdates: number;
  failedUpdates: number;
  blockedUpdates: number;
  deferredUpdates: number;
};

type PatchUpdateStateRow = {
  agentId: string;
  hostname: string;
  updateKey: string;
  title: string;
  kbArticle: string | null;
  category: PatchCategory;
  lifecycleState: string;
  approvalState: string;
  firstDetectedAt: Date;
  lastDetectedAt: Date;
  releaseDate: Date | null;
  eligibleAt: Date | null;
  installDeadlineAt: Date | null;
  rebootDeadlineAt: Date | null;
  failureHresult: number | null;
  failureMessage: string | null;
};

type PatchOverrideRow = {
  id: string;
  organizationId: string;
  scopeType: PatchDecisionOverride['scopeType'];
  scopeKey: string;
  action: PatchDecisionOverride['action'];
  updateKey: string | null;
  kbArticle: string | null;
  category: string | null;
  reason: string | null;
  deferUntil: Date | null;
  expiresAt: Date | null;
  enabled: boolean;
  createdBy: string;
  createdAt: Date;
  updatedAt: Date;
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
  details: unknown;
  decidedAt: Date;
};

type PatchProgressQueryResponse = {
  items?: unknown[];
};

type PatchActionProgressRow = {
  organizationId: string;
  agentId: string;
  operationId: string;
  actionType: string;
  status: string;
  phase: string | null;
  progress: unknown;
  evidence: unknown;
  errorMessage: string | null;
  updateKeys: unknown;
  createdAt: Date;
  updatedAt: Date;
  startedAt: Date | null;
  finishedAt: Date | null;
};

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

async function requireMembership(req: AuthedRequest, res: any): Promise<Membership | null> {
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

function readString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value !== 'string') continue;
    const trimmed = value.trim();
    if (trimmed) return trimmed;
  }
  return null;
}

function readBoolean(value: unknown): boolean | null {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') {
    const normalized = value.trim().toLowerCase();
    if (normalized === 'true' || normalized === '1') return true;
    if (normalized === 'false' || normalized === '0') return false;
  }
  return null;
}

function parseInteger(value: unknown): number | null {
  const parsed = Number(value);
  return Number.isInteger(parsed) ? parsed : null;
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function parseScopeType(value: unknown): PatchPolicyScopeType | null {
  const scope = readString(value);
  if (scope === 'organization' || scope === 'customer' || scope === 'site' || scope === 'device') {
    return scope;
  }
  return null;
}

function parseApprovalMode(value: unknown): PatchApprovalMode | null {
  const mode = readString(value);
  if (mode === 'manual' || mode === 'auto_approve_security' || mode === 'auto_approve_all') {
    return mode;
  }
  return null;
}

function parseRebootBehavior(value: unknown): PatchRebootBehavior | null {
  const behavior = readString(value);
  if (behavior === 'suppress' || behavior === 'allow' || behavior === 'force') {
    return behavior;
  }
  return null;
}

function parseDecision(value: unknown): PatchDecision | null {
  const decision = readString(value);
  if (decision === 'approved' || decision === 'denied' || decision === 'deferred') {
    return decision;
  }
  return null;
}

function parseWindowTime(value: unknown): string | null | undefined {
  if (value === undefined) return undefined;
  const text = readString(value);
  if (!text) return null;
  if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(text)) {
    throw new Error('maintenance window times must be HH:mm');
  }
  return text;
}

function parseTargetOsFamily(value: unknown): PatchPolicyTargetOsFamily | null {
  const text = readString(value)?.toLowerCase();
  if (!text) return null;
  if (text === 'all' || text === 'windows' || text === 'linux' || text === 'macos') return text;
  throw Object.assign(new Error('targetOsFamily must be all, windows, linux, or macos'), { status: 400 });
}

function normalizePolicy(row: PatchPolicyRow) {
  return {
    id: row.id,
    organizationId: row.organizationId,
    scopeType: row.scopeType,
    scopeKey: row.scopeKey,
    customerId: row.customerId,
    siteId: row.siteId,
    agentId: row.agentId,
    name: row.name,
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
    createdBy: row.createdBy,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
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

async function loadPolicies(organizationId: string): Promise<PatchPolicyRow[]> {
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

async function resolvePolicyScope(
  membership: Membership,
  body: Record<string, unknown>,
  existing?: Pick<PatchPolicyRow, 'scopeType' | 'scopeKey' | 'customerId' | 'siteId' | 'agentId' | 'isDefault'> | null
) {
  if (existing?.isDefault) {
    throw Object.assign(new Error('Default patch policy scope cannot be changed'), { status: 400 });
  }

  const scopeType = parseScopeType(body.scopeType ?? existing?.scopeType);
  if (!scopeType) {
    throw Object.assign(new Error('scopeType must be organization, customer, site, or device'), { status: 400 });
  }

  if (scopeType === 'organization') {
    return { scopeType, scopeKey: membership.organizationId, customerId: null, siteId: null, agentId: null };
  }

  if (scopeType === 'customer') {
    const customerId = readString(body.customerId, body.scopeId, existing?.customerId, existing?.scopeKey);
    if (!customerId) throw Object.assign(new Error('customerId is required for customer scope'), { status: 400 });
    const customer = await prisma.customer.findFirst({
      where: { id: customerId, organizationId: membership.organizationId },
      select: { id: true }
    });
    if (!customer) throw Object.assign(new Error('Customer not found'), { status: 404 });
    return { scopeType, scopeKey: customer.id, customerId: customer.id, siteId: null, agentId: null };
  }

  if (scopeType === 'site') {
    const siteId = readString(body.siteId, body.scopeId, existing?.siteId, existing?.scopeKey);
    if (!siteId) throw Object.assign(new Error('siteId is required for site scope'), { status: 400 });
    const site = await prisma.rmmSite.findFirst({
      where: { id: siteId, customer: { organizationId: membership.organizationId } },
      select: { id: true, customerId: true }
    });
    if (!site) throw Object.assign(new Error('Site not found'), { status: 404 });
    return { scopeType, scopeKey: site.id, customerId: site.customerId, siteId: site.id, agentId: null };
  }

  const agentId = readString(body.agentId, body.scopeId, existing?.agentId, existing?.scopeKey);
  if (!agentId) throw Object.assign(new Error('agentId is required for device scope'), { status: 400 });
  const device = await prisma.rmmDevice.findFirst({
    where: { agentId, organizationId: membership.organizationId },
    select: { agentId: true, customerId: true, siteId: true }
  });
  if (!device) throw Object.assign(new Error('Device not found'), { status: 404 });
  return {
    scopeType,
    scopeKey: device.agentId,
    customerId: device.customerId,
    siteId: device.siteId,
    agentId: device.agentId
  };
}

function parsePolicyFields(body: Record<string, unknown>, existing?: PatchPolicyRow | null) {
  const isDefault = existing?.isDefault === true;
  const approvalMode = parseApprovalMode(body.approvalMode ?? existing?.approvalMode) ?? 'manual';
  const rebootBehavior = parseRebootBehavior(body.rebootBehavior ?? existing?.rebootBehavior) ?? 'allow';
  const targetOsFamily = isDefault
    ? 'all'
    : parseTargetOsFamily(body.targetOsFamily ?? existing?.targetOsFamily) ?? 'all';
  const deferralDays = parseInteger(body.deferralDays ?? existing?.deferralDays ?? 0) ?? 0;
  if (deferralDays < 0 || deferralDays > 365) {
    throw Object.assign(new Error('deferralDays must be between 0 and 365'), { status: 400 });
  }
  const maintenanceWindowStart = parseWindowTime(body.maintenanceWindowStart);
  const maintenanceWindowEnd = parseWindowTime(body.maintenanceWindowEnd);
  const requestedManagedMode = readBoolean(body.managedMode) ?? existing?.managedMode ?? true;
  const managedMode = targetOsFamily === 'macos' ? false : requestedManagedMode;
  const requestedNativeWindowsUpdateControl =
    readBoolean(body.nativeWindowsUpdateControl) ?? existing?.nativeWindowsUpdateControl ?? true;
  const nativeWindowsUpdateControl =
    targetOsFamily === 'all' || targetOsFamily === 'windows'
      ? requestedNativeWindowsUpdateControl && managedMode
      : false;
  const priority = isDefault
    ? DEFAULT_PATCH_POLICY_PRIORITY
    : parseInteger(body.priority ?? existing?.priority ?? CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY)
      ?? CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY;
  if (!isDefault && (priority < 0 || priority > DEFAULT_PATCH_POLICY_PRIORITY - 1)) {
    throw Object.assign(new Error('priority must be between 0 and 9999'), { status: 400 });
  }
  const policyConfigInput = body.policyConfig === undefined ? existing?.policyConfig : body.policyConfig;
  const policyConfig = {
    ...defaultPatchPolicyConfig(deferralDays),
    ...asRecord(policyConfigInput),
    managedMode,
    nativeWindowsUpdateControl
  };
  return {
    name: readString(body.name, existing?.name) ?? 'Patch policy',
    targetOsFamily,
    approvalMode,
    maintenanceWindowStart: maintenanceWindowStart === undefined ? existing?.maintenanceWindowStart ?? null : maintenanceWindowStart,
    maintenanceWindowEnd: maintenanceWindowEnd === undefined ? existing?.maintenanceWindowEnd ?? null : maintenanceWindowEnd,
    maintenanceWindowTimezone:
      body.maintenanceWindowTimezone === undefined
        ? existing?.maintenanceWindowTimezone ?? 'UTC'
        : readString(body.maintenanceWindowTimezone),
    rebootBehavior,
    deferralDays,
    managedMode,
    nativeWindowsUpdateControl,
    policyConfig,
    priority,
    enabled: isDefault ? true : readBoolean(body.enabled) ?? existing?.enabled ?? true
  };
}

async function loadPatchCompliance(organizationId: string, filters: Record<string, unknown> = {}) {
  const where = [Prisma.sql`d.organization_id = ${organizationId}`];
  const customerId = readString(filters.customerId);
  const siteId = readString(filters.siteId);
  if (customerId && customerId !== 'all') where.push(Prisma.sql`d.customer_id = ${customerId}`);
  if (siteId && siteId !== 'all') where.push(Prisma.sql`d.site_id = ${siteId}`);

  const rows = await prisma.$queryRaw<ComplianceFlatRow[]>(Prisma.sql`
    SELECT
      d.agent_id AS "agentId",
      d.hostname,
      d.os,
      d.customer_id AS "customerId",
      c.name AS "customerName",
      d.site_id AS "siteId",
      s.name AS "siteName",
      ds.collected_at AS "collectedAt",
      ds.reboot_required AS "rebootRequired",
      pu.title,
      pu.title_norm AS "titleNorm",
      pu.description,
      pu.kb_article AS "kbArticle",
      pu.is_mandatory AS "isMandatory",
      pu.size_bytes AS "sizeBytes",
      pu.requires_reboot AS "requiresReboot"
    FROM public.rmm_devices d
    LEFT JOIN public.customers c ON c.id = d.customer_id
    LEFT JOIN public.rmm_sites s ON s.id = d.site_id
    LEFT JOIN rmm_telemetry.device_state ds ON ds.agent_id = d.agent_id
    LEFT JOIN rmm_telemetry.device_pending_update pu ON pu.agent_id = d.agent_id
    WHERE ${Prisma.join(where, ' AND ')}
    ORDER BY d.hostname ASC, pu.title ASC
  `);

  const devicesById = new Map<string, PatchDeviceComplianceInput>();
  for (const row of rows) {
    let device = devicesById.get(row.agentId);
    if (!device) {
      device = {
        agentId: row.agentId,
        hostname: row.hostname,
        os: row.os,
        customerId: row.customerId,
        customerName: row.customerName,
        siteId: row.siteId,
        siteName: row.siteName,
        lastScanAt: row.collectedAt,
        rebootRequired: row.rebootRequired,
        installStatus: null,
        installStatusAt: null,
        pendingUpdates: []
      };
      devicesById.set(row.agentId, device);
    }
    if (row.title) {
      device.pendingUpdates.push({
        title: row.title,
        titleNorm: row.titleNorm,
        description: row.description,
        kbArticle: row.kbArticle,
        isMandatory: row.isMandatory,
        sizeBytes: row.sizeBytes === null ? null : Number(row.sizeBytes),
        requiresReboot: row.requiresReboot
      });
    }
  }

  const agentIds = [...devicesById.keys()];
  if (agentIds.length > 0) {
    const actions = await prisma.$queryRaw<PatchActionRow[]>(Prisma.sql`
      SELECT DISTINCT ON (a.agent_id)
        a.agent_id AS "agentId",
        a.action_type AS "actionType",
        COALESCE(j.status, a.status) AS status,
        a.remediation_job_id AS "remediationJobId",
        a.remediation_command_id AS "remediationCommandId",
        a.created_at AS "createdAt"
      FROM public.rmm_patch_action a
      LEFT JOIN rmm_telemetry.remediation_job j
        ON j.id = a.remediation_job_id
        OR (a.remediation_command_id IS NOT NULL AND j.command_id = a.remediation_command_id)
      WHERE a.organization_id = ${organizationId}
        AND a.action_type = 'install'
        AND a.agent_id IN (${Prisma.join(agentIds)})
      ORDER BY a.agent_id, a.created_at DESC
    `);
    for (const action of actions) {
      const device = devicesById.get(action.agentId);
      if (device) {
        device.installStatus = action.status;
        device.installStatusAt = action.createdAt;
      }
    }
  }

  const approvals =
    agentIds.length === 0
      ? []
      : await prisma.$queryRaw<ApprovalRow[]>(Prisma.sql`
          SELECT
            agent_id AS "agentId",
            update_key AS "updateKey",
            decision,
            defer_until AS "deferUntil"
          FROM public.rmm_patch_approval
          WHERE organization_id = ${organizationId}
            AND agent_id IN (${Prisma.join(agentIds)})
        `);

  const policies = (await loadPolicies(organizationId)).map(policyForResolution);
  const summary = calculatePatchComplianceSummary(
    [...devicesById.values()],
    policies,
    approvals,
    organizationId
  );

  const complianceStatus = readString(filters.complianceStatus, filters.status);
  if (complianceStatus && complianceStatus !== 'all') {
    const items = summary.items.filter((item) => item.complianceStatus === complianceStatus);
    const recalculated = calculatePatchComplianceSummary(
      items.map((item) => ({
        agentId: item.agentId,
        hostname: item.hostname,
        os: item.os,
        customerId: item.customerId,
        customerName: item.customerName,
        siteId: item.siteId,
        siteName: item.siteName,
        lastScanAt: item.lastScanAt,
        rebootRequired: item.rebootRequired,
        installStatus: item.installStatus,
        installStatusAt: null,
        pendingUpdates: item.updates
      })),
      policies,
      approvals,
      organizationId
    );
    return recalculated;
  }

  return summary;
}

async function resolveTargetAgentIds(organizationId: string, body: Record<string, unknown>): Promise<string[]> {
  const rawAgentIds = Array.isArray(body.agentIds) ? body.agentIds : [];
  const agentIds = rawAgentIds
    .map((value) => (typeof value === 'string' ? value.trim() : ''))
    .filter(Boolean);
  if (agentIds.length > 0) {
    const devices = await prisma.rmmDevice.findMany({
      where: { organizationId, agentId: { in: agentIds } },
      select: { agentId: true }
    });
    return devices.map((device) => device.agentId);
  }

  const updateKeys = readPatchUpdateKeyList(body);
  const hasExplicitFilters = body.filters !== undefined && body.filters !== null;
  if (updateKeys.length > 0 && !hasExplicitFilters) {
    const rows = await prisma.$queryRaw<
      Array<{
        agentId: string;
        updateKey: string;
        applicabilityState: string | null;
        lifecycleState: string | null;
      }>
    >(Prisma.sql`
      SELECT
        agent_id AS "agentId",
        update_key AS "updateKey",
        applicability_state AS "applicabilityState",
        lifecycle_state AS "lifecycleState"
      FROM public.rmm_patch_device_update_state
      WHERE organization_id = ${organizationId}
        AND update_key IN (${Prisma.join(updateKeys)})
      ORDER BY agent_id ASC, update_key ASC
    `);
    return selectActionablePatchUpdateTargetAgentIds(rows, updateKeys);
  }

  const filters = asRecord(body.filters);
  const summary = await loadPatchCompliance(organizationId, filters);
  return summary.items.map((item) => item.agentId);
}

function readUpdateKeys(body: Record<string, unknown>): Set<string> | null {
  if (!Array.isArray(body.updateKeys)) return null;
  const values = body.updateKeys
    .map((value) => (typeof value === 'string' ? value.trim() : ''))
    .filter(Boolean);
  return values.length > 0 ? new Set(values) : null;
}

function isApprovalDeferred(decision: PatchDecision | null, deferUntil: string | null, now: Date) {
  if (decision !== 'deferred') return false;
  if (!deferUntil) return true;
  const parsed = Date.parse(deferUntil);
  return Number.isNaN(parsed) || parsed > now.getTime();
}

async function insertPatchAction(options: {
  organizationId: string;
  agentId: string;
  operationId?: string;
  actionType: string;
  status: string;
  updateKeys: string[];
  phase?: string | null;
  progress?: unknown;
  evidence?: unknown;
  errorMessage?: string | null;
  remediationJobId?: bigint | null;
  remediationCommandId?: string | null;
  requestedBy: string;
}) {
  const operationId = options.operationId ?? randomUUID();
  await prisma.$executeRaw(Prisma.sql`
    INSERT INTO public.rmm_patch_action
      (
        id, organization_id, agent_id, operation_id, action_type, status, phase,
        update_keys_jsonb, progress_jsonb, evidence_jsonb, error_message,
        remediation_job_id, remediation_command_id, requested_by,
        started_at, finished_at, created_at, updated_at
      )
    VALUES
      (
        ${randomUUID()},
        ${options.organizationId},
        ${options.agentId},
        ${operationId},
        ${options.actionType},
        ${options.status},
        ${options.phase ?? null},
        ${JSON.stringify(options.updateKeys)}::jsonb,
        ${JSON.stringify(options.progress ?? {})}::jsonb,
        ${JSON.stringify(options.evidence ?? {})}::jsonb,
        ${options.errorMessage ?? null},
        ${options.remediationJobId ?? null},
        ${options.remediationCommandId ?? null},
        ${options.requestedBy},
        ${options.status === 'running' ? new Date() : null},
        ${['completed', 'failed', 'cancelled'].includes(options.status) ? new Date() : null},
        NOW(),
        NOW()
      )
    ON CONFLICT (organization_id, agent_id, operation_id)
    DO UPDATE SET
      action_type = EXCLUDED.action_type,
      status = EXCLUDED.status,
      phase = EXCLUDED.phase,
      update_keys_jsonb = EXCLUDED.update_keys_jsonb,
      progress_jsonb = EXCLUDED.progress_jsonb,
      evidence_jsonb = EXCLUDED.evidence_jsonb,
      error_message = EXCLUDED.error_message,
      remediation_job_id = COALESCE(EXCLUDED.remediation_job_id, public.rmm_patch_action.remediation_job_id),
      remediation_command_id = COALESCE(EXCLUDED.remediation_command_id, public.rmm_patch_action.remediation_command_id),
      started_at = COALESCE(public.rmm_patch_action.started_at, EXCLUDED.started_at),
      finished_at = EXCLUDED.finished_at,
      updated_at = NOW()
  `);
  return operationId;
}

async function publishRemediationCommands(commands: unknown[]) {
  if (commands.length === 0) return;
  const baseUrl = env.telemetryProducerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey) {
    throw Object.assign(new Error('RMM_TELEMETRY_PRODUCER_URL and RMM_SERVER_API_KEY are required to queue remediation commands'), {
      status: 500
    });
  }

  const response = await fetch(`${baseUrl}/telemetry/remediation/commands`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-rmm-server-key': serverKey
    },
    body: JSON.stringify({ commands })
  });
  if (!response.ok) {
    const body = await response.text().catch(() => '');
    throw Object.assign(new Error(`Telemetry producer rejected remediation command batch: ${response.status} ${body}`), {
      status: 502
    });
  }
}

async function insertCommandAudit(options: {
  organizationId: string;
  customerId: string | null;
  userId: string;
  agentId: string;
  command: string;
  wasAllowed: boolean;
  denialReason?: string | null;
}) {
  await prisma.commandExecutionLog.create({
    data: {
      organizationId: options.organizationId,
      customerId: options.customerId,
      userId: options.userId,
      agentId: options.agentId,
      command: options.command,
      wasAllowed: options.wasAllowed,
      denialReason: options.denialReason ?? null,
      matchedPolicyId: null
    }
  });
}

async function notifyPatchJobsAvailable(agentIds: string[], reason: string) {
  const baseUrl = env.rmmServerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey || agentIds.length === 0) return;

  try {
    const response = await fetch(`${baseUrl}/api/rmm/internal/patch-jobs/notify`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-rmm-server-key': serverKey
      },
      body: JSON.stringify({ agentIds, reason })
    });
    if (!response.ok) {
      log.warn('patch job wake notification failed', {
        status: response.status,
        agentCount: agentIds.length
      });
    }
  } catch (error) {
    log.warn('patch job wake notification errored', {
      error: String(error),
      agentCount: agentIds.length
    });
  }
}

async function queryTelemetryPatchProgress(organizationId: string, agentIds: string[]) {
  if (agentIds.length === 0) return [];
  const rows = await prisma.$queryRaw<PatchActionProgressRow[]>(Prisma.sql`
    SELECT DISTINCT ON (agent_id, operation_id)
      agent_id AS "agentId",
      organization_id AS "organizationId",
      operation_id AS "operationId",
      action_type AS "actionType",
      status,
      phase,
      progress_jsonb AS progress,
      evidence_jsonb AS evidence,
      error_message AS "errorMessage",
      update_keys_jsonb AS "updateKeys",
      created_at AS "createdAt",
      updated_at AS "updatedAt",
      started_at AS "startedAt",
      finished_at AS "finishedAt"
    FROM public.rmm_patch_action
    WHERE organization_id = ${organizationId}
      AND agent_id IN (${Prisma.join(agentIds)})
      AND action_type IN ('scan', 'download', 'install', 'reboot')
      AND (
        status IN ('queued', 'running')
        OR (status IN ('completed', 'failed', 'cancelled') AND updated_at >= NOW() - INTERVAL '30 seconds')
      )
    ORDER BY agent_id, operation_id, updated_at DESC
  `);
  return rows.map(patchProgressPayloadFromAction);
}

function patchProgressPayloadFromAction(row: PatchActionProgressRow) {
  const progress = asRecord(row.progress);
  const summary = progress.summary && typeof progress.summary === 'object' && !Array.isArray(progress.summary)
    ? progress.summary as Record<string, unknown>
    : null;
  const actionType = row.actionType === 'download' ? 'download' : row.actionType === 'install' ? 'install' : row.actionType === 'reboot' ? 'install' : 'scan';
  const eventType = actionType === 'scan' ? 'patch.scan.progress' : 'patch.install.progress';
  const defaultPhase =
    actionType === 'scan'
      ? 'scanning'
      : actionType === 'download'
        ? 'downloading'
        : row.status === 'queued'
          ? 'searching'
          : 'installing';
  const updateKeys = Array.isArray(row.updateKeys)
    ? row.updateKeys.filter((value): value is string => typeof value === 'string')
    : [];
  return {
    schemaVersion: Number(progress.schemaVersion ?? 1),
    eventType,
    organizationId: row.organizationId,
    agentId: row.agentId,
    jobId: row.operationId,
    commandId: row.operationId,
    status: row.status,
    phase: typeof progress.phase === 'string' ? progress.phase : row.phase ?? defaultPhase,
    reportedAt: typeof progress.reportedAt === 'string' ? progress.reportedAt : row.updatedAt.toISOString(),
    receivedAt: row.updatedAt.toISOString(),
    overallPercent: typeof progress.overallPercent === 'number' ? progress.overallPercent : row.status === 'queued' || row.status === 'running' ? 0 : 100,
    phasePercent: typeof progress.phasePercent === 'number' ? progress.phasePercent : row.status === 'queued' || row.status === 'running' ? 0 : 100,
    currentUpdateIndex: progress.currentUpdateIndex ?? null,
    currentUpdatePercent: progress.currentUpdatePercent ?? null,
    currentUpdate: progress.currentUpdate ?? null,
    updates: Array.isArray(progress.updates) ? progress.updates : [],
    summary: summary ?? {
      matched: updateKeys.length,
      downloaded: 0,
      installed: 0,
      failed: row.status === 'failed' ? 1 : 0,
      skipped: 0,
      rebootRequired: false,
      pendingUpdates: null,
      snapshotRequested: actionType === 'scan'
    },
    error: typeof progress.error === 'string' ? progress.error : row.errorMessage
  };
}

const manualActionToOverride: Record<string, PatchOverrideAction> = {
  scan_now: 'force_scan',
  download_now: 'force_download',
  install_now: 'force_install',
  install_approved: 'force_install',
  install_kb: 'force_install',
  reboot_now: 'force_reboot',
  defer_update: 'defer',
  defer_reboot: 'defer_reboot',
  block_update: 'block',
  approve_update: 'approve',
  emergency_approve: 'emergency_approve',
  // Retained for future maintenance-mode UI/API reactivation.
  maintenance_mode: 'maintenance_mode',
  cancel: 'cancel'
};

function parseDateInput(value: unknown): Date | null {
  const text = readString(value);
  if (!text) return null;
  const parsed = new Date(text);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function parseOverrideScope(
  body: Record<string, unknown>,
  membership: Membership
): { scopeType: PatchDecisionOverride['scopeType']; scopeKey: string } | null {
  const scopeType = readString(body.scopeType);
  if (!scopeType) return null;
  if (
    scopeType !== 'global' &&
    scopeType !== 'organization' &&
    scopeType !== 'customer' &&
    scopeType !== 'site' &&
    scopeType !== 'group' &&
    scopeType !== 'tag' &&
    scopeType !== 'ring' &&
    scopeType !== 'device'
  ) {
    throw Object.assign(new Error('scopeType is not valid for patch override'), { status: 400 });
  }
  if (scopeType === 'global') return { scopeType, scopeKey: 'global' };
  if (scopeType === 'organization') return { scopeType, scopeKey: membership.organizationId };
  const scopeKey = readString(body.scopeKey, body.scopeId, body.customerId, body.siteId, body.agentId, body.patchRing, body.tag, body.groupId);
  if (!scopeKey) {
    throw Object.assign(new Error('scopeKey is required for scoped patch overrides'), { status: 400 });
  }
  return { scopeType, scopeKey };
}

function readPatchUpdateKeyList(body: Record<string, unknown>): string[] {
  return Array.isArray(body.updateKeys)
    ? body.updateKeys.map((value) => (typeof value === 'string' ? value.trim() : '')).filter(Boolean)
    : [];
}

function durablePatchActionType(action: PatchOverrideAction): 'scan' | 'download' | 'install' | 'reboot' | 'approval' {
  if (action === 'force_scan') return 'scan';
  if (action === 'force_download') return 'download';
  if (action === 'force_install' || action === 'emergency_approve') return 'install';
  if (action === 'force_reboot') return 'reboot';
  return 'approval';
}

patchesRouter.get('/policies', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const policies = await loadPolicies(membership.organizationId);
    return res.json({ items: policies.map(normalizePolicy) });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/policies', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can create patch policies' });

    const body = asRecord(req.body);
    const scope = await resolvePolicyScope(membership, body);
    const fields = parsePolicyFields(body);
    const id = randomUUID();
    const rows = await prisma.$queryRaw<PatchPolicyRow[]>(Prisma.sql`
      INSERT INTO public.rmm_patch_policy
        (
          id, organization_id, scope_type, scope_key, customer_id, site_id, agent_id,
          name, target_os_family, approval_mode, maintenance_window_start, maintenance_window_end,
          maintenance_window_timezone, reboot_behavior, deferral_days,
          managed_mode, native_windows_update_control, policy_config_jsonb, priority, enabled,
          created_by, created_at, updated_at
        )
      VALUES
        (
          ${id}, ${membership.organizationId}, ${scope.scopeType}, ${scope.scopeKey}, ${scope.customerId}, ${scope.siteId}, ${scope.agentId},
          ${fields.name}, ${fields.targetOsFamily}, ${fields.approvalMode}, ${fields.maintenanceWindowStart}, ${fields.maintenanceWindowEnd},
          ${fields.maintenanceWindowTimezone}, ${fields.rebootBehavior}, ${fields.deferralDays},
          ${fields.managedMode}, ${fields.nativeWindowsUpdateControl}, ${JSON.stringify(fields.policyConfig)}::jsonb, ${fields.priority}, ${fields.enabled},
          ${req.jwt!.sub}, NOW(), NOW()
        )
      RETURNING
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
    `);
    return res.status(201).json(normalizePolicy(rows[0]));
  } catch (error) {
    return next(error);
  }
});

patchesRouter.patch('/policies/:id', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can edit patch policies' });

    const existing = (await prisma.$queryRaw<PatchPolicyRow[]>(Prisma.sql`
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
      WHERE id = ${req.params.id}
        AND organization_id = ${membership.organizationId}
      LIMIT 1
    `))[0];
    if (!existing) return res.status(404).json({ error: 'Patch policy not found' });

    const body = asRecord(req.body);
    const hasScopeUpdate = Boolean(body.scopeType || body.customerId || body.siteId || body.agentId || body.scopeId);
    const scope = existing.isDefault
      ? {
          scopeType: 'organization' as PatchPolicyScopeType,
          scopeKey: DEFAULT_PATCH_POLICY_SCOPE_KEY,
          customerId: null,
          siteId: null,
          agentId: null
        }
      : hasScopeUpdate
      ? await resolvePolicyScope(membership, body, existing)
      : {
          scopeType: existing.scopeType,
          scopeKey: existing.scopeKey,
          customerId: existing.customerId,
          siteId: existing.siteId,
          agentId: existing.agentId
        };
    const fields = parsePolicyFields(body, existing);
    const rows = await prisma.$queryRaw<PatchPolicyRow[]>(Prisma.sql`
      UPDATE public.rmm_patch_policy
      SET
        scope_type = ${scope.scopeType},
        scope_key = ${scope.scopeKey},
        customer_id = ${scope.customerId},
        site_id = ${scope.siteId},
        agent_id = ${scope.agentId},
        name = ${fields.name},
        target_os_family = ${fields.targetOsFamily},
        approval_mode = ${fields.approvalMode},
        maintenance_window_start = ${fields.maintenanceWindowStart},
        maintenance_window_end = ${fields.maintenanceWindowEnd},
        maintenance_window_timezone = ${fields.maintenanceWindowTimezone},
        reboot_behavior = ${fields.rebootBehavior},
        deferral_days = ${fields.deferralDays},
        managed_mode = ${fields.managedMode},
        native_windows_update_control = ${fields.nativeWindowsUpdateControl},
        policy_config_jsonb = ${JSON.stringify(fields.policyConfig)}::jsonb,
        priority = ${fields.priority},
        enabled = ${fields.enabled},
        updated_at = NOW()
      WHERE id = ${existing.id}
      RETURNING
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
    `);
    return res.json(normalizePolicy(rows[0]));
  } catch (error) {
    return next(error);
  }
});

patchesRouter.delete('/policies/:id', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can delete patch policies' });
    const existing = (await prisma.$queryRaw<Array<{ isDefault: boolean }>>(Prisma.sql`
      SELECT is_default AS "isDefault"
      FROM public.rmm_patch_policy
      WHERE id = ${req.params.id}
        AND organization_id = ${membership.organizationId}
      LIMIT 1
    `))[0];
    if (!existing) return res.status(404).json({ error: 'Patch policy not found' });
    if (existing.isDefault) return res.status(400).json({ error: 'Default patch policy cannot be deleted' });
    await prisma.$executeRaw(Prisma.sql`
      DELETE FROM public.rmm_patch_policy
      WHERE id = ${req.params.id}
        AND organization_id = ${membership.organizationId}
    `);
    return res.status(204).end();
  } catch (error) {
    return next(error);
  }
});

patchesRouter.get('/compliance', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const summary = await loadPatchCompliance(membership.organizationId, req.query as Record<string, unknown>);
    return res.json({
      generatedAt: new Date().toISOString(),
      ...summary
    });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.get('/overview', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const overview = await loadPatchOverview(membership.organizationId);
    return res.json(overview);
  } catch (error) {
    return next(error);
  }
});

patchesRouter.get('/devices/:agentId/state', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const state = await loadPatchDeviceState(membership.organizationId, req.params.agentId);
    return res.json(state);
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/devices/:agentId/plan', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const plan = await evaluateAndPersistPatchPlan({
      agentId: req.params.agentId,
      organizationId: membership.organizationId,
      observedState: asRecord(req.body?.state),
      persist: true
    });
    return res.json({ plan });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/actions', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can perform patch actions' });

    const body = asRecord(req.body);
    const actionName = readString(body.action);
    const overrideAction = actionName ? manualActionToOverride[actionName] : null;
    if (!overrideAction) {
      return res.status(400).json({ error: 'Unsupported patch action' });
    }

    const reason = readString(body.reason) ?? `Manual ${actionName} requested`;
    const deferUntil = parseDateInput(body.deferUntil);
    const expiresAt = parseDateInput(body.expiresAt);
    const category = readString(body.category);
    const kbArticle = readString(body.kbArticle);
    const updateKeys = readPatchUpdateKeyList(body);
    const explicitScope = parseOverrideScope(body, membership);
    const agentIds = explicitScope
      ? []
      : await resolveTargetAgentIds(membership.organizationId, body);
    if (!explicitScope && agentIds.length === 0) {
      return res.status(400).json({ error: 'No target devices found' });
    }

    if ((overrideAction === 'defer' || overrideAction === 'defer_reboot') && !deferUntil) {
      return res.status(400).json({ error: 'deferUntil is required for deferral actions' });
    }
    if (overrideAction === 'maintenance_mode' && !deferUntil) {
      return res.status(400).json({ error: 'deferUntil is required as the maintenance mode end time' });
    }

    if (overrideAction === 'cancel') {
      const targetScope = explicitScope;
      if (targetScope) {
        await prisma.$executeRaw(Prisma.sql`
          UPDATE public.rmm_patch_override
          SET enabled = false, updated_at = NOW()
          WHERE organization_id = ${membership.organizationId}
            AND scope_type = ${targetScope.scopeType}
            AND scope_key = ${targetScope.scopeKey}
            AND enabled = true
        `);
      } else {
        await prisma.$executeRaw(Prisma.sql`
          UPDATE public.rmm_patch_override
          SET enabled = false, updated_at = NOW()
          WHERE organization_id = ${membership.organizationId}
            AND scope_type = 'device'
            AND scope_key IN (${Prisma.join(agentIds)})
            AND enabled = true
        `);
      }
      await notifyPatchJobsAvailable(agentIds, 'patch_action_cancelled');
      return res.json({ action: actionName, overrideAction, targetedDevices: agentIds.length, overridesCreated: 0, cancelled: true });
    }

    if (!explicitScope && overrideAction === 'maintenance_mode') {
      await updateDevicePatchControl({
        organizationId: membership.organizationId,
        agentIds,
        maintenanceModeUntil: deferUntil
      });
    }

    const created: string[] = [];
    const updateKeyTargets = updateKeys.length > 0 ? updateKeys : [null];
    const scopes = explicitScope
      ? [explicitScope]
      : agentIds.map((agentId) => ({ scopeType: 'device' as const, scopeKey: agentId }));
    const operationIdsByScopeKey = new Map(
      scopes.map((scope) => [scope.scopeKey, explicitScope ? null : randomUUID()])
    );
    for (const scope of scopes) {
      for (const updateKey of updateKeyTargets) {
        const id = await createPatchOverride({
          organizationId: membership.organizationId,
          scopeType: scope.scopeType,
          scopeKey: scope.scopeKey,
          action: overrideAction,
          operationId: operationIdsByScopeKey.get(scope.scopeKey) ?? null,
          updateKey,
          kbArticle,
          category,
          reason,
          deferUntil,
          expiresAt,
          createdBy: req.jwt!.sub
        });
        created.push(id);
      }
    }

    for (const agentId of agentIds) {
      const actionType = durablePatchActionType(overrideAction);
      await insertPatchAction({
        organizationId: membership.organizationId,
        agentId,
        operationId: operationIdsByScopeKey.get(agentId) ?? undefined,
        actionType,
        status: 'queued',
        updateKeys,
        phase: actionType === 'scan'
          ? 'scanning'
          : actionType === 'download'
            ? 'downloading'
            : actionType === 'install'
              ? 'searching'
              : null,
        requestedBy: req.jwt!.sub
      });
    }
    await notifyPatchJobsAvailable(agentIds, `patch_action_${overrideAction}`);

    return res.status(202).json({
      action: actionName,
      overrideAction,
      targetedDevices: agentIds.length,
      overridesCreated: created.length,
      overrideIds: created
    });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/progress/query', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    const body = asRecord(req.body);
    const requested = Array.isArray(body.agentIds)
      ? body.agentIds.map((value) => (typeof value === 'string' ? value.trim() : '')).filter(Boolean)
      : [];
    if (requested.length === 0) return res.json({ items: [] });

    const devices = await prisma.rmmDevice.findMany({
      where: {
        organizationId: membership.organizationId,
        agentId: { in: requested }
      },
      select: { agentId: true }
    });
    const allowedAgentIds = devices.map((device) => device.agentId);
    if (allowedAgentIds.length === 0) return res.json({ items: [] });

    const items = await queryTelemetryPatchProgress(membership.organizationId, allowedAgentIds);
    return res.json({ items });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/approvals', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can approve patch actions' });

    const body = asRecord(req.body);
    const decision = parseDecision(body.decision);
    if (!decision) return res.status(400).json({ error: 'decision must be approved, denied, or deferred' });

    const deferUntilRaw = readString(body.deferUntil);
    const deferUntil = deferUntilRaw ? new Date(deferUntilRaw) : null;
    if (decision === 'deferred' && (!deferUntil || Number.isNaN(deferUntil.getTime()))) {
      return res.status(400).json({ error: 'deferUntil is required for deferred approvals' });
    }
    const reason = readString(body.reason);
    const updateKeys = readUpdateKeys(body);
    const agentIds = await resolveTargetAgentIds(membership.organizationId, body);
    if (agentIds.length === 0) return res.status(400).json({ error: 'No target devices found' });

    const summary = await loadPatchCompliance(membership.organizationId, {});
    const targetItems = summary.items.filter((item) => agentIds.includes(item.agentId));
    let updated = 0;
    for (const item of targetItems) {
      const selectedUpdates = item.updates.filter((update) => !updateKeys || updateKeys.has(update.updateKey));
      for (const update of selectedUpdates) {
        await prisma.$executeRaw(Prisma.sql`
          INSERT INTO public.rmm_patch_approval
            (
              id, organization_id, agent_id, update_key, title_norm, kb_article,
              decision, reason, defer_until, decided_by, decided_at, updated_at
            )
          VALUES
            (
              ${randomUUID()}, ${membership.organizationId}, ${item.agentId}, ${update.updateKey}, ${update.titleNorm},
              ${update.kbArticle ?? null}, ${decision}, ${reason}, ${deferUntil}, ${req.jwt!.sub}, NOW(), NOW()
            )
          ON CONFLICT (organization_id, agent_id, update_key)
          DO UPDATE SET
            decision = EXCLUDED.decision,
            reason = EXCLUDED.reason,
            defer_until = EXCLUDED.defer_until,
            decided_by = EXCLUDED.decided_by,
            decided_at = NOW(),
            updated_at = NOW()
        `);
        updated += 1;
      }
      if (selectedUpdates.length > 0) {
        await insertPatchAction({
          organizationId: membership.organizationId,
          agentId: item.agentId,
          actionType: 'approval',
          status: decision,
          updateKeys: selectedUpdates.map((update) => update.updateKey),
          requestedBy: req.jwt!.sub
        });
      }
    }

    return res.json({ updated, targetedDevices: targetItems.length, decision });
  } catch (error) {
    return next(error);
  }
});

patchesRouter.post('/install', async (req: AuthedRequest, res, next) => {
  try {
    const membership = await requireMembership(req, res);
    if (!membership) return;
    if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can trigger patch installs' });

    const body = asRecord(req.body);
    const updateKeys = readUpdateKeys(body);
    const agentIds = await resolveTargetAgentIds(membership.organizationId, body);
    if (agentIds.length === 0) return res.status(400).json({ error: 'No target devices found' });

    const now = new Date();
    const summary = await loadPatchCompliance(membership.organizationId, {});
    const targetItems = summary.items.filter((item) => agentIds.includes(item.agentId));
    const queued: Array<{ agentId: string; remediationJobId: string; remediationCommandId: string; updateCount: number }> = [];
    const skipped: Array<{ agentId: string; reason: string }> = [];
    const commandsToPublish: unknown[] = [];
    const actionsToInsert: Array<{
      agentId: string;
      customerId: string | null;
      commandId: string;
      command: string;
      updateKeyList: string[];
    }> = [];

    for (const item of targetItems) {
      const policy = item.effectivePolicy;
      const selectedUpdates = item.updates.filter((update) => !updateKeys || updateKeys.has(update.updateKey));
      if (selectedUpdates.length === 0) {
        skipped.push({ agentId: item.agentId, reason: 'no_matching_updates' });
        continue;
      }
      const selectedInstallable = selectedUpdates.filter((update) => {
        if (update.approvalDecision === 'denied') return false;
        if (isApprovalDeferred(update.approvalDecision, update.deferUntil, now)) return false;
        return true;
      });

      if (selectedInstallable.length === 0) {
        skipped.push({ agentId: item.agentId, reason: 'updates_denied_or_deferred' });
        await insertCommandAudit({
          organizationId: membership.organizationId,
          customerId: item.customerId,
          userId: req.jwt!.sub,
          agentId: item.agentId,
          command: 'talos-patch-install',
          wasAllowed: false,
          denialReason: 'Updates are denied or deferred'
        });
        continue;
      }

      const updateKeyList = selectedInstallable.map((update) => update.updateKey);
      const rebootBehavior = policy?.rebootBehavior ?? 'allow';
      const metadata = {
        source: 'patch_management_manual_install',
        policyId: policy?.id ?? null,
        updateKeys: updateKeyList,
        updates: selectedInstallable.map((update) => ({
          title: update.title,
          kbArticle: update.kbArticle,
          severity: update.severity
        })),
        rebootBehavior
      };
      const command = `talos-patch-install --updates ${updateKeyList.join(',')} --reboot ${rebootBehavior}`;
      const commandId = randomUUID();
      const dedupeKey = `patch-install:${commandId}`;
      commandsToPublish.push({
        schemaVersion: 1,
        eventType: 'remediation.command.requested',
        commandId,
        organizationId: membership.organizationId,
        agentId: item.agentId,
        intentId: 'talos.patch.install',
        decisionId: null,
        dedupeKey,
        requestedBy: req.jwt!.sub,
        requestedAt: now.toISOString(),
        approvalState: 'approved',
        metadata,
        steps: [
          {
            stepIndex: 0,
            command,
            status: 'pending',
            evidence: metadata,
            timeoutSeconds: 7200
          }
        ],
        execution: {
          maxRetries: 0,
          timeoutSeconds: 7200,
          stopOnFailure: true
        }
      });
      actionsToInsert.push({
        agentId: item.agentId,
        customerId: item.customerId,
        commandId,
        command,
        updateKeyList
      });
      queued.push({
        agentId: item.agentId,
        remediationJobId: commandId,
        remediationCommandId: commandId,
        updateCount: updateKeyList.length
      });
    }

    await publishRemediationCommands(commandsToPublish);

    for (const action of actionsToInsert) {
      await insertPatchAction({
        organizationId: membership.organizationId,
        agentId: action.agentId,
        operationId: action.commandId,
        actionType: 'install',
        status: 'queued',
        updateKeys: action.updateKeyList,
        phase: 'searching',
        remediationCommandId: action.commandId,
        requestedBy: req.jwt!.sub
      });
      await insertCommandAudit({
        organizationId: membership.organizationId,
        customerId: action.customerId,
        userId: req.jwt!.sub,
        agentId: action.agentId,
        command: action.command,
        wasAllowed: true
      });
    }

    return res.status(202).json({ queued, skipped, targetedDevices: targetItems.length });
  } catch (error) {
    return next(error);
  }
});
