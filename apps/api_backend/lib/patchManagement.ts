export type PatchPolicyScopeType = 'organization' | 'customer' | 'site' | 'device';
export type PatchPolicyTargetOsFamily = 'all' | 'windows' | 'linux' | 'macos';
export type PatchDeviceOsFamily = 'windows' | 'linux' | 'macos' | 'unknown';
export type PatchApprovalMode = 'manual' | 'auto_approve_security' | 'auto_approve_all';
export type PatchRebootBehavior = 'suppress' | 'allow' | 'force';
export type PatchDecision = 'approved' | 'denied' | 'deferred';
export type PatchComplianceStatus = 'compliant' | 'pending' | 'security' | 'critical' | 'reboot_required' | 'unknown';

export const CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY = 100;
export const DEFAULT_PATCH_POLICY_PRIORITY = 10000;

export interface PatchPolicyForResolution {
  id: string;
  scopeType: PatchPolicyScopeType;
  scopeKey: string;
  targetOsFamily?: PatchPolicyTargetOsFamily | null;
  approvalMode: PatchApprovalMode;
  maintenanceWindowStart: string | null;
  maintenanceWindowEnd: string | null;
  maintenanceWindowTimezone: string | null;
  rebootBehavior: PatchRebootBehavior;
  deferralDays: number;
  managedMode?: boolean | null;
  nativeWindowsUpdateControl?: boolean | null;
  policyConfig?: unknown;
  priority: number;
  enabled: boolean;
  isDefault?: boolean | null;
  updatedAt?: string | Date | null;
}

export interface PatchDeviceScope {
  organizationId: string;
  customerId?: string | null;
  siteId?: string | null;
  agentId?: string | null;
  osFamily?: PatchDeviceOsFamily | string | null;
}

export interface PatchUpdateInput {
  title: string;
  titleNorm?: string | null;
  description?: string | null;
  kbArticle?: string | null;
  isMandatory?: boolean | null;
  requiresReboot?: boolean | null;
  sizeBytes?: number | null;
}

export interface PatchApprovalForSummary {
  agentId: string;
  updateKey: string;
  decision: PatchDecision;
  deferUntil?: string | Date | null;
}

export interface PatchDeviceComplianceInput {
  agentId: string;
  hostname: string;
  os: string;
  customerId?: string | null;
  customerName?: string | null;
  siteId?: string | null;
  siteName?: string | null;
  lastScanAt?: string | Date | null;
  rebootRequired?: boolean | null;
  installStatus?: string | null;
  installStatusAt?: string | Date | null;
  pendingUpdates: PatchUpdateInput[];
}

export interface PatchUpdateSummary extends PatchUpdateInput {
  updateKey: string;
  severity: 'critical' | 'security' | 'other';
  approvalDecision: PatchDecision | null;
  deferUntil: string | null;
}

export interface PatchDeviceComplianceSummary {
  agentId: string;
  hostname: string;
  os: string;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  lastScanAt: string | null;
  pendingUpdatesCount: number;
  missingCriticalCount: number;
  missingSecurityCount: number;
  rebootRequired: boolean;
  installStatus: string;
  complianceStatus: PatchComplianceStatus;
  effectivePolicy: PatchPolicyForResolution | null;
  updates: PatchUpdateSummary[];
}

export interface PatchComplianceTotals {
  devices: number;
  compliant: number;
  pending: number;
  security: number;
  critical: number;
  rebootRequired: number;
  unknown: number;
  pendingUpdates: number;
  missingCritical: number;
  missingSecurity: number;
}

export function normalizePatchText(value: unknown): string {
  if (typeof value !== 'string') return '';
  return value.trim().toLowerCase().replace(/\s+/g, ' ');
}

