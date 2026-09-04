export const MAX_PATCH_PROGRESS_ITEMS = 100;
export const MAX_PATCH_PROGRESS_BYTES = 256 * 1024;
export const MAX_PATCH_PROGRESS_EVIDENCE_BYTES = 128 * 1024;
export const MAX_PATCH_PROGRESS_FUTURE_SKEW_MS = 10 * 60 * 1000;

const MAX_IDENTIFIER_LENGTH = 255;
const MAX_EVENT_TYPE_LENGTH = 128;
const MAX_PHASE_LENGTH = 64;
const MAX_ERROR_LENGTH = 4096;
const MAX_UPDATE_KEYS = 1000;
const MAX_UPDATE_KEY_LENGTH = 512;
const RFC3339_PATTERN =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(?:Z|([+-])(\d{2}):(\d{2}))$/;
const PHASE_PATTERN = /^[a-z][a-z0-9_-]*$/;

export type PatchProgressStatus = 'running' | 'completed' | 'failed' | 'cancelled';

export const PATCH_PROGRESS_TERMINAL_STATUSES: ReadonlySet<PatchProgressStatus> = new Set([
  'completed',
  'failed',
  'cancelled',
]);

export const PATCH_PROGRESS_STATUSES: ReadonlySet<PatchProgressStatus> = new Set([
  'running',
  ...PATCH_PROGRESS_TERMINAL_STATUSES,
]);

export type PatchProgressTransition =
  'apply' | 'duplicate_terminal' | 'terminal_conflict' | 'stale';

export type NormalizedPatchProgress = {
  organizationId: string;
  agentId: string;
  operationId: string;
  eventType: string | null;
  status: PatchProgressStatus;
  phase: string;
  reportedAt: Date;
  summary: Record<string, unknown> | null;
  error: string | null;
  updateKeys: string[];
  progress: Record<string, unknown>;
  progressJson: string;
  evidence: Record<string, unknown>;
  evidenceJson: string;
};

export class PatchProgressValidationError extends Error {
  constructor(
    message: string,
    readonly httpStatus: 400 | 413 = 400,
    readonly itemIndex?: number,
  ) {
    super(message);
    this.name = 'PatchProgressValidationError';
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function firstNonEmptyString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value !== 'string') continue;
    const trimmed = value.trim();
    if (trimmed) return trimmed;
  }
  return null;
}

function requiredIdentifier(value: unknown, name: string, itemIndex: number): string {
  const normalized = firstNonEmptyString(value);
  if (!normalized) {
    throw new PatchProgressValidationError(`${name} is required`, 400, itemIndex);
  }
  if (normalized.length > MAX_IDENTIFIER_LENGTH) {
    throw new PatchProgressValidationError(
      `${name} must not exceed ${MAX_IDENTIFIER_LENGTH} characters`,
      400,
      itemIndex,
    );
  }
  return normalized;
}

function encodedLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

function serializeJson(value: unknown, name: string, itemIndex: number): string {
  try {
    const serialized = JSON.stringify(value);
    if (serialized === undefined) {
      throw new Error('value is not JSON serializable');
    }
    return serialized;
  } catch {
    throw new PatchProgressValidationError(`${name} must be valid JSON`, 400, itemIndex);
  }
}

