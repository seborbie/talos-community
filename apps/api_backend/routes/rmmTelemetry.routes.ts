import crypto from 'crypto';
import { Router } from 'express';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import { decryptSecret } from '../lib/crypto';
import { env } from '../lib/env';
import {
  buildUpdateKeyFromParts,
  classifyPatchCategory
} from '../lib/patchDecisionEngine';
import {
  evaluateAndPersistPatchPlan,
  inferPatchProgressActionType,
  recordPatchActionResult
} from '../lib/patchDecisionService';
import { AuthedRequest, requireAuth } from '../middleware/auth';
import {
  alertRuleMatchesCandidate,
  buildAlertFingerprint,
  highestSeverity,
  normalizeAlertMatchOperator,
  normalizeAlertSeverity,
  normalizeAlertSourceDomain,
  normalizeAlertStatus,
  planAlertLifecycle
} from '../lib/alertLifecycle';
import { dispatchAlertNotifications, normalizeNotificationChannels } from '../lib/alertNotifications';
import { projectPatchProgressBatch } from '../lib/patchProgressPersistence';
import {
  parsePatchProgressBatch,
  PatchProgressValidationError,
  type NormalizedPatchProgress
} from '../lib/patchProgressProjection';
import {
  persistedRemediationMetadata,
  remediationDispatchSnapshot
} from '../lib/remediationDispatch';
import {
  parseRemediationStatusReport,
  PATCH_INSTALL_INTENT_ID,
  transitionRemediationStatus,
  type RemediationTransitionResult
} from '../lib/remediationStatusTransitions';
import { remediationDispatchRouter } from './remediationDispatch.routes';

export const rmmTelemetryRouter = Router();
rmmTelemetryRouter.use('/remediation', remediationDispatchRouter);

type JsonObject = Record<string, unknown>;

type PendingPatchSnapshotUpdate = {
  title: string;
  titleNorm: string;
  description: string | null;
  kbArticle: string | null;
  isMandatory: boolean | null;
  sizeBytes: bigint | null;
  requiresReboot: boolean | null;
};

type NormalizedEvent = {
  eventId: string;
  occurredAt: Date;
  receivedAt: Date;
  eventType: string;
  severity: string;
  source: string;
  serviceName: string | null;
  processName: string | null;
  code: string | null;
  message: string | null;
  attributes: JsonObject;
};

function asRecord(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return null;
  }
  return value as JsonObject;
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function valueAtPath(value: unknown, path: string[]): unknown {
  let current: unknown = value;
  for (const part of path) {
    const record = asRecord(current);
    if (!record) return undefined;
    current = record[part];
  }
  return current;
}

function arrayAtAnyPath(value: unknown, paths: string[][]): unknown[] {
  for (const path of paths) {
    const candidate = valueAtPath(value, path);
    if (Array.isArray(candidate)) return candidate;
  }
  return [];
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

function readBoolean(value: unknown): boolean | null {
  if (typeof value === 'boolean') return value;
  return null;
}

function readFlexibleBoolean(...values: unknown[]): boolean | null {
  for (const value of values) {
    if (typeof value === 'boolean') return value;
    if (typeof value === 'number' && Number.isFinite(value)) {
      if (value === 1) return true;
      if (value === 0) return false;
    }
    if (typeof value === 'string' && value.trim()) {
      const normalized = value.trim().toLowerCase();
      if (['1', 'true', 'yes', 'enabled', 'running'].includes(normalized)) return true;
      if (['0', 'false', 'no', 'disabled', 'stopped'].includes(normalized)) return false;
    }
  }
  return null;
}

function parseDate(value: unknown): Date | null {
  if (value instanceof Date && !Number.isNaN(value.getTime())) return value;
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (!trimmed) return null;
  const parsed = new Date(trimmed);
  if (Number.isNaN(parsed.getTime())) return null;
  return parsed;
}

function parseInteger(value: unknown): number | null {
  if (typeof value === 'number' && Number.isInteger(value)) return value;
  if (typeof value === 'string') {
    const parsed = Number(value);
    if (Number.isInteger(parsed)) return parsed;
  }
  return null;
}

function parseNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string') {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
}

function parseBigIntValue(value: unknown): bigint | null {
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value)) {
    return BigInt(value);
  }
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed) return null;
    try {
      return BigInt(trimmed);
    } catch {
      return null;
    }
  }
  return null;
}

function iso(value: unknown): string | null {
  if (value instanceof Date) return value.toISOString();
  const parsed = parseDate(value);
  return parsed ? parsed.toISOString() : null;
}

function parsePositiveInt(value: unknown, fallback: number, min = 1, max = 2000): number {
  const parsed = parseInteger(value);
  if (parsed === null) return fallback;
  return Math.min(Math.max(parsed, min), max);
}

function patchRunningJobStaleMinutes(): number {
  return parsePositiveInt(process.env.RMM_PATCH_RUNNING_JOB_STALE_MINUTES, 360, 60, 1440);
}

async function failStalePatchJobsForAgent(agentId: string) {
  const staleMinutes = patchRunningJobStaleMinutes();
  const staleError = `Patch job marked failed after being stale for ${staleMinutes} minutes.`;
  const jobs = await prisma.$queryRaw<Array<{ id: bigint; commandId: string | null }>>(Prisma.sql`
    UPDATE rmm_telemetry.remediation_job
    SET status = 'failed',
        finished_at = NOW()
    WHERE agent_id = ${agentId}
      AND intent_id = ${PATCH_INSTALL_INTENT_ID}
      AND status = 'running'
      AND COALESCE(started_at, requested_at) < NOW() - (${staleMinutes}::int * INTERVAL '1 minute')
    RETURNING id, command_id AS "commandId"
  `);
  if (jobs.length === 0) return;

  const jobIds = jobs.map((job) => job.id);
  const commandIds = jobs.map((job) => job.commandId).filter((value): value is string => Boolean(value));
  const operationIds = [...jobIds.map((id) => id.toString()), ...commandIds];
  await prisma.$executeRaw(Prisma.sql`
    UPDATE public.rmm_patch_action
    SET status = 'failed',
        phase = 'stale',
        error_message = ${staleError},
        finished_at = COALESCE(finished_at, NOW()),
        updated_at = NOW()
    WHERE status NOT IN ('completed', 'failed', 'cancelled')
      AND (
        remediation_job_id IN (${Prisma.join(jobIds)})
        ${commandIds.length > 0 ? Prisma.sql`OR remediation_command_id IN (${Prisma.join(commandIds)})` : Prisma.empty}
        OR operation_id IN (${Prisma.join(operationIds)})
      )
  `);
}

function parseBooleanFlag(value: unknown): boolean {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') {
    const normalized = value.trim().toLowerCase();
    return normalized === '1' || normalized === 'true' || normalized === 'yes';
  }
  return false;
}

function toJsonString(value: unknown): string {
  return JSON.stringify(value === undefined ? null : value);
}

function sha256Hex(value: string): string {
  return crypto.createHash('sha256').update(value).digest('hex');
}

type BaselineScopeType = 'organization' | 'customer' | 'site' | 'device';
type ScopedBaselineWriteType = Exclude<BaselineScopeType, 'device'>;

type MembershipWithOrg = NonNullable<Awaited<ReturnType<typeof getCurrentMembership>>>;
type RoutingAction = 'ignore' | 'ticket' | 'recommend' | 'auto_remediate' | 'llm_router';
type RoutingMatchOperator =
  | 'equals'
  | 'not_equals'
  | 'contains'
  | 'not_contains'
  | 'starts_with'
  | 'ends_with'
  | 'exists';

const SCOPE_BASELINE_MIN_DEVICES = Math.max(
  1,
  Number.parseInt(process.env.RMM_TELEMETRY_SCOPE_BASELINE_MIN_DEVICES || '2', 10) || 2
);
const SCOPE_BASELINE_MIN_SUPPORT_RATIO = (() => {
  const parsed = Number.parseFloat(process.env.RMM_TELEMETRY_SCOPE_BASELINE_MIN_SUPPORT_RATIO || '0.7');
  if (!Number.isFinite(parsed)) return 0.7;
  return Math.min(Math.max(parsed, 0), 1);
})();
const ROUTING_ALLOWED_ACTIONS = new Set<RoutingAction>([
  'ignore',
  'ticket',
  'recommend',
  'auto_remediate',
  'llm_router'
]);
const ROUTING_ALLOWED_OPERATORS = new Set<RoutingMatchOperator>([
  'equals',
  'not_equals',
  'contains',
  'not_contains',
  'starts_with',
  'ends_with',
  'exists'
]);
const LLM_ROUTER_ENABLED = parseBooleanFlag(process.env.RMM_TELEMETRY_ENABLE_LLM_ROUTER);

function normalizeScopeType(value: unknown): BaselineScopeType | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  if (normalized === 'organization' || normalized === 'customer' || normalized === 'site' || normalized === 'device') {
    return normalized;
  }
  return null;
}

function stableJsonValueKey(value: unknown): string {
  return toJsonString(value);
}

function readBigIntString(value: unknown): string | null {
  const parsed = parseBigIntValue(value);
  return parsed === null ? null : parsed.toString();
}

type StabilityOverrideRow = {
  id: bigint;
  factKeyPattern: string;
  stabilityClass: string;
  reason: string | null;
  createdBy: string;
  createdAt: Date;
  updatedAt: Date;
};

type FactKeyImpactRow = {
  factKey: string;
  currentFactCount: number;
  scopedBaselineCount: number;
  latestSeenAt: string | null;
};

type FactTrustMetadata = {
  effectiveStabilityClass: string | null;
  baselineEligible: boolean;
  promotionState: string;
  overrideMatched: boolean;
  overrideId: string | null;
  overridePattern: string | null;
  overrideStabilityClass: string | null;
  overrideReason: string | null;
  trustWarnings: string[];
};

function normalizeStabilityClass(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  return normalized ? normalized : null;
}

function overrideSpecificity(pattern: string): number {
  return Array.from(pattern).filter((ch) => ch !== '*' && ch !== '?').length;
}

function wildcardMatchChars(pattern: string[], value: string[]): boolean {
  let patIdx = 0;
  let valIdx = 0;
  let starIdx: number | null = null;
  let matchIdx = 0;

  while (valIdx < value.length) {
    if (patIdx < pattern.length && (pattern[patIdx] === '?' || pattern[patIdx] === value[valIdx])) {
      patIdx += 1;
      valIdx += 1;
      continue;
    }
    if (patIdx < pattern.length && pattern[patIdx] === '*') {
      starIdx = patIdx;
      matchIdx = valIdx;
      patIdx += 1;
      continue;
    }
    if (starIdx !== null) {
      patIdx = starIdx + 1;
      matchIdx += 1;
      valIdx = matchIdx;
      continue;
    }
    return false;
  }

  while (patIdx < pattern.length && pattern[patIdx] === '*') {
    patIdx += 1;
  }

  return patIdx === pattern.length;
}

function wildcardPatternMatches(pattern: string, value: string): boolean {
  return wildcardMatchChars(
    Array.from(pattern.toLowerCase()),
    Array.from(value.toLowerCase())
  );
}

function findBestStabilityOverride(
  factKey: string,
  overrides: StabilityOverrideRow[]
): StabilityOverrideRow | null {
  let bestMatch: StabilityOverrideRow | null = null;
  let bestSpecificity = -1;

  for (const override of overrides) {
    if (!wildcardPatternMatches(override.factKeyPattern, factKey)) continue;
    const specificity = overrideSpecificity(override.factKeyPattern);
    if (specificity > bestSpecificity) {
      bestMatch = override;
      bestSpecificity = specificity;
    }
  }

  return bestMatch;
}

function buildTrustWarnings(options: {
  scopeType: BaselineScopeType;
  isStable: boolean;
  sampleSize?: number | null;
  supportRatio?: number | null;
  promotionState: string;
  baselineEligible: boolean;
}): string[] {
  const warnings: string[] = [];

  if (!options.baselineEligible) {
    warnings.push('not_baseline_eligible');
  }
  if (options.promotionState === 'suppressed_by_override') {
    warnings.push('ignored_by_override');
  } else if (options.promotionState === 'tracked_as_noisy') {
    warnings.push('overridden_noisy');
  } else if (!options.isStable) {
    warnings.push('pending_promotion');
  }

  if (options.scopeType !== 'device') {
    if (typeof options.sampleSize === 'number' && options.sampleSize > 0 && options.sampleSize < SCOPE_BASELINE_MIN_DEVICES) {
      warnings.push('low_sample_size');
    }
    if (typeof options.supportRatio === 'number' && options.supportRatio > 0 && options.supportRatio < SCOPE_BASELINE_MIN_SUPPORT_RATIO) {
      warnings.push('low_support_ratio');
    }
  }

  return warnings;
}

function buildBaselineTrustMetadata(options: {
  scopeType: BaselineScopeType;
  factKey: string;
  overrides: StabilityOverrideRow[];
  promotedValue: unknown;
  candidateCount: number;
  isStable: boolean;
  currentStabilityClass?: string | null;
  sampleSize?: number | null;
  supportRatio?: number | null;
}): FactTrustMetadata {
  const override = findBestStabilityOverride(options.factKey, options.overrides);
  const overrideStabilityClass = normalizeStabilityClass(override?.stabilityClass);
  const currentStabilityClass = normalizeStabilityClass(options.currentStabilityClass);
  const effectiveStabilityClass = overrideStabilityClass ?? currentStabilityClass;

  let promotionState = 'pending';
  if (effectiveStabilityClass === 'ignored') {
    promotionState = 'suppressed_by_override';
  } else if (effectiveStabilityClass === 'noisy') {
    promotionState = 'tracked_as_noisy';
  } else if (options.promotedValue !== null && options.promotedValue !== undefined) {
    promotionState = 'stable_baseline';
  } else if (options.candidateCount > 0) {
    promotionState = 'candidate';
  }

  const baselineEligible = effectiveStabilityClass
    ? effectiveStabilityClass === 'stable'
    : promotionState === 'stable_baseline' || promotionState === 'candidate' || options.isStable;

  return {
    effectiveStabilityClass,
    baselineEligible,
    promotionState,
    overrideMatched: Boolean(override),
    overrideId: override ? override.id.toString() : null,
    overridePattern: override?.factKeyPattern ?? null,
    overrideStabilityClass,
    overrideReason: override?.reason ?? null,
    trustWarnings: buildTrustWarnings({
      scopeType: options.scopeType,
      isStable: options.isStable,
      sampleSize: options.sampleSize,
      supportRatio: options.supportRatio,
      promotionState,
      baselineEligible
    })
  };
}

function buildDriftTrustMetadata(options: {
  scopeType: Exclude<BaselineScopeType, 'device'>;
  factKey: string;
  overrides: StabilityOverrideRow[];
  scopeSampleSize?: number | null;
  scopeSupportRatio?: number | null;
  scopeIsStable: boolean;
  deviceValue: unknown;
}): FactTrustMetadata {
  const override = findBestStabilityOverride(options.factKey, options.overrides);
  const overrideStabilityClass = normalizeStabilityClass(override?.stabilityClass);

  let promotionState = 'drifting_from_scope';
  if (overrideStabilityClass === 'ignored') {
    promotionState = 'suppressed_by_override';
  } else if (overrideStabilityClass === 'noisy') {
    promotionState = 'tracked_as_noisy';
  } else if (options.deviceValue === null || options.deviceValue === undefined) {
    promotionState = 'missing_device_baseline';
  }

  const baselineEligible = overrideStabilityClass
    ? overrideStabilityClass === 'stable'
    : true;

  return {
    effectiveStabilityClass: overrideStabilityClass,
    baselineEligible,
    promotionState,
    overrideMatched: Boolean(override),
    overrideId: override ? override.id.toString() : null,
    overridePattern: override?.factKeyPattern ?? null,
    overrideStabilityClass,
    overrideReason: override?.reason ?? null,
    trustWarnings: buildTrustWarnings({
      scopeType: options.scopeType,
      isStable: options.scopeIsStable,
      sampleSize: options.scopeSampleSize,
      supportRatio: options.scopeSupportRatio,
      promotionState,
      baselineEligible
    })
  };
}

async function loadStabilityOverridesForOrganization(organizationId: string): Promise<StabilityOverrideRow[]> {
  return prisma.rmmTelemetryFactStabilityOverride.findMany({
    where: { organizationId },
    orderBy: { factKeyPattern: 'asc' }
  });
}

function buildPatternHint(pattern: string): string | null {
  const fragments = pattern
    .split(/[\*\?]+/)
    .map((fragment) => fragment.trim())
    .filter(Boolean)
    .sort((a, b) => b.length - a.length);

  return fragments[0]?.length ? fragments[0] : null;
}

async function loadFactKeyImpactRows(
  organizationId: string,
  patternHint?: string | null
): Promise<FactKeyImpactRow[]> {
  const currentHint = patternHint ? Prisma.sql`AND fact_key ILIKE ${`%${patternHint}%`}` : Prisma.empty;
  const [currentRows, scopedRows] = await Promise.all([
    prisma.$queryRaw<Array<{ fact_key: string; current_fact_count: bigint; latest_seen_at: Date | null }>>(Prisma.sql`
      SELECT
        fact_key,
        COUNT(*)::bigint AS current_fact_count,
        MAX(updated_at) AS latest_seen_at
      FROM rmm_telemetry.fact_state_current
      WHERE organization_id = ${organizationId}
        ${currentHint}
      GROUP BY fact_key
    `),
    prisma.$queryRaw<Array<{ fact_key: string; scoped_baseline_count: bigint }>>(Prisma.sql`
      SELECT
        fact_key,
        COUNT(*)::bigint AS scoped_baseline_count
      FROM rmm_telemetry.fact_baseline_scope
      WHERE organization_id = ${organizationId}
        ${currentHint}
      GROUP BY fact_key
    `)
  ]);

  const byFactKey = new Map<string, FactKeyImpactRow>();
  for (const row of currentRows) {
    byFactKey.set(row.fact_key, {
      factKey: row.fact_key,
      currentFactCount: Number(row.current_fact_count ?? 0n),
      scopedBaselineCount: 0,
      latestSeenAt: iso(row.latest_seen_at)
    });
  }
  for (const row of scopedRows) {
    const existing = byFactKey.get(row.fact_key);
    if (existing) {
      existing.scopedBaselineCount = Number(row.scoped_baseline_count ?? 0n);
      continue;
    }
    byFactKey.set(row.fact_key, {
      factKey: row.fact_key,
      currentFactCount: 0,
      scopedBaselineCount: Number(row.scoped_baseline_count ?? 0n),
      latestSeenAt: null
    });
  }

  return Array.from(byFactKey.values());
}

function summarizePatternImpact(
  pattern: string,
  impactRows: FactKeyImpactRow[],
  limit = 8
) {
  const matches = impactRows
    .filter((row) => wildcardPatternMatches(pattern, row.factKey))
    .sort((left, right) => {
      if (right.currentFactCount !== left.currentFactCount) {
        return right.currentFactCount - left.currentFactCount;
      }
      if (right.scopedBaselineCount !== left.scopedBaselineCount) {
        return right.scopedBaselineCount - left.scopedBaselineCount;
      }
      return left.factKey.localeCompare(right.factKey);
    });

  return {
    matchedFactKeyCount: matches.length,
    matchedCurrentFactCount: matches.reduce((sum, row) => sum + row.currentFactCount, 0),
    matchedScopedBaselineCount: matches.reduce((sum, row) => sum + row.scopedBaselineCount, 0),
    sampleFactKeys: matches.slice(0, limit).map((row) => row.factKey),
    items: matches.slice(0, limit)
  };
}

type RoutingRuleRow = {
  id: bigint;
  organization_id: string;
  customer_id: string | null;
  site_id: string | null;
  agent_id: string | null;
  trigger_domain: string;
  trigger_key: string;
  match_operator: string;
  match_value: string | null;
  previous_match_operator: string | null;
  previous_match_value: string | null;
  min_support_ratio: number | null;
  min_confidence_score: number | null;
  scope_type_filter: string | null;
  action: string;
  intent_id: string | null;
  cooldown_seconds: number;
  enabled: boolean;
  priority: number;
  created_at?: Date;
  updated_at?: Date;
};

type RoutingDecisionRow = {
  id: bigint;
  organization_id: string;
  agent_id: string;
  domain: string;
  trigger_key: string;
  trigger_value: unknown;
  action: string;
  matched_rule_id: bigint | null;
  intent_id: string | null;
  reason: string | null;
  dedupe_key: string | null;
  source: string;
  source_ts: Date;
  decided_at: Date;
  execution_status: string;
  external_ref: string | null;
  outcome_message: string | null;
};

type RoutingIntentSummary = {
  id: string;
  name: string;
  enabled: boolean;
  requires_approval: boolean;
  steps: unknown;
  allow_list: unknown;
  max_retries: number;
  timeout_seconds: number;
  trigger_domain: string | null;
  trigger_key: string | null;
};

type HaloProviderStatus = {
  provider: 'halo';
  ready: boolean;
  baseUrl: string | null;
  clientId: string | null;
  clientSecret: string | null;
  reason: string | null;
};

type RoutingCandidate = {
  domain: string;
  triggerKey: string;
  currentValue: unknown;
  currentValueText: string;
  previousValue: unknown;
  previousValueText: string | null;
  supportRatio: number | null;
  confidenceScore: number | null;
  scopeType: BaselineScopeType | null;
  organizationId: string | null;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
};

type RoutingRuleInput = {
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  triggerDomain: 'baseline' | 'scope_drift' | 'event';
  triggerKey: string;
  matchOperator: RoutingMatchOperator;
  matchValue: string | null;
  previousMatchOperator: RoutingMatchOperator | null;
  previousMatchValue: string | null;
  minSupportRatio: number | null;
  minConfidenceScore: number | null;
  scopeTypeFilter: BaselineScopeType | null;
  action: RoutingAction;
  intentId: string | null;
  cooldownSeconds: number;
  enabled: boolean;
  priority: number;
};

type RoutingRuleReadModel = {
  id: string;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  triggerDomain: string;
  triggerKey: string;
  matchOperator: string;
  matchValue: string | null;
  previousMatchOperator: string | null;
  previousMatchValue: string | null;
  minSupportRatio: number | null;
  minConfidenceScore: number | null;
  scopeTypeFilter: string | null;
  action: string;
  intentId: string | null;
  cooldownSeconds: number;
  enabled: boolean;
  priority: number;
  createdAt: string | null;
  updatedAt: string | null;
  specificity: 'agent' | 'site' | 'customer' | 'organization';
  blockedReasons: string[];
  readiness: {
    intentReady: boolean;
    intentRequiresApproval: boolean | null;
    ticketProviderReady: boolean;
    llmRouterEnabled: boolean;
  };
};

type AlertRuleRow = {
  id: bigint;
  organization_id: string;
  customer_id: string | null;
  site_id: string | null;
  agent_id: string | null;
  name: string;
  trigger_domain: string;
  trigger_key: string;
  match_operator: string;
  match_value: string | null;
  severity: string;
  min_severity: string | null;
  dedupe_window_seconds: number;
  enabled: boolean;
  priority: number;
  notification_channels_jsonb: unknown;
  created_by: string;
  created_at: Date;
  updated_at: Date;
};

type AlertRow = {
  id: bigint;
  organization_id: string;
  customer_id: string | null;
  site_id: string | null;
  agent_id: string;
  rule_id: bigint | null;
  status: string;
  severity: string;
  source_domain: string;
  source_key: string;
  source_event_id: string | null;
  source_fact_key: string | null;
  source_decision_id: bigint | null;
  title: string;
  summary: string | null;
  fingerprint: string;
  first_seen_at: Date;
  last_seen_at: Date;
  occurrence_count: number;
  owner_user_id: string | null;
  owner_email?: string | null;
  acknowledged_by: string | null;
  acknowledged_at: Date | null;
  snoozed_until: Date | null;
  resolved_by: string | null;
  resolved_at: Date | null;
  suppressed_until: Date | null;
  metadata_jsonb: unknown;
  created_at: Date;
  updated_at: Date;
  hostname?: string | null;
  customer_name?: string | null;
  site_name?: string | null;
};

type AlertCandidate = {
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string;
  domain: 'event' | 'baseline' | 'scope_drift' | 'decision';
  triggerKey: string;
  valueText: string;
  severity: string;
  sourceEventId?: string | null;
  sourceFactKey?: string | null;
  sourceDecisionId?: bigint | null;
  title: string;
  summary: string | null;
  metadata: JsonObject;
};

function normalizeRoutingDomain(value: unknown): 'baseline' | 'scope_drift' | 'event' | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  if (normalized === 'baseline' || normalized === 'scope_drift' || normalized === 'event') {
    return normalized;
  }
  return null;
}

function normalizeRoutingAction(value: unknown): RoutingAction | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase().replace(/-/g, '_');
  if (normalized === 'auto_remediate' || normalized === 'autoremediate') {
    return 'auto_remediate';
  }
  if (ROUTING_ALLOWED_ACTIONS.has(normalized as RoutingAction)) {
    return normalized as RoutingAction;
  }
  return null;
}

function normalizeRoutingOperator(
  value: unknown,
  fallback: RoutingMatchOperator = 'equals'
): RoutingMatchOperator | null {
  if (value === null || value === undefined || value === '') return fallback;
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase() as RoutingMatchOperator;
  return ROUTING_ALLOWED_OPERATORS.has(normalized) ? normalized : null;
}

function toRoutingValueText(value: unknown): string {
  if (typeof value === 'string') return value;
  return toJsonString(value);
}

function routingRuleSpecificity(rule: Pick<RoutingRuleRow, 'agent_id' | 'site_id' | 'customer_id'>): 'agent' | 'site' | 'customer' | 'organization' {
  if (rule.agent_id) return 'agent';
  if (rule.site_id) return 'site';
  if (rule.customer_id) return 'customer';
  return 'organization';
}

function routingRuleAppliesToCandidate(
  rule: Pick<RoutingRuleRow, 'organization_id' | 'customer_id' | 'site_id' | 'agent_id'>,
  candidate: RoutingCandidate
): boolean {
  if (candidate.organizationId && rule.organization_id !== candidate.organizationId) {
    return false;
  }
  if (rule.customer_id && rule.customer_id !== candidate.customerId) {
    return false;
  }
  if (rule.site_id && rule.site_id !== candidate.siteId) {
    return false;
  }
  if (rule.agent_id && rule.agent_id !== candidate.agentId) {
    return false;
  }
  return true;
}