export function normalizePatchOsFamily(os: unknown): PatchDeviceOsFamily {
  const text = normalizePatchText(os);
  if (!text) return 'unknown';
  if (text.includes('windows')) return 'windows';
  if (
    text.includes('linux') ||
    text.includes('ubuntu') ||
    text.includes('debian') ||
    text.includes('fedora') ||
    text.includes('red hat') ||
    text.includes('rhel') ||
    text.includes('centos') ||
    text.includes('alma') ||
    text.includes('rocky') ||
    text.includes('suse')
  ) {
    return 'linux';
  }
  if (text.includes('macos') || text.includes('mac os') || text.includes('darwin')) return 'macos';
  return 'unknown';
}

export function buildPatchUpdateKey(update: Pick<PatchUpdateInput, 'title' | 'titleNorm' | 'kbArticle'>): string {
  const titleNorm = normalizePatchText(update.titleNorm || update.title);
  const kb = normalizePatchText(update.kbArticle || '');
  return `${titleNorm}|${kb}`;
}

export function classifyPatchSeverity(update: PatchUpdateInput): 'critical' | 'security' | 'other' {
  const text = normalizePatchText(
    [update.title, update.description, update.kbArticle].filter(Boolean).join(' ')
  );
  if (/\bcritical\b/.test(text)) return 'critical';
  if (/\bsecurity\b/.test(text) || /\bdefender\b/.test(text) || /\bmalicious software removal\b/.test(text)) {
    return 'security';
  }
  if (update.isMandatory === true) return 'security';
  return 'other';
}

function policyUpdatedAtMs(policy: PatchPolicyForResolution): number {
  if (!policy.updatedAt) return 0;
  const value = policy.updatedAt instanceof Date ? policy.updatedAt.getTime() : Date.parse(policy.updatedAt);
  return Number.isNaN(value) ? 0 : value;
}

function policyPriority(policy: PatchPolicyForResolution): number {
  if (Number.isInteger(policy.priority)) return policy.priority;
  return policy.isDefault ? DEFAULT_PATCH_POLICY_PRIORITY : CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY;
}

export function resolveEffectivePatchPolicy(
  policies: PatchPolicyForResolution[],
  scope: PatchDeviceScope
): PatchPolicyForResolution | null {
  const candidates = policies.filter((policy) => {
    if (!policy.enabled) return false;
    const targetOsFamily = policy.targetOsFamily ?? 'all';
    if (targetOsFamily !== 'all' && targetOsFamily !== scope.osFamily) return false;
    if (policy.isDefault) return policy.scopeType === 'organization';
    if (policy.scopeType === 'device') return Boolean(scope.agentId && policy.scopeKey === scope.agentId);
    if (policy.scopeType === 'site') return Boolean(scope.siteId && policy.scopeKey === scope.siteId);
    if (policy.scopeType === 'customer') return Boolean(scope.customerId && policy.scopeKey === scope.customerId);
    return policy.scopeType === 'organization' && policy.scopeKey === scope.organizationId;
  });

  const specificity: Record<PatchPolicyScopeType, number> = {
    organization: 1,
    customer: 2,
    site: 3,
    device: 4
  };

  candidates.sort((a, b) => {
    const priorityDelta = policyPriority(a) - policyPriority(b);
    if (priorityDelta !== 0) return priorityDelta;

    const specificityDelta =
      (b.isDefault ? 0 : specificity[b.scopeType]) - (a.isDefault ? 0 : specificity[a.scopeType]);
    if (specificityDelta !== 0) return specificityDelta;

    return policyUpdatedAtMs(b) - policyUpdatedAtMs(a);
  });

  return candidates[0] ?? null;
}