function parseReportedAt(value: unknown, itemIndex: number, serverNow: Date): Date {
  const reportedAt = firstNonEmptyString(value);
  const parts = reportedAt ? RFC3339_PATTERN.exec(reportedAt) : null;
  if (!reportedAt || !parts) {
    throw new PatchProgressValidationError(
      'reportedAt must be an RFC3339 timestamp with an explicit timezone',
      400,
      itemIndex,
    );
  }
  const year = Number(parts[1]);
  const month = Number(parts[2]);
  const day = Number(parts[3]);
  const hour = Number(parts[4]);
  const minute = Number(parts[5]);
  const second = Number(parts[6]);
  const offsetHour = parts[8] === undefined ? 0 : Number(parts[8]);
  const offsetMinute = parts[9] === undefined ? 0 : Number(parts[9]);
  const daysInMonth =
    month >= 1 && month <= 12 ? new Date(Date.UTC(year, month, 0)).getUTCDate() : 0;
  if (
    day < 1 ||
    day > daysInMonth ||
    hour > 23 ||
    minute > 59 ||
    second > 59 ||
    offsetHour > 23 ||
    offsetMinute > 59
  ) {
    throw new PatchProgressValidationError('reportedAt must be a valid timestamp', 400, itemIndex);
  }
  const parsed = new Date(reportedAt);
  if (Number.isNaN(parsed.getTime())) {
    throw new PatchProgressValidationError('reportedAt must be a valid timestamp', 400, itemIndex);
  }
  if (parsed.getTime() > serverNow.getTime() + MAX_PATCH_PROGRESS_FUTURE_SKEW_MS) {
    throw new PatchProgressValidationError(
      `reportedAt must not be more than ${MAX_PATCH_PROGRESS_FUTURE_SKEW_MS / 60_000} minutes in the future`,
      400,
      itemIndex,
    );
  }
  return parsed;
}

function parseStatus(value: unknown, itemIndex: number): PatchProgressStatus {
  const status = firstNonEmptyString(value);
  if (!status || !PATCH_PROGRESS_STATUSES.has(status as PatchProgressStatus)) {
    throw new PatchProgressValidationError(
      'status must be running, completed, failed, or cancelled',
      400,
      itemIndex,
    );
  }
  return status as PatchProgressStatus;
}

function parsePhase(value: unknown, itemIndex: number): string {
  const phase = firstNonEmptyString(value);
  if (!phase || phase.length > MAX_PHASE_LENGTH || !PHASE_PATTERN.test(phase)) {
    throw new PatchProgressValidationError(
      `phase must match ${PHASE_PATTERN} and not exceed ${MAX_PHASE_LENGTH} characters`,
      400,
      itemIndex,
    );
  }
  return phase;
}

function parseOptionalString(
  value: unknown,
  name: string,
  maxLength: number,
  itemIndex: number,
): string | null {
  if (value === undefined || value === null) return null;
  const normalized = firstNonEmptyString(value);
  if (!normalized) return null;
  if (normalized.length > maxLength) {
    throw new PatchProgressValidationError(
      `${name} must not exceed ${maxLength} characters`,
      400,
      itemIndex,
    );
  }
  return normalized;
}

function parseUpdateKeys(value: unknown, itemIndex: number): string[] {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) {
    throw new PatchProgressValidationError('updateKeys must be an array', 400, itemIndex);
  }
  if (value.length > MAX_UPDATE_KEYS) {
    throw new PatchProgressValidationError(
      `updateKeys must not contain more than ${MAX_UPDATE_KEYS} entries`,
      400,
      itemIndex,
    );
  }
  return value.map((entry, entryIndex) => {
    const updateKey = firstNonEmptyString(entry);
    if (!updateKey || updateKey.length > MAX_UPDATE_KEY_LENGTH) {
      throw new PatchProgressValidationError(
        `updateKeys[${entryIndex}] must be a non-empty string no longer than ${MAX_UPDATE_KEY_LENGTH} characters`,
        400,
        itemIndex,
      );
    }
    return updateKey;
  });
}