function routingOperatorMatches(
  operator: RoutingMatchOperator,
  expected: string | null,
  candidate: string
): boolean {
  const desired = expected?.trim() ?? '';
  switch (operator) {
    case 'exists':
      return candidate.trim().length > 0;
    case 'contains':
      return desired.length > 0 && candidate.includes(desired);
    case 'not_contains':
      return desired.length > 0 && !candidate.includes(desired);
    case 'starts_with':
      return desired.length > 0 && candidate.startsWith(desired);
    case 'ends_with':
      return desired.length > 0 && candidate.endsWith(desired);
    case 'not_equals':
      return desired.length > 0 && candidate !== desired;
    case 'equals':
    default:
      return desired.length > 0 ? candidate === desired : true;
  }
}

function buildRoutingDedupeKey(
  candidate: RoutingCandidate,
  rule: Pick<RoutingRuleRow, 'id' | 'action'>,
  action: RoutingAction
): string {
  return sha256Hex([
    candidate.agentId || 'none',
    String(rule.id),
    candidate.domain,
    candidate.triggerKey,
    candidate.currentValueText,
    candidate.previousValueText || 'none',
    action
  ].join('|'));
}

async function loadRoutingIntentSummary(
  organizationId: string,
  intentId: string | null
): Promise<RoutingIntentSummary | null> {
  if (!intentId) return null;
  const rows = await prisma.$queryRaw<RoutingIntentSummary[]>(Prisma.sql`
    SELECT
      id,
      name,
      enabled,
      requires_approval,
      steps,
      allow_list,
      max_retries,
      timeout_seconds,
      trigger_domain,
      trigger_key
    FROM rmm_telemetry.intent
    WHERE organization_id = ${organizationId}
      AND id = ${intentId}
    LIMIT 1
  `);
  return rows[0] ?? null;
}

async function loadHaloProviderStatus(organizationId: string): Promise<HaloProviderStatus> {
  const org = await prisma.organization.findUnique({
    where: { id: organizationId },
    select: {
      haloBaseUrlEnc: true,
      haloClientIdEnc: true,
      haloClientSecretEnc: true
    } as any
  });
  const baseUrl = decryptSecret((org as any)?.haloBaseUrlEnc) || null;
  const clientId = decryptSecret((org as any)?.haloClientIdEnc) || null;
  const clientSecret = decryptSecret((org as any)?.haloClientSecretEnc) || null;
  const ready = Boolean(baseUrl && clientId && clientSecret);
  return {
    provider: 'halo',
    ready,
    baseUrl,
    clientId,
    clientSecret,
    reason: ready ? null : 'halo_provider_not_configured'
  };
}

async function validateRoutingSelectors(
  organizationId: string,
  selectors: { customerId: string | null; siteId: string | null; agentId: string | null }
): Promise<string[]> {
  const [customer, site, device] = await Promise.all([
    selectors.customerId
      ? prisma.customer.findUnique({
          where: { id: selectors.customerId },
          select: { id: true, organizationId: true }
        })
      : Promise.resolve(null),
    selectors.siteId
      ? prisma.rmmSite.findFirst({
          where: { id: selectors.siteId },
          select: {
            id: true,
            customerId: true,
            customer: {
              select: { organizationId: true }
            }
          }
        })
      : Promise.resolve(null),
    selectors.agentId
      ? prisma.rmmDevice.findUnique({
          where: { agentId: selectors.agentId },
          select: { agentId: true, customerId: true, siteId: true, organizationId: true }
        })
      : Promise.resolve(null)
  ]);

  const errors: string[] = [];

  if (selectors.customerId && (!customer || customer.organizationId !== organizationId)) {
    errors.push('customerId does not belong to this organization');
  }
  if (selectors.siteId && (!site || site.customer?.organizationId !== organizationId)) {
    errors.push('siteId does not belong to this organization');
  }
  if (selectors.agentId && (!device || device.organizationId !== organizationId)) {
    errors.push('agentId does not belong to this organization');
  }
  if (customer && site && site.customerId !== customer.id) {
    errors.push('siteId does not belong to customerId');
  }
  if (device && site && device.siteId !== site.id) {
    errors.push('agentId does not belong to siteId');
  }
  if (device && customer && device.customerId !== customer.id) {
    errors.push('agentId does not belong to customerId');
  }

  return errors;
}

async function loadRoutingRuleRow(
  organizationId: string,
  id: bigint
): Promise<RoutingRuleRow | null> {
  const rows = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
    FROM rmm_telemetry.routing_rule
    WHERE organization_id = ${organizationId}
      AND id = ${id}
    LIMIT 1
  `);
  return rows[0] ?? null;
}

async function loadRoutingRulesForOrganization(
  organizationId: string,
  options?: {
    enabled?: boolean | null;
    triggerDomain?: string | null;
    action?: string | null;
  }
): Promise<RoutingRuleRow[]> {
  const clauses: Prisma.Sql[] = [Prisma.sql`organization_id = ${organizationId}`];
  if (typeof options?.enabled === 'boolean') {
    clauses.push(Prisma.sql`enabled = ${options.enabled}`);
  }
  const triggerDomain = normalizeRoutingDomain(options?.triggerDomain);
  if (triggerDomain) {
    clauses.push(Prisma.sql`trigger_domain = ${triggerDomain}`);
  }
  const action = normalizeRoutingAction(options?.action);
  if (action) {
    clauses.push(Prisma.sql`action = ${action}`);
  }

  const whereClause = clauses.reduce<Prisma.Sql>(
    (sql, clause, index) => index === 0 ? clause : Prisma.sql`${sql} AND ${clause}`,
    Prisma.sql`TRUE`
  );

  return prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
    FROM rmm_telemetry.routing_rule
    WHERE ${whereClause}
    ORDER BY
      CASE
        WHEN agent_id IS NOT NULL THEN 1
        WHEN site_id IS NOT NULL THEN 2
        WHEN customer_id IS NOT NULL THEN 3
        ELSE 4
      END ASC,
      priority ASC,
      id ASC
  `);
}

async function evaluateRoutingRuleReadiness(
  organizationId: string,
  rule: Pick<RoutingRuleInput, 'action' | 'intentId' | 'enabled'>
): Promise<{
  blockedReasons: string[];
  intent: RoutingIntentSummary | null;
  halo: HaloProviderStatus;
}> {
  const blockedReasons: string[] = [];
  const halo = await loadHaloProviderStatus(organizationId);
  const intent = await loadRoutingIntentSummary(organizationId, rule.intentId);

  if (rule.action === 'recommend' || rule.action === 'auto_remediate') {
    if (!rule.intentId) {
      blockedReasons.push('intent_required');
    } else if (!intent) {
      blockedReasons.push('intent_not_found');
    } else if (!intent.enabled) {
      blockedReasons.push('intent_disabled');
    }
  }

  if (rule.action === 'ticket' && rule.enabled && !halo.ready) {
    blockedReasons.push('ticket_provider_not_ready');
  }

  if (rule.action === 'llm_router' && !LLM_ROUTER_ENABLED) {
    blockedReasons.push('llm_router_disabled');
  }

  return { blockedReasons, intent, halo };
}

async function parseRoutingRuleInput(
  organizationId: string,
  body: JsonObject,
  existing?: RoutingRuleRow | null
): Promise<{ rule: RoutingRuleInput | null; errors: string[]; blockedReasons: string[]; intent: RoutingIntentSummary | null; halo: HaloProviderStatus; }> {
  const triggerDomain = normalizeRoutingDomain(
    body.triggerDomain ?? body.trigger_domain ?? existing?.trigger_domain
  );
  const triggerKey = readString(body.triggerKey, body.trigger_key, existing?.trigger_key);
  const matchOperator = normalizeRoutingOperator(
    body.matchOperator ?? body.match_operator ?? existing?.match_operator,
    'equals'
  );
  const matchValue = readString(body.matchValue, body.match_value, existing?.match_value);
  const previousMatchOperatorRaw = body.previousMatchOperator ?? body.previous_match_operator ?? existing?.previous_match_operator ?? null;
  const previousMatchOperator = previousMatchOperatorRaw === null
    ? null
    : normalizeRoutingOperator(previousMatchOperatorRaw, 'equals');
  const previousMatchValue = readString(
    body.previousMatchValue,
    body.previous_match_value,
    existing?.previous_match_value
  );
  const minSupportRatioRaw = body.minSupportRatio ?? body.min_support_ratio ?? existing?.min_support_ratio;
  const minSupportRatio = minSupportRatioRaw === null || minSupportRatioRaw === undefined || minSupportRatioRaw === ''
    ? null
    : parseNumber(minSupportRatioRaw);
  const minConfidenceScoreRaw = body.minConfidenceScore ?? body.min_confidence_score ?? existing?.min_confidence_score;
  const minConfidenceScore = minConfidenceScoreRaw === null || minConfidenceScoreRaw === undefined || minConfidenceScoreRaw === ''
    ? null
    : parseNumber(minConfidenceScoreRaw);
  const scopeTypeFilterRaw = body.scopeTypeFilter ?? body.scope_type_filter ?? existing?.scope_type_filter ?? null;
  const scopeTypeFilter = scopeTypeFilterRaw === null || scopeTypeFilterRaw === ''
    ? null
    : normalizeScopeType(scopeTypeFilterRaw);
  const action = normalizeRoutingAction(body.action ?? existing?.action);
  const intentId = readString(body.intentId, body.intent_id, existing?.intent_id);
  const customerId = readString(body.customerId, body.customer_id, existing?.customer_id);
  const siteId = readString(body.siteId, body.site_id, existing?.site_id);
  const agentId = readString(body.agentId, body.agent_id, existing?.agent_id);
  const cooldownSeconds = parseInteger(body.cooldownSeconds ?? body.cooldown_seconds ?? existing?.cooldown_seconds) ?? 0;
  const priority = parseInteger(body.priority ?? existing?.priority) ?? 100;
  const enabled = readBoolean(body.enabled) ?? existing?.enabled ?? true;

  const errors: string[] = [];

  if (!triggerDomain) errors.push('triggerDomain must be baseline, scope_drift, or event');
  if (!triggerKey) errors.push('triggerKey is required');
  if (!matchOperator) errors.push('matchOperator is invalid');
  if (previousMatchOperatorRaw !== null && previousMatchOperatorRaw !== undefined && !previousMatchOperator) {
    errors.push('previousMatchOperator is invalid');
  }
  if (minSupportRatio !== null && (minSupportRatio < 0 || minSupportRatio > 1)) {
    errors.push('minSupportRatio must be between 0 and 1');
  }
  if (minConfidenceScore !== null && minConfidenceScore < 0) {
    errors.push('minConfidenceScore must be greater than or equal to 0');
  }
  if (scopeTypeFilterRaw !== null && scopeTypeFilterRaw !== undefined && scopeTypeFilterRaw !== '' && !scopeTypeFilter) {
    errors.push('scopeTypeFilter must be organization, customer, site, or device');
  }
  if (!action) errors.push('action is invalid');
  if (cooldownSeconds < 0) errors.push('cooldownSeconds must be greater than or equal to 0');
  if (priority < 0) errors.push('priority must be greater than or equal to 0');

  errors.push(...await validateRoutingSelectors(organizationId, { customerId: customerId ?? null, siteId: siteId ?? null, agentId: agentId ?? null }));

  if (errors.length > 0 || !triggerDomain || !triggerKey || !matchOperator || !action) {
    const halo = await loadHaloProviderStatus(organizationId);
    return { rule: null, errors, blockedReasons: [], intent: null, halo };
  }

  const rule: RoutingRuleInput = {
    organizationId,
    customerId: customerId ?? null,
    siteId: siteId ?? null,
    agentId: agentId ?? null,
    triggerDomain,
    triggerKey,
    matchOperator,
    matchValue,
    previousMatchOperator,
    previousMatchValue,
    minSupportRatio,
    minConfidenceScore,
    scopeTypeFilter,
    action,
    intentId: action === 'recommend' || action === 'auto_remediate' ? intentId ?? null : null,
    cooldownSeconds,
    enabled,
    priority
  };

  const { blockedReasons, intent, halo } = await evaluateRoutingRuleReadiness(organizationId, rule);
  return { rule, errors, blockedReasons, intent, halo };
}

function buildRoutingCandidate(body: JsonObject, defaults?: Partial<RoutingCandidate>): RoutingCandidate | null {
  const domain = normalizeRoutingDomain(body.domain ?? defaults?.domain);
  const triggerKey = readString(body.triggerKey, body.trigger_key, defaults?.triggerKey);
  if (!domain || !triggerKey) return null;

  const currentValue = Object.prototype.hasOwnProperty.call(body, 'currentValue')
    ? body.currentValue
    : Object.prototype.hasOwnProperty.call(body, 'current_value')
      ? body.current_value
      : defaults?.currentValue ?? null;
  const previousValue = Object.prototype.hasOwnProperty.call(body, 'previousValue')
    ? body.previousValue
    : Object.prototype.hasOwnProperty.call(body, 'previous_value')
      ? body.previous_value
      : defaults?.previousValue ?? null;
  const currentValueText = readString(body.currentValueText, body.current_value_text) ?? toRoutingValueText(currentValue);
  const previousValueText = readString(body.previousValueText, body.previous_value_text) ?? (previousValue === null || previousValue === undefined ? null : toRoutingValueText(previousValue));
  const supportRatioRaw = body.supportRatio ?? body.support_ratio ?? defaults?.supportRatio ?? null;
  const confidenceScoreRaw = body.confidenceScore ?? body.confidence_score ?? defaults?.confidenceScore ?? null;
  const scopeTypeRaw = body.scopeType ?? body.scope_type ?? defaults?.scopeType ?? null;

  return {
    domain,
    triggerKey,
    currentValue,
    currentValueText,
    previousValue,
    previousValueText,
    supportRatio: supportRatioRaw === null || supportRatioRaw === undefined || supportRatioRaw === '' ? null : parseNumber(supportRatioRaw),
    confidenceScore: confidenceScoreRaw === null || confidenceScoreRaw === undefined || confidenceScoreRaw === '' ? null : parseNumber(confidenceScoreRaw),
    scopeType: scopeTypeRaw === null || scopeTypeRaw === undefined || scopeTypeRaw === '' ? null : normalizeScopeType(scopeTypeRaw),
    organizationId: readString(body.organizationId, body.organization_id, defaults?.organizationId),
    customerId: readString(body.customerId, body.customer_id, defaults?.customerId),
    siteId: readString(body.siteId, body.site_id, defaults?.siteId),
    agentId: readString(body.agentId, body.agent_id, defaults?.agentId)
  };
}

