import { Prisma } from '@prisma/client';

export const PATCH_INSTALL_INTENT_ID = 'talos.patch.install';
export const MAX_REMEDIATION_EVIDENCE_BYTES = 32 * 1024;

const TERMINAL_STATUSES = new Set<RemediationTerminalStatus>(['completed', 'failed', 'cancelled']);
const REPORTABLE_STATUSES = new Set<RemediationReportableStatus>(['running', ...TERMINAL_STATUSES]);

type JsonObject = Record<string, unknown>;

export type RemediationTerminalStatus = 'completed' | 'failed' | 'cancelled';
export type RemediationReportableStatus = 'running' | RemediationTerminalStatus;

export type RemediationStatusReport = {
  status: RemediationReportableStatus;
  stepIndex: number;
  hasEvidence: boolean;
  evidence: unknown;
};

export type RemediationStatusSelector = {
  agentId: string;
  organizationId?: string;
  intentScope: 'generic' | 'patch';
} & ({ commandId: string; jobId?: never } | { jobId: bigint; commandId?: never });

type DurableJobState = {
  id: bigint;
  command_id: string | null;
  organization_id: string;
  agent_id: string;
  intent_id: string;
  status: string;
};

type DurableStepState = {
  step_index: number;
  command: string;
  status: string;
};

type TerminalStepProjection = {
  stepIndex: number;
  status: RemediationTerminalStatus;
  evidence: JsonObject;
};

export type RemediationTransitionResult =
  | { outcome: 'not_found' }
  | { outcome: 'step_not_found' }
  | { outcome: 'conflict'; currentStatus: string }
  | { outcome: 'step_conflict'; stepIndex: number; currentStatus: string }
  | { outcome: 'invalid_evidence'; error: string }
  | {
      outcome: 'updated';
      reportedStatus: RemediationReportableStatus;
      jobStatus: RemediationReportableStatus;
      jobId: bigint;
      commandId: string | null;
      organizationId: string;
      agentId: string;
    };

export type ParsedRemediationStatusReport =
  | { ok: true; report: RemediationStatusReport }
  | { ok: false; httpStatus: 400 | 413; error: string };

function asRecord(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as JsonObject;
}

function readString(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed || null;
}

function readStepIndex(value: unknown): number | null {
  if (value === undefined || value === null || value === '') return 0;
  const parsed = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(parsed) || parsed < 0 || parsed > 1000) return null;
  return parsed;
}

function readRequiredStepIndex(value: unknown): number | null {
  if (value === undefined || value === null || value === '') return null;
  return readStepIndex(value);
}

function encodedJsonBytes(value: unknown): number | null {
  try {
    return new TextEncoder().encode(JSON.stringify(value ?? null)).byteLength;
  } catch {
    return null;
  }
}

export function parseRemediationStatusReport(value: unknown): ParsedRemediationStatusReport {
  const body = asRecord(value) || {};
  const status = readString(body.status);
  if (!status || !REPORTABLE_STATUSES.has(status as RemediationReportableStatus)) {
    return {
      ok: false,
      httpStatus: 400,
      error: 'status must be running, completed, failed, or cancelled',
    };
  }

  const stepIndex = readStepIndex(body.stepIndex ?? body.step_index);
  if (stepIndex === null) {
    return {
      ok: false,
      httpStatus: 400,
      error: 'stepIndex must be an integer between 0 and 1000',
    };
  }

  const hasEvidence = Object.prototype.hasOwnProperty.call(body, 'evidence');
  const evidence = body.evidence ?? null;
  if (hasEvidence) {
    const encodedBytes = encodedJsonBytes(evidence);
    if (encodedBytes === null) {
      return { ok: false, httpStatus: 400, error: 'evidence must be JSON serializable' };
    }
    if (encodedBytes > MAX_REMEDIATION_EVIDENCE_BYTES) {
      return {
        ok: false,
        httpStatus: 413,
        error: `evidence must not exceed ${MAX_REMEDIATION_EVIDENCE_BYTES} encoded bytes`,
      };
    }
  }

  return {
    ok: true,
    report: {
      status: status as RemediationReportableStatus,
      stepIndex,
      hasEvidence,
      evidence,
    },
  };
}

export function isTerminalRemediationStatus(value: string): value is RemediationTerminalStatus {
  return TERMINAL_STATUSES.has(value as RemediationTerminalStatus);
}

function stepTransitionAllowed(
  currentStatus: string,
  nextStatus: RemediationReportableStatus,
): boolean {
  if (currentStatus === nextStatus) return true;
  if (currentStatus === 'pending') return true;
  return currentStatus === 'running' && isTerminalRemediationStatus(nextStatus);
}

function deriveJobStatus(stepStatuses: string[]): RemediationReportableStatus {
  if (!stepStatuses.every(isTerminalRemediationStatus)) return 'running';
  if (stepStatuses.includes('failed')) return 'failed';
  if (stepStatuses.includes('cancelled')) return 'cancelled';
  return 'completed';
}

