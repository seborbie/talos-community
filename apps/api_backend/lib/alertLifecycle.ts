import crypto from 'crypto';

export const ALERT_STATUSES = ['open', 'acknowledged', 'snoozed', 'resolved', 'suppressed'] as const;
export const ALERT_SEVERITIES = ['critical', 'high', 'medium', 'low', 'info'] as const;
export const ALERT_SOURCE_DOMAINS = ['event', 'baseline', 'scope_drift', 'decision'] as const;
export const ALERT_MATCH_OPERATORS = [
  'equals',
  'not_equals',
  'contains',
  'not_contains',
  'starts_with',
  'ends_with',
  'exists'
] as const;

export type AlertStatus = typeof ALERT_STATUSES[number];
export type AlertSeverity = typeof ALERT_SEVERITIES[number];
export type AlertSourceDomain = typeof ALERT_SOURCE_DOMAINS[number];
export type AlertMatchOperator = typeof ALERT_MATCH_OPERATORS[number];
export type AlertSeverityFilter = AlertSeverity | 'all';

export type AlertLifecycleState = {
  status: AlertStatus | string;
  firstSeenAt: Date;
  lastSeenAt: Date;
  occurrenceCount: number;
  acknowledgedAt?: Date | null;
  snoozedUntil?: Date | null;
  resolvedAt?: Date | null;
  suppressedUntil?: Date | null;
};

export type AlertLifecycleOptions = {
  dedupeWindowSeconds?: number | null;
};

export type AlertLifecyclePlan = {
  status: AlertStatus;
  firstSeenAt: Date;
  lastSeenAt: Date;
  occurrenceCount: number;
  acknowledgedAt: Date | null;
  snoozedUntil: Date | null;
  resolvedAt: Date | null;
  suppressedUntil: Date | null;
  duplicateSuppressed: boolean;
  reopened: boolean;
  snoozeExpired: boolean;
  notificationSuggested: boolean;
  reason: string;
};

export type AlertRuleMatchInput = {
  enabled?: boolean;
  organizationId?: string | null;
  customerId?: string | null;
  siteId?: string | null;
  agentId?: string | null;
  triggerDomain: string;
  triggerKey: string;
  matchOperator?: string | null;
  matchValue?: string | null;
  minSeverity?: string | null;
};

export type AlertCandidateInput = {
  organizationId?: string | null;
  customerId?: string | null;
  siteId?: string | null;
  agentId?: string | null;
  domain: string;
  triggerKey: string;
  valueText?: string | null;
  severity?: string | null;
};

const SEVERITY_RANK: Record<AlertSeverity, number> = {
  critical: 5,
  high: 4,
  medium: 3,
  low: 2,
  info: 1
};

function isOneOf<T extends readonly string[]>(values: T, value: string): value is T[number] {
  return (values as readonly string[]).includes(value);
}

export function normalizeAlertStatus(value: unknown): AlertStatus | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  return isOneOf(ALERT_STATUSES, normalized) ? normalized : null;
}

export function normalizeAlertSeverity(value: unknown, fallback: AlertSeverity = 'info'): AlertSeverity {
  if (typeof value !== 'string') return fallback;
  const normalized = value.trim().toLowerCase();
  if (normalized === 'fatal' || normalized === 'sev1' || normalized === 'emergency') return 'critical';
  if (normalized === 'warn' || normalized === 'warning') return 'medium';
  if (normalized === 'error') return 'high';
  return isOneOf(ALERT_SEVERITIES, normalized) ? normalized : fallback;
}

export function normalizeAlertSourceDomain(value: unknown): AlertSourceDomain | null {
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase();
  return isOneOf(ALERT_SOURCE_DOMAINS, normalized) ? normalized : null;
}

export function normalizeAlertMatchOperator(value: unknown, fallback: AlertMatchOperator = 'equals'): AlertMatchOperator | null {
  if (value === null || value === undefined || value === '') return fallback;
  if (typeof value !== 'string') return null;
  const normalized = value.trim().toLowerCase() as AlertMatchOperator;
  return isOneOf(ALERT_MATCH_OPERATORS, normalized) ? normalized : null;
}

export function severityRank(value: unknown): number {
  return SEVERITY_RANK[normalizeAlertSeverity(value)] ?? SEVERITY_RANK.info;
}

export function highestSeverity(left: unknown, right: unknown): AlertSeverity {
  const normalizedLeft = normalizeAlertSeverity(left);
  const normalizedRight = normalizeAlertSeverity(right);
  return severityRank(normalizedRight) > severityRank(normalizedLeft) ? normalizedRight : normalizedLeft;
}