function evaluateRoutingRuleMatch(
  rule: RoutingRuleRow,
  candidate: RoutingCandidate
): { matched: boolean; blockedReasons: string[]; explanation: string[]; dedupeKey: string | null } {
  const blockedReasons: string[] = [];
  const explanation: string[] = [];

  if (rule.trigger_domain !== candidate.domain) {
    explanation.push('domain mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (!wildcardPatternMatches(rule.trigger_key, candidate.triggerKey)) {
    explanation.push('triggerKey mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (!routingRuleAppliesToCandidate(rule, candidate)) {
    explanation.push('scope selector mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (rule.scope_type_filter && rule.scope_type_filter !== candidate.scopeType) {
    explanation.push('scopeTypeFilter mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (!routingOperatorMatches(
    normalizeRoutingOperator(rule.match_operator, 'equals') || 'equals',
    rule.match_value,
    candidate.currentValueText
  )) {
    explanation.push('current value mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (rule.previous_match_operator && !routingOperatorMatches(
    normalizeRoutingOperator(rule.previous_match_operator, 'equals') || 'equals',
    rule.previous_match_value,
    candidate.previousValueText || ''
  )) {
    explanation.push('previous value mismatch');
    return { matched: false, blockedReasons, explanation, dedupeKey: null };
  }
  if (rule.min_support_ratio !== null && rule.min_support_ratio !== undefined) {
    if (candidate.supportRatio === null || candidate.supportRatio < rule.min_support_ratio) {
      explanation.push('support ratio below threshold');
      return { matched: false, blockedReasons, explanation, dedupeKey: null };
    }
  }
  if (rule.min_confidence_score !== null && rule.min_confidence_score !== undefined) {
    if (candidate.confidenceScore === null || candidate.confidenceScore < rule.min_confidence_score) {
      explanation.push('confidence score below threshold');
      return { matched: false, blockedReasons, explanation, dedupeKey: null };
    }
  }

  const action = normalizeRoutingAction(rule.action) || 'ignore';
  return {
    matched: true,
    blockedReasons,
    explanation: ['matched'],
    dedupeKey: buildRoutingDedupeKey(candidate, rule, action)
  };
}

function serializeRoutingRule(
  row: RoutingRuleRow,
  readiness: { blockedReasons: string[]; intent: RoutingIntentSummary | null; halo: HaloProviderStatus; }
): RoutingRuleReadModel {
  return {
    id: String(row.id),
    organizationId: row.organization_id,
    customerId: row.customer_id,
    siteId: row.site_id,
    agentId: row.agent_id,
    triggerDomain: row.trigger_domain,
    triggerKey: row.trigger_key,
    matchOperator: row.match_operator,
    matchValue: row.match_value,
    previousMatchOperator: row.previous_match_operator,
    previousMatchValue: row.previous_match_value,
    minSupportRatio: row.min_support_ratio,
    minConfidenceScore: row.min_confidence_score,
    scopeTypeFilter: row.scope_type_filter,
    action: row.action,
    intentId: row.intent_id,
    cooldownSeconds: row.cooldown_seconds,
    enabled: row.enabled,
    priority: row.priority,
    createdAt: row.created_at?.toISOString() ?? null,
    updatedAt: row.updated_at?.toISOString() ?? null,
    specificity: routingRuleSpecificity(row),
    blockedReasons: readiness.blockedReasons,
    readiness: {
      intentReady: Boolean(!row.intent_id || readiness.intent?.enabled),
      intentRequiresApproval: readiness.intent?.requires_approval ?? null,
      ticketProviderReady: readiness.halo.ready,
      llmRouterEnabled: LLM_ROUTER_ENABLED
    }
  };
}

async function buildRoutingRuleReadModels(rows: RoutingRuleRow[]): Promise<RoutingRuleReadModel[]> {
  const haloByOrganization = new Map<string, HaloProviderStatus>();
  const intentCache = new Map<string, RoutingIntentSummary | null>();

  return Promise.all(rows.map(async (row) => {
    let halo = haloByOrganization.get(row.organization_id);
    if (!halo) {
      halo = await loadHaloProviderStatus(row.organization_id);
      haloByOrganization.set(row.organization_id, halo);
    }

    let intent: RoutingIntentSummary | null = null;
    if (row.intent_id) {
      const cacheKey = `${row.organization_id}:${row.intent_id}`;
      if (intentCache.has(cacheKey)) {
        intent = intentCache.get(cacheKey) ?? null;
      } else {
        intent = await loadRoutingIntentSummary(row.organization_id, row.intent_id);
        intentCache.set(cacheKey, intent);
      }
    }

    const action = normalizeRoutingAction(row.action) || 'ignore';
    const blockedReasons: string[] = [];
    if ((action === 'recommend' || action === 'auto_remediate') && !row.intent_id) {
      blockedReasons.push('intent_required');
    } else if ((action === 'recommend' || action === 'auto_remediate') && !intent) {
      blockedReasons.push('intent_not_found');
    } else if ((action === 'recommend' || action === 'auto_remediate') && !(intent?.enabled ?? false)) {
      blockedReasons.push('intent_disabled');
    }
    if (action === 'ticket' && row.enabled && !halo.ready) {
      blockedReasons.push('ticket_provider_not_ready');
    }
    if (action === 'llm_router' && !LLM_ROUTER_ENABLED) {
      blockedReasons.push('llm_router_disabled');
    }

    return serializeRoutingRule(row, { blockedReasons, intent, halo });
  }));
}

type AlertRuleInput = {
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  name: string;
  triggerDomain: 'event' | 'baseline' | 'scope_drift' | 'decision';
  triggerKey: string;
  matchOperator: string;
  matchValue: string | null;
  severity: string;
  minSeverity: string | null;
  dedupeWindowSeconds: number;
  enabled: boolean;
  priority: number;
  notificationChannels: string[];
  createdBy: string;
};

function serializeAlertRule(row: AlertRuleRow) {
  return {
    id: String(row.id),
    organizationId: row.organization_id,
    customerId: row.customer_id,
    siteId: row.site_id,
    agentId: row.agent_id,
    name: row.name,
    triggerDomain: row.trigger_domain,
    triggerKey: row.trigger_key,
    matchOperator: row.match_operator,
    matchValue: row.match_value,
    severity: row.severity,
    minSeverity: row.min_severity,
    dedupeWindowSeconds: row.dedupe_window_seconds,
    enabled: row.enabled,
    priority: row.priority,
    notificationChannels: normalizeNotificationChannels(row.notification_channels_jsonb),
    createdBy: row.created_by,
    createdAt: row.created_at.toISOString(),
    updatedAt: row.updated_at.toISOString()
  };
}

function serializeAlert(row: AlertRow) {
  return {
    id: String(row.id),
    organizationId: row.organization_id,
    customerId: row.customer_id,
    customerName: row.customer_name ?? null,
    siteId: row.site_id,
    siteName: row.site_name ?? null,
    agentId: row.agent_id,
    hostname: row.hostname ?? null,
    ruleId: row.rule_id ? String(row.rule_id) : null,
    status: row.status,
    severity: row.severity,
    sourceDomain: row.source_domain,
    sourceKey: row.source_key,
    sourceEventId: row.source_event_id,
    sourceFactKey: row.source_fact_key,
    sourceDecisionId: row.source_decision_id ? String(row.source_decision_id) : null,
    title: row.title,
    summary: row.summary,
    fingerprint: row.fingerprint,
    firstSeenAt: row.first_seen_at.toISOString(),
    lastSeenAt: row.last_seen_at.toISOString(),
    occurrenceCount: row.occurrence_count,
    ownerUserId: row.owner_user_id,
    ownerEmail: row.owner_email ?? null,
    acknowledgedBy: row.acknowledged_by,
    acknowledgedAt: iso(row.acknowledged_at),
    snoozedUntil: iso(row.snoozed_until),
    resolvedBy: row.resolved_by,
    resolvedAt: iso(row.resolved_at),
    suppressedUntil: iso(row.suppressed_until),
    metadata: row.metadata_jsonb,
    createdAt: row.created_at.toISOString(),
    updatedAt: row.updated_at.toISOString()
  };
}

async function loadAlertById(organizationId: string, id: bigint): Promise<AlertRow | null> {
  const rows = await prisma.$queryRaw<AlertRow[]>(Prisma.sql`
    SELECT
      a.id,
      a.organization_id,
      a.customer_id,
      a.site_id,
      a.agent_id,
      a.rule_id,
      a.status,
      a.severity,
      a.source_domain,
      a.source_key,
      a.source_event_id,
      a.source_fact_key,
      a.source_decision_id,
      a.title,
      a.summary,
      a.fingerprint,
      a.first_seen_at,
      a.last_seen_at,
      a.occurrence_count,
      a.owner_user_id,
      owner.email AS owner_email,
      a.acknowledged_by,
      a.acknowledged_at,
      a.snoozed_until,
      a.resolved_by,
      a.resolved_at,
      a.suppressed_until,
      a.metadata_jsonb,
      a.created_at,
      a.updated_at,
      d.hostname,
      c.name AS customer_name,
      s.name AS site_name
    FROM rmm_telemetry.alert a
    LEFT JOIN public.rmm_devices d ON d.agent_id = a.agent_id
    LEFT JOIN public.customers c ON c.id = a.customer_id
    LEFT JOIN public.rmm_sites s ON s.id = a.site_id
    LEFT JOIN public."User" owner ON owner.id = a.owner_user_id
    WHERE a.organization_id = ${organizationId}
      AND a.id = ${id}
    LIMIT 1
  `);
  return rows[0] ?? null;
}

async function loadAlertRuleRow(
  organizationId: string,
  id: bigint
): Promise<AlertRuleRow | null> {
  const rows = await prisma.$queryRaw<AlertRuleRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      name,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      severity,
      min_severity,
      dedupe_window_seconds,
      enabled,
      priority,
      notification_channels_jsonb,
      created_by,
      created_at,
      updated_at
    FROM rmm_telemetry.alert_rule
    WHERE organization_id = ${organizationId}
      AND id = ${id}
    LIMIT 1
  `);
  return rows[0] ?? null;
}

async function loadAlertRulesForOrganization(
  organizationId: string,
  options?: { enabled?: boolean | null; triggerDomain?: string | null }
): Promise<AlertRuleRow[]> {
  const clauses: Prisma.Sql[] = [Prisma.sql`organization_id = ${organizationId}`];
  if (typeof options?.enabled === 'boolean') {
    clauses.push(Prisma.sql`enabled = ${options.enabled}`);
  }
  const triggerDomain = normalizeAlertSourceDomain(options?.triggerDomain);
  if (triggerDomain) {
    clauses.push(Prisma.sql`trigger_domain = ${triggerDomain}`);
  }
  const whereClause = clauses.reduce<Prisma.Sql>(
    (sql, clause, index) => index === 0 ? clause : Prisma.sql`${sql} AND ${clause}`,
    Prisma.sql`TRUE`
  );

  return prisma.$queryRaw<AlertRuleRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      name,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      severity,
      min_severity,
      dedupe_window_seconds,
      enabled,
      priority,
      notification_channels_jsonb,
      created_by,
      created_at,
      updated_at
    FROM rmm_telemetry.alert_rule
    WHERE ${whereClause}
    ORDER BY
      CASE
        WHEN agent_id IS NOT NULL THEN 1
        WHEN site_id IS NOT NULL THEN 2
        WHEN customer_id IS NOT NULL THEN 3
        ELSE 4
      END ASC,
      priority ASC,
      id ASC
  `);
}

async function parseAlertRuleInput(
  organizationId: string,
  userId: string,
  body: JsonObject,
  existing?: AlertRuleRow | null
): Promise<{ rule: AlertRuleInput | null; errors: string[] }> {
  const name = readString(body.name, existing?.name);
  const triggerDomain = normalizeAlertSourceDomain(body.triggerDomain ?? body.trigger_domain ?? existing?.trigger_domain);
  const triggerKey = readString(body.triggerKey, body.trigger_key, existing?.trigger_key);
  const matchOperator = normalizeAlertMatchOperator(
    body.matchOperator ?? body.match_operator ?? existing?.match_operator,
    'equals'
  );
  const matchValue = readString(body.matchValue, body.match_value, existing?.match_value);
  const severity = normalizeAlertSeverity(body.severity ?? existing?.severity, 'medium');
  const minSeverityRaw = body.minSeverity ?? body.min_severity ?? existing?.min_severity ?? null;
  const minSeverity = minSeverityRaw === null || minSeverityRaw === undefined || minSeverityRaw === ''
    ? null
    : normalizeAlertSeverity(minSeverityRaw, 'info');
  const dedupeWindowSeconds = parseInteger(
    body.dedupeWindowSeconds ?? body.dedupe_window_seconds ?? existing?.dedupe_window_seconds
  ) ?? 300;
  const priority = parseInteger(body.priority ?? existing?.priority) ?? 100;
  const enabled = readBoolean(body.enabled) ?? existing?.enabled ?? true;
  const customerId = readString(body.customerId, body.customer_id, existing?.customer_id);
  const siteId = readString(body.siteId, body.site_id, existing?.site_id);
  const agentId = readString(body.agentId, body.agent_id, existing?.agent_id);
  const notificationChannels =
    Object.prototype.hasOwnProperty.call(body, 'notificationChannels') ||
    Object.prototype.hasOwnProperty.call(body, 'notification_channels')
      ? normalizeNotificationChannels(body.notificationChannels ?? body.notification_channels)
      : normalizeNotificationChannels(existing?.notification_channels_jsonb);

  const errors: string[] = [];
  if (!name) errors.push('name is required');
  if (!triggerDomain) errors.push('triggerDomain must be event, baseline, scope_drift, or decision');
  if (!triggerKey) errors.push('triggerKey is required');
  if (!matchOperator) errors.push('matchOperator is invalid');
  if (dedupeWindowSeconds < 0 || dedupeWindowSeconds > 86400) {
    errors.push('dedupeWindowSeconds must be between 0 and 86400');
  }
  if (priority < 0) errors.push('priority must be greater than or equal to 0');
  errors.push(...await validateRoutingSelectors(organizationId, {
    customerId: customerId ?? null,
    siteId: siteId ?? null,
    agentId: agentId ?? null
  }));

  if (errors.length > 0 || !name || !triggerDomain || !triggerKey || !matchOperator) {
    return { rule: null, errors };
  }

  return {
    rule: {
      organizationId,
      customerId: customerId ?? null,
      siteId: siteId ?? null,
      agentId: agentId ?? null,
      name,
      triggerDomain,
      triggerKey,
      matchOperator,
      matchValue,
      severity,
      minSeverity,
      dedupeWindowSeconds,
      enabled,
      priority,
      notificationChannels,
      createdBy: existing?.created_by ?? userId
    },
    errors
  };
}

function readAlertCandidateSeverity(...values: unknown[]): string {
  for (const value of values) {
    if (typeof value === 'string' && value.trim()) {
      return normalizeAlertSeverity(value, 'info');
    }
    const record = asRecord(value);
    if (record) {
      const severity = readString(record.severity, record.level);
      if (severity) return normalizeAlertSeverity(severity, 'info');
    }
  }
  return 'info';
}

function valuePreview(value: unknown, max = 240): string {
  const text = typeof value === 'string' ? value : toJsonString(value);
  return text.length > max ? `${text.slice(0, max - 3)}...` : text;
}

async function recordAlertNotificationDeliveries(
  tx: Prisma.TransactionClient,
  alertId: bigint,
  channels: unknown
): Promise<void> {
  const deliveries = await dispatchAlertNotifications({
    alertId: alertId.toString(),
    channels
  });
  for (const delivery of deliveries) {
    await tx.$executeRaw(Prisma.sql`
      INSERT INTO rmm_telemetry.alert_notification_delivery
        (alert_id, channel, adapter, status, detail, external_ref, attempted_at)
      VALUES
        (${alertId}, ${delivery.channel}, ${delivery.adapter}, ${delivery.status}, ${delivery.detail}, ${delivery.externalRef}, NOW())
    `);
  }
}

async function upsertAlertForCandidate(
  tx: Prisma.TransactionClient,
  rule: AlertRuleRow,
  candidate: AlertCandidate
): Promise<{ alertId: string; created: boolean; lifecycleReason: string }> {
  const now = new Date();
  const fingerprint = buildAlertFingerprint(rule.id, candidate);
  const existingRows = await tx.$queryRaw<AlertRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      rule_id,
      status,
      severity,
      source_domain,
      source_key,
      source_event_id,
      source_fact_key,
      source_decision_id,
      title,
      summary,
      fingerprint,
      first_seen_at,
      last_seen_at,
      occurrence_count,
      owner_user_id,
      acknowledged_by,
      acknowledged_at,
      snoozed_until,
      resolved_by,
      resolved_at,
      suppressed_until,
      metadata_jsonb,
      created_at,
      updated_at
    FROM rmm_telemetry.alert
    WHERE organization_id = ${candidate.organizationId}
      AND fingerprint = ${fingerprint}
    LIMIT 1
  `);
  const existing = existingRows[0] ?? null;
  const lifecycle = planAlertLifecycle(existing ? {
    status: existing.status,
    firstSeenAt: existing.first_seen_at,
    lastSeenAt: existing.last_seen_at,
    occurrenceCount: existing.occurrence_count,
    acknowledgedAt: existing.acknowledged_at,
    snoozedUntil: existing.snoozed_until,
    resolvedAt: existing.resolved_at,
    suppressedUntil: existing.suppressed_until
  } : null, now, {
    dedupeWindowSeconds: rule.dedupe_window_seconds
  });

  const metadataJson = toJsonString({
    ...candidate.metadata,
    alertRuleId: rule.id.toString(),
    lifecycleReason: lifecycle.reason,
    duplicateSuppressed: lifecycle.duplicateSuppressed
  });
  const candidateSeverity = normalizeAlertSeverity(candidate.severity);
  const ruleSeverity = normalizeAlertSeverity(rule.severity || candidateSeverity, candidateSeverity);
  const nextSeverity = existing ? highestSeverity(existing.severity, ruleSeverity) : ruleSeverity;

  if (!existing) {
    const rows = await tx.$queryRaw<AlertRow[]>(Prisma.sql`
      INSERT INTO rmm_telemetry.alert
        (
          organization_id, customer_id, site_id, agent_id, rule_id,
          status, severity, source_domain, source_key, source_event_id,
          source_fact_key, source_decision_id, title, summary, fingerprint,
          first_seen_at, last_seen_at, occurrence_count, metadata_jsonb,
          created_at, updated_at
        )
      VALUES
        (
          ${candidate.organizationId}, ${candidate.customerId}, ${candidate.siteId}, ${candidate.agentId}, ${rule.id},
          ${lifecycle.status}, ${nextSeverity}, ${candidate.domain}, ${candidate.triggerKey}, ${candidate.sourceEventId ?? null},
          ${candidate.sourceFactKey ?? null}, ${candidate.sourceDecisionId ?? null}, ${candidate.title}, ${candidate.summary}, ${fingerprint},
          ${lifecycle.firstSeenAt}, ${lifecycle.lastSeenAt}, ${lifecycle.occurrenceCount}, ${metadataJson}::jsonb,
          NOW(), NOW()
        )
      RETURNING
        id,
        organization_id,
        customer_id,
        site_id,
        agent_id,
        rule_id,
        status,
        severity,
        source_domain,
        source_key,
        source_event_id,
        source_fact_key,
        source_decision_id,
        title,
        summary,
        fingerprint,
        first_seen_at,
        last_seen_at,
        occurrence_count,
        owner_user_id,
        acknowledged_by,
        acknowledged_at,
        snoozed_until,
        resolved_by,
        resolved_at,
        suppressed_until,
        metadata_jsonb,
        created_at,
        updated_at
    `);
    const alertId = rows[0]!.id;
    if (lifecycle.notificationSuggested) {
      await recordAlertNotificationDeliveries(tx, alertId, rule.notification_channels_jsonb);
    }
    return { alertId: alertId.toString(), created: true, lifecycleReason: lifecycle.reason };
  }

  const rows = await tx.$queryRaw<AlertRow[]>(Prisma.sql`
    UPDATE rmm_telemetry.alert
    SET
      customer_id = ${candidate.customerId},
      site_id = ${candidate.siteId},
      rule_id = ${rule.id},
      status = ${lifecycle.status},
      severity = ${nextSeverity},
      source_domain = ${candidate.domain},
      source_key = ${candidate.triggerKey},
      source_event_id = ${candidate.sourceEventId ?? null},
      source_fact_key = ${candidate.sourceFactKey ?? null},
      source_decision_id = ${candidate.sourceDecisionId ?? null},
      title = ${candidate.title},
      summary = ${candidate.summary},
      last_seen_at = ${lifecycle.lastSeenAt},
      occurrence_count = ${lifecycle.occurrenceCount},
      acknowledged_by = ${lifecycle.acknowledgedAt ? existing.acknowledged_by : null},
      acknowledged_at = ${lifecycle.acknowledgedAt},
      snoozed_until = ${lifecycle.snoozedUntil},
      resolved_by = ${lifecycle.resolvedAt ? existing.resolved_by : null},
      resolved_at = ${lifecycle.resolvedAt},
      suppressed_until = ${lifecycle.suppressedUntil},
      metadata_jsonb = ${metadataJson}::jsonb,
      updated_at = NOW()
    WHERE id = ${existing.id}
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      rule_id,
      status,
      severity,
      source_domain,
      source_key,
      source_event_id,
      source_fact_key,
      source_decision_id,
      title,
      summary,
      fingerprint,
      first_seen_at,
      last_seen_at,
      occurrence_count,
      owner_user_id,
      acknowledged_by,
      acknowledged_at,
      snoozed_until,
      resolved_by,
      resolved_at,
      suppressed_until,
      metadata_jsonb,
      created_at,
      updated_at
  `);
  const alertId = rows[0]?.id ?? existing.id;
  if (lifecycle.notificationSuggested) {
    await recordAlertNotificationDeliveries(tx, alertId, rule.notification_channels_jsonb);
  }
  return { alertId: alertId.toString(), created: false, lifecycleReason: lifecycle.reason };
}

async function applyAlertRulesForCandidate(
  tx: Prisma.TransactionClient,
  candidate: AlertCandidate
): Promise<{ matchedRules: number; alertsTouched: number }> {
  const rows = await tx.$queryRaw<AlertRuleRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      name,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      severity,
      min_severity,
      dedupe_window_seconds,
      enabled,
      priority,
      notification_channels_jsonb,
      created_by,
      created_at,
      updated_at
    FROM rmm_telemetry.alert_rule
    WHERE organization_id = ${candidate.organizationId}
      AND enabled = TRUE
      AND trigger_domain = ${candidate.domain}
    ORDER BY
      CASE
        WHEN agent_id IS NOT NULL THEN 1
        WHEN site_id IS NOT NULL THEN 2
        WHEN customer_id IS NOT NULL THEN 3
        ELSE 4
      END ASC,
      priority ASC,
      id ASC
  `);

  let matchedRules = 0;
  let alertsTouched = 0;
  for (const rule of rows) {
    if (!alertRuleMatchesCandidate({
      enabled: rule.enabled,
      organizationId: rule.organization_id,
      customerId: rule.customer_id,
      siteId: rule.site_id,
      agentId: rule.agent_id,
      triggerDomain: rule.trigger_domain,
      triggerKey: rule.trigger_key,
      matchOperator: rule.match_operator,
      matchValue: rule.match_value,
      minSeverity: rule.min_severity
    }, candidate)) {
      continue;
    }
    matchedRules += 1;
    await upsertAlertForCandidate(tx, rule, candidate);
    alertsTouched += 1;
  }

  return { matchedRules, alertsTouched };
}

type RemediationJobInsertOptions = {
  commandId?: string | null;
  organizationId: string;
  agentId: string;
  decisionId: bigint | null;
  intentId: string;
  status?: string;
  dedupeKey?: string | null;
  requestedBy?: string;
  metadata?: unknown;
  steps?: unknown;
  execution?: unknown;
};

class RemediationProjectionConflictError extends Error {
  constructor() {
    super('commandId or dedupeKey is already owned by another remediation job');
  }
}

class RemediationProjectionScopeError extends Error {
  constructor() {
    super('device not found in the requested organization');
  }
}

class DeviceScopeError extends Error {}

async function insertRemediationJob(
  tx: Prisma.TransactionClient,
  options: RemediationJobInsertOptions
): Promise<{ jobId: string; status: string; insertedSteps: number; }> {
  try {
    await requireDeviceScope(tx, options.agentId, new Date(), options.organizationId);
  } catch (error) {
    if (error instanceof DeviceScopeError) {
      throw new RemediationProjectionScopeError();
    }
    throw error;
  }

  const status = options.status || 'queued';
  const metadataJson = toJsonString(persistedRemediationMetadata(options));
  const steps = asArray(options.steps);
  const commandId = options.commandId ?? null;
  const jobRows = commandId && options.dedupeKey
    ? await tx.$queryRaw<{ id: bigint; status: string }[]>(Prisma.sql`
        INSERT INTO rmm_telemetry.remediation_job
          (command_id, organization_id, agent_id, decision_id, intent_id, status, dedupe_key, requested_by, metadata_jsonb, requested_at)
        VALUES
          (${commandId}, ${options.organizationId}, ${options.agentId}, ${options.decisionId}, ${options.intentId}, ${status}, ${options.dedupeKey}, ${options.requestedBy || 'consumer'}, ${metadataJson}::jsonb, NOW())
        ON CONFLICT (dedupe_key)
        DO UPDATE SET
          command_id = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.command_id
            ELSE EXCLUDED.command_id
          END,
          status = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.status
            ELSE EXCLUDED.status
          END,
          requested_by = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.requested_by
            ELSE EXCLUDED.requested_by
          END,
          requested_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.requested_at
            ELSE EXCLUDED.requested_at
          END,
          metadata_jsonb = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.metadata_jsonb
            ELSE EXCLUDED.metadata_jsonb
          END,
          started_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.started_at
            ELSE NULL
          END,
          finished_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.finished_at
            ELSE NULL
          END
        WHERE rmm_telemetry.remediation_job.command_id = EXCLUDED.command_id
          AND rmm_telemetry.remediation_job.organization_id = EXCLUDED.organization_id
          AND rmm_telemetry.remediation_job.agent_id = EXCLUDED.agent_id
          AND rmm_telemetry.remediation_job.intent_id = EXCLUDED.intent_id
        RETURNING id, status
      `)
    : commandId
    ? await tx.$queryRaw<{ id: bigint; status: string }[]>(Prisma.sql`
        INSERT INTO rmm_telemetry.remediation_job
          (command_id, organization_id, agent_id, decision_id, intent_id, status, dedupe_key, requested_by, metadata_jsonb, requested_at)
        VALUES
          (${commandId}, ${options.organizationId}, ${options.agentId}, ${options.decisionId}, ${options.intentId}, ${status}, ${options.dedupeKey}, ${options.requestedBy || 'consumer'}, ${metadataJson}::jsonb, NOW())
        ON CONFLICT (command_id) WHERE command_id IS NOT NULL
        DO UPDATE SET
          status = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.status
            ELSE EXCLUDED.status
          END,
          requested_by = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.requested_by
            ELSE EXCLUDED.requested_by
          END,
          requested_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.requested_at
            ELSE EXCLUDED.requested_at
          END,
          metadata_jsonb = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.metadata_jsonb
            ELSE EXCLUDED.metadata_jsonb
          END,
          started_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.started_at
            ELSE NULL
          END,
          finished_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.finished_at
            ELSE NULL
          END
        WHERE rmm_telemetry.remediation_job.organization_id = EXCLUDED.organization_id
          AND rmm_telemetry.remediation_job.agent_id = EXCLUDED.agent_id
          AND rmm_telemetry.remediation_job.intent_id = EXCLUDED.intent_id
        RETURNING id, status
      `)
    : options.dedupeKey
    ? await tx.$queryRaw<{ id: bigint; status: string }[]>(Prisma.sql`
        INSERT INTO rmm_telemetry.remediation_job
          (organization_id, agent_id, decision_id, intent_id, status, dedupe_key, requested_by, metadata_jsonb, requested_at)
        VALUES
          (${options.organizationId}, ${options.agentId}, ${options.decisionId}, ${options.intentId}, ${status}, ${options.dedupeKey}, ${options.requestedBy || 'consumer'}, ${metadataJson}::jsonb, NOW())
        ON CONFLICT (dedupe_key)
        DO UPDATE SET
          requested_at = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.requested_at
            ELSE EXCLUDED.requested_at
          END,
          metadata_jsonb = CASE
            WHEN rmm_telemetry.remediation_job.status IN ('running', 'completed', 'failed', 'cancelled') THEN rmm_telemetry.remediation_job.metadata_jsonb
            ELSE EXCLUDED.metadata_jsonb
          END
        WHERE rmm_telemetry.remediation_job.command_id IS NULL
          AND rmm_telemetry.remediation_job.organization_id = EXCLUDED.organization_id
          AND rmm_telemetry.remediation_job.agent_id = EXCLUDED.agent_id
          AND rmm_telemetry.remediation_job.intent_id = EXCLUDED.intent_id
        RETURNING id, status
      `)
    : await tx.$queryRaw<{ id: bigint; status: string }[]>(Prisma.sql`
        INSERT INTO rmm_telemetry.remediation_job
          (organization_id, agent_id, decision_id, intent_id, status, dedupe_key, requested_by, metadata_jsonb, requested_at)
        VALUES
          (${options.organizationId}, ${options.agentId}, ${options.decisionId}, ${options.intentId}, ${status}, NULL, ${options.requestedBy || 'consumer'}, ${metadataJson}::jsonb, NOW())
        RETURNING id, status
      `);

  const jobId = jobRows[0]?.id;
  if (!jobId) {
    throw new RemediationProjectionConflictError();
  }

  const refreshSteps = !['running', 'completed', 'failed', 'cancelled'].includes(jobRows[0]!.status);
  let insertedSteps = 0;
  for (let i = 0; i < steps.length; i += 1) {
    const stepRecord = asRecord(steps[i]);
    if (!stepRecord) continue;
    const command = readString(stepRecord.command);
    if (!command) continue;
    const stepIndex = parseInteger(stepRecord.stepIndex ?? stepRecord.step_index) ?? i;
    const stepStatus = readString(stepRecord.status) || 'pending';
    const evidence = asRecord(stepRecord.evidence) || null;
    const evidenceJson = toJsonString(evidence);
    const inserted = await tx.$executeRaw(
      Prisma.sql`
        INSERT INTO rmm_telemetry.remediation_step
          (organization_id, job_id, step_index, command, status, evidence_jsonb)
        VALUES
          (${options.organizationId}, ${jobId}, ${stepIndex}, ${command}, ${stepStatus}, ${evidenceJson}::jsonb)
        ON CONFLICT (job_id, step_index)
        DO UPDATE SET
          command = CASE WHEN ${refreshSteps} THEN EXCLUDED.command ELSE rmm_telemetry.remediation_step.command END,
          status = CASE WHEN ${refreshSteps} THEN EXCLUDED.status ELSE rmm_telemetry.remediation_step.status END,
          evidence_jsonb = CASE WHEN ${refreshSteps} THEN EXCLUDED.evidence_jsonb ELSE rmm_telemetry.remediation_step.evidence_jsonb END,
          started_at = CASE WHEN ${refreshSteps} THEN NULL ELSE rmm_telemetry.remediation_step.started_at END,
          finished_at = CASE WHEN ${refreshSteps} THEN NULL ELSE rmm_telemetry.remediation_step.finished_at END
        WHERE rmm_telemetry.remediation_step.organization_id = EXCLUDED.organization_id
      `
    );
    insertedSteps += inserted;
  }

  return { jobId: String(jobId), status: jobRows[0]!.status, insertedSteps };
}

function normalizePatchText(value: unknown): string {
  return typeof value === 'string' ? value.trim().toLowerCase().replace(/\s+/g, ' ') : '';
}

function firstString(...values: unknown[]): string | null {
  for (const value of values) {
    const text = readString(value);
    if (text) return text;
  }
  return null;
}

function patchPendingCountFromSnapshot(snapshot: unknown): number | null {
  const paths = [
    ['collection', 'operating_system', 'updates', 'windows_update', 'pending_count'],
    ['collection', 'operatingSystem', 'updates', 'windowsUpdate', 'pendingCount'],
    ['collection', 'software', 'windows_updates', 'pending_count'],
    ['collection', 'software', 'windowsUpdates', 'pendingCount'],
    ['operating_system', 'updates', 'windows_update', 'pending_count'],
    ['operatingSystem', 'updates', 'windowsUpdate', 'pendingCount'],
    ['software', 'windows_updates', 'pending_count'],
    ['software', 'windowsUpdates', 'pendingCount'],
    ['snapshot', 'collection', 'operating_system', 'updates', 'windows_update', 'pending_count'],
    ['snapshot', 'collection', 'operatingSystem', 'updates', 'windowsUpdate', 'pendingCount'],
    ['snapshot', 'collection', 'software', 'windows_updates', 'pending_count'],
    ['snapshot', 'collection', 'software', 'windowsUpdates', 'pendingCount']
  ];
  for (const path of paths) {
    const value = parseNumber(valueAtPath(snapshot, path));
    if (value !== null) return value;
  }
  return null;
}

function parsePendingPatchUpdatesFromSnapshot(snapshot: unknown): PendingPatchSnapshotUpdate[] {
  const pendingUpdates = arrayAtAnyPath(snapshot, [
    ['collection', 'operating_system', 'updates', 'windows_update', 'pending_updates'],
    ['collection', 'operating_system', 'updates', 'windows_update', 'pending'],
    ['collection', 'operatingSystem', 'updates', 'windowsUpdate', 'pendingUpdates'],
    ['operating_system', 'updates', 'windows_update', 'pending_updates'],
    ['operating_system', 'updates', 'windows_update', 'pending'],
    ['operatingSystem', 'updates', 'windowsUpdate', 'pendingUpdates'],
    ['snapshot', 'collection', 'operating_system', 'updates', 'windows_update', 'pending_updates'],
    ['snapshot', 'collection', 'operating_system', 'updates', 'windows_update', 'pending'],
    ['snapshot', 'collection', 'operatingSystem', 'updates', 'windowsUpdate', 'pendingUpdates']
  ]);
  const dedupe = new Map<string, PendingPatchSnapshotUpdate>();

  for (const update of pendingUpdates) {
    const record = asRecord(update);
    if (!record) continue;
    const title = firstString(record.title, record.name, record.kb) ?? '';
    if (!title) continue;
    const titleNorm = normalizePatchText(title);
    if (!titleNorm) continue;
    const kbArticle = firstString(record.kb_article, record.kbArticle, record.kb);
    dedupe.set(`${titleNorm}|${kbArticle ?? ''}`, {
      title,
      titleNorm,
      description: firstString(record.description),
      kbArticle,
      isMandatory: readFlexibleBoolean(record.is_mandatory, record.isMandatory),
      sizeBytes: parseBigIntValue(record.size_bytes ?? record.sizeBytes ?? record.size),
      requiresReboot: readFlexibleBoolean(record.requires_reboot, record.requiresReboot)
    });
  }

  return [...dedupe.values()];
}

function uniquePendingPatchUpdatesByUpdateKey(
  updates: PendingPatchSnapshotUpdate[]
): PendingPatchSnapshotUpdate[] {
  const seen = new Set<string>();
  const unique: PendingPatchSnapshotUpdate[] = [];
  for (const update of updates) {
    const updateKey = buildUpdateKeyFromParts(update.title, update.kbArticle);
    if (seen.has(updateKey)) continue;
    seen.add(updateKey);
    unique.push(update);
  }
  return unique;
}

async function projectPatchSnapshotUpdates(
  tx: Prisma.TransactionClient,
  input: {
    organizationId: string;
    agentId: string;
    collectedAt: Date;
    snapshot: unknown;
  }
) {
  const parsedPendingUpdates = parsePendingPatchUpdatesFromSnapshot(input.snapshot);
  const pendingUpdates = uniquePendingPatchUpdatesByUpdateKey(parsedPendingUpdates);
  const reportedPendingUpdatesCount = patchPendingCountFromSnapshot(input.snapshot);
  console.info('rmm compat snapshot patch ingestion parsed', {
    agentId: input.agentId,
    parsedPendingUpdates: parsedPendingUpdates.length,
    uniquePendingUpdateKeys: pendingUpdates.length,
    reportedPendingUpdatesCount,
    destination: 'rmm_patch_update_catalog'
  });

  if (pendingUpdates.length === 0) {
    if ((reportedPendingUpdatesCount ?? 0) > 0) {
      console.warn('rmm compat snapshot reported pending updates but parser found no rows', {
        agentId: input.agentId,
        reportedPendingUpdatesCount
      });
      return { parsedPendingUpdates: 0, catalogRows: 0, stateRows: 0 };
    }
    await tx.$executeRaw(
      Prisma.sql`
        UPDATE public.rmm_patch_device_update_state
        SET applicability_state = 'not_applicable',
            lifecycle_state = CASE
              WHEN lifecycle_state IN ('installed', 'failed', 'superseded') THEN lifecycle_state
              ELSE 'superseded'
            END,
            updated_at = NOW()
        WHERE organization_id = ${input.organizationId}
          AND agent_id = ${input.agentId}
          AND lifecycle_state NOT IN ('installed', 'failed')
      `
    );
    return { parsedPendingUpdates: 0, catalogRows: 0, stateRows: 0 };
  }

  const catalogValues = Prisma.join(
    pendingUpdates.map((update) => {
      const updateKey = buildUpdateKeyFromParts(update.title, update.kbArticle);
      const category = classifyPatchCategory({ title: update.title, kbArticle: update.kbArticle });
      return Prisma.sql`
        (
          ${crypto.randomUUID()}, ${input.organizationId}, ${updateKey}, ${update.title}, ${update.titleNorm},
          ${update.kbArticle}, ${category}, ${input.collectedAt}, ${input.collectedAt}, NOW()
        )
      `;
    })
  );

  const catalogRows = await tx.$executeRaw(
    Prisma.sql`
      INSERT INTO public.rmm_patch_update_catalog
        (
          id, organization_id, update_key, title, title_norm, kb_article,
          category, first_seen_at, last_seen_at, updated_at
        )
      VALUES ${catalogValues}
      ON CONFLICT (organization_id, update_key)
      DO UPDATE SET
        title = EXCLUDED.title,
        title_norm = EXCLUDED.title_norm,
        kb_article = EXCLUDED.kb_article,
        category = EXCLUDED.category,
        last_seen_at = EXCLUDED.last_seen_at,
        updated_at = NOW()
    `
  );

  const stateValues = Prisma.join(
    pendingUpdates.map((update) => {
      const updateKey = buildUpdateKeyFromParts(update.title, update.kbArticle);
      const category = classifyPatchCategory({ title: update.title, kbArticle: update.kbArticle });
      return Prisma.sql`
        (
          ${crypto.randomUUID()}, ${input.organizationId}, ${input.agentId}, ${updateKey},
          ${update.title}, ${update.titleNorm}, ${update.kbArticle}, ${category},
          'applicable', 'detected', 'detected', ${input.collectedAt}, ${input.collectedAt},
          ${update.requiresReboot}, ${JSON.stringify({
            source: 'compat_snapshot',
            description: update.description,
            sizeBytes: update.sizeBytes === null ? null : Number(update.sizeBytes),
            isMandatory: update.isMandatory
          })}::jsonb,
          NOW()
        )
      `;
    })
  );

  const stateRows = await tx.$executeRaw(
    Prisma.sql`
      INSERT INTO public.rmm_patch_device_update_state
        (
          id, organization_id, agent_id, update_key, title, title_norm, kb_article, category,
          applicability_state, approval_state, lifecycle_state, first_detected_at, last_detected_at,
          requires_reboot, metadata_jsonb, updated_at
        )
      VALUES ${stateValues}
      ON CONFLICT (organization_id, agent_id, update_key)
      DO UPDATE SET
        title = EXCLUDED.title,
        title_norm = EXCLUDED.title_norm,
        kb_article = EXCLUDED.kb_article,
        category = EXCLUDED.category,
        applicability_state = 'applicable',
        approval_state = CASE
          WHEN public.rmm_patch_device_update_state.approval_state IN ('blocked', 'deferred', 'approved', 'emergency_approved')
            THEN public.rmm_patch_device_update_state.approval_state
          ELSE EXCLUDED.approval_state
        END,
        lifecycle_state = CASE
          WHEN public.rmm_patch_device_update_state.lifecycle_state IN ('installed', 'failed')
            THEN public.rmm_patch_device_update_state.lifecycle_state
          ELSE EXCLUDED.lifecycle_state
        END,
        last_detected_at = EXCLUDED.last_detected_at,
        requires_reboot = EXCLUDED.requires_reboot,
        metadata_jsonb = EXCLUDED.metadata_jsonb,
        updated_at = NOW()
    `
  );

  await tx.$executeRaw(
    Prisma.sql`
      UPDATE public.rmm_patch_device_update_state
      SET applicability_state = 'not_applicable',
          lifecycle_state = CASE
            WHEN lifecycle_state IN ('installed', 'failed', 'superseded') THEN lifecycle_state
            ELSE 'superseded'
          END,
          updated_at = NOW()
      WHERE organization_id = ${input.organizationId}
        AND agent_id = ${input.agentId}
        AND update_key NOT IN (${Prisma.join(pendingUpdates.map((update) => buildUpdateKeyFromParts(update.title, update.kbArticle)))})
        AND lifecycle_state NOT IN ('installed', 'failed')
    `
  );

  console.info('rmm compat patch catalog projection applied', {
    agentId: input.agentId,
    parsedPendingUpdates: pendingUpdates.length,
    catalogRows,
    stateRows
  });

  return { parsedPendingUpdates: pendingUpdates.length, catalogRows, stateRows };
}

function normalizeCommandSteps(stepsInput: unknown, allowListInput: unknown, fallbackTimeoutSeconds?: number): JsonObject[] {
  const allowList = asArray(allowListInput)
    .filter((value): value is string => typeof value === 'string' && value.trim().length > 0)
    .map((value) => value.trim());
  const steps = asArray(stepsInput);
  if (steps.length === 0) {
    throw Object.assign(new Error('Remediation intent must define at least one step'), { status: 400 });
  }

  return steps.map((step, index) => {
    const stepRecord = asRecord(step);
    const command = readString(stepRecord?.command);
    if (!command) {
      throw Object.assign(new Error(`Remediation step ${index} must include a non-empty command`), { status: 400 });
    }
    if (allowList.length > 0 && !allowList.includes(command)) {
      throw Object.assign(new Error(`Remediation command is not in the intent allow list: ${command}`), { status: 400 });
    }
    return {
      stepIndex: parseInteger(stepRecord?.stepIndex ?? stepRecord?.step_index) ?? index,
      command,
      status: readString(stepRecord?.status) || 'pending',
      description: readString(stepRecord?.description) ?? undefined,
      timeoutSeconds:
        parseInteger(stepRecord?.timeoutSeconds ?? stepRecord?.timeout_seconds) ??
        fallbackTimeoutSeconds ??
        undefined
    };
  });
}

async function publishRemediationCommands(commands: unknown[]) {
  if (commands.length === 0) return;
  const baseUrl = env.telemetryProducerUrl?.trim().replace(/\/+$/, '');
  const serverKey = env.rmmServerApiKey?.trim();
  if (!baseUrl || !serverKey) {
    throw Object.assign(new Error('RMM_TELEMETRY_PRODUCER_URL and RMM_SERVER_API_KEY are required to publish remediation commands'), {
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
    throw Object.assign(new Error(`Telemetry producer rejected remediation commands: ${response.status} ${body}`), {
      status: 502
    });
  }
}

function buildRemediationCommandEvent(options: {
  commandId?: string;
  organizationId: string;
  agentId: string;
  decisionId?: bigint | string | null;
  intentId: string;
  dedupeKey?: string | null;
  requestedBy: string;
  approvalState: 'approved' | 'pending_approval';
  metadata: unknown;
  steps: unknown[];
  timeoutSeconds?: number;
  maxRetries?: number;
}) {
  const commandId = options.commandId || crypto.randomUUID();
  return {
    schemaVersion: 1,
    eventType: 'remediation.command.requested',
    commandId,
    organizationId: options.organizationId,
    agentId: options.agentId,
    intentId: options.intentId,
    decisionId: options.decisionId === null || options.decisionId === undefined ? null : String(options.decisionId),
    dedupeKey: options.dedupeKey ?? null,
    requestedBy: options.requestedBy,
    requestedAt: new Date().toISOString(),
    approvalState: options.approvalState,
    metadata: options.metadata ?? {},
    steps: options.steps,
    execution: {
      maxRetries: options.maxRetries ?? 0,
      timeoutSeconds: options.timeoutSeconds ?? 300,
      stopOnFailure: true
    }
  };
}

async function updateRoutingDecisionExecution(
  decisionId: bigint,
  executionStatus: string,
  outcomeMessage?: string | null,
  externalRef?: string | null
): Promise<void> {
  await prisma.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.routing_decision
    SET
      execution_status = ${executionStatus},
      outcome_message = ${outcomeMessage ?? null},
      external_ref = ${externalRef ?? null}
    WHERE id = ${decisionId}
  `);
}

async function executeRoutingDecision(
  decision: RoutingDecisionRow
): Promise<{ executionStatus: string; outcomeMessage: string | null; externalRef: string | null; remediationJobId?: string | null; }> {
  const action = normalizeRoutingAction(decision.action) || 'ignore';
  const intent = await loadRoutingIntentSummary(decision.organization_id, decision.intent_id);
  const halo = await loadHaloProviderStatus(decision.organization_id);

  if ((action === 'recommend' || action === 'auto_remediate') && (!intent || !intent.enabled)) {
    return {
      executionStatus: 'failed',
      outcomeMessage: !decision.intent_id ? 'intent is required for this action' : 'intent is missing or disabled',
      externalRef: null
    };
  }

  if (action === 'ignore') {
    return {
      executionStatus: 'completed',
      outcomeMessage: 'decision ignored by routing policy',
      externalRef: null
    };
  }

  if (action === 'recommend') {
    return {
      executionStatus: 'completed',
      outcomeMessage: `recommended intent ${intent?.name || decision.intent_id}`,
      externalRef: decision.intent_id
    };
  }

  if (action === 'ticket') {
    if (!halo.ready) {
      return {
        executionStatus: 'failed',
        outcomeMessage: 'Halo PSA is not configured for this organization',
        externalRef: null
      };
    }
    return {
      executionStatus: 'completed',
      outcomeMessage: 'ticket handoff accepted by Halo placeholder adapter',
      externalRef: `halo-placeholder:${decision.id.toString()}`
    };
  }

  if (action === 'llm_router') {
    if (!LLM_ROUTER_ENABLED) {
      return {
        executionStatus: 'skipped',
        outcomeMessage: 'llm_router is disabled by feature flag',
        externalRef: null
      };
    }
    return {
      executionStatus: 'completed',
      outcomeMessage: 'llm_router handoff recorded',
      externalRef: `llm-router:${decision.id.toString()}`
    };
  }

  const metadata = asRecord(decision.trigger_value) || {
    triggerValue: decision.trigger_value
  };
  const approvalState = intent!.requires_approval ? 'pending_approval' : 'approved';
  const steps = normalizeCommandSteps(intent!.steps, intent!.allow_list, intent!.timeout_seconds ?? 300);
  const command = buildRemediationCommandEvent({
    organizationId: decision.organization_id,
    agentId: decision.agent_id,
    decisionId: decision.id,
    intentId: decision.intent_id!,
    dedupeKey: decision.dedupe_key,
    requestedBy: 'routing-engine',
    approvalState,
    metadata: {
      ...metadata,
      decisionId: decision.id.toString(),
      matchedRuleId: decision.matched_rule_id ? decision.matched_rule_id.toString() : null,
      action: decision.action
    },
    steps,
    timeoutSeconds: intent!.timeout_seconds ?? 300,
    maxRetries: intent!.max_retries ?? 0
  });
  await publishRemediationCommands([command]);

  if (approvalState === 'pending_approval') {
    return {
      executionStatus: 'pending_approval',
      outcomeMessage: `remediation command ${command.commandId} requires approval`,
      externalRef: command.commandId,
      remediationJobId: command.commandId
    };
  }

  return {
    executionStatus: 'completed',
    outcomeMessage: `published remediation command ${command.commandId}`,
    externalRef: command.commandId,
    remediationJobId: command.commandId
  };
}

const SHARED_SERVICE_KEY_PLACEHOLDER = 'replace_with_shared_service_key';

function normalizeServiceKey(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  if (!trimmed || trimmed === SHARED_SERVICE_KEY_PLACEHOLDER) return null;
  return trimmed;
}

function configuredServiceKeys(): string[] {
  return Array.from(
    new Set(
      [
        normalizeServiceKey(process.env.RMM_TELEMETRY_SERVICE_KEY),
        normalizeServiceKey(env.serviceKey)
      ].filter((value): value is string => Boolean(value))
    )
  );
}

function hasValidServiceKey(headerValue: unknown): boolean {
  const presented = normalizeServiceKey(headerValue);
  if (!presented) return false;
  return configuredServiceKeys().includes(presented);
}

function hasValidRmmServerKey(headerValue: unknown): boolean {
  const expected = (env.rmmServerApiKey || '').trim();
  if (!expected) return false;
  return typeof headerValue === 'string' && headerValue.trim() === expected;
}

function requireInternalKey(req: any, res: any, opts?: { allowRmmServerKey?: boolean }): boolean {
  const serviceHeader = req.header('x-service-key');
  if (hasValidServiceKey(serviceHeader)) {
    return true;
  }
  if (opts?.allowRmmServerKey) {
    const rmmServerHeader = req.header('x-rmm-server-key');
    if (hasValidRmmServerKey(rmmServerHeader)) {
      return true;
    }
  }
  res.status(401).json({ error: 'Unauthorized' });
  return false;
}

function sendRemediationTransitionFailure(
  res: any,
  result: Exclude<RemediationTransitionResult, { outcome: 'updated' }>,
  nextStatus: string
) {
  if (result.outcome === 'not_found') {
    return res.status(404).json({ error: 'Remediation command not found in the requested scope' });
  }
  if (result.outcome === 'step_not_found') {
    return res.status(404).json({ error: 'Remediation step not found' });
  }
  if (result.outcome === 'conflict') {
    return res.status(409).json({
      error: `Cannot transition remediation command from ${result.currentStatus} to ${nextStatus}`
    });
  }
  if (result.outcome === 'step_conflict') {
    return res.status(409).json({
      error: `Cannot transition remediation step ${result.stepIndex} from ${result.currentStatus} to ${nextStatus}`
    });
  }
  return res.status(400).json({ error: result.error });
}

class RemediationTransitionRejected extends Error {
  constructor(
    readonly result: Exclude<RemediationTransitionResult, { outcome: 'updated' }>,
    readonly nextStatus: string
  ) {
    super(`remediation transition rejected: ${result.outcome}`);
  }
}

function assertUser(req: AuthedRequest, res: any): boolean {
  if (req.jwt?.type !== 'user') {
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

async function requireMembership(req: AuthedRequest, res: any): Promise<MembershipWithOrg | null> {
  if (!assertUser(req, res)) return null;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) {
    res.status(404).json({ error: 'No organization', needsOnboarding: true });
    return null;
  }
  return membership;
}

async function assertUserCanReadDevice(req: AuthedRequest, res: any, agentId: string): Promise<boolean> {
  return Boolean(await requireDeviceReadMembership(req, res, agentId));
}

async function requireDeviceReadMembership(
  req: AuthedRequest,
  res: any,
  agentId: string
): Promise<MembershipWithOrg | null> {
  const membership = await requireMembership(req, res);
  if (!membership) return null;
  const device = await prisma.rmmDevice.findFirst({
    where: {
      agentId,
      organizationId: membership.organizationId
    },
    select: { agentId: true }
  });
  if (!device) {
    res.status(404).json({ error: 'Device not found' });
    return null;
  }
  return membership;
}

type DeviceScope = {
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  hostname: string | null;
};

async function requireDeviceScope(
  tx: Prisma.TransactionClient,
  agentId: string,
  at: Date,
  organizationIdInput?: string | null
): Promise<DeviceScope> {
  const device = await tx.rmmDevice.findUnique({
    where: { agentId },
    select: {
      organizationId: true,
      customerId: true,
      siteId: true,
      hostname: true
    }
  });
  if (!device) {
    throw new DeviceScopeError(`device not found for agentId=${agentId}`);
  }
  if (organizationIdInput && device.organizationId !== organizationIdInput) {
    throw new DeviceScopeError(`organization mismatch for agentId=${agentId}`);
  }
  await tx.rmmDevice.update({
    where: { agentId },
    data: { lastSeen: at }
  });
  return {
    organizationId: device.organizationId,
    customerId: device.customerId,
    siteId: device.siteId,
    hostname: device.hostname
  };
}

async function insertSnapshotManifest(
  tx: Prisma.TransactionClient,
  payload: {
    organizationId: string;
    agentId: string;
    collectedAt: Date;
    receivedAt: Date;
    blobContainer: string;
    blobName: string;
    blobContentEncoding: string | null;
    blobSizeBytes: bigint | null;
  }
): Promise<number> {
  await requireDeviceScope(tx, payload.agentId, payload.collectedAt, payload.organizationId);
  const inserted = await tx.$executeRaw(
    Prisma.sql`
      INSERT INTO rmm_telemetry.snapshot_ingest
        (organization_id, agent_id, collected_at, received_at, blob_container, blob_name, blob_content_encoding, blob_size_bytes)
      VALUES
        (
          ${payload.organizationId},
          ${payload.agentId},
          ${payload.collectedAt},
          ${payload.receivedAt},
          ${payload.blobContainer},
          ${payload.blobName},
          ${payload.blobContentEncoding},
          ${payload.blobSizeBytes}
        )
      ON CONFLICT (agent_id, collected_at) DO NOTHING
    `
  );
  // A manifest only proves the blob landed; completion is recorded after the
  // device_state and detail-table projections are updated.
  return inserted;
}

function normalizeEventInput(agentId: string, value: unknown, fallbackReceivedAt: Date, uniqueSeed: string): NormalizedEvent | null {
  const record = asRecord(value);
  if (!record) return null;

  const occurredAt =
    parseDate(record.occurredAt) ||
    parseDate(record.occurred_at) ||
    parseDate(record.timestamp) ||
    parseDate(record.ts) ||
    fallbackReceivedAt;

  const receivedAt = parseDate(record.receivedAt) || parseDate(record.received_at) || fallbackReceivedAt;

  const eventType =
    readString(record.eventType, record.event_type, record.type, record.kind) ||
    'unknown';
  const severity = readString(record.severity, record.level) || 'info';
  const source = readString(record.source, record.origin) || 'agent';
  const serviceName = readString(record.serviceName, record.service_name, record.service);
  const processName = readString(record.processName, record.process_name, record.process);
  const code = readString(record.code, record.errorCode, record.error_code, record.id);
  const message = readString(record.message, record.description, record.title);

  const attributesSource = asRecord(record.attributes) || record;
  const eventId =
    readString(record.eventId, record.event_id) ||
    sha256Hex(`${agentId}|${eventType}|${occurredAt.toISOString()}|${uniqueSeed}|${toJsonString(attributesSource)}`);

  return {
    eventId,
    occurredAt,
    receivedAt,
    eventType,
    severity,
    source,
    serviceName: serviceName || null,
    processName: processName || null,
    code: code || null,
    message: message || null,
    attributes: attributesSource
  };
}

async function insertEvents(
  tx: Prisma.TransactionClient,
  organizationId: string,
  agentId: string,
  eventsRaw: unknown[],
  fallbackReceivedAt: Date
): Promise<{ inserted: number; parsed: number }> {
  const deviceScope = await requireDeviceScope(tx, agentId, fallbackReceivedAt, organizationId);
  let inserted = 0;
  let parsed = 0;

  for (let i = 0; i < eventsRaw.length; i += 1) {
    const normalized = normalizeEventInput(agentId, eventsRaw[i], fallbackReceivedAt, `event-${i}`);
    if (!normalized) continue;
    parsed += 1;
    const attrsJson = toJsonString(normalized.attributes);
    const rowInserted = await tx.$executeRaw(
      Prisma.sql`
        INSERT INTO rmm_telemetry.device_event
          (
            event_id, organization_id, agent_id, occurred_at, received_at, event_type,
            severity, source, service_name, process_name, code, message, attributes_jsonb
          )
        VALUES
          (
            ${normalized.eventId},
            ${organizationId},
            ${agentId},
            ${normalized.occurredAt},
            ${normalized.receivedAt},
            ${normalized.eventType},
            ${normalized.severity},
            ${normalized.source},
            ${normalized.serviceName},
            ${normalized.processName},
            ${normalized.code},
            ${normalized.message},
            ${attrsJson}::jsonb
          )
        ON CONFLICT (event_id) DO NOTHING
      `
    );
    inserted += rowInserted;
    if (rowInserted > 0) {
      await applyAlertRulesForCandidate(tx, {
        organizationId,
        customerId: deviceScope.customerId,
        siteId: deviceScope.siteId,
        agentId,
        domain: 'event',
        triggerKey: normalized.eventType,
        valueText: [normalized.severity, normalized.code, normalized.message].filter(Boolean).join(' '),
        severity: normalizeAlertSeverity(normalized.severity),
        sourceEventId: normalized.eventId,
        title: normalized.message
          ? `${normalized.eventType}: ${normalized.message}`
          : `Telemetry event: ${normalized.eventType}`,
        summary: normalized.code ? `Code ${normalized.code}` : normalized.source,
        metadata: {
          eventId: normalized.eventId,
          eventType: normalized.eventType,
          source: normalized.source,
          code: normalized.code,
          attributes: normalized.attributes
        }
      });
    }
  }

  return { inserted, parsed };
}

type DeviceBaselineAggregateRow = {
  agent_id: string;
  hostname: string;
  organization_id: string;
  customer_id: string | null;
  site_id: string | null;
  fact_key: string;
  promoted_value: unknown;
  last_changed_at: Date | null;
  updated_at: Date;
};

type ScopeAggregateValue = {
  value: unknown;
  count: number;
  lastChangedAt: Date | null;
};

type ScopeAggregate = {
  scopeType: ScopedBaselineWriteType;
  scopeKey: string;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  factKey: string;
  totalCount: number;
  values: Map<string, ScopeAggregateValue>;
};

type ScopedBaselineRecomputeResult = {
  organizationId: string;
  processedRows: number;
  persistedRows: number;
  scopeCounts: {
    organization: number;
    customer: number;
    site: number;
  };
  minDevices: number;
  minSupportRatio: number;
  recomputedAt: string;
};

function maxDate(a: Date | null, b: Date | null): Date | null {
  if (!a) return b;
  if (!b) return a;
  return a.getTime() >= b.getTime() ? a : b;
}

function round4(value: number): number {
  return Math.round(value * 10000) / 10000;
}

function supportConfidence(supportRatio: number, sampleSize: number): number {
  if (sampleSize <= 0) return 0;
  const depthFactor = Math.min(1, Math.log10(sampleSize + 1) / 2);
  return round4(supportRatio * depthFactor);
}

function addAggregate(
  map: Map<string, ScopeAggregate>,
  row: DeviceBaselineAggregateRow,
  scopeType: ScopedBaselineWriteType,
  scopeKey: string,
  customerId: string | null,
  siteId: string | null
) {
  const mapKey = `${scopeType}|${scopeKey}|${row.fact_key}`;
  let aggregate = map.get(mapKey);
  if (!aggregate) {
    aggregate = {
      scopeType,
      scopeKey,
      organizationId: row.organization_id,
      customerId,
      siteId,
      factKey: row.fact_key,
      totalCount: 0,
      values: new Map()
    };
    map.set(mapKey, aggregate);
  }

  aggregate.totalCount += 1;
  const valueKey = stableJsonValueKey(row.promoted_value);
  const currentValue = aggregate.values.get(valueKey);
  if (currentValue) {
    currentValue.count += 1;
    currentValue.lastChangedAt = maxDate(currentValue.lastChangedAt, row.last_changed_at);
  } else {
    aggregate.values.set(valueKey, {
      value: row.promoted_value,
      count: 1,
      lastChangedAt: row.last_changed_at
    });
  }
}

async function recomputeScopedBaselinesForOrganization(
  organizationId: string
): Promise<ScopedBaselineRecomputeResult> {
  return prisma.$transaction(async (tx) => {
    await tx.$executeRaw(Prisma.sql`
      SELECT pg_advisory_xact_lock(hashtext(${organizationId}))
    `);

    const rows = await tx.$queryRaw<DeviceBaselineAggregateRow[]>(Prisma.sql`
      SELECT
        d.agent_id,
        d.hostname,
        d.organization_id,
        d.customer_id,
        d.site_id,
        b.fact_key,
        b.promoted_value,
        b.last_changed_at,
        b.updated_at
      FROM rmm_telemetry.fact_baseline b
      INNER JOIN public.rmm_devices d
        ON d.agent_id = b.agent_id
      WHERE d.organization_id = ${organizationId}
        AND b.promoted_value IS NOT NULL
    `);

    const aggregateMap = new Map<string, ScopeAggregate>();
    for (const row of rows) {
      addAggregate(aggregateMap, row, 'organization', row.organization_id, null, null);
      if (row.customer_id) {
        addAggregate(aggregateMap, row, 'customer', row.customer_id, row.customer_id, null);
      }
      if (row.site_id) {
        addAggregate(aggregateMap, row, 'site', row.site_id, row.customer_id, row.site_id);
      }
    }

    const now = new Date();
    const writes: Prisma.RmmTelemetryFactBaselineScopeCreateManyInput[] = [];
    const scopeCounts = {
      organization: new Set<string>(),
      customer: new Set<string>(),
      site: new Set<string>()
    };

    for (const aggregate of aggregateMap.values()) {
      const candidates = [...aggregate.values.entries()]
        .map(([valueKey, value]) => ({ valueKey, ...value }))
        .sort((a, b) => {
          if (a.count !== b.count) return b.count - a.count;
          return a.valueKey.localeCompare(b.valueKey);
        });

      const winner = candidates[0];
      if (!winner) continue;

      const totalCount = aggregate.totalCount;
      const supportCount = winner.count;
      const supportRatio = totalCount > 0 ? supportCount / totalCount : 0;
      const sampleSize = totalCount;
      const isStable =
        totalCount >= SCOPE_BASELINE_MIN_DEVICES &&
        supportRatio >= SCOPE_BASELINE_MIN_SUPPORT_RATIO;

      writes.push({
        scopeType: aggregate.scopeType,
        scopeKey: aggregate.scopeKey,
        organizationId: aggregate.organizationId,
        customerId: aggregate.customerId,
        siteId: aggregate.siteId,
        agentId: null,
        factKey: aggregate.factKey,
        promotedValue: winner.value as Prisma.InputJsonValue,
        candidateValue: winner.value as Prisma.InputJsonValue,
        candidateCount: supportCount,
        windowCount: totalCount,
        supportCount,
        totalCount,
        supportRatio: round4(supportRatio),
        sampleSize,
        confidenceScore: supportConfidence(supportRatio, sampleSize),
        isStable,
        lastChangedAt: winner.lastChangedAt,
        updatedAt: now
      });

      if (aggregate.scopeType === 'organization') scopeCounts.organization.add(aggregate.scopeKey);
      if (aggregate.scopeType === 'customer') scopeCounts.customer.add(aggregate.scopeKey);
      if (aggregate.scopeType === 'site') scopeCounts.site.add(aggregate.scopeKey);
    }

    await tx.rmmTelemetryFactBaselineScope.deleteMany({
      where: { organizationId }
    });
    if (writes.length > 0) {
      await tx.rmmTelemetryFactBaselineScope.createMany({
        data: writes
      });
    }

    return {
      organizationId,
      processedRows: rows.length,
      persistedRows: writes.length,
      scopeCounts: {
        organization: scopeCounts.organization.size,
        customer: scopeCounts.customer.size,
        site: scopeCounts.site.size
      },
      minDevices: SCOPE_BASELINE_MIN_DEVICES,
      minSupportRatio: SCOPE_BASELINE_MIN_SUPPORT_RATIO,
      recomputedAt: now.toISOString()
    };
  });
}

async function recomputeScopedBaselinesForAgent(
  agentId: string
): Promise<ScopedBaselineRecomputeResult | null> {
  const scope = await prisma.rmmDevice.findUnique({
    where: { agentId },
    select: {
      organizationId: true
    }
  });
  const organizationId = scope?.organizationId || null;
  if (!organizationId) return null;
  return recomputeScopedBaselinesForOrganization(organizationId);
}

async function validateScopeAccess(
  membership: MembershipWithOrg,
  scopeType: BaselineScopeType,
  scopeIdRaw: string | null
): Promise<{
  scopeType: BaselineScopeType;
  scopeId: string;
  scopeName: string;
  where: Prisma.RmmTelemetryFactBaselineScopeWhereInput | null;
}> {
  if (scopeType === 'organization') {
    const scopeId = scopeIdRaw || membership.organizationId;
    if (scopeId !== membership.organizationId) {
      throw new Error('organization scope not found');
    }
    return {
      scopeType,
      scopeId,
      scopeName: membership.organization.name,
      where: {
        scopeType: 'organization',
        scopeKey: scopeId,
        organizationId: scopeId
      }
    };
  }

  if (!scopeIdRaw) {
    throw new Error('scopeId is required');
  }

  if (scopeType === 'customer') {
    const customer = await prisma.customer.findFirst({
      where: {
        id: scopeIdRaw,
        organizationId: membership.organizationId
      },
      select: { id: true, name: true }
    });
    if (!customer) {
      throw new Error('customer scope not found');
    }
    return {
      scopeType,
      scopeId: customer.id,
      scopeName: customer.name,
      where: {
        scopeType: 'customer',
        scopeKey: customer.id,
        organizationId: membership.organizationId
      }
    };
  }

  if (scopeType === 'site') {
    const site = await prisma.rmmSite.findFirst({
      where: {
        id: scopeIdRaw,
        customer: {
          organizationId: membership.organizationId
        }
      },
      select: { id: true, name: true }
    });
    if (!site) {
      throw new Error('site scope not found');
    }
    return {
      scopeType,
      scopeId: site.id,
      scopeName: site.name,
      where: {
        scopeType: 'site',
        scopeKey: site.id,
        organizationId: membership.organizationId
      }
    };
  }

  return {
    scopeType,
    scopeId: scopeIdRaw,
    scopeName: scopeIdRaw,
    where: null
  };
}

rmmTelemetryRouter.post('/manifest/snapshots', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const collectedAt = parseDate(body.collectedAt || body.collected_at);
  const receivedAt = parseDate(body.receivedAt || body.received_at) || new Date();
  const blobContainer = readString(body.blobContainer, body.blob_container);
  const blobName = readString(body.blobName, body.blob_name);
  const blobContentEncoding = readString(body.blobContentEncoding, body.blob_content_encoding);
  const blobSizeBytes = parseBigIntValue(body.blobSizeBytes ?? body.blob_size_bytes);

  if (!organizationId || !agentId || !collectedAt || !blobContainer || !blobName) {
    return res.status(400).json({
      error: 'organizationId, agentId, collectedAt, blobContainer, and blobName are required'
    });
  }
  if (blobSizeBytes !== null && blobSizeBytes < 0n) {
    return res.status(400).json({ error: 'blobSizeBytes must be a non-negative integer' });
  }

  const inserted = await prisma.$transaction(async (tx) => {
    return insertSnapshotManifest(tx, {
      organizationId,
      agentId,
      collectedAt,
      receivedAt,
      blobContainer,
      blobName,
      blobContentEncoding: blobContentEncoding || null,
      blobSizeBytes
    });
  });

  return res.status(202).json({
    accepted: true,
    duplicate: inserted === 0
  });
});

rmmTelemetryRouter.post('/events/batch', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const eventsRaw = asArray(body.events);
  const receivedAt = parseDate(body.receivedAt || body.received_at) || new Date();

  if (!organizationId || !agentId) {
    return res.status(400).json({ error: 'organizationId and agentId are required' });
  }
  if (eventsRaw.length === 0) {
    return res.status(202).json({ accepted: true, inserted: 0, duplicate: true });
  }
  if (eventsRaw.length > 2000) {
    return res.status(400).json({ error: 'events batch too large (max 2000)' });
  }

  const result = await prisma.$transaction(async (tx) => {
    return insertEvents(tx, organizationId, agentId, eventsRaw, receivedAt);
  });

  return res.status(202).json({
    accepted: true,
    inserted: result.inserted,
    duplicate: result.inserted === 0,
    parsed: result.parsed
  });
});

rmmTelemetryRouter.post('/messages/processed', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const source = asRecord(body.source) || body;
  const sourceTopic = readString(source.topic, source.sourceTopic, source.source_topic);
  const sourcePartition = parseInteger(source.partition ?? source.sourcePartition ?? source.source_partition);
  const sourceOffset = parseBigIntValue(source.offset ?? source.sourceOffset ?? source.source_offset);

  if (!sourceTopic || sourcePartition === null || sourceOffset === null) {
    return res.status(400).json({
      error: 'source(topic, partition, offset) is required'
    });
  }

  const rows = await prisma.$queryRaw<
    Array<{
      processed_at: Date;
      message_type: string;
      organization_id: string;
      agent_id: string;
    }>
  >(
    Prisma.sql`
      SELECT processed_at, message_type, organization_id, agent_id
      FROM rmm_telemetry.processed_message_log
      WHERE source_topic = ${sourceTopic}
        AND source_partition = ${sourcePartition}
        AND source_offset = ${sourceOffset}
      LIMIT 1
    `
  );

  const row = rows[0] ?? null;
  return res.json({
    accepted: true,
    processed: Boolean(row),
    processedAt: row ? row.processed_at.toISOString() : null,
    messageType: row?.message_type ?? null,
    organizationId: row?.organization_id ?? null,
    agentId: row?.agent_id ?? null
  });
});

rmmTelemetryRouter.post('/graph/apply-batch', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const source = asRecord(body.source);
  const sourceTopic = readString(source?.topic, source?.sourceTopic, source?.source_topic);
  const sourcePartition = parseInteger(source?.partition ?? source?.sourcePartition ?? source?.source_partition);
  const sourceOffset = parseBigIntValue(source?.offset ?? source?.sourceOffset ?? source?.source_offset);
  const sourceTs = parseDate(source?.ts ?? source?.sourceTs ?? source?.source_ts ?? source?.timestamp) || new Date();
  const sourceKey = readString(source?.key, source?.sourceKey, source?.source_key);
  const messageType = readString(source?.messageType, source?.message_type, source?.type) || 'telemetry';
  const payloadSha = readString(body.idempotencyKey, body.idempotency_key) || sha256Hex(toJsonString(body));

  if (!organizationId || !agentId || !sourceTopic || sourcePartition === null || sourceOffset === null) {
    return res.status(400).json({
      error: 'organizationId, agentId and source(topic, partition, offset) are required'
    });
  }

  const facts = asArray(body.facts);
  const changes = asArray(body.changes);
  const baselines = asArray(body.baselines);
  const decision = asRecord(body.decision);

  const transactionResult = await prisma.$transaction(async (tx) => {
    const deviceScope = await requireDeviceScope(tx, agentId, sourceTs, organizationId);

    const processedRows = await tx.$queryRaw<{ id: bigint }[]>(
      Prisma.sql`
        INSERT INTO rmm_telemetry.processed_message_log
          (
            source_topic, source_partition, source_offset, source_ts, source_key,
            organization_id, agent_id, message_type, payload_sha256
          )
        VALUES
          (
            ${sourceTopic},
            ${sourcePartition},
            ${sourceOffset},
            ${sourceTs},
            ${sourceKey},
            ${organizationId},
            ${agentId},
            ${messageType},
            ${payloadSha}
          )
        ON CONFLICT (source_topic, source_partition, source_offset) DO NOTHING
        RETURNING id
      `
    );

    if (processedRows.length === 0) {
      return {
        duplicate: true,
        appliedFacts: 0,
        appliedChanges: 0,
        appliedBaselines: 0,
        decisionId: null as string | null,
        alertRulesMatched: 0,
        alertsTouched: 0
      };
    }

    let alertRulesMatched = 0;
    let alertsTouched = 0;
    let appliedFacts = 0;
    for (const rawFact of facts) {
      const fact = asRecord(rawFact);
      if (!fact) continue;
      const factKey = readString(fact.factKey, fact.fact_key);
      if (!factKey) continue;
      const factValue = Object.prototype.hasOwnProperty.call(fact, 'factValue')
        ? fact.factValue
        : fact.fact_value;
      const factJson = toJsonString(factValue);
      const factValueText = factJson;
      const stabilityClass = readString(fact.stabilityClass, fact.stability_class) || 'stable';
      const factSource = readString(fact.source) || messageType;
      const factSourceTs = parseDate(fact.sourceTs || fact.source_ts) || sourceTs;
      await tx.$executeRaw(
        Prisma.sql`
          INSERT INTO rmm_telemetry.fact_state_current
            (organization_id, agent_id, fact_key, fact_value, fact_value_text, stability_class, source, source_ts, updated_at)
          VALUES
            (${organizationId}, ${agentId}, ${factKey}, ${factJson}::jsonb, ${factValueText}, ${stabilityClass}, ${factSource}, ${factSourceTs}, NOW())
          ON CONFLICT (agent_id, fact_key)
          DO UPDATE SET
            fact_value = EXCLUDED.fact_value,
            fact_value_text = EXCLUDED.fact_value_text,
            stability_class = EXCLUDED.stability_class,
            source = EXCLUDED.source,
            source_ts = EXCLUDED.source_ts,
            updated_at = NOW()
        `
      );
      appliedFacts += 1;
    }

    let appliedChanges = 0;
    for (const rawChange of changes) {
      const change = asRecord(rawChange);
      if (!change) continue;
      const factKey = readString(change.factKey, change.fact_key);
      if (!factKey) continue;
      const prevValue = Object.prototype.hasOwnProperty.call(change, 'previousValue')
        ? change.previousValue
        : change.prev_value;
      const nextValue = Object.prototype.hasOwnProperty.call(change, 'nextValue')
        ? change.nextValue
        : change.next_value;
      const prevJson = toJsonString(prevValue);
      const nextJson = toJsonString(nextValue);
      const changeKind = readString(change.changeKind, change.change_kind) || 'update';
      const changeSource = readString(change.source) || messageType;
      const changeSourceTs = parseDate(change.sourceTs || change.source_ts) || sourceTs;

      await tx.$executeRaw(
        Prisma.sql`
          INSERT INTO rmm_telemetry.fact_change_log
            (organization_id, agent_id, fact_key, prev_value, next_value, change_kind, source, source_ts, ts)
          VALUES
            (${organizationId}, ${agentId}, ${factKey}, ${prevJson}::jsonb, ${nextJson}::jsonb, ${changeKind}, ${changeSource}, ${changeSourceTs}, NOW())
        `
      );
      appliedChanges += 1;
      const alertResult = await applyAlertRulesForCandidate(tx, {
        organizationId,
        customerId: deviceScope.customerId,
        siteId: deviceScope.siteId,
        agentId,
        domain: 'baseline',
        triggerKey: factKey,
        valueText: toRoutingValueText(nextValue),
        severity: readAlertCandidateSeverity(change.severity, change.level, nextValue),
        sourceFactKey: factKey,
        title: `Fact changed: ${factKey}`,
        summary: `${changeKind}: ${valuePreview(prevValue)} -> ${valuePreview(nextValue)}`,
        metadata: {
          factKey,
          previousValue: prevValue,
          nextValue,
          changeKind,
          source: changeSource
        }
      });
      alertRulesMatched += alertResult.matchedRules;
      alertsTouched += alertResult.alertsTouched;
    }

    let appliedBaselines = 0;
    for (const rawBaseline of baselines) {
      const baseline = asRecord(rawBaseline);
      if (!baseline) continue;
      const factKey = readString(baseline.factKey, baseline.fact_key);
      if (!factKey) continue;
      const promotedJson = toJsonString(
        Object.prototype.hasOwnProperty.call(baseline, 'promotedValue')
          ? baseline.promotedValue
          : baseline.promoted_value
      );
      const candidateJson = toJsonString(
        Object.prototype.hasOwnProperty.call(baseline, 'candidateValue')
          ? baseline.candidateValue
          : baseline.candidate_value
      );
      const candidateCount = parseInteger(baseline.candidateCount ?? baseline.candidate_count) ?? 0;
      const windowCount = parseInteger(baseline.windowCount ?? baseline.window_count) ?? 0;
      const lastChangedAt = parseDate(baseline.lastChangedAt || baseline.last_changed_at);

      await tx.$executeRaw(
        Prisma.sql`
          INSERT INTO rmm_telemetry.fact_baseline
            (
              agent_id, fact_key, promoted_value, candidate_value,
              organization_id,
              candidate_count, window_count, last_changed_at, updated_at
            )
          VALUES
            (
              ${agentId}, ${factKey}, ${promotedJson}::jsonb, ${candidateJson}::jsonb, ${organizationId},
              ${candidateCount}, ${windowCount}, ${lastChangedAt}, NOW()
            )
          ON CONFLICT (agent_id, fact_key)
          DO UPDATE SET
            promoted_value = EXCLUDED.promoted_value,
            candidate_value = EXCLUDED.candidate_value,
            candidate_count = EXCLUDED.candidate_count,
            window_count = EXCLUDED.window_count,
            last_changed_at = EXCLUDED.last_changed_at,
            updated_at = NOW()
        `
      );
      appliedBaselines += 1;
    }

    let decisionId: string | null = null;
    if (decision) {
      const domain = readString(decision.domain) || 'event';
      const triggerKey = readString(decision.triggerKey, decision.trigger_key) || 'unknown';
      const triggerJson = toJsonString(
        Object.prototype.hasOwnProperty.call(decision, 'triggerValue')
          ? decision.triggerValue
          : decision.trigger_value
      );
      const action = readString(decision.action) || 'ignore';
      const matchedRuleId = parseBigIntValue(decision.matchedRuleId ?? decision.matched_rule_id);
      const intentId = readString(decision.intentId, decision.intent_id);
      const reason = readString(decision.reason);
      const dedupeKey = readString(decision.dedupeKey, decision.dedupe_key);
      const decisionSource = readString(decision.source) || messageType;
      const decisionSourceTs = parseDate(decision.sourceTs || decision.source_ts) || sourceTs;

      const insertedDecision = await tx.$queryRaw<{ id: bigint }[]>(
        Prisma.sql`
          INSERT INTO rmm_telemetry.routing_decision
            (
              organization_id, agent_id, domain, trigger_key, trigger_value, action,
              matched_rule_id, intent_id, reason, dedupe_key, source, source_ts, decided_at
            )
          VALUES
            (
              ${organizationId}, ${agentId}, ${domain}, ${triggerKey}, ${triggerJson}::jsonb, ${action},
              ${matchedRuleId}, ${intentId}, ${reason}, ${dedupeKey}, ${decisionSource}, ${decisionSourceTs}, NOW()
            )
          RETURNING id
        `
      );
      const decisionIdBigInt = insertedDecision[0]?.id ?? null;
      decisionId = decisionIdBigInt ? String(decisionIdBigInt) : null;
      const alertDomain = normalizeAlertSourceDomain(domain) || 'decision';
      const alertResult = await applyAlertRulesForCandidate(tx, {
        organizationId,
        customerId: deviceScope.customerId,
        siteId: deviceScope.siteId,
        agentId,
        domain: alertDomain,
        triggerKey,
        valueText: toRoutingValueText(
          Object.prototype.hasOwnProperty.call(decision, 'triggerValue')
            ? decision.triggerValue
            : decision.trigger_value
        ),
        severity: readAlertCandidateSeverity(decision.triggerValue, decision.trigger_value, decision.severity, decision.level),
        sourceDecisionId: decisionIdBigInt,
        title: `Routing decision: ${triggerKey}`,
        summary: `${action}: ${reason || 'matched telemetry rule'}`,
        metadata: {
          decisionId,
          matchedRuleId: matchedRuleId ? matchedRuleId.toString() : null,
          action,
          reason
        }
      });
      alertRulesMatched += alertResult.matchedRules;
      alertsTouched += alertResult.alertsTouched;
    }

    return {
      duplicate: false,
      appliedFacts,
      appliedChanges,
      appliedBaselines,
      decisionId,
      alertRulesMatched,
      alertsTouched
    };
  });

  let scopedBaselineRecompute: ScopedBaselineRecomputeResult | null = null;
  if (!transactionResult.duplicate) {
    try {
      scopedBaselineRecompute = await recomputeScopedBaselinesForAgent(agentId);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      console.warn('scoped baseline recompute failed', msg);
    }
  }

  return res.status(202).json({
    accepted: true,
    duplicate: transactionResult.duplicate,
    applied: !transactionResult.duplicate,
    appliedFacts: transactionResult.appliedFacts,
    appliedChanges: transactionResult.appliedChanges,
    appliedBaselines: transactionResult.appliedBaselines,
    decisionId: transactionResult.decisionId,
    alertRulesMatched: transactionResult.alertRulesMatched,
    alertsTouched: transactionResult.alertsTouched,
    scopedBaselineRecompute
  });
});

rmmTelemetryRouter.post('/internal/recompute-baselines', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const agentId = readString(body.agentId, body.agent_id);
  const organizationIdInput = readString(body.organizationId, body.organization_id);

  if (!agentId && !organizationIdInput) {
    return res.status(400).json({
      error: 'agentId or organizationId is required'
    });
  }

  try {
    let result: ScopedBaselineRecomputeResult | null = null;
    if (organizationIdInput) {
      result = await recomputeScopedBaselinesForOrganization(organizationIdInput);
    } else if (agentId) {
      result = await recomputeScopedBaselinesForAgent(agentId);
    }

    return res.status(202).json({
      accepted: true,
      recomputed: Boolean(result),
      result
    });
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return res.status(500).json({ error: `recompute failed: ${msg}` });
  }
});

rmmTelemetryRouter.post('/remediation/jobs', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const commandId = readString(body.commandId, body.command_id) || crypto.randomUUID();
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const decisionId = parseBigIntValue(body.decisionId ?? body.decision_id);
  const intentId = readString(body.intentId, body.intent_id);
  const dedupeKey = readString(body.dedupeKey, body.dedupe_key);
  const requestedBy = readString(body.requestedBy, body.requested_by) || 'consumer';
  const metadata = asRecord(body.metadata) || {};
  const steps = asArray(body.steps);

  if (!organizationId || !agentId || !intentId) {
    return res.status(400).json({ error: 'organizationId, agentId and intentId are required' });
  }

  const intent = await loadRoutingIntentSummary(organizationId, intentId);
  if (!intent) {
    return res.status(404).json({ error: 'Intent not found' });
  }
  if (!intent.enabled) {
    return res.status(400).json({ error: 'Intent is disabled' });
  }

  const approvalState = intent.requires_approval ? 'pending_approval' : 'approved';
  const normalizedSteps = normalizeCommandSteps(steps.length > 0 ? steps : intent.steps, intent.allow_list, intent.timeout_seconds);
  const command = buildRemediationCommandEvent({
    commandId,
    organizationId,
    agentId,
    decisionId,
    intentId,
    dedupeKey,
    requestedBy,
    approvalState,
    metadata,
    steps: normalizedSteps,
    timeoutSeconds: intent.timeout_seconds,
    maxRetries: intent.max_retries
  });
  await publishRemediationCommands([command]);

  return res.status(202).json({
    accepted: true,
    commandId,
    jobId: commandId,
    status: approvalState === 'pending_approval' ? 'pending_approval' : 'queued',
    insertedSteps: normalizedSteps.length
  });
});

rmmTelemetryRouter.post('/remediation/commands/project', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const body = asRecord(req.body) || {};
  const commandId = readString(body.commandId, body.command_id);
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const decisionId = parseBigIntValue(body.decisionId ?? body.decision_id);
  const intentId = readString(body.intentId, body.intent_id);
  const dedupeKey = readString(body.dedupeKey, body.dedupe_key);
  const requestedBy = readString(body.requestedBy, body.requested_by) || 'consumer';
  const approvalState = readString(body.approvalState, body.approval_state) || 'approved';
  const status = approvalState === 'pending_approval' ? 'pending_approval' : 'queued';
  const metadata = asRecord(body.metadata) || {};
  const steps = asArray(body.steps);
  const execution = asRecord(body.execution) || {};

  if (!commandId || !organizationId || !agentId || !intentId) {
    return res.status(400).json({ error: 'commandId, organizationId, agentId and intentId are required' });
  }

  let result: Awaited<ReturnType<typeof insertRemediationJob>>;
  try {
    result = await prisma.$transaction(async (tx) => {
      return insertRemediationJob(tx, {
        commandId,
        organizationId,
        agentId,
        decisionId,
        intentId,
        status,
        dedupeKey,
        requestedBy,
        metadata,
        steps,
        execution
      });
    });
  } catch (error) {
    if (error instanceof RemediationProjectionScopeError) {
      return res.status(404).json({ error: error.message });
    }
    const databaseCode = readString(
      (error as { code?: unknown })?.code,
      (error as { meta?: { code?: unknown } })?.meta?.code
    );
    if (
      error instanceof RemediationProjectionConflictError
      || databaseCode === '23505'
      || databaseCode === 'P2002'
    ) {
      return res.status(409).json({
        error: 'commandId or dedupeKey is already owned by another remediation job'
      });
    }
    throw error;
  }

  return res.status(202).json({
    accepted: true,
    commandId,
    jobId: result.jobId,
    status: result.status,
    insertedSteps: result.insertedSteps
  });
});

rmmTelemetryRouter.post('/remediation/commands/status', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const body = asRecord(req.body) || {};
  const statuses = asArray(body.statuses).length > 0 ? asArray(body.statuses) : [body];
  if (statuses.length > 100) {
    return res.status(413).json({ error: 'at most 100 remediation statuses are accepted per request' });
  }

  const parsedStatuses: Array<{
    commandId: string;
    organizationId: string;
    agentId: string;
    report: Extract<ReturnType<typeof parseRemediationStatusReport>, { ok: true }>['report'];
  }> = [];
  for (const item of statuses) {
    const record = asRecord(item);
    if (!record) return res.status(400).json({ error: 'remediation status must be an object' });
    const commandId = readString(record.commandId, record.command_id);
    const organizationId = readString(record.organizationId, record.organization_id);
    const agentId = readString(record.agentId, record.agent_id);
    if (!commandId || !organizationId || !agentId) {
      return res.status(400).json({
        error: 'commandId, organizationId and agentId are required'
      });
    }
    const parsed = parseRemediationStatusReport(record);
    if (!parsed.ok) {
      return res.status(parsed.httpStatus).json({ error: parsed.error });
    }
    parsedStatuses.push({ commandId, organizationId, agentId, report: parsed.report });
  }

  let updated: number;
  try {
    updated = await prisma.$transaction(async (tx) => {
      for (const item of parsedStatuses) {
        const result = await transitionRemediationStatus(tx, {
          commandId: item.commandId,
          organizationId: item.organizationId,
          agentId: item.agentId,
          intentScope: 'generic'
        }, item.report);
        if (result.outcome !== 'updated') {
          throw new RemediationTransitionRejected(result, item.report.status);
        }
      }
      return parsedStatuses.length;
    });
  } catch (error) {
    if (error instanceof RemediationTransitionRejected) {
      return sendRemediationTransitionFailure(res, error.result, error.nextStatus);
    }
    throw error;
  }

  return res.status(202).json({ accepted: true, updated });
});

rmmTelemetryRouter.get('/rules/:agentId', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  const agentId = readString(req.params.agentId);
  const organizationIdInput = readString(req.query.organizationId, req.query.organization_id);
  if (!agentId || !organizationIdInput) return res.status(400).json({ error: 'agentId and organizationId are required' });

  const device = await prisma.rmmDevice.findUnique({
    where: { agentId },
    include: {
      customer: {
        select: { id: true, organizationId: true }
      }
    }
  });

  if (!device) {
    return res.status(404).json({ error: 'Device not found' });
  }

  const customerId = device.customerId || null;
  const siteId = device.siteId || null;
  const organizationId = device.organizationId;
  if (organizationId !== organizationIdInput) {
    return res.status(403).json({ error: 'organization mismatch' });
  }
  const hasCustomer = customerId !== null;
  const hasSite = siteId !== null;

  let rules: RoutingRuleRow[] = [];

  try {
    rules = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
      SELECT
        id,
        organization_id,
        customer_id,
        site_id,
        agent_id,
        trigger_domain,
        trigger_key,
        match_operator,
        match_value,
        previous_match_operator,
        previous_match_value,
        min_support_ratio,
        min_confidence_score,
        scope_type_filter,
        action,
        intent_id,
        cooldown_seconds,
        enabled,
        priority,
        created_at,
        updated_at
      FROM rmm_telemetry.routing_rule
      WHERE enabled = TRUE
        AND organization_id = ${organizationId}
        AND (agent_id IS NULL OR agent_id = ${agentId})
        AND (
          customer_id IS NULL
          OR (${hasCustomer} = TRUE AND customer_id = ${customerId})
        )
        AND (
          site_id IS NULL
          OR (${hasSite} = TRUE AND site_id = ${siteId})
        )
      ORDER BY
        CASE
          WHEN agent_id = ${agentId} THEN 1
          WHEN (${hasSite} = TRUE AND site_id = ${siteId} AND agent_id IS NULL) THEN 2
          WHEN (${hasCustomer} = TRUE AND customer_id = ${customerId} AND site_id IS NULL AND agent_id IS NULL) THEN 3
          WHEN (customer_id IS NULL AND site_id IS NULL AND agent_id IS NULL) THEN 4
          ELSE 99
        END ASC,
        priority ASC,
        id ASC
    `);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    console.warn('routing rules query failed, continuing with empty rules', msg);
    rules = [];
  }

  const currentFacts = await prisma.$queryRaw<
    Array<{
      fact_key: string;
      fact_value: unknown;
      fact_value_text: string;
      stability_class: string;
      source: string;
      source_ts: Date;
      updated_at: Date;
    }>
  >(Prisma.sql`
    SELECT
      fact_key,
      fact_value,
      fact_value_text,
      stability_class,
      source,
      source_ts,
      updated_at
    FROM rmm_telemetry.fact_state_current
    WHERE agent_id = ${agentId}
    ORDER BY fact_key ASC
  `);

  const baselines = await prisma.$queryRaw<
    Array<{
      fact_key: string;
      promoted_value: unknown;
      candidate_value: unknown;
      candidate_count: number;
      window_count: number;
      last_changed_at: Date | null;
      updated_at: Date;
    }>
  >(Prisma.sql`
    SELECT
      fact_key,
      promoted_value,
      candidate_value,
      candidate_count,
      window_count,
      last_changed_at,
      updated_at
    FROM rmm_telemetry.fact_baseline
    WHERE agent_id = ${agentId}
    ORDER BY fact_key ASC
  `);

  const recentDecisions = await prisma.$queryRaw<
    Array<{
      matched_rule_id: bigint;
      decided_at: Date;
    }>
  >(Prisma.sql`
    SELECT matched_rule_id, MAX(decided_at) as decided_at
    FROM rmm_telemetry.routing_decision
    WHERE agent_id = ${agentId}
      AND matched_rule_id IS NOT NULL
      AND decided_at > NOW() - INTERVAL '24 hours'
    GROUP BY matched_rule_id
  `);

  let scopeBaselines: Array<{
    fact_key: string;
    promoted_value: unknown;
    scope_type: string;
    support_ratio: number;
    sample_size: number;
    confidence_score: number;
    is_stable: boolean;
  }> = [];
  let stabilityOverrides: Array<{
    fact_key_pattern: string;
    stability_class: string;
  }> = [];

  try {
    const scopeQuery = siteId
      ? Prisma.sql`
          SELECT DISTINCT ON (fact_key)
            fact_key, promoted_value, scope_type, support_ratio, sample_size, confidence_score, is_stable
          FROM rmm_telemetry.fact_baseline_scope
          WHERE organization_id = ${organizationId}
            AND is_stable = TRUE
            AND (
              (scope_type = 'site' AND scope_key = ${siteId})
              OR (scope_type = 'customer' AND scope_key = ${customerId})
              OR (scope_type = 'organization' AND scope_key = ${organizationId})
            )
          ORDER BY fact_key,
            CASE scope_type WHEN 'site' THEN 1 WHEN 'customer' THEN 2 ELSE 3 END ASC
        `
      : customerId
      ? Prisma.sql`
          SELECT DISTINCT ON (fact_key)
            fact_key, promoted_value, scope_type, support_ratio, sample_size, confidence_score, is_stable
          FROM rmm_telemetry.fact_baseline_scope
          WHERE organization_id = ${organizationId}
            AND is_stable = TRUE
            AND (
              (scope_type = 'customer' AND scope_key = ${customerId})
              OR (scope_type = 'organization' AND scope_key = ${organizationId})
            )
          ORDER BY fact_key,
            CASE scope_type WHEN 'customer' THEN 1 ELSE 2 END ASC
        `
      : Prisma.sql`
          SELECT fact_key, promoted_value, scope_type, support_ratio, sample_size, confidence_score, is_stable
          FROM rmm_telemetry.fact_baseline_scope
          WHERE organization_id = ${organizationId}
            AND is_stable = TRUE
            AND scope_type = 'organization'
            AND scope_key = ${organizationId}
          ORDER BY fact_key
        `;
    scopeBaselines = await prisma.$queryRaw(scopeQuery);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    console.warn('scope baselines query failed, continuing with empty', msg);
  }

  try {
    stabilityOverrides = await prisma.$queryRaw<
      Array<{
        fact_key_pattern: string;
        stability_class: string;
      }>
    >(Prisma.sql`
      SELECT fact_key_pattern, stability_class
      FROM rmm_telemetry.fact_stability_override
      WHERE organization_id = ${organizationId}
      ORDER BY fact_key_pattern ASC
    `);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    console.warn('stability overrides query failed, continuing with empty overrides', msg);
  }

  return res.json({
    scope: {
      organizationId,
      customerId,
      siteId,
      agentId
    },
    rules: rules.map((rule) => ({
      id: String(rule.id),
      organizationId: rule.organization_id,
      customerId: rule.customer_id,
      siteId: rule.site_id,
      agentId: rule.agent_id,
      triggerDomain: rule.trigger_domain,
      triggerKey: rule.trigger_key,
      matchOperator: rule.match_operator,
      matchValue: rule.match_value,
      previousMatchOperator: rule.previous_match_operator,
      previousMatchValue: rule.previous_match_value,
      minSupportRatio: rule.min_support_ratio,
      minConfidenceScore: rule.min_confidence_score,
      scopeTypeFilter: rule.scope_type_filter,
      action: rule.action,
      intentId: rule.intent_id,
      cooldownSeconds: rule.cooldown_seconds,
      enabled: readBoolean(rule.enabled) ?? true,
      priority: rule.priority
    })),
    currentFacts: currentFacts.map((fact) => ({
      factKey: fact.fact_key,
      factValue: fact.fact_value,
      factValueText: fact.fact_value_text,
      stabilityClass: fact.stability_class,
      source: fact.source,
      sourceTs: fact.source_ts.toISOString(),
      updatedAt: fact.updated_at.toISOString()
    })),
    baselines: baselines.map((baseline) => ({
      factKey: baseline.fact_key,
      promotedValue: baseline.promoted_value,
      candidateValue: baseline.candidate_value,
      candidateCount: baseline.candidate_count,
      windowCount: baseline.window_count,
      lastChangedAt: baseline.last_changed_at ? baseline.last_changed_at.toISOString() : null,
      updatedAt: baseline.updated_at.toISOString()
    })),
    recentDecisions: recentDecisions.map((d) => ({
      ruleId: String(d.matched_rule_id),
      decidedAt: d.decided_at.toISOString()
    })),
    scopeBaselines: scopeBaselines.map((sb) => ({
      factKey: sb.fact_key,
      promotedValue: sb.promoted_value,
      scopeType: sb.scope_type,
      supportRatio: sb.support_ratio,
      sampleSize: sb.sample_size,
      confidenceScore: sb.confidence_score,
      isStable: readBoolean(sb.is_stable) ?? false
    })),
    stabilityOverrides: stabilityOverrides.map((override) => ({
      factKeyPattern: override.fact_key_pattern,
      stabilityClass: override.stability_class
    }))
  });
});