function terminalStepProjection(
  evidence: unknown,
  reportedStatus: RemediationTerminalStatus,
  reportedStepIndex: number,
  durableSteps: DurableStepState[],
):
  | { kind: 'direct' }
  | { kind: 'invalid'; error: string }
  | { kind: 'aggregate'; steps: TerminalStepProjection[] } {
  const evidenceRecord = asRecord(evidence);
  if (!evidenceRecord || !Object.prototype.hasOwnProperty.call(evidenceRecord, 'steps')) {
    const evidenceStatus = readString(evidenceRecord?.status);
    if (evidenceStatus && evidenceStatus !== reportedStatus) {
      return {
        kind: 'invalid',
        error: 'terminal evidence status does not match the report status',
      };
    }
    const evidenceStepIndex = readRequiredStepIndex(
      evidenceRecord?.stepIndex ?? evidenceRecord?.step_index,
    );
    if (evidenceStepIndex !== null && evidenceStepIndex !== reportedStepIndex) {
      return {
        kind: 'invalid',
        error: 'terminal evidence stepIndex does not match the report stepIndex',
      };
    }
    return { kind: 'direct' };
  }
  if (!Array.isArray(evidenceRecord.steps)) {
    return { kind: 'invalid', error: 'terminal evidence.steps must be an array when present' };
  }

  const durableByIndex = new Map(durableSteps.map((step) => [step.step_index, step]));
  const reportedByIndex = new Map<number, TerminalStepProjection>();
  for (const value of evidenceRecord.steps) {
    const record = asRecord(value);
    if (!record) {
      return { kind: 'invalid', error: 'terminal evidence.steps entries must be objects' };
    }
    const stepIndex = readRequiredStepIndex(record.stepIndex ?? record.step_index);
    if (stepIndex === null) {
      return {
        kind: 'invalid',
        error: 'terminal evidence stepIndex must be an integer between 0 and 1000',
      };
    }
    const status = readString(record.status);
    if (!status || !isTerminalRemediationStatus(status)) {
      return {
        kind: 'invalid',
        error: 'terminal evidence step status must be completed, failed, or cancelled',
      };
    }
    if (!durableByIndex.has(stepIndex)) {
      return { kind: 'invalid', error: `terminal evidence references unknown step ${stepIndex}` };
    }
    if (reportedByIndex.has(stepIndex)) {
      return { kind: 'invalid', error: `terminal evidence contains duplicate step ${stepIndex}` };
    }
    const normalizedEvidence: JsonObject = { ...record, stepIndex, status };
    delete normalizedEvidence.step_index;
    reportedByIndex.set(stepIndex, { stepIndex, status, evidence: normalizedEvidence });
  }

  if (!reportedByIndex.has(reportedStepIndex)) {
    return {
      kind: 'invalid',
      error: `terminal evidence must include the reported step ${reportedStepIndex}`,
    };
  }

  const projectedSteps: TerminalStepProjection[] = [];
  for (const durableStep of durableSteps) {
    const reported = reportedByIndex.get(durableStep.step_index);
    if (reported) {
      if (!stepTransitionAllowed(durableStep.status, reported.status)) {
        return {
          kind: 'invalid',
          error: `cannot transition remediation step ${durableStep.step_index} from ${durableStep.status} to ${reported.status}`,
        };
      }
      projectedSteps.push(reported);
      continue;
    }

    if (reportedStatus === 'completed') {
      return {
        kind: 'invalid',
        error: `completed terminal evidence is missing durable step ${durableStep.step_index}`,
      };
    }
    if (!stepTransitionAllowed(durableStep.status, 'cancelled')) {
      return {
        kind: 'invalid',
        error: `terminal evidence is missing already-terminal step ${durableStep.step_index}`,
      };
    }
    projectedSteps.push({
      stepIndex: durableStep.step_index,
      status: 'cancelled',
      evidence: {
        stepIndex: durableStep.step_index,
        status: 'cancelled',
        reason: 'not_executed_after_terminal_outcome',
      },
    });
  }

  const projectedJobStatus = deriveJobStatus(projectedSteps.map((step) => step.status));
  if (projectedJobStatus !== reportedStatus) {
    return {
      kind: 'invalid',
      error: `terminal evidence resolves to ${projectedJobStatus}, not ${reportedStatus}`,
    };
  }
  return { kind: 'aggregate', steps: projectedSteps };
}

/**
 * Applies an agent-scoped remediation status report under row locks.
 *
 * Callers must run this function inside a Prisma transaction. A status report never creates a
 * job or step: the immutable command projection owns those records. This is important for Kafka
 * replay and for preventing a caller-supplied global command ID from re-parenting another job.
 */
