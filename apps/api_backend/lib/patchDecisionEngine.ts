import { randomUUID } from 'crypto';
import {
  buildPatchUpdateKey,
  classifyPatchSeverity,
  normalizePatchText,
  PatchPolicyForResolution
} from './patchManagement';

export type PatchDeviceType = 'server' | 'workstation' | 'laptop' | 'unknown';
export type PatchRing = 'pilot' | 'early' | 'broad' | 'critical_servers' | 'excluded';
export type PatchCategory =
  | 'security'
  | 'critical'
  | 'cumulative'
  | 'feature'
  | 'driver'
  | 'firmware'
  | 'microsoft_product'
  | 'uwp_app'
  | 'definition'
  | 'optional'
  | 'preview'
  | 'other';
export type PatchLifecycleState =
  | 'unknown'
  | 'detected'
  | 'approved'
  | 'deferred'
  | 'blocked'
  | 'downloaded'
  | 'install_pending'
  | 'installed'
  | 'reboot_pending'
  | 'failed'
  | 'superseded';
export type PatchOverrideAction =
  | 'approve'
  | 'block'
  | 'defer'
  | 'force_install'
  | 'force_scan'
  | 'force_download'
  | 'force_reboot'
  | 'defer_reboot'
  | 'maintenance_mode'
  | 'emergency_approve'
  | 'cancel';
export type PatchPlanActionType =
  | 'applyNativeControl'
  | 'scan'
  | 'download'
  | 'install'
  | 'reboot'
  | 'defer'
  | 'blocked'
  | 'reportOnly';

export interface PatchPhaseWindow {
  enabled: boolean;
  start: string | null;
  end: string | null;
  timezone: string;
}

export interface PatchCategoryRule {
  approval: 'auto' | 'manual' | 'blocked';
  installAfterDays: number;
  forceInstallByDays: number | null;
  forceRebootByDays: number | null;
}

export interface PatchPolicyConfig {
  managedMode: boolean;
  nativeWindowsUpdateControl: boolean;
  checkInIntervalMinutes: number;
  windows: Record<'scan' | 'download' | 'install' | 'reboot', PatchPhaseWindow>;
  categories: Record<PatchCategory, PatchCategoryRule>;
  reboot: {
    allowAutomaticReboot: boolean;
    forceRebootAfterDeadline: boolean;
    warningMinutes: number;
    maxUserDeferrals: number;
    activeHoursProtection: boolean;
    serverBehavior: 'window_only' | 'deadline_window' | 'manual_only';
    workstationBehavior: 'window_or_deadline' | 'allow_anytime_after_deadline';
  };
}

export interface PatchDecisionDevice {
  organizationId: string;
  agentId: string;
  hostname: string;
  os: string;
  customerId?: string | null;
  siteId?: string | null;
  deviceType: PatchDeviceType;
  patchRing: PatchRing;
  patchManaged: boolean;
  nativeWindowsUpdateControl: boolean;
  patchMaintenanceModeUntil?: string | Date | null;
  patchTags?: string[];
  rebootRequired?: boolean | null;
  lastScanAt?: string | Date | null;
  lastCheckInAt?: string | Date | null;
}

export interface PatchDecisionUpdate {
  updateKey: string;
  title: string;
  titleNorm?: string | null;
  kbArticle?: string | null;
  category?: PatchCategory | string | null;
  lifecycleState?: PatchLifecycleState | string | null;
  approvalState?: string | null;
  releaseDate?: string | Date | null;
  firstDetectedAt?: string | Date | null;
  lastDetectedAt?: string | Date | null;
  downloadedAt?: string | Date | null;
  installedAt?: string | Date | null;
  requiresReboot?: boolean | null;
  superseded?: boolean | null;
  failedAt?: string | Date | null;
}

export interface PatchDecisionOverride {
  id: string;
  scopeType: 'global' | 'organization' | 'customer' | 'site' | 'group' | 'tag' | 'ring' | 'device';
  scopeKey: string;
  action: PatchOverrideAction;
  operationId?: string | null;
  updateKey?: string | null;
  kbArticle?: string | null;
  category?: string | null;
  reason?: string | null;
  deferUntil?: string | Date | null;
  expiresAt?: string | Date | null;
  enabled?: boolean | null;
}

export interface PatchActionPlanItem {
  operationId: string;
  action: PatchPlanActionType;
  updateKeys: string[];
  category?: PatchCategory | null;
  window?: 'scan' | 'download' | 'install' | 'reboot' | null;
  notBefore?: string | null;
  deadlineAt?: string | null;
  forced: boolean;
  reason: string;
  metadata: Record<string, unknown>;
}