// Backward-compatible endpoint used when talos_server sends telemetry directly to API backend.
rmmTelemetryRouter.post('/snapshots', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const body = asRecord(req.body) || {};
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const collectedAt = parseDate(body.collectedAt || body.collected_at);
  const receivedAt = parseDate(body.receivedAt || body.received_at) || new Date();
  const snapshot = body.snapshot;

  if (!organizationId || !agentId || !collectedAt) {
    return res.status(400).json({ error: 'organizationId, agentId and collectedAt are required' });
  }

  const result = await prisma.$transaction(async (tx) => {
    const blobName = `legacy-inline/${agentId}/${collectedAt.toISOString()}.json`;
    const inserted = await insertSnapshotManifest(tx, {
      organizationId,
      agentId,
      collectedAt,
      receivedAt,
      blobContainer: 'legacy-inline',
      blobName,
      blobContentEncoding: null,
      blobSizeBytes: null
    });
    const patchProjection =
      snapshot && typeof snapshot === 'object' && !Array.isArray(snapshot)
        ? await projectPatchSnapshotUpdates(tx, {
            organizationId,
            agentId,
            collectedAt,
            snapshot
          })
        : { parsedPendingUpdates: 0, catalogRows: 0, stateRows: 0 };
    return { inserted, patchProjection };
  });

  return res.status(202).json({
    accepted: true,
    duplicate: result.inserted === 0,
    patchProjection: result.patchProjection
  });
});