function parseProgressItem(
  value: unknown,
  itemIndex: number,
  serverNow: Date,
): NormalizedPatchProgress {
  const record = asRecord(value);
  if (!record) {
    throw new PatchProgressValidationError('progress item must be an object', 400, itemIndex);
  }

  const organizationId = requiredIdentifier(
    record.organizationId ?? record.organization_id,
    'organizationId',
    itemIndex,
  );
  const agentId = requiredIdentifier(record.agentId ?? record.agent_id, 'agentId', itemIndex);
  const operationId = requiredIdentifier(
    firstNonEmptyString(
      record.commandId ?? record.command_id,
      record.jobId ?? record.job_id,
      record.operationId ?? record.operation_id,
    ),
    'commandId, jobId, or operationId',
    itemIndex,
  );
  const status = parseStatus(record.status, itemIndex);
  const phase = parsePhase(record.phase, itemIndex);
  const reportedAt = parseReportedAt(record.reportedAt ?? record.reported_at, itemIndex, serverNow);
  const eventType = parseOptionalString(
    record.eventType ?? record.event_type,
    'eventType',
    MAX_EVENT_TYPE_LENGTH,
    itemIndex,
  );
  const error = parseOptionalString(record.error, 'error', MAX_ERROR_LENGTH, itemIndex);
  const summary =
    record.summary === undefined || record.summary === null ? null : asRecord(record.summary);
  if (record.summary !== undefined && record.summary !== null && !summary) {
    throw new PatchProgressValidationError('summary must be an object or null', 400, itemIndex);
  }
  const updateKeys = parseUpdateKeys(record.updateKeys ?? record.update_keys, itemIndex);

  const progress = {
    ...record,
    organizationId,
    agentId,
    status,
    phase,
    reportedAt: reportedAt.toISOString(),
  };
  const evidence = {
    summary: record.summary ?? null,
    updates: record.updates ?? [],
    currentUpdate: record.currentUpdate ?? null,
  };
  const progressJson = serializeJson(progress, 'progress', itemIndex);
  const evidenceJson = serializeJson(evidence, 'progress evidence', itemIndex);

  if (encodedLength(progressJson) > MAX_PATCH_PROGRESS_BYTES) {
    throw new PatchProgressValidationError(
      `progress must not exceed ${MAX_PATCH_PROGRESS_BYTES} encoded bytes`,
      413,
      itemIndex,
    );
  }
  if (encodedLength(evidenceJson) > MAX_PATCH_PROGRESS_EVIDENCE_BYTES) {
    throw new PatchProgressValidationError(
      `progress evidence must not exceed ${MAX_PATCH_PROGRESS_EVIDENCE_BYTES} encoded bytes`,
      413,
      itemIndex,
    );
  }

  return {
    organizationId,
    agentId,
    operationId,
    eventType,
    status,
    phase,
    reportedAt,
    summary,
    error,
    updateKeys,
    progress,
    progressJson,
    evidence,
    evidenceJson,
  };
}

export function parsePatchProgressBatch(
  body: unknown,
  serverNow: Date = new Date(),
): NormalizedPatchProgress[] {
  const record = asRecord(body);
  if (!record) {
    throw new PatchProgressValidationError('request body must be an object');
  }
  const items = Object.prototype.hasOwnProperty.call(record, 'progress')
    ? record.progress
    : [record];
  if (!Array.isArray(items) || items.length === 0) {
    throw new PatchProgressValidationError('progress must be a non-empty array');
  }
  if (items.length > MAX_PATCH_PROGRESS_ITEMS) {
    throw new PatchProgressValidationError(
      `progress must not contain more than ${MAX_PATCH_PROGRESS_ITEMS} items`,
      413,
    );
  }
  return items.map((item, itemIndex) => parseProgressItem(item, itemIndex, serverNow));
}

export function classifyPatchProgressTransition(
  existing: { status: string; reportedAt: Date | null } | null,
  incoming: Pick<NormalizedPatchProgress, 'status' | 'reportedAt'>,
): PatchProgressTransition {
  if (!existing) return 'apply';
  if (PATCH_PROGRESS_TERMINAL_STATUSES.has(existing.status as PatchProgressStatus)) {
    return existing.status === incoming.status ? 'duplicate_terminal' : 'terminal_conflict';
  }
  if (existing.reportedAt && existing.reportedAt.getTime() > incoming.reportedAt.getTime()) {
    return 'stale';
  }
  return 'apply';
}
