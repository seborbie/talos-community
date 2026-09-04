export type JsonObject = Record<string, unknown>;

export const REMEDIATION_DISPATCH_SNAPSHOT_KEY = '__talosDispatchV1';

function asRecord(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as JsonObject;
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function parseInteger(value: unknown): number | null {
  if (typeof value === 'number' && Number.isInteger(value)) return value;
  if (typeof value !== 'string' || !value.trim()) return null;
  const parsed = Number(value);
  return Number.isInteger(parsed) ? parsed : null;
}

export function persistedRemediationMetadata(options: {
  metadata?: unknown;
  execution?: unknown;
  steps?: unknown;
}): JsonObject {
  return {
    ...(asRecord(options.metadata) || {}),
    [REMEDIATION_DISPATCH_SNAPSHOT_KEY]: {
      execution: asRecord(options.execution) || {},
      steps: asArray(options.steps),
    },
  };
}

export function remediationDispatchSnapshot(metadata: unknown): {
  metadata: JsonObject;
  execution: JsonObject;
  steps: unknown[];
} {
  const persisted = { ...(asRecord(metadata) || {}) };
  const snapshot = asRecord(persisted[REMEDIATION_DISPATCH_SNAPSHOT_KEY]);
  delete persisted[REMEDIATION_DISPATCH_SNAPSHOT_KEY];
  return {
    metadata: persisted,
    execution: asRecord(snapshot?.execution) || {},
    steps: asArray(snapshot?.steps),
  };
}

export type RemediationDispatchJobRow = {
  id: bigint;
  command_id: string;
  organization_id: string;
  agent_id: string;
  decision_id: bigint | null;
  intent_id: string;
  dedupe_key: string | null;
  requested_by: string;
  requested_at: Date;
  metadata_jsonb: unknown;
};

export type RemediationDispatchStepRow = {
  id: bigint;
  job_id: bigint;
  step_index: number;
  command: string;
  status: string;
  evidence_jsonb: unknown | null;
};

export function buildRemediationDispatchJobs(
  jobRows: RemediationDispatchJobRow[],
  stepRows: RemediationDispatchStepRow[],
) {
  const jobs = [...jobRows].sort((left, right) => {
    const requestedAtOrder = left.requested_at.getTime() - right.requested_at.getTime();
    if (requestedAtOrder !== 0) return requestedAtOrder;
    return left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
  });

  const stepsByJob = new Map<string, RemediationDispatchStepRow[]>();
  for (const step of stepRows) {
    const key = String(step.job_id);
    const existing = stepsByJob.get(key) ?? [];
    existing.push(step);
    stepsByJob.set(key, existing);
  }

  return jobs.map((job) => {
    const snapshot = remediationDispatchSnapshot(job.metadata_jsonb);
    const frozenStepsByIndex = new Map<number, JsonObject>();
    for (const [index, value] of snapshot.steps.entries()) {
      const record = asRecord(value);
      if (!record) continue;
      const stepIndex = parseInteger(record.stepIndex ?? record.step_index) ?? index;
      frozenStepsByIndex.set(stepIndex, record);
    }

    return {
      commandId: job.command_id,
      organizationId: job.organization_id,
      agentId: job.agent_id,
      intentId: job.intent_id,
      decisionId: job.decision_id === null ? null : String(job.decision_id),
      dedupeKey: job.dedupe_key,
      requestedBy: job.requested_by,
      requestedAt: job.requested_at.toISOString(),
      approvalState: 'approved',
      metadata: snapshot.metadata,
      steps: (stepsByJob.get(String(job.id)) ?? [])
        .sort((left, right) => left.step_index - right.step_index)
        .map((step) => ({
          ...(frozenStepsByIndex.get(step.step_index) || {}),
          stepIndex: step.step_index,
          command: step.command,
          status: step.status,
          evidence: step.evidence_jsonb,
        })),
      execution: {
        maxRetries: 0,
        timeoutSeconds: 300,
        stopOnFailure: true,
        ...snapshot.execution,
      },
    };
  });
}