// Backward-compatible endpoint used when talos_server sends telemetry directly to API backend.
rmmTelemetryRouter.post('/events', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const body = asRecord(req.body) || {};
  const organizationId = readString(body.organizationId, body.organization_id);
  const agentId = readString(body.agentId, body.agent_id);
  const events = asArray(body.events);
  const receivedAt = parseDate(body.receivedAt || body.received_at) || new Date();

  if (!organizationId || !agentId) {
    return res.status(400).json({ error: 'organizationId and agentId are required' });
  }

  if (events.length === 0) {
    return res.status(202).json({ accepted: true, inserted: 0, duplicate: true });
  }

  const result = await prisma.$transaction(async (tx) => {
    return insertEvents(tx, organizationId, agentId, events, receivedAt);
  });

  return res.status(202).json({
    accepted: true,
    inserted: result.inserted,
    duplicate: result.inserted === 0,
    parsed: result.parsed
  });
});

rmmTelemetryRouter.get('/read/events/:agentId', requireAuth, async (req: AuthedRequest, res) => {
  const agentId = readString(req.params.agentId);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const membership = await requireDeviceReadMembership(req, res, agentId);
  if (!membership) return;

  const rawLimit = parseInteger(req.query.limit);
  const limit = rawLimit !== null ? Math.min(Math.max(rawLimit, 1), 500) : 200;
  const rows = await prisma.$queryRaw<
    Array<{
      event_id: string;
      occurred_at: Date;
      received_at: Date;
      event_type: string;
      severity: string;
      source: string;
      service_name: string | null;
      process_name: string | null;
      code: string | null;
      message: string | null;
      attributes_jsonb: unknown;
      created_at: Date;
    }>
  >(Prisma.sql`
    SELECT
      event_id,
      occurred_at,
      received_at,
      event_type,
      severity,
      source,
      service_name,
      process_name,
      code,
      message,
      attributes_jsonb,
      created_at
    FROM rmm_telemetry.device_event
    WHERE agent_id = ${agentId}
      AND organization_id = ${membership.organizationId}
    ORDER BY occurred_at DESC
    LIMIT ${limit}
  `);

  return res.json({
    items: rows.map((row) => ({
      eventId: row.event_id,
      occurredAt: row.occurred_at.toISOString(),
      receivedAt: row.received_at.toISOString(),
      eventType: row.event_type,
      severity: row.severity,
      source: row.source,
      serviceName: row.service_name,
      processName: row.process_name,
      code: row.code,
      message: row.message,
      attributes: row.attributes_jsonb,
      createdAt: row.created_at.toISOString()
    }))
  });
});

