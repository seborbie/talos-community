export type AgentHealthStatus = 'healthy' | 'warning' | 'critical' | 'offline';
export type AgentHealthSeverity = 'info' | 'warning' | 'critical';

export type AgentHealthReason = {
  code: string;
  severity: AgentHealthSeverity;
  summary: string;
  detail: string | null;
  observedAt: string | null;
  ageMs: number | null;
  alertKey: string;
};

export type AgentHealthSummary = {
  status: AgentHealthStatus;
  severity: AgentHealthSeverity;
  summary: string;
  reasons: AgentHealthReason[];
  computedAt: string;
  signals: {
    websocketStatus: 'connected' | 'disconnected' | 'unknown';
    lastSeenAt: string | null;
    telemetryCollectedAt: string | null;
    agentVersion: string | null;
    targetVersion: string | null;
    commandFailureCount: number;
    updaterFailureCount: number;
    remediationFailureCount: number;
  };
};

export type AgentHealthThresholds = {
  staleAgentMs: number;
  offlineAgentMs: number;
  staleTelemetryMs: number;
  rebootRequiredMs: number;
  repeatedUpdaterFailureCount: number;
};

export type AgentHealthInput = {
  now: Date;
  lastSeenAt?: Date | string | null;
  websocketStatus?: string | null;
  telemetryCollectedAt?: Date | string | null;
  agentVersion?: string | null;
  telemetryAgentVersion?: string | null;
  targetAgentVersion?: string | null;
  rebootRequired?: boolean | null;
  commandFailureCount?: number | null;
  updaterFailureCount?: number | null;
  remediationFailureCount?: number | null;
  latestCommandFailureAt?: Date | string | null;
  latestUpdaterFailureAt?: Date | string | null;
  latestRemediationFailureAt?: Date | string | null;
};

export type ExistingHealthAlert = {
  alertKey: string;
  status: 'active' | 'resolved' | string;
};

export type HealthAlertReconciliation = {
  activeKeys: string[];
  newKeys: string[];
  recurringKeys: string[];
  resolveKeys: string[];
};

export const DEFAULT_AGENT_HEALTH_THRESHOLDS: AgentHealthThresholds = {
  staleAgentMs: 10 * 60 * 1000,
  offlineAgentMs: 30 * 60 * 1000,
  staleTelemetryMs: 2 * 60 * 60 * 1000,
  rebootRequiredMs: 24 * 60 * 60 * 1000,
  repeatedUpdaterFailureCount: 2
};