export async function transitionRemediationStatus(
  tx: Prisma.TransactionClient,
  selector: RemediationStatusSelector,
  report: RemediationStatusReport,
): Promise<RemediationTransitionResult> {
  const identityPredicate =
    'commandId' in selector
      ? Prisma.sql`job.command_id = ${selector.commandId}`
      : Prisma.sql`job.id = ${selector.jobId}`;
  const organizationPredicate = selector.organizationId
    ? Prisma.sql`AND job.organization_id = ${selector.organizationId}`
    : Prisma.empty;
  const intentPredicate =
    selector.intentScope === 'patch'
      ? Prisma.sql`job.intent_id = ${PATCH_INSTALL_INTENT_ID}`
      : Prisma.sql`job.intent_id <> ${PATCH_INSTALL_INTENT_ID}`;

  const existingJobs = await tx.$queryRaw<DurableJobState[]>(Prisma.sql`
    SELECT
      job.id,
      job.command_id,
      job.organization_id,
      job.agent_id,
      job.intent_id,
      job.status
    FROM rmm_telemetry.remediation_job job
    WHERE ${identityPredicate}
      AND job.agent_id = ${selector.agentId}
      ${organizationPredicate}
      AND ${intentPredicate}
    FOR UPDATE
  `);
  const existingJob = existingJobs[0];
  if (!existingJob) return { outcome: 'not_found' };

  const transitionAllowed =
    existingJob.status === report.status || existingJob.status === 'running';
  if (!transitionAllowed) {
    return { outcome: 'conflict', currentStatus: existingJob.status };
  }

  const durableSteps = await tx.$queryRaw<DurableStepState[]>(Prisma.sql`
    SELECT step_index, command, status
    FROM rmm_telemetry.remediation_step
    WHERE job_id = ${existingJob.id}
      AND organization_id = ${existingJob.organization_id}
    ORDER BY step_index ASC
    FOR UPDATE
  `);
  const targetStep = durableSteps.find((step) => step.step_index === report.stepIndex);
  if (!targetStep) return { outcome: 'step_not_found' };
  if (!stepTransitionAllowed(targetStep.status, report.status)) {
    return {
      outcome: 'step_conflict',
      stepIndex: report.stepIndex,
      currentStatus: targetStep.status,
    };
  }

  const projection = isTerminalRemediationStatus(report.status)
    ? terminalStepProjection(report.evidence, report.status, report.stepIndex, durableSteps)
    : { kind: 'direct' as const };
  if (projection.kind === 'invalid') {
    return { outcome: 'invalid_evidence', error: projection.error };
  }

  const projectedStatuses = new Map(durableSteps.map((step) => [step.step_index, step.status]));
  if (projection.kind === 'aggregate') {
    for (const step of projection.steps) {
      const stepEvidenceJson = JSON.stringify(step.evidence);
      await tx.$executeRaw(Prisma.sql`
        UPDATE rmm_telemetry.remediation_step
        SET status = ${step.status},
            evidence_jsonb = ${stepEvidenceJson}::jsonb,
            started_at = COALESCE(started_at, NOW()),
            finished_at = COALESCE(finished_at, NOW())
        WHERE job_id = ${existingJob.id}
          AND organization_id = ${existingJob.organization_id}
          AND step_index = ${step.stepIndex}
      `);
      projectedStatuses.set(step.stepIndex, step.status);
    }
  } else {
    const finished = isTerminalRemediationStatus(report.status);
    const evidenceJson = JSON.stringify(report.evidence ?? null);
    const updatedSteps = report.hasEvidence
      ? await tx.$executeRaw(Prisma.sql`
          UPDATE rmm_telemetry.remediation_step
          SET status = ${report.status},
              evidence_jsonb = ${evidenceJson}::jsonb,
              started_at = COALESCE(started_at, NOW()),
              finished_at = CASE
                WHEN ${finished} THEN COALESCE(finished_at, NOW())
                ELSE finished_at
              END
          WHERE job_id = ${existingJob.id}
            AND organization_id = ${existingJob.organization_id}
            AND step_index = ${report.stepIndex}
        `)
      : await tx.$executeRaw(Prisma.sql`
          UPDATE rmm_telemetry.remediation_step
          SET status = ${report.status},
              started_at = COALESCE(started_at, NOW()),
              finished_at = CASE
                WHEN ${finished} THEN COALESCE(finished_at, NOW())
                ELSE finished_at
              END
          WHERE job_id = ${existingJob.id}
            AND organization_id = ${existingJob.organization_id}
            AND step_index = ${report.stepIndex}
        `);
    if (updatedSteps === 0) return { outcome: 'step_not_found' };
    projectedStatuses.set(report.stepIndex, report.status);
  }

  const jobStatus = deriveJobStatus([...projectedStatuses.values()]);
  const jobFinished = isTerminalRemediationStatus(jobStatus);
  await tx.$executeRaw(Prisma.sql`
    UPDATE rmm_telemetry.remediation_job
    SET status = ${jobStatus},
        started_at = COALESCE(started_at, NOW()),
        finished_at = CASE
          WHEN ${jobFinished} THEN COALESCE(finished_at, NOW())
          ELSE finished_at
        END
    WHERE id = ${existingJob.id}
      AND organization_id = ${existingJob.organization_id}
      AND agent_id = ${existingJob.agent_id}
  `);

  return {
    outcome: 'updated',
    reportedStatus: report.status,
    jobStatus,
    jobId: existingJob.id,
    commandId: existingJob.command_id,
    organizationId: existingJob.organization_id,
    agentId: existingJob.agent_id,
  };
}