rmmTelemetryRouter.get('/read/facts/:agentId', requireAuth, async (req: AuthedRequest, res) => {
  const agentId = readString(req.params.agentId);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const membership = await requireDeviceReadMembership(req, res, agentId);
  if (!membership) return;

  const rows = await prisma.$queryRaw<
    Array<{
      fact_key: string;
      fact_value: unknown;
      fact_value_text: string;
      stability_class: string;
      source: string;
      source_ts: Date;
      updated_at: Date;
    }>
  >(Prisma.sql`
    SELECT
      fact_key,
      fact_value,
      fact_value_text,
      stability_class,
      source,
      source_ts,
      updated_at
    FROM rmm_telemetry.fact_state_current
    WHERE agent_id = ${agentId}
      AND organization_id = ${membership.organizationId}
    ORDER BY fact_key ASC
  `);

  return res.json({
    items: rows.map((row) => ({
      factKey: row.fact_key,
      factValue: row.fact_value,
      factValueText: row.fact_value_text,
      stabilityClass: row.stability_class,
      source: row.source,
      sourceTs: row.source_ts.toISOString(),
      updatedAt: row.updated_at.toISOString()
    }))
  });
});

rmmTelemetryRouter.get('/read/baselines/scopes', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const deviceLimit = parsePositiveInt(req.query.deviceLimit, 300, 1, 2000);
  const organizationId = membership.organizationId;

  const [customers, sites, devices, baselineCountsByScope, baselineCountsByCustomer, baselineCountsBySite] = await Promise.all([
    prisma.customer.findMany({
      where: { organizationId },
      select: {
        id: true,
        name: true,
        _count: { select: { devices: true } }
      },
      orderBy: { name: 'asc' }
    }),
    prisma.rmmSite.findMany({
      where: {
        customer: { organizationId }
      },
      select: {
        id: true,
        name: true,
        timezone: true,
        customerId: true,
        customer: {
          select: { name: true }
        },
        _count: {
          select: { devices: true }
        }
      },
      orderBy: [{ customer: { name: 'asc' } }, { name: 'asc' }]
    }),
    prisma.rmmDevice.findMany({
      where: {
        customer: { organizationId }
      },
      select: {
        agentId: true,
        hostname: true,
        customerId: true,
        siteId: true,
        customer: { select: { name: true } },
        site: { select: { name: true } }
      },
      orderBy: [{ hostname: 'asc' }, { agentId: 'asc' }],
      take: deviceLimit
    }),
    prisma.$queryRaw<Array<{ scope_type: string; baseline_count: bigint }>>(Prisma.sql`
      SELECT scope_type, COUNT(*)::bigint AS baseline_count
      FROM rmm_telemetry.fact_baseline_scope
      WHERE organization_id = ${organizationId}
      GROUP BY scope_type
    `),
    prisma.$queryRaw<Array<{ customer_id: string; baseline_count: bigint }>>(Prisma.sql`
      SELECT customer_id, COUNT(*)::bigint AS baseline_count
      FROM rmm_telemetry.fact_baseline_scope
      WHERE organization_id = ${organizationId}
        AND scope_type = 'customer'
        AND customer_id IS NOT NULL
      GROUP BY customer_id
    `),
    prisma.$queryRaw<Array<{ site_id: string; baseline_count: bigint }>>(Prisma.sql`
      SELECT site_id, COUNT(*)::bigint AS baseline_count
      FROM rmm_telemetry.fact_baseline_scope
      WHERE organization_id = ${organizationId}
        AND scope_type = 'site'
        AND site_id IS NOT NULL
      GROUP BY site_id
    `)
  ]);

  const scopeCountLookup = new Map<string, number>();
  for (const row of baselineCountsByScope) {
    scopeCountLookup.set(row.scope_type, Number(row.baseline_count));
  }
  const customerCountLookup = new Map<string, number>();
  for (const row of baselineCountsByCustomer) {
    customerCountLookup.set(row.customer_id, Number(row.baseline_count));
  }
  const siteCountLookup = new Map<string, number>();
  for (const row of baselineCountsBySite) {
    siteCountLookup.set(row.site_id, Number(row.baseline_count));
  }

  return res.json({
    organization: {
      id: membership.organizationId,
      name: membership.organization.name,
      baselineCount: scopeCountLookup.get('organization') ?? 0
    },
    customers: customers.map((customer) => ({
      id: customer.id,
      name: customer.name,
      deviceCount: customer._count.devices,
      baselineCount: customerCountLookup.get(customer.id) ?? 0
    })),
    sites: sites.map((site) => ({
      id: site.id,
      name: site.name,
      timezone: site.timezone,
      customerId: site.customerId,
      customerName: site.customer.name,
      deviceCount: site._count.devices,
      baselineCount: siteCountLookup.get(site.id) ?? 0
    })),
    devices: devices.map((device) => ({
      agentId: device.agentId,
      hostname: device.hostname,
      customerId: device.customerId,
      customerName: device.customer?.name ?? null,
      siteId: device.siteId,
      siteName: device.site?.name ?? null
    })),
    totals: {
      deviceCount: devices.length,
      customerCount: customers.length,
      siteCount: sites.length
    }
  });
});

rmmTelemetryRouter.get('/read/baselines/scope', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const scopeType = normalizeScopeType(req.query.scopeType);
  if (!scopeType) {
    return res.status(400).json({ error: 'scopeType must be one of organization, customer, site, device' });
  }
  const scopeIdInput = readString(req.query.scopeId);
  const factKey = readString(req.query.factKey);
  const onlyUnstable = parseBooleanFlag(req.query.onlyUnstable);
  const limit = parsePositiveInt(req.query.limit, 500, 1, 2000);
  const overrides = await loadStabilityOverridesForOrganization(membership.organizationId);

  if (scopeType === 'device') {
    if (!scopeIdInput) {
      return res.status(400).json({ error: 'scopeId (agentId) is required for device scope' });
    }
    if (!(await assertUserCanReadDevice(req, res, scopeIdInput))) return;

    const rows = await prisma.$queryRaw<
      Array<{
        fact_key: string;
        promoted_value: unknown;
        candidate_value: unknown;
        candidate_count: number;
        window_count: number;
        last_changed_at: Date | null;
        updated_at: Date;
        stability_class: string | null;
      }>
    >(Prisma.sql`
      SELECT
        fb.fact_key,
        fb.promoted_value,
        fb.candidate_value,
        fb.candidate_count,
        fb.window_count,
        fb.last_changed_at,
        fb.updated_at,
        fsc.stability_class
      FROM rmm_telemetry.fact_baseline fb
      LEFT JOIN rmm_telemetry.fact_state_current fsc
        ON fsc.agent_id = fb.agent_id
       AND fsc.fact_key = fb.fact_key
      WHERE fb.agent_id = ${scopeIdInput}
        AND fb.organization_id = ${membership.organizationId}
        ${factKey ? Prisma.sql`AND fb.fact_key ILIKE ${`%${factKey}%`}` : Prisma.empty}
      ORDER BY fb.fact_key ASC
      LIMIT ${limit}
    `);

    return res.json({
      scope: {
        scopeType,
        scopeId: scopeIdInput,
        scopeName: scopeIdInput
      },
      items: rows.map((row) => {
        const supportCount = Math.max(row.candidate_count || 0, 0);
        const totalCount = Math.max(row.window_count || 0, 0);
        const supportRatio = totalCount > 0 ? supportCount / totalCount : 0;
        const trust = buildBaselineTrustMetadata({
          scopeType,
          factKey: row.fact_key,
          overrides,
          promotedValue: row.promoted_value,
          candidateCount: row.candidate_count,
          isStable: row.promoted_value !== null,
          currentStabilityClass: row.stability_class,
          sampleSize: Math.max(row.window_count || 0, 1),
          supportRatio
        });

        return {
          factKey: row.fact_key,
          promotedValue: row.promoted_value,
          candidateValue: row.candidate_value,
          candidateCount: row.candidate_count,
          windowCount: row.window_count,
          supportCount,
          totalCount,
          supportRatio,
          sampleSize: Math.max(row.window_count || 0, 1),
          confidenceScore: supportRatio,
          isStable: row.promoted_value !== null,
          lastChangedAt: iso(row.last_changed_at),
          updatedAt: row.updated_at.toISOString(),
          ...trust
        };
      })
    });
  }

  let scope;
  try {
    scope = await validateScopeAccess(membership, scopeType, scopeIdInput);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return res.status(404).json({ error: msg });
  }

  const where: Prisma.RmmTelemetryFactBaselineScopeWhereInput = {
    ...(scope.where || {}),
    ...(factKey ? { factKey: { contains: factKey, mode: 'insensitive' } } : {}),
    ...(onlyUnstable ? { isStable: false } : {})
  };

  const rows = await prisma.rmmTelemetryFactBaselineScope.findMany({
    where,
    orderBy: [{ factKey: 'asc' }],
    take: limit
  });

  return res.json({
    scope: {
      scopeType,
      scopeId: scope.scopeId,
      scopeName: scope.scopeName
    },
    items: rows.map((row) => {
      const trust = buildBaselineTrustMetadata({
        scopeType,
        factKey: row.factKey,
        overrides,
        promotedValue: row.promotedValue,
        candidateCount: row.candidateCount,
        isStable: row.isStable,
        sampleSize: row.sampleSize,
        supportRatio: row.supportRatio
      });

      return {
        factKey: row.factKey,
        promotedValue: row.promotedValue,
        candidateValue: row.candidateValue,
        candidateCount: row.candidateCount,
        windowCount: row.windowCount,
        supportCount: row.supportCount,
        totalCount: row.totalCount,
        supportRatio: row.supportRatio,
        sampleSize: row.sampleSize,
        confidenceScore: row.confidenceScore,
        isStable: row.isStable,
        lastChangedAt: iso(row.lastChangedAt),
        updatedAt: row.updatedAt.toISOString(),
        ...trust
      };
    })
  });
});

rmmTelemetryRouter.get('/read/baselines/scope/:scopeType/:scopeId/summary', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const scopeType = normalizeScopeType(req.params.scopeType);
  if (!scopeType) {
    return res.status(400).json({ error: 'scopeType must be one of organization, customer, site, device' });
  }
  const scopeId = readString(req.params.scopeId);
  if (!scopeId) return res.status(400).json({ error: 'scopeId is required' });

  if (scopeType === 'device') {
    if (!(await assertUserCanReadDevice(req, res, scopeId))) return;
    const rows = await prisma.$queryRaw<Array<{
      total: bigint;
      stable: bigint;
      avg_support_ratio: number | null;
      latest_updated_at: Date | null;
    }>>(Prisma.sql`
      SELECT
        COUNT(*)::bigint AS total,
        COUNT(*) FILTER (WHERE promoted_value IS NOT NULL)::bigint AS stable,
        AVG(
          CASE
            WHEN window_count > 0 THEN candidate_count::float / window_count::float
            ELSE 0
          END
        ) AS avg_support_ratio,
        MAX(updated_at) AS latest_updated_at
      FROM rmm_telemetry.fact_baseline
      WHERE agent_id = ${scopeId}
        AND organization_id = ${membership.organizationId}
    `);
    const total = Number(rows[0]?.total ?? 0n);
    const stable = Number(rows[0]?.stable ?? 0n);
    return res.json({
      scope: { scopeType, scopeId, scopeName: scopeId },
      summary: {
        totalFacts: total,
        stableFacts: stable,
        unstableFacts: Math.max(total - stable, 0),
        avgSupportRatio: rows[0]?.avg_support_ratio ?? 0,
        avgConfidenceScore: rows[0]?.avg_support_ratio ?? 0,
        latestUpdatedAt: iso(rows[0]?.latest_updated_at ?? null)
      }
    });
  }

  let scope;
  try {
    scope = await validateScopeAccess(membership, scopeType, scopeId);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return res.status(404).json({ error: msg });
  }

  const summaryRows = await prisma.$queryRaw<
    Array<{
      total: bigint;
      stable: bigint;
      avg_support_ratio: number | null;
      avg_confidence_score: number | null;
      latest_updated_at: Date | null;
    }>
  >(Prisma.sql`
    SELECT
      COUNT(*)::bigint AS total,
      COUNT(*) FILTER (WHERE is_stable = TRUE)::bigint AS stable,
      AVG(support_ratio) AS avg_support_ratio,
      AVG(confidence_score) AS avg_confidence_score,
      MAX(updated_at) AS latest_updated_at
    FROM rmm_telemetry.fact_baseline_scope
    WHERE scope_type = ${scope.scopeType}
      AND scope_key = ${scope.scopeId}
      AND organization_id = ${membership.organizationId}
  `);
  const row = summaryRows[0];
  const total = Number(row?.total ?? 0n);
  const stable = Number(row?.stable ?? 0n);

  return res.json({
    scope: {
      scopeType: scope.scopeType,
      scopeId: scope.scopeId,
      scopeName: scope.scopeName
    },
    summary: {
      totalFacts: total,
      stableFacts: stable,
      unstableFacts: Math.max(total - stable, 0),
      avgSupportRatio: row?.avg_support_ratio ?? 0,
      avgConfidenceScore: row?.avg_confidence_score ?? 0,
      latestUpdatedAt: iso(row?.latest_updated_at ?? null)
    }
  });
});