function parsePositiveNumber(value: unknown, fallback: number): number {
  const parsed = typeof value === 'string' ? Number(value) : typeof value === 'number' ? value : NaN;
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export function readAgentHealthThresholds(env: NodeJS.ProcessEnv = process.env): AgentHealthThresholds {
  return {
    staleAgentMs: parsePositiveNumber(env.RMM_AGENT_HEALTH_STALE_MS, DEFAULT_AGENT_HEALTH_THRESHOLDS.staleAgentMs),
    offlineAgentMs: parsePositiveNumber(env.RMM_AGENT_HEALTH_OFFLINE_MS, DEFAULT_AGENT_HEALTH_THRESHOLDS.offlineAgentMs),
    staleTelemetryMs: parsePositiveNumber(
      env.RMM_AGENT_HEALTH_TELEMETRY_STALE_MS,
      DEFAULT_AGENT_HEALTH_THRESHOLDS.staleTelemetryMs
    ),
    rebootRequiredMs: parsePositiveNumber(
      env.RMM_AGENT_HEALTH_REBOOT_REQUIRED_MS,
      DEFAULT_AGENT_HEALTH_THRESHOLDS.rebootRequiredMs
    ),
    repeatedUpdaterFailureCount: Math.max(
      1,
      Math.floor(parsePositiveNumber(
        env.RMM_AGENT_HEALTH_UPDATER_FAILURE_COUNT,
        DEFAULT_AGENT_HEALTH_THRESHOLDS.repeatedUpdaterFailureCount
      ))
    )
  };
}

export function normalizeVersion(value: string | null | undefined): number[] | null {
  if (!value) return null;
  const parts = value.trim().replace(/^v/i, '').split('.');
  if (parts.length === 0 || parts.some((part) => !part.trim())) return null;
  const parsed = parts.map((part) => Number(part));
  if (parsed.some((part) => !Number.isInteger(part) || part < 0)) return null;
  return parsed;
}

export function compareVersions(left: string | null | undefined, right: string | null | undefined): number | null {
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

function parseDate(value: Date | string | null | undefined): Date | null {
  if (value instanceof Date && !Number.isNaN(value.getTime())) return value;
  if (typeof value !== 'string' || !value.trim()) return null;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function clampCount(value: number | null | undefined): number {
  if (!Number.isFinite(value ?? NaN)) return 0;
  return Math.max(0, Math.floor(value as number));
}

function normalizeWebsocketStatus(value: string | null | undefined): 'connected' | 'disconnected' | 'unknown' {
  const normalized = value?.trim().toLowerCase();
  if (normalized === 'connected' || normalized === 'disconnected') return normalized;
  return 'unknown';
}

function formatDuration(ageMs: number): string {
  const minutes = Math.max(1, Math.round(ageMs / 60000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours}h`;
  return `${Math.round(hours / 24)}d`;
}

function toIso(value: Date | null): string | null {
  return value ? value.toISOString() : null;
}

function meaningfulString(value: string | null | undefined): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

export function buildAgentHealth(
  input: AgentHealthInput,
  thresholds: AgentHealthThresholds = DEFAULT_AGENT_HEALTH_THRESHOLDS
): AgentHealthSummary {
  const now = input.now;
  const lastSeenAt = parseDate(input.lastSeenAt);
  const telemetryCollectedAt = parseDate(input.telemetryCollectedAt);
  const websocketStatus = normalizeWebsocketStatus(input.websocketStatus);
  const agentVersion = meaningfulString(input.telemetryAgentVersion) ?? meaningfulString(input.agentVersion);
  const targetVersion = meaningfulString(input.targetAgentVersion);
  const commandFailureCount = clampCount(input.commandFailureCount);
  const updaterFailureCount = clampCount(input.updaterFailureCount);
  const remediationFailureCount = clampCount(input.remediationFailureCount);
  const reasons: AgentHealthReason[] = [];

  const addReason = (
    code: string,
    severity: AgentHealthSeverity,
    summary: string,
    detail: string | null,
    observedAt: Date | null,
    ageMs: number | null
  ) => {
    reasons.push({
      code,
      severity,
      summary,
      detail,
      observedAt: toIso(observedAt),
      ageMs,
      alertKey: code
    });
  };

  if (!lastSeenAt) {
    addReason('last_seen_missing', 'critical', 'Agent has never checked in', null, null, null);
  } else {
    const ageMs = now.getTime() - lastSeenAt.getTime();
    if (ageMs >= thresholds.offlineAgentMs) {
      addReason(
        'agent_offline',
        'critical',
        `Agent has not checked in for ${formatDuration(ageMs)}`,
        'The last agent check-in is beyond the offline threshold.',
        lastSeenAt,
        ageMs
      );
    } else if (ageMs >= thresholds.staleAgentMs) {
      addReason(
        'agent_stale',
        'warning',
        `Agent check-in is stale by ${formatDuration(ageMs)}`,
        'The agent is past the stale threshold but has not crossed the offline threshold.',
        lastSeenAt,
        ageMs
      );
    }
  }

  if (websocketStatus === 'disconnected') {
    addReason(
      'websocket_disconnected',
      'warning',
      'Live websocket is disconnected',
      'Remote actions may fail until the agent reconnects.',
      lastSeenAt,
      lastSeenAt ? now.getTime() - lastSeenAt.getTime() : null
    );
  }

  if (!telemetryCollectedAt) {
    addReason(
      'telemetry_missing',
      'warning',
      'No telemetry snapshot has been received',
      'Inventory, patch, and reboot signals are unavailable.',
      null,
      null
    );
  } else {
    const ageMs = now.getTime() - telemetryCollectedAt.getTime();
    if (ageMs >= thresholds.staleTelemetryMs) {
      addReason(
        'telemetry_stale',
        'warning',
        `Telemetry is stale by ${formatDuration(ageMs)}`,
        'The latest snapshot is older than the telemetry freshness threshold.',
        telemetryCollectedAt,
        ageMs
      );
    }
  }

  if (targetVersion && agentVersion && compareVersions(agentVersion, targetVersion) === -1) {
    addReason(
      'agent_version_drift',
      'warning',
      `Agent version ${agentVersion} is behind ${targetVersion}`,
      'The endpoint has not reached the configured target agent version.',
      lastSeenAt,
      lastSeenAt ? now.getTime() - lastSeenAt.getTime() : null
    );
  }

  if (updaterFailureCount >= thresholds.repeatedUpdaterFailureCount) {
    addReason(
      'updater_repeated_failures',
      'critical',
      `${updaterFailureCount} updater failures in the last 24h`,
      'Repeated updater errors can prevent self-healing and security updates.',
      parseDate(input.latestUpdaterFailureAt),
      null
    );
  }

  if (commandFailureCount > 0) {
    addReason(
      'recent_command_failures',
      commandFailureCount >= 3 ? 'critical' : 'warning',
      `${commandFailureCount} command failures in the last 24h`,
      'Recent denied or failed command executions need review.',
      parseDate(input.latestCommandFailureAt),
      null
    );
  }

  if (remediationFailureCount > 0) {
    addReason(
      'recent_remediation_failures',
      'critical',
      `${remediationFailureCount} remediation failures in the last 24h`,
      'Automated remediation jobs failed or were cancelled.',
      parseDate(input.latestRemediationFailureAt),
      null
    );
  }

  if (input.rebootRequired && telemetryCollectedAt) {
    const ageMs = now.getTime() - telemetryCollectedAt.getTime();
    if (ageMs >= thresholds.rebootRequiredMs) {
      addReason(
        'reboot_required_aged',
        'warning',
        `Reboot required for at least ${formatDuration(ageMs)}`,
        'The endpoint has reported a pending reboot beyond the configured age threshold.',
        telemetryCollectedAt,
        ageMs
      );
    }
  }

  const hasCritical = reasons.some((reason) => reason.severity === 'critical');
  const status: AgentHealthStatus = reasons.some((reason) => reason.code === 'agent_offline' || reason.code === 'last_seen_missing')
    ? 'offline'
    : hasCritical
      ? 'critical'
      : reasons.length > 0
        ? 'warning'
        : 'healthy';
  const severity: AgentHealthSeverity = hasCritical || status === 'offline'
    ? 'critical'
    : reasons.length > 0
      ? 'warning'
      : 'info';

  return {
    status,
    severity,
    summary: reasons[0]?.summary ?? 'No health issues detected',
    reasons,
    computedAt: now.toISOString(),
    signals: {
      websocketStatus,
      lastSeenAt: toIso(lastSeenAt),
      telemetryCollectedAt: toIso(telemetryCollectedAt),
      agentVersion,
      targetVersion,
      commandFailureCount,
      updaterFailureCount,
      remediationFailureCount
    }
  };
}

export function reconcileHealthAlerts(
  existingAlerts: ExistingHealthAlert[],
  activeReasons: AgentHealthReason[]
): HealthAlertReconciliation {
  const activeKeys = Array.from(new Set(activeReasons.map((reason) => reason.alertKey))).sort();
  const activeKeySet = new Set(activeKeys);
  const activeExisting = new Set(
    existingAlerts
      .filter((alert) => alert.status === 'active')
      .map((alert) => alert.alertKey)
  );
  const allExisting = new Set(existingAlerts.map((alert) => alert.alertKey));

  return {
    activeKeys,
    newKeys: activeKeys.filter((key) => !allExisting.has(key)),
    recurringKeys: activeKeys.filter((key) => allExisting.has(key) && !activeExisting.has(key)),
    resolveKeys: Array.from(activeExisting).filter((key) => !activeKeySet.has(key)).sort()
  };
}