function normalizeIso(value: string | Date | null | undefined): string | null {
  if (!value) return null;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

function dateMs(value: string | Date | null | undefined): number | null {
  if (!value) return null;
  const date = value instanceof Date ? value : new Date(value);
  const ms = date.getTime();
  return Number.isNaN(ms) ? null : ms;
}

function normalizeInstallStatus(device: PatchDeviceComplianceInput, pendingUpdatesCount: number): string {
  const status = device.installStatus || 'not_requested';
  if (status.toLowerCase() !== 'completed' || pendingUpdatesCount === 0) return status;

  const lastScanMs = dateMs(device.lastScanAt);
  const installStatusMs = dateMs(device.installStatusAt);
  if (lastScanMs !== null && installStatusMs !== null && lastScanMs > installStatusMs) {
    return 'not_requested';
  }

  return status;
}

export function calculatePatchComplianceSummary(
  devices: PatchDeviceComplianceInput[],
  policies: PatchPolicyForResolution[],
  approvals: PatchApprovalForSummary[],
  organizationId: string
): { totals: PatchComplianceTotals; items: PatchDeviceComplianceSummary[] } {
  const approvalsByDeviceUpdate = new Map<string, PatchApprovalForSummary>();
  for (const approval of approvals) {
    approvalsByDeviceUpdate.set(`${approval.agentId}|${approval.updateKey}`, approval);
  }

  const items = devices.map((device): PatchDeviceComplianceSummary => {
    const updates = device.pendingUpdates.map((update) => {
      const updateKey = buildPatchUpdateKey(update);
      const approval = approvalsByDeviceUpdate.get(`${device.agentId}|${updateKey}`) ?? null;
      return {
        ...update,
        titleNorm: update.titleNorm ?? normalizePatchText(update.title),
        updateKey,
        severity: classifyPatchSeverity(update),
        approvalDecision: approval?.decision ?? null,
        deferUntil: normalizeIso(approval?.deferUntil ?? null)
      };
    });

    const missingCriticalCount = updates.filter((update) => update.severity === 'critical').length;
    const missingSecurityCount = updates.filter((update) => update.severity === 'security').length;
    const pendingUpdatesCount = updates.length;
    const rebootRequired = device.rebootRequired === true || updates.some((update) => update.requiresReboot === true);
    const effectivePolicy = resolveEffectivePatchPolicy(policies, {
      organizationId,
      customerId: device.customerId ?? null,
      siteId: device.siteId ?? null,
      agentId: device.agentId,
      osFamily: normalizePatchOsFamily(device.os)
    });

    let complianceStatus: PatchComplianceStatus;
    if (!device.lastScanAt) {
      complianceStatus = 'unknown';
    } else if (missingCriticalCount > 0) {
      complianceStatus = 'critical';
    } else if (missingSecurityCount > 0) {
      complianceStatus = 'security';
    } else if (pendingUpdatesCount > 0) {
      complianceStatus = 'pending';
    } else if (rebootRequired) {
      complianceStatus = 'reboot_required';
    } else {
      complianceStatus = 'compliant';
    }

    return {
      agentId: device.agentId,
      hostname: device.hostname,
      os: device.os,
      customerId: device.customerId ?? null,
      customerName: device.customerName ?? null,
      siteId: device.siteId ?? null,
      siteName: device.siteName ?? null,
      lastScanAt: normalizeIso(device.lastScanAt ?? null),
      pendingUpdatesCount,
      missingCriticalCount,
      missingSecurityCount,
      rebootRequired,
      installStatus: normalizeInstallStatus(device, pendingUpdatesCount),
      complianceStatus,
      effectivePolicy,
      updates
    };
  });

  const totals = items.reduce<PatchComplianceTotals>(
    (acc, item) => {
      acc.devices += 1;
      acc.pendingUpdates += item.pendingUpdatesCount;
      acc.missingCritical += item.missingCriticalCount;
      acc.missingSecurity += item.missingSecurityCount;
      if (item.rebootRequired) acc.rebootRequired += 1;
      if (item.complianceStatus === 'compliant') acc.compliant += 1;
      if (item.complianceStatus === 'pending') acc.pending += 1;
      if (item.complianceStatus === 'security') acc.security += 1;
      if (item.complianceStatus === 'critical') acc.critical += 1;
      if (item.complianceStatus === 'unknown') acc.unknown += 1;
      return acc;
    },
    {
      devices: 0,
      compliant: 0,
      pending: 0,
      security: 0,
      critical: 0,
      rebootRequired: 0,
      unknown: 0,
      pendingUpdates: 0,
      missingCritical: 0,
      missingSecurity: 0
    }
  );

  return { totals, items };
}