export function filterAlertsBySeverity<T extends { severity: string }>(
  items: T[],
  filter: AlertSeverityFilter | string | null | undefined
): T[] {
  if (!filter || filter === 'all') return items;
  const normalized = normalizeAlertSeverity(filter, 'info');
  return items.filter((item) => normalizeAlertSeverity(item.severity, 'info') === normalized);
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

export function wildcardPatternMatches(pattern: string, value: string): boolean {
  return wildcardMatchChars(
    Array.from(pattern.toLowerCase()),
    Array.from(value.toLowerCase())
  );
}

export function alertOperatorMatches(
  operator: AlertMatchOperator,
  expected: string | null | undefined,
  candidate: string | null | undefined
): boolean {
  const desired = expected?.trim() ?? '';
  const actual = candidate?.trim() ?? '';
  switch (operator) {
    case 'exists':
      return actual.length > 0;
    case 'contains':
      return desired.length > 0 && actual.includes(desired);
    case 'not_contains':
      return desired.length > 0 && !actual.includes(desired);
    case 'starts_with':
      return desired.length > 0 && actual.startsWith(desired);
    case 'ends_with':
      return desired.length > 0 && actual.endsWith(desired);
    case 'not_equals':
      return desired.length > 0 && actual !== desired;
    case 'equals':
    default:
      return desired.length > 0 ? actual === desired : true;
  }
}

export function alertRuleMatchesCandidate(
  rule: AlertRuleMatchInput,
  candidate: AlertCandidateInput
): boolean {
  if (rule.enabled === false) return false;
  if (rule.organizationId && candidate.organizationId && rule.organizationId !== candidate.organizationId) return false;
  if (rule.customerId && rule.customerId !== candidate.customerId) return false;
  if (rule.siteId && rule.siteId !== candidate.siteId) return false;
  if (rule.agentId && rule.agentId !== candidate.agentId) return false;

  const domain = normalizeAlertSourceDomain(candidate.domain);
  const ruleDomain = normalizeAlertSourceDomain(rule.triggerDomain);
  if (!domain || !ruleDomain || domain !== ruleDomain) return false;
  if (!wildcardPatternMatches(rule.triggerKey, candidate.triggerKey)) return false;

  const operator = normalizeAlertMatchOperator(rule.matchOperator, 'equals') || 'equals';
  if (!alertOperatorMatches(operator, rule.matchValue ?? null, candidate.valueText ?? '')) return false;

  if (rule.minSeverity && severityRank(candidate.severity) < severityRank(rule.minSeverity)) return false;
  return true;
}

export function buildAlertFingerprint(
  ruleId: string | number | bigint | null | undefined,
  candidate: Pick<AlertCandidateInput, 'organizationId' | 'agentId' | 'domain' | 'triggerKey'>
): string {
  return crypto.createHash('sha256').update([
    candidate.organizationId || 'none',
    candidate.agentId || 'none',
    String(ruleId ?? 'manual'),
    candidate.domain,
    candidate.triggerKey
  ].join('|')).digest('hex');
}

function secondsBetween(left: Date, right: Date): number {
  return Math.abs(left.getTime() - right.getTime()) / 1000;
}

export function planAlertLifecycle(
  existing: AlertLifecycleState | null,
  now: Date = new Date(),
  options: AlertLifecycleOptions = {}
): AlertLifecyclePlan {
  const dedupeWindowSeconds = Math.max(0, options.dedupeWindowSeconds ?? 0);
  const status = normalizeAlertStatus(existing?.status) || 'open';
  const withinDedupeWindow = Boolean(
    existing &&
    dedupeWindowSeconds > 0 &&
    secondsBetween(now, existing.lastSeenAt) < dedupeWindowSeconds
  );

  if (!existing) {
    return {
      status: 'open',
      firstSeenAt: now,
      lastSeenAt: now,
      occurrenceCount: 1,
      acknowledgedAt: null,
      snoozedUntil: null,
      resolvedAt: null,
      suppressedUntil: null,
      duplicateSuppressed: false,
      reopened: false,
      snoozeExpired: false,
      notificationSuggested: true,
      reason: 'new_alert'
    };
  }

  const base = {
    firstSeenAt: existing.firstSeenAt,
    lastSeenAt: now,
    occurrenceCount: Math.max(0, existing.occurrenceCount || 0) + 1,
    acknowledgedAt: existing.acknowledgedAt ?? null,
    snoozedUntil: existing.snoozedUntil ?? null,
    resolvedAt: existing.resolvedAt ?? null,
    suppressedUntil: existing.suppressedUntil ?? null,
    duplicateSuppressed: withinDedupeWindow,
    reopened: false,
    snoozeExpired: false
  };

  if (status === 'resolved') {
    return {
      ...base,
      status: 'open',
      acknowledgedAt: null,
      snoozedUntil: null,
      resolvedAt: null,
      duplicateSuppressed: withinDedupeWindow,
      reopened: true,
      notificationSuggested: !withinDedupeWindow,
      reason: withinDedupeWindow ? 'resolved_duplicate_suppressed' : 'resolved_reopened'
    };
  }

  if (status === 'snoozed') {
    if (base.snoozedUntil && base.snoozedUntil > now) {
      return {
        ...base,
        status: 'snoozed',
        duplicateSuppressed: true,
        notificationSuggested: false,
        reason: 'snoozed'
      };
    }
    return {
      ...base,
      status: 'open',
      snoozedUntil: null,
      snoozeExpired: true,
      notificationSuggested: !withinDedupeWindow,
      reason: withinDedupeWindow ? 'snooze_expired_duplicate_suppressed' : 'snooze_expired'
    };
  }

  if (status === 'suppressed') {
    if (base.suppressedUntil && base.suppressedUntil > now) {
      return {
        ...base,
        status: 'suppressed',
        duplicateSuppressed: true,
        notificationSuggested: false,
        reason: 'suppressed'
      };
    }
    return {
      ...base,
      status: 'open',
      suppressedUntil: null,
      notificationSuggested: !withinDedupeWindow,
      reason: withinDedupeWindow ? 'suppression_expired_duplicate_suppressed' : 'suppression_expired'
    };
  }

  if (withinDedupeWindow) {
    return {
      ...base,
      status,
      notificationSuggested: false,
      reason: 'duplicate_suppressed'
    };
  }

  return {
    ...base,
    status,
    notificationSuggested: status === 'open',
    reason: status === 'acknowledged' ? 'acknowledged_recurrence' : 'recurrence'
  };
}