export interface PatchActionPlan {
  schemaVersion: 1;
  generatedAt: string;
  organizationId: string;
  agentId: string;
  policyId: string | null;
  managedMode: boolean;
  nativeWindowsUpdateControl: boolean;
  nextCheckInAt: string;
  actions: PatchActionPlanItem[];
}

export interface EvaluatePatchPlanInput {
  now?: string | Date;
  device: PatchDecisionDevice;
  policy: (PatchPolicyForResolution & { policyConfig?: unknown; managedMode?: boolean; nativeWindowsUpdateControl?: boolean }) | null;
  updates: PatchDecisionUpdate[];
  overrides?: PatchDecisionOverride[];
  groupIds?: string[];
}

const ONE_DAY_MS = 24 * 60 * 60 * 1000;

const baseWindow = (enabled = true): PatchPhaseWindow => ({
  enabled,
  start: null,
  end: null,
  timezone: 'UTC'
});

const categoryRule = (
  approval: PatchCategoryRule['approval'],
  installAfterDays: number,
  forceInstallByDays: number | null,
  forceRebootByDays: number | null
): PatchCategoryRule => ({
  approval,
  installAfterDays,
  forceInstallByDays,
  forceRebootByDays
});

export function defaultPatchPolicyConfig(deferralDays = 0): PatchPolicyConfig {
  return {
    managedMode: true,
    nativeWindowsUpdateControl: true,
    checkInIntervalMinutes: 60,
    windows: {
      scan: baseWindow(true),
      download: baseWindow(true),
      install: baseWindow(true),
      reboot: baseWindow(true)
    },
    categories: {
      security: categoryRule('auto', deferralDays, 21, 24),
      critical: categoryRule('auto', deferralDays, 14, 17),
      cumulative: categoryRule('auto', deferralDays, 21, 24),
      definition: categoryRule('auto', 0, 2, 3),
      microsoft_product: categoryRule('auto', deferralDays, 21, 24),
      uwp_app: categoryRule('manual', deferralDays, null, null),
      feature: categoryRule('manual', 30, 90, 97),
      driver: categoryRule('manual', 14, null, null),
      firmware: categoryRule('manual', 30, null, null),
      optional: categoryRule('manual', 14, null, null),
      preview: categoryRule('blocked', 365, null, null),
      other: categoryRule('manual', deferralDays, null, null)
    },
    reboot: {
      allowAutomaticReboot: true,
      forceRebootAfterDeadline: true,
      warningMinutes: 60,
      maxUserDeferrals: 3,
      activeHoursProtection: true,
      serverBehavior: 'window_only',
      workstationBehavior: 'window_or_deadline'
    }
  };
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function supportsNativeWindowsUpdateControl(os: string): boolean {
  return /\bwindows\b/i.test(os);
}

function asBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function asNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function asNullableNumber(value: unknown, fallback: number | null): number | null {
  if (value === null) return null;
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function asString(value: unknown, fallback: string): string {
  return typeof value === 'string' && value.trim() ? value.trim() : fallback;
}

function mergeWindow(base: PatchPhaseWindow, raw: unknown): PatchPhaseWindow {
  const value = asRecord(raw);
  return {
    enabled: asBoolean(value.enabled, base.enabled),
    start: typeof value.start === 'string' && value.start ? value.start : base.start,
    end: typeof value.end === 'string' && value.end ? value.end : base.end,
    timezone: asString(value.timezone, base.timezone)
  };
}

function constrainDownloadWindow(download: PatchPhaseWindow, install: PatchPhaseWindow): PatchPhaseWindow {
  if (!download.enabled || download.start || download.end || (!install.start && !install.end)) return download;
  return {
    ...download,
    start: install.start,
    end: install.end,
    timezone: install.timezone
  };
}

function mergeRule(base: PatchCategoryRule, raw: unknown): PatchCategoryRule {
  const value = asRecord(raw);
  const approval = value.approval === 'auto' || value.approval === 'manual' || value.approval === 'blocked'
    ? value.approval
    : base.approval;
  return {
    approval,
    installAfterDays: asNumber(value.installAfterDays, base.installAfterDays),
    forceInstallByDays: asNullableNumber(value.forceInstallByDays, base.forceInstallByDays),
    forceRebootByDays: asNullableNumber(value.forceRebootByDays, base.forceRebootByDays)
  };
}

export function coercePatchPolicyConfig(
  policy?: (PatchPolicyForResolution & { policyConfig?: unknown; managedMode?: boolean; nativeWindowsUpdateControl?: boolean }) | null
): PatchPolicyConfig {
  const base = defaultPatchPolicyConfig(policy?.deferralDays ?? 0);
  const raw = asRecord(policy?.policyConfig);
  const windows = asRecord(raw.windows);
  const categories = asRecord(raw.categories);
  const reboot = asRecord(raw.reboot);
  const scanWindow = mergeWindow(base.windows.scan, windows.scan);
  const installWindow = mergeWindow(base.windows.install, windows.install);
  const downloadWindow = constrainDownloadWindow(mergeWindow(base.windows.download, windows.download), installWindow);
  const rebootWindow = mergeWindow(base.windows.reboot, windows.reboot);
  return {
    managedMode: asBoolean(raw.managedMode, policy?.managedMode ?? base.managedMode),
    nativeWindowsUpdateControl: asBoolean(
      raw.nativeWindowsUpdateControl,
      policy?.nativeWindowsUpdateControl ?? base.nativeWindowsUpdateControl
    ),
    checkInIntervalMinutes: asNumber(raw.checkInIntervalMinutes, base.checkInIntervalMinutes),
    windows: {
      scan: scanWindow,
      download: downloadWindow,
      install: installWindow,
      reboot: rebootWindow
    },
    categories: {
      security: mergeRule(base.categories.security, categories.security),
      critical: mergeRule(base.categories.critical, categories.critical),
      cumulative: mergeRule(base.categories.cumulative, categories.cumulative),
      feature: mergeRule(base.categories.feature, categories.feature),
      driver: mergeRule(base.categories.driver, categories.driver),
      firmware: mergeRule(base.categories.firmware, categories.firmware),
      microsoft_product: mergeRule(base.categories.microsoft_product, categories.microsoft_product),
      uwp_app: mergeRule(base.categories.uwp_app, categories.uwp_app),
      definition: mergeRule(base.categories.definition, categories.definition),
      optional: mergeRule(base.categories.optional, categories.optional),
      preview: mergeRule(base.categories.preview, categories.preview),
      other: mergeRule(base.categories.other, categories.other)
    },
    reboot: {
      allowAutomaticReboot: asBoolean(reboot.allowAutomaticReboot, base.reboot.allowAutomaticReboot),
      forceRebootAfterDeadline: asBoolean(reboot.forceRebootAfterDeadline, base.reboot.forceRebootAfterDeadline),
      warningMinutes: asNumber(reboot.warningMinutes, base.reboot.warningMinutes),
      maxUserDeferrals: asNumber(reboot.maxUserDeferrals, base.reboot.maxUserDeferrals),
      activeHoursProtection: asBoolean(reboot.activeHoursProtection, base.reboot.activeHoursProtection),
      serverBehavior:
        reboot.serverBehavior === 'window_only' || reboot.serverBehavior === 'deadline_window' || reboot.serverBehavior === 'manual_only'
          ? reboot.serverBehavior
          : base.reboot.serverBehavior,
      workstationBehavior:
        reboot.workstationBehavior === 'window_or_deadline' || reboot.workstationBehavior === 'allow_anytime_after_deadline'
          ? reboot.workstationBehavior
          : base.reboot.workstationBehavior
    }
  };
}

export function classifyPatchCategory(update: Pick<PatchDecisionUpdate, 'title' | 'kbArticle' | 'category'>): PatchCategory {
  if (typeof update.category === 'string') {
    const normalized = update.category.trim().toLowerCase();
    if (
      normalized === 'security' ||
      normalized === 'critical' ||
      normalized === 'cumulative' ||
      normalized === 'feature' ||
      normalized === 'driver' ||
      normalized === 'firmware' ||
      normalized === 'microsoft_product' ||
      normalized === 'uwp_app' ||
      normalized === 'definition' ||
      normalized === 'optional' ||
      normalized === 'preview' ||
      normalized === 'other'
    ) {
      return normalized;
    }
  }
  const text = normalizePatchText(`${update.title} ${update.kbArticle ?? ''}`);
  if (/\bpreview\b/.test(text)) return 'preview';
  if (/\bdriver\b/.test(text)) return 'driver';
  if (/\bfirmware\b|bios\b/.test(text)) return 'firmware';
  if (/\bfeature update\b|enablement package\b/.test(text)) return 'feature';
  if (/\bdefinition\b|defender\b|security intelligence\b/.test(text)) return 'definition';
  if (
    /\buwp\b|\bappx\b|\bmsix\b|\bwindowsapp(runtime)?\b|\bwindows app runtime\b|\bdevhome\b|\bwindows\.devhome\b|\bvclibs\b|\bcrossdevice\b|\bmicrosoftwindows\.[a-z0-9_.-]+/.test(text) ||
    /^[a-z0-9]{12}-microsoft\./.test(text)
  ) {
    return 'uwp_app';
  }
  if (/\bcumulative\b/.test(text)) return 'cumulative';
  const severity = classifyPatchSeverity({ title: update.title, kbArticle: update.kbArticle });
  if (severity === 'critical' || severity === 'security') return severity;
  if (isMacosOsVersionUpdateTitle(update.title)) return 'feature';
  return 'other';
}

function isMacosOsVersionUpdateTitle(title: unknown): boolean {
  const text = normalizePatchText(title);
  return /^mac\s?os\b.*\b\d+(?:\.\d+)*\b/.test(text);
}

function isMacosDeviceOs(os: string): boolean {
  const text = normalizePatchText(os);
  return /\b(macos|mac os|mac os x|os x|darwin)\b/.test(text);
}

function macosOsVersionFromTitle(title: unknown): number[] | null {
  const text = normalizePatchText(title);
  if (!/^mac\s?os\b/.test(text)) return null;
  const match = /\b(\d+(?:\.\d+)*)\b/.exec(text);
  if (!match) return null;
  return match[1].split('.').map((part) => Number(part)).filter((part) => Number.isFinite(part));
}

function compareVersionPartsDesc(a: number[] | null, b: number[] | null): number {
  const left = a ?? [];
  const right = b ?? [];
  const length = Math.max(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    const diff = (right[index] ?? 0) - (left[index] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

function parseDateMs(value: string | Date | null | undefined): number | null {
  if (!value) return null;
  const ms = value instanceof Date ? value.getTime() : Date.parse(value);
  return Number.isNaN(ms) ? null : ms;
}

function addDaysIso(baseMs: number, days: number): string {
  return new Date(baseMs + days * ONE_DAY_MS).toISOString();
}

function minutesFromTime(value: string | null): number | null {
  if (!value) return null;
  const match = /^([01]\d|2[0-3]):([0-5]\d)$/.exec(value);
  if (!match) return null;
  return Number(match[1]) * 60 + Number(match[2]);
}

function localMinutes(now: Date, timezone: string): number {
  try {
    const parts = new Intl.DateTimeFormat('en-GB', {
      timeZone: timezone || 'UTC',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23'
    }).formatToParts(now);
    const hour = Number(parts.find((part) => part.type === 'hour')?.value ?? '0');
    const minute = Number(parts.find((part) => part.type === 'minute')?.value ?? '0');
    return hour * 60 + minute;
  } catch {
    return now.getUTCHours() * 60 + now.getUTCMinutes();
  }
}

export function isWithinPatchWindow(window: PatchPhaseWindow, now: string | Date): boolean {
  if (!window.enabled) return false;
  const start = minutesFromTime(window.start);
  const end = minutesFromTime(window.end);
  if (start === null || end === null) return true;
  const date = now instanceof Date ? now : new Date(now);
  if (Number.isNaN(date.getTime())) return false;
  const current = localMinutes(date, window.timezone);
  if (start === end) return true;
  if (start < end) return current >= start && current < end;
  return current >= start || current < end;
}

function isExpired(override: PatchDecisionOverride, nowMs: number): boolean {
  if (override.enabled === false) return true;
  const expiresMs = parseDateMs(override.expiresAt ?? null);
  return expiresMs !== null && expiresMs <= nowMs;
}

function overrideScopeMatches(
  override: PatchDecisionOverride,
  device: PatchDecisionDevice,
  groupIds: string[]
): boolean {
  if (override.scopeType === 'global') return true;
  if (override.scopeType === 'organization') return override.scopeKey === device.organizationId;
  if (override.scopeType === 'customer') return Boolean(device.customerId && override.scopeKey === device.customerId);
  if (override.scopeType === 'site') return Boolean(device.siteId && override.scopeKey === device.siteId);
  if (override.scopeType === 'ring') return override.scopeKey === device.patchRing;
  if (override.scopeType === 'device') return override.scopeKey === device.agentId;
  if (override.scopeType === 'tag') return (device.patchTags ?? []).includes(override.scopeKey);
  if (override.scopeType === 'group') return groupIds.includes(override.scopeKey);
  return false;
}

function overrideUpdateMatches(override: PatchDecisionOverride, update?: PatchDecisionUpdate, category?: PatchCategory): boolean {
  if (!update) return !override.updateKey && !override.kbArticle && !override.category;
  if (override.updateKey && override.updateKey !== update.updateKey) return false;
  if (override.kbArticle && normalizePatchText(override.kbArticle) !== normalizePatchText(update.kbArticle ?? '')) return false;
  if (override.category && override.category !== category) return false;
  return true;
}

function matchingOverrides(
  input: EvaluatePatchPlanInput,
  nowMs: number,
  update?: PatchDecisionUpdate,
  category?: PatchCategory
): PatchDecisionOverride[] {
  return (input.overrides ?? [])
    .filter((override) => !isExpired(override, nowMs))
    .filter((override) => overrideScopeMatches(override, input.device, input.groupIds ?? []))
    .filter((override) => overrideUpdateMatches(override, update, category));
}

function overrideIds(overrides: PatchDecisionOverride[]): string[] {
  return overrides.map((override) => override.id);
}

function overrideOperationId(overrides: PatchDecisionOverride[]): string | undefined {
  return overrides.find((override) => typeof override.operationId === 'string' && override.operationId.trim())?.operationId ?? undefined;
}

function action(
  type: PatchPlanActionType,
  reason: string,
  options: Partial<Omit<PatchActionPlanItem, 'operationId' | 'action' | 'reason' | 'metadata'>> & {
    operationId?: string;
    metadata?: Record<string, unknown>;
  } = {}
): PatchActionPlanItem {
  return {
    operationId: options.operationId ?? randomUUID(),
    action: type,
    updateKeys: options.updateKeys ?? [],
    category: options.category ?? null,
    window: options.window ?? null,
    notBefore: options.notBefore ?? null,
    deadlineAt: options.deadlineAt ?? null,
    forced: options.forced ?? false,
    reason,
    metadata: options.metadata ?? {}
  };
}

function uniqueActions(actions: PatchActionPlanItem[]): PatchActionPlanItem[] {
  const seen = new Set<string>();
  return actions.filter((item) => {
    const key = `${item.action}|${item.updateKeys.slice().sort().join(',')}|${item.reason}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function constrainMacosOsVersionInstallSelection(options: {
  device: PatchDecisionDevice;
  updates: PatchDecisionUpdate[];
  installKeys: string[];
}): {
  installKeys: string[];
  blocked: Array<{ update: PatchDecisionUpdate; category: PatchCategory; reason: string; metadata: Record<string, unknown> }>;
} {
  if (!isMacosDeviceOs(options.device.os)) return { installKeys: options.installKeys, blocked: [] };

  const installKeySet = new Set(options.installKeys);
  const selectedByKey = new Map(options.updates.filter((update) => installKeySet.has(update.updateKey)).map((update) => [update.updateKey, update]));
  const osUpdateCandidates = [...selectedByKey.values()]
    .map((update) => {
      const category = classifyPatchCategory(update);
      const version = category === 'feature' ? macosOsVersionFromTitle(update.title) : null;
      return {
        update,
        category,
        version,
        major: version?.[0] ?? null
      };
    })
    .filter((candidate) => candidate.major !== null);

  if (osUpdateCandidates.length <= 1) return { installKeys: options.installKeys, blocked: [] };

  const selected = osUpdateCandidates
    .slice()
    .sort((a, b) => compareVersionPartsDesc(a.version, b.version) || a.update.title.localeCompare(b.update.title))[0];
  if (!selected) return { installKeys: options.installKeys, blocked: [] };

  const selectedKey = selected.update.updateKey;
  const blocked = osUpdateCandidates
    .filter((candidate) => candidate.update.updateKey !== selectedKey)
    .map((candidate) => ({
      update: candidate.update,
      category: candidate.category,
      reason: `Skipped to avoid installing multiple macOS OS-version updates in one run; ${selected.update.title} was selected.`,
      metadata: {
        selectedUpdateKey: selectedKey,
        selectedTitle: selected.update.title,
        candidateMajor: candidate.major,
        competingUpdateKeys: osUpdateCandidates.map((item) => item.update.updateKey)
      }
    }));
  const blockedKeys = new Set(blocked.map((item) => item.update.updateKey));
  return {
    installKeys: options.installKeys.filter((updateKey) => !blockedKeys.has(updateKey)),
    blocked
  };
}

export function evaluatePatchActionPlan(input: EvaluatePatchPlanInput): PatchActionPlan {
  const now = input.now instanceof Date ? input.now : new Date(input.now ?? Date.now());
  const nowMs = Number.isNaN(now.getTime()) ? Date.now() : now.getTime();
  const generatedAt = new Date(nowMs).toISOString();
  const policyConfig = coercePatchPolicyConfig(input.policy);
  const maintenanceUntilMs = parseDateMs(input.device.patchMaintenanceModeUntil ?? null);
  const activeOverrides = matchingOverrides(input, nowMs);
  const inMaintenance = maintenanceUntilMs !== null && maintenanceUntilMs > nowMs
    || activeOverrides.some((override) => override.action === 'maintenance_mode');
  const managedMode = input.device.patchManaged && policyConfig.managedMode;
  const nativeWindowsUpdateControlSupported = supportsNativeWindowsUpdateControl(input.device.os);
  const nativeWindowsUpdateControl =
    nativeWindowsUpdateControlSupported
    && managedMode
    && input.device.nativeWindowsUpdateControl
    && policyConfig.nativeWindowsUpdateControl;
  const actions: PatchActionPlanItem[] = [];
  const forceRebootOverrides = activeOverrides.filter((override) => override.action === 'force_reboot');
  const hasManualInstallOnly = forceRebootOverrides.length === 0
    && activeOverrides.some((override) => override.action === 'force_install' || override.action === 'force_download');

  if (nativeWindowsUpdateControlSupported) {
    actions.push(action('applyNativeControl', nativeWindowsUpdateControl
      ? 'Talos patch control is enabled by policy.'
      : 'Native Windows Update control is restored or unmanaged by policy.', {
        forced: false,
        metadata: {
          enabled: nativeWindowsUpdateControl,
          overrideIds: []
        }
      }));
  }

  if (!managedMode) {
    actions.push(action('reportOnly', 'Device is unmanaged by policy.', {
        metadata: {
          managedMode,
          overrideIds: []
        }
      }));
    return {
      schemaVersion: 1,
      generatedAt,
      organizationId: input.device.organizationId,
      agentId: input.device.agentId,
      policyId: input.policy?.id ?? null,
      managedMode,
      nativeWindowsUpdateControl,
      nextCheckInAt: new Date(nowMs + policyConfig.checkInIntervalMinutes * 60_000).toISOString(),
      actions: uniqueActions(actions)
    };
  }

  if (forceRebootOverrides.length > 0) {
    actions.push(action('reboot', 'Manual reboot override requested.', {
      operationId: overrideOperationId(forceRebootOverrides),
      window: 'reboot',
      forced: true,
      metadata: { overrideIds: overrideIds(forceRebootOverrides) }
    }));
    return {
      schemaVersion: 1,
      generatedAt,
      organizationId: input.device.organizationId,
      agentId: input.device.agentId,
      policyId: input.policy?.id ?? null,
      managedMode,
      nativeWindowsUpdateControl,
      nextCheckInAt: new Date(nowMs + policyConfig.checkInIntervalMinutes * 60_000).toISOString(),
      actions: uniqueActions(actions)
    };
  }

  if (inMaintenance) {
    const maintenanceOverrides = activeOverrides.filter((override) => override.action === 'maintenance_mode');
    actions.push(action('reportOnly', 'Device is in patch maintenance mode.', {
      notBefore: maintenanceUntilMs ? new Date(maintenanceUntilMs).toISOString() : null,
      metadata: { overrideIds: overrideIds(maintenanceOverrides) }
    }));
    return {
      schemaVersion: 1,
      generatedAt,
      organizationId: input.device.organizationId,
      agentId: input.device.agentId,
      policyId: input.policy?.id ?? null,
      managedMode,
      nativeWindowsUpdateControl,
      nextCheckInAt: new Date(nowMs + policyConfig.checkInIntervalMinutes * 60_000).toISOString(),
      actions: uniqueActions(actions)
    };
  }

  const scanOverrides = activeOverrides.filter((override) => override.action === 'force_scan');
  const scanWindowOpen = isWithinPatchWindow(policyConfig.windows.scan, new Date(nowMs));
  const lastScanMs = parseDateMs(input.device.lastScanAt ?? null);
  const scanDue = lastScanMs === null || nowMs - lastScanMs >= policyConfig.checkInIntervalMinutes * 60_000;
  if (!hasManualInstallOnly && (scanOverrides.length > 0 || (scanDue && scanWindowOpen))) {
    actions.push(action('scan', scanOverrides.length > 0 ? 'Manual scan override requested.' : 'Scan window is open and scan is due.', {
      operationId: overrideOperationId(scanOverrides),
      window: 'scan',
      forced: scanOverrides.length > 0,
      metadata: { overrideIds: overrideIds(scanOverrides) }
    }));
    if (scanOverrides.length > 0) {
      return {
        schemaVersion: 1,
        generatedAt,
        organizationId: input.device.organizationId,
        agentId: input.device.agentId,
        policyId: input.policy?.id ?? null,
        managedMode,
        nativeWindowsUpdateControl,
        nextCheckInAt: new Date(nowMs + policyConfig.checkInIntervalMinutes * 60_000).toISOString(),
        actions: uniqueActions(actions)
      };
    }
  } else if (!hasManualInstallOnly && scanDue && !scanWindowOpen) {
    actions.push(action('defer', 'Scan is due but outside the scan window.', {
      window: 'scan'
    }));
  }

  const installKeys: string[] = [];
  const installOnlyOverrideIds = new Set<string>();
  for (const update of input.updates) {
    const lifecycle = update.lifecycleState ?? 'detected';
    const downloadedTransactionMember = Boolean(update.downloadedAt) && (lifecycle === 'superseded' || update.superseded);
    if (lifecycle === 'installed' || (!downloadedTransactionMember && (lifecycle === 'superseded' || update.superseded))) continue;
    const category = classifyPatchCategory(update);
    const rule = policyConfig.categories[category] ?? policyConfig.categories.other;
    const updateOverrides = matchingOverrides(input, nowMs, update, category);
    const block = updateOverrides.find((override) => override.action === 'block');
    const defer = updateOverrides.find((override) => override.action === 'defer');
    const emergency = updateOverrides.find((override) => override.action === 'emergency_approve');
    const forceInstall = updateOverrides.find((override) => override.action === 'force_install');
    const forceDownload = updateOverrides.find((override) => override.action === 'force_download');
    const force = forceInstall ?? forceDownload;
    const approve = updateOverrides.find((override) => override.action === 'approve');
    if (lifecycle === 'failed' && !force && !emergency) continue;
    const releaseMs = parseDateMs(update.releaseDate ?? null) ?? parseDateMs(update.firstDetectedAt ?? null) ?? nowMs;
    const eligibleMs = releaseMs + rule.installAfterDays * ONE_DAY_MS;
    const installDeadlineMs = rule.forceInstallByDays === null ? null : releaseMs + rule.forceInstallByDays * ONE_DAY_MS;
    const rebootDeadlineMs = rule.forceRebootByDays === null ? null : releaseMs + rule.forceRebootByDays * ONE_DAY_MS;
    const forceDeadlineReached = installDeadlineMs !== null && installDeadlineMs <= nowMs;

    if (block) {
      actions.push(action('blocked', block.reason || 'Update is blocked by override.', {
        updateKeys: [update.updateKey],
        category,
        metadata: { overrideId: block.id }
      }));
      continue;
    }

    if (defer) {
      const deferMs = parseDateMs(defer.deferUntil ?? null);
      if (deferMs === null || deferMs > nowMs) {
        actions.push(action('defer', defer.reason || 'Update is deferred by override.', {
          updateKeys: [update.updateKey],
          category,
          notBefore: deferMs ? new Date(deferMs).toISOString() : null,
          metadata: { overrideId: defer.id }
        }));
        continue;
      }
    }

    if (force && !emergency) {
      installOnlyOverrideIds.add(force.id);
    }

    if (!force && !emergency && rule.approval === 'blocked') {
      actions.push(action('blocked', `${category} updates are blocked by policy.`, {
        updateKeys: [update.updateKey],
        category
      }));
      continue;
    }

    if (!force && !emergency && eligibleMs > nowMs && !forceDeadlineReached) {
      actions.push(action('defer', `Update is deferred until ${new Date(eligibleMs).toISOString()}.`, {
        updateKeys: [update.updateKey],
        category,
        notBefore: new Date(eligibleMs).toISOString(),
        deadlineAt: installDeadlineMs ? new Date(installDeadlineMs).toISOString() : null,
        metadata: { releaseDate: new Date(releaseMs).toISOString(), installAfterDays: rule.installAfterDays }
      }));
      continue;
    }

    if (!force && !emergency && rule.approval === 'manual' && !approve) {
      actions.push(action('blocked', `${category} updates require manual approval.`, {
        updateKeys: [update.updateKey],
        category
      }));
      continue;
    }

    if (hasManualInstallOnly && !force) {
      continue;
    }

    if (force || emergency || forceDeadlineReached || isWithinPatchWindow(policyConfig.windows.install, new Date(nowMs))) {
      installKeys.push(update.updateKey);
      continue;
    }

    actions.push(action('defer', 'Update is approved but outside the install window.', {
      updateKeys: [update.updateKey],
      category,
      window: 'install',
      deadlineAt: installDeadlineMs ? new Date(installDeadlineMs).toISOString() : null
    }));
  }

  const macosInstallSelection = constrainMacosOsVersionInstallSelection({
    device: input.device,
    updates: input.updates,
    installKeys
  });
  for (const blocked of macosInstallSelection.blocked) {
    actions.push(action('blocked', blocked.reason, {
      updateKeys: [blocked.update.updateKey],
      category: blocked.category,
      metadata: blocked.metadata
    }));
  }

  if (macosInstallSelection.installKeys.length > 0) {
    const installOverrides = activeOverrides.filter((override) =>
      override.action === 'force_install' || override.action === 'force_download' || override.action === 'emergency_approve' || override.action === 'approve'
    );
    const hasDownloadInstallOverride = installOverrides.some((override) => override.action === 'force_download');
    actions.push(action('install', 'Install is authorized by policy or override.', {
      operationId: overrideOperationId(installOverrides),
      updateKeys: [...new Set(macosInstallSelection.installKeys)],
      window: 'install',
      forced: installOverrides.length > 0,
      metadata: {
        overrideIds: overrideIds(installOverrides),
        rebootBehavior: hasDownloadInstallOverride ? 'suppress' : 'allow',
        manualInstallOnly: installOnlyOverrideIds.size > 0
      }
    }));
  }

  const rebootOverrides = activeOverrides.filter((override) => override.action === 'force_reboot' || override.action === 'defer_reboot');
  const deferReboot = rebootOverrides.find((override) => override.action === 'defer_reboot');
  const forceReboot = rebootOverrides.find((override) => override.action === 'force_reboot');
  if (input.device.rebootRequired && !hasManualInstallOnly) {
    const deferMs = parseDateMs(deferReboot?.deferUntil ?? null);
    const rebootWindowOpen = isWithinPatchWindow(policyConfig.windows.reboot, new Date(nowMs));
    const serverManualOnly = input.device.deviceType === 'server' && policyConfig.reboot.serverBehavior === 'manual_only';
    if (deferReboot && (deferMs === null || deferMs > nowMs)) {
      actions.push(action('defer', deferReboot.reason || 'Reboot is deferred by override.', {
        window: 'reboot',
        notBefore: deferMs ? new Date(deferMs).toISOString() : null,
        metadata: { overrideId: deferReboot.id }
      }));
    } else if (forceReboot || (!serverManualOnly && policyConfig.reboot.allowAutomaticReboot && rebootWindowOpen)) {
      actions.push(action('reboot', forceReboot ? 'Manual reboot override requested.' : 'Reboot window is open.', {
        operationId: forceReboot?.operationId ?? undefined,
        window: 'reboot',
        forced: Boolean(forceReboot),
        metadata: { overrideIds: forceReboot ? [forceReboot.id] : [] }
      }));
    } else {
      actions.push(action('defer', serverManualOnly ? 'Server reboot policy requires manual reboot.' : 'Reboot is pending but outside the reboot window.', {
        window: 'reboot'
      }));
    }
  }

  if (actions.length === (nativeWindowsUpdateControlSupported ? 1 : 0)) {
    actions.push(action('reportOnly', 'No patch action is currently required.'));
  }

  return {
    schemaVersion: 1,
    generatedAt,
    organizationId: input.device.organizationId,
    agentId: input.device.agentId,
    policyId: input.policy?.id ?? null,
    managedMode,
    nativeWindowsUpdateControl,
    nextCheckInAt: new Date(nowMs + policyConfig.checkInIntervalMinutes * 60_000).toISOString(),
    actions: uniqueActions(actions)
  };
}

export function buildUpdateKeyFromParts(title: string, kbArticle?: string | null): string {
  return buildPatchUpdateKey({ title, kbArticle });
}