rmmTelemetryRouter.get('/read/baselines/scope/:scopeType/:scopeId/drift', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const scopeType = normalizeScopeType(req.params.scopeType);
  if (!scopeType) {
    return res.status(400).json({ error: 'scopeType must be one of organization, customer, site, device' });
  }
  if (scopeType === 'device') {
    return res.status(400).json({ error: 'drift is not available for device scope' });
  }

  const scopeId = readString(req.params.scopeId);
  if (!scopeId) return res.status(400).json({ error: 'scopeId is required' });
  const factKey = readString(req.query.factKey);
  const limit = parsePositiveInt(req.query.limit, 200, 1, 2000);

  let scope;
  try {
    scope = await validateScopeAccess(membership, scopeType, scopeId);
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return res.status(404).json({ error: msg });
  }
  const overrides = await loadStabilityOverridesForOrganization(membership.organizationId);

  type DriftRow = {
    agent_id: string;
    hostname: string;
    customer_id: string | null;
    customer_name: string | null;
    site_id: string | null;
    site_name: string | null;
    fact_key: string;
    scope_value: unknown;
    device_value: unknown;
    device_updated_at: Date | null;
    scope_updated_at: Date;
    scope_sample_size: number;
    scope_support_ratio: number;
    scope_confidence_score: number;
    scope_is_stable: boolean;
  };

  let rows: DriftRow[] = [];
  if (scopeType === 'organization') {
    rows = await prisma.$queryRaw<DriftRow[]>(Prisma.sql`
      SELECT
        d.agent_id,
        d.hostname,
        d.customer_id,
        c.name AS customer_name,
        d.site_id,
        s.name AS site_name,
        sb.fact_key,
        sb.promoted_value AS scope_value,
        db.promoted_value AS device_value,
        db.updated_at AS device_updated_at,
        sb.updated_at AS scope_updated_at,
        sb.sample_size AS scope_sample_size,
        sb.support_ratio AS scope_support_ratio,
        sb.confidence_score AS scope_confidence_score,
        sb.is_stable AS scope_is_stable
      FROM rmm_telemetry.fact_baseline_scope sb
      INNER JOIN public.customers c
        ON c.organization_id = ${scope.scopeId}
      INNER JOIN public.rmm_devices d
        ON d.customer_id = c.id
      LEFT JOIN public.rmm_sites s
        ON s.id = d.site_id
      LEFT JOIN rmm_telemetry.fact_baseline db
        ON db.agent_id = d.agent_id
       AND db.fact_key = sb.fact_key
      WHERE sb.scope_type = 'organization'
        AND sb.scope_key = ${scope.scopeId}
        AND sb.is_stable = TRUE
        ${factKey ? Prisma.sql`AND sb.fact_key ILIKE ${`%${factKey}%`}` : Prisma.empty}
        AND (db.fact_key IS NULL OR db.promoted_value IS DISTINCT FROM sb.promoted_value)
      ORDER BY sb.fact_key ASC, d.hostname ASC
      LIMIT ${limit}
    `);
  } else if (scopeType === 'customer') {
    rows = await prisma.$queryRaw<DriftRow[]>(Prisma.sql`
      SELECT
        d.agent_id,
        d.hostname,
        d.customer_id,
        c.name AS customer_name,
        d.site_id,
        s.name AS site_name,
        sb.fact_key,
        sb.promoted_value AS scope_value,
        db.promoted_value AS device_value,
        db.updated_at AS device_updated_at,
        sb.updated_at AS scope_updated_at,
        sb.sample_size AS scope_sample_size,
        sb.support_ratio AS scope_support_ratio,
        sb.confidence_score AS scope_confidence_score,
        sb.is_stable AS scope_is_stable
      FROM rmm_telemetry.fact_baseline_scope sb
      INNER JOIN public.customers c
        ON c.id = ${scope.scopeId}
      INNER JOIN public.rmm_devices d
        ON d.customer_id = c.id
      LEFT JOIN public.rmm_sites s
        ON s.id = d.site_id
      LEFT JOIN rmm_telemetry.fact_baseline db
        ON db.agent_id = d.agent_id
       AND db.fact_key = sb.fact_key
      WHERE sb.scope_type = 'customer'
        AND sb.scope_key = ${scope.scopeId}
        AND sb.is_stable = TRUE
        ${factKey ? Prisma.sql`AND sb.fact_key ILIKE ${`%${factKey}%`}` : Prisma.empty}
        AND (db.fact_key IS NULL OR db.promoted_value IS DISTINCT FROM sb.promoted_value)
      ORDER BY sb.fact_key ASC, d.hostname ASC
      LIMIT ${limit}
    `);
  } else if (scopeType === 'site') {
    rows = await prisma.$queryRaw<DriftRow[]>(Prisma.sql`
      SELECT
        d.agent_id,
        d.hostname,
        d.customer_id,
        c.name AS customer_name,
        d.site_id,
        s.name AS site_name,
        sb.fact_key,
        sb.promoted_value AS scope_value,
        db.promoted_value AS device_value,
        db.updated_at AS device_updated_at,
        sb.updated_at AS scope_updated_at,
        sb.sample_size AS scope_sample_size,
        sb.support_ratio AS scope_support_ratio,
        sb.confidence_score AS scope_confidence_score,
        sb.is_stable AS scope_is_stable
      FROM rmm_telemetry.fact_baseline_scope sb
      INNER JOIN public.rmm_sites s
        ON s.id = ${scope.scopeId}
      INNER JOIN public.customers c
        ON c.id = s.customer_id
      INNER JOIN public.rmm_devices d
        ON d.site_id = s.id
      LEFT JOIN rmm_telemetry.fact_baseline db
        ON db.agent_id = d.agent_id
       AND db.fact_key = sb.fact_key
      WHERE sb.scope_type = 'site'
        AND sb.scope_key = ${scope.scopeId}
        AND sb.is_stable = TRUE
        ${factKey ? Prisma.sql`AND sb.fact_key ILIKE ${`%${factKey}%`}` : Prisma.empty}
        AND (db.fact_key IS NULL OR db.promoted_value IS DISTINCT FROM sb.promoted_value)
      ORDER BY sb.fact_key ASC, d.hostname ASC
      LIMIT ${limit}
    `);
  }

  return res.json({
    scope: {
      scopeType: scope.scopeType,
      scopeId: scope.scopeId,
      scopeName: scope.scopeName
    },
    items: rows.map((row) => {
      const trust = buildDriftTrustMetadata({
        scopeType,
        factKey: row.fact_key,
        overrides,
        scopeSampleSize: row.scope_sample_size,
        scopeSupportRatio: row.scope_support_ratio,
        scopeIsStable: row.scope_is_stable,
        deviceValue: row.device_value
      });

      return {
        agentId: row.agent_id,
        hostname: row.hostname,
        customerId: row.customer_id,
        customerName: row.customer_name,
        siteId: row.site_id,
        siteName: row.site_name,
        factKey: row.fact_key,
        scopeValue: row.scope_value,
        deviceValue: row.device_value,
        deviceUpdatedAt: iso(row.device_updated_at),
        scopeUpdatedAt: row.scope_updated_at.toISOString(),
        scopeSampleSize: row.scope_sample_size,
        scopeSupportRatio: row.scope_support_ratio,
        scopeConfidenceScore: row.scope_confidence_score,
        scopeIsStable: row.scope_is_stable,
        ...trust
      };
    })
  });
});

rmmTelemetryRouter.get('/read/baselines/:agentId', requireAuth, async (req: AuthedRequest, res) => {
  const agentId = readString(req.params.agentId);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const membership = await requireDeviceReadMembership(req, res, agentId);
  if (!membership) return;

  const rows = await prisma.$queryRaw<
    Array<{
      fact_key: string;
      promoted_value: unknown;
      candidate_value: unknown;
      candidate_count: number;
      window_count: number;
      last_changed_at: Date | null;
      updated_at: Date;
    }>
  >(Prisma.sql`
    SELECT
      fact_key,
      promoted_value,
      candidate_value,
      candidate_count,
      window_count,
      last_changed_at,
      updated_at
    FROM rmm_telemetry.fact_baseline
    WHERE agent_id = ${agentId}
      AND organization_id = ${membership.organizationId}
    ORDER BY fact_key ASC
  `);

  return res.json({
    items: rows.map((row) => ({
      factKey: row.fact_key,
      promotedValue: row.promoted_value,
      candidateValue: row.candidate_value,
      candidateCount: row.candidate_count,
      windowCount: row.window_count,
      lastChangedAt: iso(row.last_changed_at),
      updatedAt: row.updated_at.toISOString()
    }))
  });
});

rmmTelemetryRouter.get('/read/alerts', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const clauses: Prisma.Sql[] = [Prisma.sql`a.organization_id = ${membership.organizationId}`];
  const statusRaw = readString(req.query.status);
  const severityRaw = readString(req.query.severity);
  const status = statusRaw && statusRaw !== 'all' ? normalizeAlertStatus(statusRaw) : null;
  const severity = severityRaw && severityRaw !== 'all' ? normalizeAlertSeverity(severityRaw, 'info') : null;
  const agentId = readString(req.query.agentId, req.query.agent_id);
  const customerId = readString(req.query.customerId, req.query.customer_id);
  const siteId = readString(req.query.siteId, req.query.site_id);
  const ownerUserId = readString(req.query.ownerUserId, req.query.owner_user_id);
  const q = readString(req.query.q, req.query.query);
  const rawLimit = parseInteger(req.query.limit);
  const limit = rawLimit !== null ? Math.min(Math.max(rawLimit, 1), 500) : 200;

  if (status) clauses.push(Prisma.sql`a.status = ${status}`);
  if (severity) clauses.push(Prisma.sql`a.severity = ${severity}`);
  if (agentId) clauses.push(Prisma.sql`a.agent_id = ${agentId}`);
  if (customerId) clauses.push(Prisma.sql`a.customer_id = ${customerId}`);
  if (siteId) clauses.push(Prisma.sql`a.site_id = ${siteId}`);
  if (ownerUserId) clauses.push(Prisma.sql`a.owner_user_id = ${ownerUserId}`);
  if (q) {
    const pattern = `%${q}%`;
    clauses.push(Prisma.sql`(a.title ILIKE ${pattern} OR a.summary ILIKE ${pattern} OR d.hostname ILIKE ${pattern})`);
  }

  const whereClause = clauses.reduce<Prisma.Sql>(
    (sql, clause, index) => index === 0 ? clause : Prisma.sql`${sql} AND ${clause}`,
    Prisma.sql`TRUE`
  );

  const rows = await prisma.$queryRaw<AlertRow[]>(Prisma.sql`
    SELECT
      a.id,
      a.organization_id,
      a.customer_id,
      a.site_id,
      a.agent_id,
      a.rule_id,
      a.status,
      a.severity,
      a.source_domain,
      a.source_key,
      a.source_event_id,
      a.source_fact_key,
      a.source_decision_id,
      a.title,
      a.summary,
      a.fingerprint,
      a.first_seen_at,
      a.last_seen_at,
      a.occurrence_count,
      a.owner_user_id,
      owner.email AS owner_email,
      a.acknowledged_by,
      a.acknowledged_at,
      a.snoozed_until,
      a.resolved_by,
      a.resolved_at,
      a.suppressed_until,
      a.metadata_jsonb,
      a.created_at,
      a.updated_at,
      d.hostname,
      c.name AS customer_name,
      s.name AS site_name
    FROM rmm_telemetry.alert a
    LEFT JOIN public.rmm_devices d ON d.agent_id = a.agent_id
    LEFT JOIN public.customers c ON c.id = a.customer_id
    LEFT JOIN public.rmm_sites s ON s.id = a.site_id
    LEFT JOIN public."User" owner ON owner.id = a.owner_user_id
    WHERE ${whereClause}
    ORDER BY
      CASE a.severity
        WHEN 'critical' THEN 1
        WHEN 'high' THEN 2
        WHEN 'medium' THEN 3
        WHEN 'low' THEN 4
        ELSE 5
      END ASC,
      a.last_seen_at DESC
    LIMIT ${limit}
  `);

  return res.json({
    items: rows.map(serializeAlert),
    filters: { status: status ?? 'all', severity: severity ?? 'all' }
  });
});

rmmTelemetryRouter.post('/alerts/:id/acknowledge', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') return res.status(403).json({ error: 'Insufficient permissions' });

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const updated = await prisma.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.alert
    SET
      status = 'acknowledged',
      owner_user_id = ${req.jwt!.sub},
      acknowledged_by = ${req.jwt!.sub},
      acknowledged_at = NOW(),
      snoozed_until = NULL,
      resolved_by = NULL,
      resolved_at = NULL,
      suppressed_until = NULL,
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  if (updated === 0) return res.status(404).json({ error: 'Alert not found' });
  const alert = await loadAlertById(membership.organizationId, id);
  return res.json(alert ? serializeAlert(alert) : { updated: true });
});

rmmTelemetryRouter.post('/alerts/:id/snooze', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') return res.status(403).json({ error: 'Insufficient permissions' });

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });
  const body = asRecord(req.body) || {};
  const until = parseDate(body.until ?? body.snoozedUntil ?? body.snoozed_until);
  const minutes = parseInteger(body.minutes) ?? 60;
  const snoozedUntil = until ?? new Date(Date.now() + Math.min(Math.max(minutes, 1), 60 * 24 * 30) * 60 * 1000);

  const updated = await prisma.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.alert
    SET
      status = 'snoozed',
      owner_user_id = COALESCE(owner_user_id, ${req.jwt!.sub}),
      snoozed_until = ${snoozedUntil},
      resolved_by = NULL,
      resolved_at = NULL,
      suppressed_until = NULL,
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  if (updated === 0) return res.status(404).json({ error: 'Alert not found' });
  const alert = await loadAlertById(membership.organizationId, id);
  return res.json(alert ? serializeAlert(alert) : { updated: true });
});

rmmTelemetryRouter.post('/alerts/:id/resolve', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') return res.status(403).json({ error: 'Insufficient permissions' });

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const updated = await prisma.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.alert
    SET
      status = 'resolved',
      resolved_by = ${req.jwt!.sub},
      resolved_at = NOW(),
      snoozed_until = NULL,
      suppressed_until = NULL,
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  if (updated === 0) return res.status(404).json({ error: 'Alert not found' });
  const alert = await loadAlertById(membership.organizationId, id);
  return res.json(alert ? serializeAlert(alert) : { updated: true });
});

rmmTelemetryRouter.post('/alerts/:id/suppress', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') return res.status(403).json({ error: 'Insufficient permissions' });

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });
  const body = asRecord(req.body) || {};
  const until = parseDate(body.until ?? body.suppressedUntil ?? body.suppressed_until);
  const minutes = parseInteger(body.minutes) ?? 1440;
  const suppressedUntil = until ?? new Date(Date.now() + Math.min(Math.max(minutes, 1), 60 * 24 * 90) * 60 * 1000);

  const updated = await prisma.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.alert
    SET
      status = 'suppressed',
      suppressed_until = ${suppressedUntil},
      snoozed_until = NULL,
      resolved_by = NULL,
      resolved_at = NULL,
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  if (updated === 0) return res.status(404).json({ error: 'Alert not found' });
  const alert = await loadAlertById(membership.organizationId, id);
  return res.json(alert ? serializeAlert(alert) : { updated: true });
});

rmmTelemetryRouter.get('/read/decisions/:agentId', requireAuth, async (req: AuthedRequest, res) => {
  const agentId = readString(req.params.agentId);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const membership = await requireDeviceReadMembership(req, res, agentId);
  if (!membership) return;

  const rawLimit = parseInteger(req.query.limit);
  const limit = rawLimit !== null ? Math.min(Math.max(rawLimit, 1), 500) : 200;
  const matchedRuleId = parseBigIntValue(req.query.matchedRuleId ?? req.query.matched_rule_id);
  const rows = await prisma.$queryRaw<RoutingDecisionRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      agent_id,
      domain,
      trigger_key,
      trigger_value,
      action,
      matched_rule_id,
      intent_id,
      reason,
      dedupe_key,
      source,
      source_ts,
      decided_at,
      execution_status,
      external_ref,
      outcome_message
    FROM rmm_telemetry.routing_decision
    WHERE agent_id = ${agentId}
      AND organization_id = ${membership.organizationId}
      ${matchedRuleId !== null ? Prisma.sql`AND matched_rule_id = ${matchedRuleId}` : Prisma.empty}
    ORDER BY decided_at DESC
    LIMIT ${limit}
  `);

  return res.json({
    items: rows.map((row) => ({
      id: String(row.id),
      domain: row.domain,
      triggerKey: row.trigger_key,
      triggerValue: row.trigger_value,
      action: row.action,
      matchedRuleId: row.matched_rule_id ? String(row.matched_rule_id) : null,
      intentId: row.intent_id,
      reason: row.reason,
      dedupeKey: row.dedupe_key,
      source: row.source,
      sourceTs: row.source_ts.toISOString(),
      decidedAt: row.decided_at.toISOString(),
      executionStatus: row.execution_status,
      externalRef: row.external_ref,
      outcomeMessage: row.outcome_message
    }))
  });
});

rmmTelemetryRouter.get('/read/routing-rules', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const enabledRaw = readString(req.query.enabled);
  const enabledFilter = enabledRaw === null ? null : parseBooleanFlag(enabledRaw);
  const rows = await loadRoutingRulesForOrganization(membership.organizationId, {
    enabled: enabledFilter,
    triggerDomain: readString(req.query.triggerDomain, req.query.trigger_domain),
    action: readString(req.query.action)
  });
  const items = await buildRoutingRuleReadModels(rows);
  return res.json({ items });
});

rmmTelemetryRouter.get('/read/alert-rules', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const enabledRaw = readString(req.query.enabled);
  const enabledFilter = enabledRaw === null ? null : parseBooleanFlag(enabledRaw);
  const rows = await loadAlertRulesForOrganization(membership.organizationId, {
    enabled: enabledFilter,
    triggerDomain: readString(req.query.triggerDomain, req.query.trigger_domain)
  });
  return res.json({ items: rows.map(serializeAlertRule) });
});

rmmTelemetryRouter.post('/alert-rules', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const body = asRecord(req.body) || {};
  const parsed = await parseAlertRuleInput(membership.organizationId, req.jwt!.sub, body);
  if (!parsed.rule) {
    return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid alert rule' });
  }

  const notificationChannelsJson = toJsonString(parsed.rule.notificationChannels);
  const rows = await prisma.$queryRaw<AlertRuleRow[]>(Prisma.sql`
    INSERT INTO rmm_telemetry.alert_rule
      (
        organization_id, customer_id, site_id, agent_id, name,
        trigger_domain, trigger_key, match_operator, match_value,
        severity, min_severity, dedupe_window_seconds, enabled, priority,
        notification_channels_jsonb, created_by, created_at, updated_at
      )
    VALUES
      (
        ${parsed.rule.organizationId}, ${parsed.rule.customerId}, ${parsed.rule.siteId}, ${parsed.rule.agentId}, ${parsed.rule.name},
        ${parsed.rule.triggerDomain}, ${parsed.rule.triggerKey}, ${parsed.rule.matchOperator}, ${parsed.rule.matchValue},
        ${parsed.rule.severity}, ${parsed.rule.minSeverity}, ${parsed.rule.dedupeWindowSeconds}, ${parsed.rule.enabled}, ${parsed.rule.priority},
        ${notificationChannelsJson}::jsonb, ${parsed.rule.createdBy}, NOW(), NOW()
      )
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      name,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      severity,
      min_severity,
      dedupe_window_seconds,
      enabled,
      priority,
      notification_channels_jsonb,
      created_by,
      created_at,
      updated_at
  `);
  return res.status(201).json(serializeAlertRule(rows[0]!));
});

rmmTelemetryRouter.patch('/alert-rules/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });
  const existing = await loadAlertRuleRow(membership.organizationId, id);
  if (!existing) return res.status(404).json({ error: 'Alert rule not found' });

  const body = asRecord(req.body) || {};
  const parsed = await parseAlertRuleInput(membership.organizationId, req.jwt!.sub, body, existing);
  if (!parsed.rule) {
    return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid alert rule' });
  }

  const notificationChannelsJson = toJsonString(parsed.rule.notificationChannels);
  const rows = await prisma.$queryRaw<AlertRuleRow[]>(Prisma.sql`
    UPDATE rmm_telemetry.alert_rule
    SET
      customer_id = ${parsed.rule.customerId},
      site_id = ${parsed.rule.siteId},
      agent_id = ${parsed.rule.agentId},
      name = ${parsed.rule.name},
      trigger_domain = ${parsed.rule.triggerDomain},
      trigger_key = ${parsed.rule.triggerKey},
      match_operator = ${parsed.rule.matchOperator},
      match_value = ${parsed.rule.matchValue},
      severity = ${parsed.rule.severity},
      min_severity = ${parsed.rule.minSeverity},
      dedupe_window_seconds = ${parsed.rule.dedupeWindowSeconds},
      enabled = ${parsed.rule.enabled},
      priority = ${parsed.rule.priority},
      notification_channels_jsonb = ${notificationChannelsJson}::jsonb,
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      name,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      severity,
      min_severity,
      dedupe_window_seconds,
      enabled,
      priority,
      notification_channels_jsonb,
      created_by,
      created_at,
      updated_at
  `);
  return res.json(serializeAlertRule(rows[0]!));
});

rmmTelemetryRouter.delete('/alert-rules/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });
  const deleted = await prisma.$executeRaw(Prisma.sql`
    DELETE FROM rmm_telemetry.alert_rule
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  return res.json({ deleted: deleted > 0 });
});

rmmTelemetryRouter.post('/routing-rules', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const body = asRecord(req.body) || {};
  const parsed = await parseRoutingRuleInput(membership.organizationId, body);
  if (!parsed.rule) {
    return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid routing rule' });
  }

  const hardBlockedReasons = parsed.blockedReasons.filter((reason) =>
    reason.startsWith('intent_') || (reason === 'ticket_provider_not_ready' && parsed.rule!.enabled)
  );
  if (hardBlockedReasons.length > 0) {
    return res.status(400).json({ error: hardBlockedReasons.join('; '), blockedReasons: parsed.blockedReasons });
  }

  const rows = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    INSERT INTO rmm_telemetry.routing_rule
      (
        organization_id, customer_id, site_id, agent_id,
        trigger_domain, trigger_key, match_operator, match_value,
        previous_match_operator, previous_match_value,
        min_support_ratio, min_confidence_score, scope_type_filter,
        action, intent_id, cooldown_seconds, enabled, priority,
        created_at, updated_at
      )
    VALUES
      (
        ${parsed.rule.organizationId}, ${parsed.rule.customerId}, ${parsed.rule.siteId}, ${parsed.rule.agentId},
        ${parsed.rule.triggerDomain}, ${parsed.rule.triggerKey}, ${parsed.rule.matchOperator}, ${parsed.rule.matchValue},
        ${parsed.rule.previousMatchOperator}, ${parsed.rule.previousMatchValue},
        ${parsed.rule.minSupportRatio}, ${parsed.rule.minConfidenceScore}, ${parsed.rule.scopeTypeFilter},
        ${parsed.rule.action}, ${parsed.rule.intentId}, ${parsed.rule.cooldownSeconds}, ${parsed.rule.enabled}, ${parsed.rule.priority},
        NOW(), NOW()
      )
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
  `);
  const item = (await buildRoutingRuleReadModels(rows))[0];
  return res.status(201).json(item);
});

rmmTelemetryRouter.patch('/routing-rules/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const existing = await loadRoutingRuleRow(membership.organizationId, id);
  if (!existing) {
    return res.status(404).json({ error: 'Routing rule not found' });
  }

  const body = asRecord(req.body) || {};
  const parsed = await parseRoutingRuleInput(membership.organizationId, body, existing);
  if (!parsed.rule) {
    return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid routing rule' });
  }

  const hardBlockedReasons = parsed.blockedReasons.filter((reason) =>
    reason.startsWith('intent_') || (reason === 'ticket_provider_not_ready' && parsed.rule!.enabled)
  );
  if (hardBlockedReasons.length > 0) {
    return res.status(400).json({ error: hardBlockedReasons.join('; '), blockedReasons: parsed.blockedReasons });
  }

  const rows = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    UPDATE rmm_telemetry.routing_rule
    SET
      customer_id = ${parsed.rule.customerId},
      site_id = ${parsed.rule.siteId},
      agent_id = ${parsed.rule.agentId},
      trigger_domain = ${parsed.rule.triggerDomain},
      trigger_key = ${parsed.rule.triggerKey},
      match_operator = ${parsed.rule.matchOperator},
      match_value = ${parsed.rule.matchValue},
      previous_match_operator = ${parsed.rule.previousMatchOperator},
      previous_match_value = ${parsed.rule.previousMatchValue},
      min_support_ratio = ${parsed.rule.minSupportRatio},
      min_confidence_score = ${parsed.rule.minConfidenceScore},
      scope_type_filter = ${parsed.rule.scopeTypeFilter},
      action = ${parsed.rule.action},
      intent_id = ${parsed.rule.intentId},
      cooldown_seconds = ${parsed.rule.cooldownSeconds},
      enabled = ${parsed.rule.enabled},
      priority = ${parsed.rule.priority},
      updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
  `);
  const item = (await buildRoutingRuleReadModels(rows))[0];
  return res.json(item);
});

rmmTelemetryRouter.delete('/routing-rules/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const deleted = await prisma.$executeRaw(Prisma.sql`
    DELETE FROM rmm_telemetry.routing_rule
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
  `);
  return res.json({ deleted: deleted > 0 });
});

rmmTelemetryRouter.post('/routing-rules/:id/enable', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const existing = await loadRoutingRuleRow(membership.organizationId, id);
  if (!existing) {
    return res.status(404).json({ error: 'Routing rule not found' });
  }

  const parsed = await parseRoutingRuleInput(membership.organizationId, { enabled: true }, existing);
  if (!parsed.rule) {
    return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid routing rule' });
  }
  const hardBlockedReasons = parsed.blockedReasons.filter((reason) =>
    reason.startsWith('intent_') || reason === 'ticket_provider_not_ready'
  );
  if (hardBlockedReasons.length > 0) {
    return res.status(400).json({ error: hardBlockedReasons.join('; '), blockedReasons: parsed.blockedReasons });
  }

  const rows = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    UPDATE rmm_telemetry.routing_rule
    SET enabled = TRUE, updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
  `);
  const item = (await buildRoutingRuleReadModels(rows))[0];
  return res.json(item);
});

rmmTelemetryRouter.post('/routing-rules/:id/disable', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const rows = await prisma.$queryRaw<RoutingRuleRow[]>(Prisma.sql`
    UPDATE rmm_telemetry.routing_rule
    SET enabled = FALSE, updated_at = NOW()
    WHERE organization_id = ${membership.organizationId}
      AND id = ${id}
    RETURNING
      id,
      organization_id,
      customer_id,
      site_id,
      agent_id,
      trigger_domain,
      trigger_key,
      match_operator,
      match_value,
      previous_match_operator,
      previous_match_value,
      min_support_ratio,
      min_confidence_score,
      scope_type_filter,
      action,
      intent_id,
      cooldown_seconds,
      enabled,
      priority,
      created_at,
      updated_at
  `);
  if (!rows[0]) {
    return res.status(404).json({ error: 'Routing rule not found' });
  }
  const item = (await buildRoutingRuleReadModels(rows))[0];
  return res.json(item);
});

rmmTelemetryRouter.post('/routing-rules/test', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const body = asRecord(req.body) || {};
  const ruleId = parseBigIntValue(body.ruleId ?? body.rule_id);
  const candidateBody = asRecord(body.candidate) || body;

  let ruleRow: RoutingRuleRow | null = null;
  let readinessSource: { blockedReasons: string[]; intent: RoutingIntentSummary | null; halo: HaloProviderStatus; } | null = null;
  if (ruleId !== null) {
    ruleRow = await loadRoutingRuleRow(membership.organizationId, ruleId);
    if (!ruleRow) {
      return res.status(404).json({ error: 'Routing rule not found' });
    }
    const models = await buildRoutingRuleReadModels([ruleRow]);
    const item = models[0];
    readinessSource = {
      blockedReasons: item.blockedReasons,
      intent: await loadRoutingIntentSummary(membership.organizationId, ruleRow.intent_id),
      halo: await loadHaloProviderStatus(membership.organizationId)
    };
  } else {
    const draftRuleRecord = asRecord(body.rule);
    if (!draftRuleRecord) {
      return res.status(400).json({ error: 'ruleId or rule is required' });
    }
    const parsed = await parseRoutingRuleInput(membership.organizationId, draftRuleRecord);
    if (!parsed.rule) {
      return res.status(400).json({ error: parsed.errors.join('; ') || 'Invalid routing rule draft' });
    }
    ruleRow = {
      id: 0n,
      organization_id: membership.organizationId,
      customer_id: parsed.rule.customerId,
      site_id: parsed.rule.siteId,
      agent_id: parsed.rule.agentId,
      trigger_domain: parsed.rule.triggerDomain,
      trigger_key: parsed.rule.triggerKey,
      match_operator: parsed.rule.matchOperator,
      match_value: parsed.rule.matchValue,
      previous_match_operator: parsed.rule.previousMatchOperator,
      previous_match_value: parsed.rule.previousMatchValue,
      min_support_ratio: parsed.rule.minSupportRatio,
      min_confidence_score: parsed.rule.minConfidenceScore,
      scope_type_filter: parsed.rule.scopeTypeFilter,
      action: parsed.rule.action,
      intent_id: parsed.rule.intentId,
      cooldown_seconds: parsed.rule.cooldownSeconds,
      enabled: parsed.rule.enabled,
      priority: parsed.rule.priority
    };
    readinessSource = {
      blockedReasons: parsed.blockedReasons,
      intent: parsed.intent,
      halo: parsed.halo
    };
  }

  const candidate = buildRoutingCandidate(candidateBody, {
    organizationId: membership.organizationId,
    customerId: ruleRow.customer_id,
    siteId: ruleRow.site_id,
    agentId: ruleRow.agent_id
  });
  if (!candidate) {
    return res.status(400).json({ error: 'candidate.domain and candidate.triggerKey are required' });
  }

  const evaluation = evaluateRoutingRuleMatch(ruleRow, candidate);
  let cooldownBlocked = false;
  if (evaluation.matched && ruleId !== null && ruleRow.cooldown_seconds > 0 && candidate.agentId) {
    const recentRows = await prisma.$queryRaw<Array<{ decided_at: Date }>>(Prisma.sql`
      SELECT decided_at
      FROM rmm_telemetry.routing_decision
      WHERE agent_id = ${candidate.agentId}
        AND organization_id = ${membership.organizationId}
        AND matched_rule_id = ${ruleId}
        AND decided_at > NOW() - INTERVAL '24 hours'
      ORDER BY decided_at DESC
      LIMIT 1
    `);
    const decidedAt = recentRows[0]?.decided_at ?? null;
    if (decidedAt) {
      const elapsedSeconds = (Date.now() - decidedAt.getTime()) / 1000;
      cooldownBlocked = elapsedSeconds < ruleRow.cooldown_seconds;
    }
  }

  return res.json({
    wouldMatch: evaluation.matched,
    cooldownBlocked,
    action: ruleRow.action,
    dedupeKey: evaluation.dedupeKey,
    blockedReasons: readinessSource.blockedReasons,
    explanation: evaluation.explanation,
    readiness: {
      intentReady: Boolean(!ruleRow.intent_id || readinessSource.intent?.enabled),
      intentRequiresApproval: readinessSource.intent?.requires_approval ?? null,
      ticketProviderReady: readinessSource.halo.ready,
      llmRouterEnabled: LLM_ROUTER_ENABLED
    },
    rule: serializeRoutingRule(ruleRow, readinessSource),
    candidate
  });
});

rmmTelemetryRouter.post('/internal/decisions/execute', async (req, res) => {
  if (!requireInternalKey(req, res)) return;

  const body = asRecord(req.body) || {};
  const decisionId = parseBigIntValue(body.decisionId ?? body.decision_id);
  if (decisionId === null) {
    return res.status(400).json({ error: 'decisionId is required' });
  }

  const rows = await prisma.$queryRaw<RoutingDecisionRow[]>(Prisma.sql`
    SELECT
      id,
      organization_id,
      agent_id,
      domain,
      trigger_key,
      trigger_value,
      action,
      matched_rule_id,
      intent_id,
      reason,
      dedupe_key,
      source,
      source_ts,
      decided_at,
      execution_status,
      external_ref,
      outcome_message
    FROM rmm_telemetry.routing_decision
    WHERE id = ${decisionId}
    LIMIT 1
  `);
  const decision = rows[0];
  if (!decision) {
    return res.status(404).json({ error: 'Routing decision not found' });
  }

  if (decision.execution_status && decision.execution_status !== 'pending') {
    return res.json({
      accepted: true,
      alreadyExecuted: true,
      decisionId: decision.id.toString(),
      executionStatus: decision.execution_status,
      externalRef: decision.external_ref,
      outcomeMessage: decision.outcome_message
    });
  }

  const outcome = await executeRoutingDecision(decision);
  await updateRoutingDecisionExecution(
    decision.id,
    outcome.executionStatus,
    outcome.outcomeMessage,
    outcome.externalRef
  );

  return res.status(202).json({
    accepted: true,
    alreadyExecuted: false,
    decisionId: decision.id.toString(),
    executionStatus: outcome.executionStatus,
    externalRef: outcome.externalRef,
    outcomeMessage: outcome.outcomeMessage,
    remediationJobId: outcome.remediationJobId ?? null
  });
});

// GET /read/stability-overrides
rmmTelemetryRouter.get('/read/stability-overrides', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const rows = await loadStabilityOverridesForOrganization(membership.organizationId);
  const impactRows = rows.length > 0
    ? await loadFactKeyImpactRows(membership.organizationId)
    : [];

  return res.json({
    items: rows.map((row) => {
      const impact = summarizePatternImpact(row.factKeyPattern, impactRows, 6);

      return {
        id: String(row.id),
        factKeyPattern: row.factKeyPattern,
        stabilityClass: row.stabilityClass,
        reason: row.reason,
        createdBy: row.createdBy,
        createdAt: row.createdAt.toISOString(),
        updatedAt: row.updatedAt.toISOString(),
        matchedFactKeyCount: impact.matchedFactKeyCount,
        matchedCurrentFactCount: impact.matchedCurrentFactCount,
        matchedScopedBaselineCount: impact.matchedScopedBaselineCount,
        sampleFactKeys: impact.sampleFactKeys
      };
    })
  });
});

rmmTelemetryRouter.get('/read/stability-overrides/preview', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const factKeyPattern = readString(req.query.factKeyPattern);
  const limit = parsePositiveInt(req.query.limit, 8, 1, 50);
  if (!factKeyPattern) {
    return res.status(400).json({ error: 'factKeyPattern is required' });
  }

  const impactRows = await loadFactKeyImpactRows(
    membership.organizationId,
    buildPatternHint(factKeyPattern)
  );
  const impact = summarizePatternImpact(factKeyPattern, impactRows, limit);

  return res.json({
    factKeyPattern,
    matchedFactKeyCount: impact.matchedFactKeyCount,
    matchedCurrentFactCount: impact.matchedCurrentFactCount,
    matchedScopedBaselineCount: impact.matchedScopedBaselineCount,
    items: impact.items
  });
});

// POST /stability-overrides
rmmTelemetryRouter.post('/stability-overrides', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const body = asRecord(req.body) || {};
  const factKeyPattern = readString(body.factKeyPattern);
  const stabilityClass = readString(body.stabilityClass);
  const reason = readString(body.reason);

  if (!factKeyPattern || !stabilityClass) {
    return res.status(400).json({ error: 'factKeyPattern and stabilityClass are required' });
  }
  if (!['stable', 'noisy', 'ignored'].includes(stabilityClass)) {
    return res.status(400).json({ error: 'stabilityClass must be stable, noisy, or ignored' });
  }

  const row = await prisma.rmmTelemetryFactStabilityOverride.upsert({
    where: {
      organizationId_factKeyPattern: {
        organizationId: membership.organizationId,
        factKeyPattern
      }
    },
    create: {
      organizationId: membership.organizationId,
      factKeyPattern,
      stabilityClass,
      reason,
      createdBy: req.jwt!.sub
    },
    update: {
      stabilityClass,
      reason
    }
  });
  const impactRows = await loadFactKeyImpactRows(
    membership.organizationId,
    buildPatternHint(row.factKeyPattern)
  );
  const impact = summarizePatternImpact(row.factKeyPattern, impactRows, 6);

  return res.status(201).json({
    id: String(row.id),
    factKeyPattern: row.factKeyPattern,
    stabilityClass: row.stabilityClass,
    reason: row.reason,
    createdBy: row.createdBy,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
    matchedFactKeyCount: impact.matchedFactKeyCount,
    matchedCurrentFactCount: impact.matchedCurrentFactCount,
    matchedScopedBaselineCount: impact.matchedScopedBaselineCount,
    sampleFactKeys: impact.sampleFactKeys
  });
});

rmmTelemetryRouter.patch('/stability-overrides/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  const existing = await prisma.rmmTelemetryFactStabilityOverride.findFirst({
    where: { id, organizationId: membership.organizationId }
  });
  if (!existing) {
    return res.status(404).json({ error: 'Override not found' });
  }

  const body = asRecord(req.body) || {};
  const nextPattern = readString(body.factKeyPattern) ?? existing.factKeyPattern;
  const nextClass = readString(body.stabilityClass) ?? existing.stabilityClass;
  const reasonInput = body.reason;
  const nextReason = typeof reasonInput === 'string'
    ? reasonInput.trim() || null
    : reasonInput === null
      ? null
      : existing.reason;

  if (!['stable', 'noisy', 'ignored'].includes(nextClass)) {
    return res.status(400).json({ error: 'stabilityClass must be stable, noisy, or ignored' });
  }

  try {
    const row = await prisma.rmmTelemetryFactStabilityOverride.update({
      where: { id: existing.id },
      data: {
        factKeyPattern: nextPattern,
        stabilityClass: nextClass,
        reason: nextReason
      }
    });
    const impactRows = await loadFactKeyImpactRows(
      membership.organizationId,
      buildPatternHint(row.factKeyPattern)
    );
    const impact = summarizePatternImpact(row.factKeyPattern, impactRows, 6);

    return res.json({
      id: String(row.id),
      factKeyPattern: row.factKeyPattern,
      stabilityClass: row.stabilityClass,
      reason: row.reason,
      createdBy: row.createdBy,
      createdAt: row.createdAt.toISOString(),
      updatedAt: row.updatedAt.toISOString(),
      matchedFactKeyCount: impact.matchedFactKeyCount,
      matchedCurrentFactCount: impact.matchedCurrentFactCount,
      matchedScopedBaselineCount: impact.matchedScopedBaselineCount,
      sampleFactKeys: impact.sampleFactKeys
    });
  } catch (error) {
    if (error instanceof Prisma.PrismaClientKnownRequestError && error.code === 'P2002') {
      return res.status(409).json({ error: 'An override already exists for that factKeyPattern' });
    }
    throw error;
  }
});

// DELETE /stability-overrides/:id
rmmTelemetryRouter.delete('/stability-overrides/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const id = parseBigIntValue(req.params.id);
  if (id === null) return res.status(400).json({ error: 'Invalid id' });

  await prisma.rmmTelemetryFactStabilityOverride.deleteMany({
    where: { id, organizationId: membership.organizationId }
  });

  return res.json({ deleted: true });
});

// GET /read/intents
rmmTelemetryRouter.get('/read/intents', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const rows = await prisma.rmmTelemetryIntent.findMany({
    where: { organizationId: membership.organizationId },
    orderBy: [{ enabled: 'desc' }, { name: 'asc' }]
  });

  return res.json({
    items: rows.map((row) => ({
      id: row.id,
      name: row.name,
      description: row.description,
      type: row.type,
      allowList: row.allowList,
      steps: row.steps,
      aiPrompt: row.aiPrompt,
      triggerDomain: row.triggerDomain,
      triggerKey: row.triggerKey,
      requiresApproval: row.requiresApproval,
      maxRetries: row.maxRetries,
      timeoutSeconds: row.timeoutSeconds,
      enabled: row.enabled,
      createdBy: row.createdBy,
      createdAt: row.createdAt.toISOString(),
      updatedAt: row.updatedAt.toISOString()
    }))
  });
});

// POST /intents
rmmTelemetryRouter.post('/intents', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const body = asRecord(req.body) || {};
  const name = readString(body.name);
  if (!name) return res.status(400).json({ error: 'name is required' });

  const description = readString(body.description);
  const type = readString(body.type) || 'hardcoded';
  const allowList = body.allowList ?? body.allow_list ?? null;
  const steps = body.steps ?? null;
  const aiPrompt = readString(body.aiPrompt, body.ai_prompt);
  const triggerDomain = readString(body.triggerDomain, body.trigger_domain);
  const triggerKey = readString(body.triggerKey, body.trigger_key);
  const requiresApproval = readBoolean(body.requiresApproval ?? body.requires_approval) ?? true;
  const maxRetries = parseInteger(body.maxRetries ?? body.max_retries) ?? 1;
  const timeoutSeconds = parseInteger(body.timeoutSeconds ?? body.timeout_seconds) ?? 300;
  const enabled = readBoolean(body.enabled) ?? true;

  const row = await prisma.rmmTelemetryIntent.create({
    data: {
      organizationId: membership.organizationId,
      name,
      description,
      type,
      allowList: allowList as Prisma.InputJsonValue,
      steps: steps as Prisma.InputJsonValue,
      aiPrompt,
      triggerDomain,
      triggerKey,
      requiresApproval,
      maxRetries,
      timeoutSeconds,
      enabled,
      createdBy: req.jwt!.sub
    }
  });

  return res.status(201).json({
    id: row.id,
    name: row.name,
    description: row.description,
    type: row.type,
    allowList: row.allowList,
    steps: row.steps,
    aiPrompt: row.aiPrompt,
    triggerDomain: row.triggerDomain,
    triggerKey: row.triggerKey,
    requiresApproval: row.requiresApproval,
    maxRetries: row.maxRetries,
    timeoutSeconds: row.timeoutSeconds,
    enabled: row.enabled,
    createdBy: row.createdBy,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  });
});

// PATCH /intents/:id
rmmTelemetryRouter.patch('/intents/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const intentId = readString(req.params.id);
  if (!intentId) return res.status(400).json({ error: 'id is required' });

  const existing = await prisma.rmmTelemetryIntent.findFirst({
    where: { id: intentId, organizationId: membership.organizationId }
  });
  if (!existing) return res.status(404).json({ error: 'Intent not found' });

  const body = asRecord(req.body) || {};
  const data: Record<string, unknown> = {};

  if (body.name !== undefined) data.name = readString(body.name) || existing.name;
  if (body.description !== undefined) data.description = readString(body.description);
  if (body.type !== undefined) data.type = readString(body.type) || existing.type;
  if (body.allowList !== undefined || body.allow_list !== undefined) data.allowList = body.allowList ?? body.allow_list;
  if (body.steps !== undefined) data.steps = body.steps;
  if (body.aiPrompt !== undefined || body.ai_prompt !== undefined) data.aiPrompt = readString(body.aiPrompt, body.ai_prompt);
  if (body.triggerDomain !== undefined || body.trigger_domain !== undefined) data.triggerDomain = readString(body.triggerDomain, body.trigger_domain);
  if (body.triggerKey !== undefined || body.trigger_key !== undefined) data.triggerKey = readString(body.triggerKey, body.trigger_key);
  if (body.requiresApproval !== undefined || body.requires_approval !== undefined) data.requiresApproval = readBoolean(body.requiresApproval ?? body.requires_approval) ?? existing.requiresApproval;
  if (body.maxRetries !== undefined || body.max_retries !== undefined) data.maxRetries = parseInteger(body.maxRetries ?? body.max_retries) ?? existing.maxRetries;
  if (body.timeoutSeconds !== undefined || body.timeout_seconds !== undefined) data.timeoutSeconds = parseInteger(body.timeoutSeconds ?? body.timeout_seconds) ?? existing.timeoutSeconds;
  if (body.enabled !== undefined) data.enabled = readBoolean(body.enabled) ?? existing.enabled;

  const row = await prisma.rmmTelemetryIntent.update({
    where: { id: intentId },
    data: data as Prisma.RmmTelemetryIntentUpdateInput
  });

  return res.json({
    id: row.id,
    name: row.name,
    description: row.description,
    type: row.type,
    allowList: row.allowList,
    steps: row.steps,
    aiPrompt: row.aiPrompt,
    triggerDomain: row.triggerDomain,
    triggerKey: row.triggerKey,
    requiresApproval: row.requiresApproval,
    maxRetries: row.maxRetries,
    timeoutSeconds: row.timeoutSeconds,
    enabled: row.enabled,
    createdBy: row.createdBy,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  });
});

// DELETE /intents/:id
rmmTelemetryRouter.delete('/intents/:id', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;
  if (membership.role === 'VIEWER') {
    return res.status(403).json({ error: 'Insufficient permissions' });
  }

  const intentId = readString(req.params.id);
  if (!intentId) return res.status(400).json({ error: 'id is required' });

  const deleted = await prisma.rmmTelemetryIntent.deleteMany({
    where: { id: intentId, organizationId: membership.organizationId }
  });

  return res.json({ deleted: deleted.count > 0 });
});

// POST /patch/checkin (RMM server key only)
rmmTelemetryRouter.post('/patch/checkin', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const body = asRecord(req.body) || {};
  const agentId = readString(body.agentId ?? body.agent_id);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const organizationId = readString(body.organizationId ?? body.organization_id);
  const serverNow = new Date();
  const state = asRecord(body.state) || {};

  try {
    const plan = await evaluateAndPersistPatchPlan({
      agentId,
      organizationId,
      observedState: state,
      now: serverNow,
      persist: true
    });
    return res.json({ plan });
  } catch (error) {
    const status = typeof (error as any)?.status === 'number' ? (error as any).status : 500;
    return res.status(status).json({ error: (error as Error).message || 'patch check-in failed' });
  }
});

// POST /patch/progress (RMM server key only)
rmmTelemetryRouter.post('/patch/progress', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  let progressItems: NormalizedPatchProgress[];
  try {
    progressItems = parsePatchProgressBatch(req.body);
  } catch (error) {
    if (error instanceof PatchProgressValidationError) {
      return res.status(error.httpStatus).json({
        error: error.message,
        ...(error.itemIndex === undefined ? {} : { itemIndex: error.itemIndex })
      });
    }
    throw error;
  }

  let projection: { updated: number; ignored: number };
  try {
    projection = await prisma.$transaction((tx) => projectPatchProgressBatch(tx, progressItems));
  } catch (error) {
    if (error instanceof PatchProgressValidationError) {
      return res.status(error.httpStatus).json({
        error: error.message,
        ...(error.itemIndex === undefined ? {} : { itemIndex: error.itemIndex })
      });
    }
    return res.status(500).json({ error: 'patch progress projection failed' });
  }

  return res.status(202).json({
    accepted: true,
    updated: projection.updated,
    ignored: projection.ignored
  });
});

// POST /patch/action-result (RMM server key only)
rmmTelemetryRouter.post('/patch/action-result', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const body = asRecord(req.body) || {};
  const agentId = readString(body.agentId ?? body.agent_id);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  const action = readString(body.action);
  if (!action) return res.status(400).json({ error: 'action is required' });
  const status = readString(body.status) ?? 'reported';
  const updateKeys = asArray(body.updateKeys ?? body.update_keys)
    .map((value) => (typeof value === 'string' ? value.trim() : ''))
    .filter(Boolean);

  try {
    await recordPatchActionResult({
      organizationId: readString(body.organizationId ?? body.organization_id),
      agentId,
      operationId: readString(body.operationId ?? body.operation_id),
      action,
      status,
      updateKeys,
      evidence: body.evidence ?? null
    });
    return res.json({ accepted: true });
  } catch (error) {
    const responseStatus = typeof (error as any)?.status === 'number' ? (error as any).status : 500;
    return res.status(responseStatus).json({ error: (error as Error).message || 'patch action result failed' });
  }
});

// POST /remediation/agents/:agentId/patch-jobs/claim (RMM server key only)
rmmTelemetryRouter.post('/remediation/agents/:agentId/patch-jobs/claim', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const agentId = readString(req.params.agentId);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });

  const body = asRecord(req.body) || {};
  const limit = parsePositiveInt(body.limit, 1, 1, 3);

  await failStalePatchJobsForAgent(agentId);

  const jobs = await prisma.$queryRaw<
    Array<{
      id: bigint;
      organization_id: string;
      agent_id: string;
      intent_id: string;
      status: string;
      dedupe_key: string | null;
      metadata_jsonb: unknown;
      requested_at: Date;
      started_at: Date | null;
      finished_at: Date | null;
    }>
  >(Prisma.sql`
    UPDATE rmm_telemetry.remediation_job
    SET status = 'running',
        started_at = COALESCE(started_at, NOW()),
        finished_at = NULL
    WHERE id IN (
      SELECT id FROM rmm_telemetry.remediation_job
      WHERE status = 'queued'
        AND agent_id = ${agentId}
        AND intent_id = ${PATCH_INSTALL_INTENT_ID}
      ORDER BY requested_at ASC
      LIMIT ${limit}
      FOR UPDATE SKIP LOCKED
    )
    RETURNING
      id,
      organization_id,
      agent_id,
      intent_id,
      status,
      dedupe_key,
      metadata_jsonb,
      requested_at,
      started_at,
      finished_at
  `);

  const jobIds = jobs.map((job) => job.id);
  const steps = jobIds.length === 0
    ? []
    : await prisma.$queryRaw<
        Array<{
          id: bigint;
          job_id: bigint;
          step_index: number;
          command: string;
          status: string;
          evidence_jsonb: unknown | null;
          started_at: Date | null;
          finished_at: Date | null;
        }>
      >(Prisma.sql`
        SELECT
          id,
          job_id,
          step_index,
          command,
          status,
          evidence_jsonb,
          started_at,
          finished_at
        FROM rmm_telemetry.remediation_step
        WHERE job_id IN (${Prisma.join(jobIds)})
        ORDER BY job_id ASC, step_index ASC
      `);

  const stepsByJob = new Map<string, typeof steps>();
  for (const step of steps) {
    const key = String(step.job_id);
    const existing = stepsByJob.get(key) ?? [];
    existing.push(step);
    stepsByJob.set(key, existing);
  }

  return res.json({
    jobs: jobs.map((job) => ({
      id: String(job.id),
      organizationId: job.organization_id,
      agentId: job.agent_id,
      intentId: job.intent_id,
      status: job.status,
      dedupeKey: job.dedupe_key,
      metadata: remediationDispatchSnapshot(job.metadata_jsonb).metadata,
      requestedAt: job.requested_at.toISOString(),
      startedAt: job.started_at?.toISOString() ?? null,
      finishedAt: job.finished_at?.toISOString() ?? null,
      steps: (stepsByJob.get(String(job.id)) ?? []).map((step) => ({
        id: String(step.id),
        stepIndex: step.step_index,
        command: step.command,
        status: step.status,
        evidence: step.evidence_jsonb,
        startedAt: step.started_at?.toISOString() ?? null,
        finishedAt: step.finished_at?.toISOString() ?? null
      }))
    }))
  });
});

// PATCH /remediation/agents/:agentId/patch-jobs/:id/status (RMM server key only)
rmmTelemetryRouter.patch('/remediation/agents/:agentId/patch-jobs/:id/status', async (req, res) => {
  if (!requireInternalKey(req, res, { allowRmmServerKey: true })) return;
  const agentId = readString(req.params.agentId);
  const jobId = parseBigIntValue(req.params.id);
  if (!agentId) return res.status(400).json({ error: 'agentId is required' });
  if (jobId === null) return res.status(400).json({ error: 'Invalid job id' });

  const parsed = parseRemediationStatusReport(req.body);
  if (!parsed.ok) {
    return res.status(parsed.httpStatus).json({ error: parsed.error });
  }

  const result = await prisma.$transaction(async (tx) => {
    const transition = await transitionRemediationStatus(tx, {
      jobId,
      agentId,
      intentScope: 'patch'
    }, parsed.report);
    if (transition.outcome !== 'updated') return transition;

    await tx.$executeRaw(Prisma.sql`
      UPDATE public.rmm_patch_action
      SET status = ${parsed.report.status}
      WHERE remediation_job_id = ${transition.jobId}
        AND organization_id = ${transition.organizationId}
        AND agent_id = ${transition.agentId}
        AND (
          status NOT IN ('completed', 'failed', 'cancelled')
          OR status = ${parsed.report.status}
        )
    `);
    return transition;
  });
  if (result.outcome !== 'updated') {
    return sendRemediationTransitionFailure(res, result, parsed.report.status);
  }
  return res.json({ updated: true, status: parsed.report.status });
});

// PATCH /remediation/jobs/:id/status (deprecated service-key route)
rmmTelemetryRouter.patch('/remediation/jobs/:id/status', async (req, res) => {
  if (!requireInternalKey(req, res)) return;
  return res.status(410).json({
    error: 'This unscoped remediation status route has been retired',
    replacement: '/rmm/telemetry/remediation/agents/{agentId}/jobs/{commandId}/status'
  });
});

// GET /read/remediation/jobs (user auth)
rmmTelemetryRouter.get('/read/remediation/jobs', requireAuth, async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const limit = parsePositiveInt(req.query.limit, 50, 1, 200);
  const status = readString(req.query.status);

  const where: Prisma.RmmTelemetryRemediationJobWhereInput = { organizationId: membership.organizationId };
  if (status) where.status = status;

  const jobs = await prisma.rmmTelemetryRemediationJob.findMany({
    where,
    include: {
      steps: { orderBy: { stepIndex: 'asc' } }
    },
    orderBy: { requestedAt: 'desc' },
    take: limit
  });

  return res.json({
    items: jobs.map((j) => ({
      id: String(j.id),
      commandId: (j as any).commandId ?? null,
      organizationId: j.organizationId,
      agentId: j.agentId,
      decisionId: j.decisionId ? String(j.decisionId) : null,
      intentId: j.intentId,
      status: j.status,
      dedupeKey: j.dedupeKey,
      requestedAt: j.requestedAt.toISOString(),
      startedAt: j.startedAt?.toISOString() ?? null,
      finishedAt: j.finishedAt?.toISOString() ?? null,
      requestedBy: j.requestedBy,
      metadata: remediationDispatchSnapshot(j.metadata).metadata,
      steps: j.steps.map((s) => ({
        id: String(s.id),
        stepIndex: s.stepIndex,
        command: s.command,
        status: s.status,
        evidence: s.evidence,
        startedAt: s.startedAt?.toISOString() ?? null,
        finishedAt: s.finishedAt?.toISOString() ?? null
      }))
    }))
  });
});
