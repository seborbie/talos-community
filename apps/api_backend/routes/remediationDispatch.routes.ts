import { Prisma } from '@prisma/client';
import { Router } from 'express';
import {
  buildRemediationDispatchJobs,
  type RemediationDispatchJobRow,
  type RemediationDispatchStepRow,
} from '../lib/remediationDispatch';
import {
  parseRemediationStatusReport,
  PATCH_INSTALL_INTENT_ID,
  transitionRemediationStatus,
} from '../lib/remediationStatusTransitions';
import { prisma } from '../lib/prisma';
import {
  attachRmmServerAuth,
  requireRmmServer,
  type RmmServerRequest,
} from '../middleware/rmmServerKey';

type JsonObject = Record<string, unknown>;

function asRecord(value: unknown): JsonObject | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as JsonObject;
}

function readString(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed || null;
}

function readLimit(value: unknown): number {
  const parsed = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(parsed)) return 1;
  return Math.min(10, Math.max(1, parsed));
}

export const remediationDispatchRouter = Router();
remediationDispatchRouter.use(attachRmmServerAuth);

remediationDispatchRouter.post(
  '/agents/:agentId/jobs/claim',
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = readString(req.params.agentId);
    if (!agentId) return res.status(400).json({ error: 'agentId is required' });
    const limit = readLimit(asRecord(req.body)?.limit);

    const { jobs, steps } = await prisma.$transaction(async (tx) => {
      const jobs = await tx.$queryRaw<RemediationDispatchJobRow[]>(Prisma.sql`
        WITH candidates AS (
          SELECT job.id
          FROM rmm_telemetry.remediation_job job
          WHERE job.status = 'queued'
            AND job.agent_id = ${agentId}
            AND job.intent_id <> ${PATCH_INSTALL_INTENT_ID}
            AND job.command_id IS NOT NULL
            AND EXISTS (
              SELECT 1
              FROM rmm_telemetry.remediation_step step
              WHERE step.job_id = job.id
            )
          ORDER BY job.requested_at ASC, job.id ASC
          LIMIT ${limit}
          FOR UPDATE SKIP LOCKED
        )
        UPDATE rmm_telemetry.remediation_job job
        SET status = 'running',
            started_at = COALESCE(job.started_at, NOW()),
            finished_at = NULL
        FROM candidates
        WHERE job.id = candidates.id
        RETURNING
          job.id,
          job.command_id,
          job.organization_id,
          job.agent_id,
          job.decision_id,
          job.intent_id,
          job.dedupe_key,
          job.requested_by,
          job.requested_at,
          job.metadata_jsonb
      `);

      const jobIds = jobs.map((job) => job.id);
      const steps =
        jobIds.length === 0
          ? []
          : await tx.$queryRaw<RemediationDispatchStepRow[]>(Prisma.sql`
            SELECT id, job_id, step_index, command, status, evidence_jsonb
            FROM rmm_telemetry.remediation_step
            WHERE job_id IN (${Prisma.join(jobIds)})
            ORDER BY job_id ASC, step_index ASC
          `);
      return { jobs, steps };
    });

    return res.json({ jobs: buildRemediationDispatchJobs(jobs, steps) });
  },
);

remediationDispatchRouter.patch(
  '/agents/:agentId/jobs/:commandId/status',
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = readString(req.params.agentId);
    const commandId = readString(req.params.commandId);
    if (!agentId) return res.status(400).json({ error: 'agentId is required' });
    if (!commandId) return res.status(400).json({ error: 'commandId is required' });

    const parsed = parseRemediationStatusReport(req.body);
    if (!parsed.ok) {
      return res.status(parsed.httpStatus).json({ error: parsed.error });
    }
    const result = await prisma.$transaction((tx) =>
      transitionRemediationStatus(
        tx,
        {
          commandId,
          agentId,
          intentScope: 'generic',
        },
        parsed.report,
      ),
    );

    if (result.outcome === 'not_found') {
      return res.status(404).json({ error: 'Remediation command not found' });
    }
    if (result.outcome === 'step_not_found') {
      return res.status(404).json({ error: 'Remediation step not found' });
    }
    if (result.outcome === 'conflict') {
      return res.status(409).json({
        error: `Cannot transition remediation command from ${result.currentStatus} to ${parsed.report.status}`,
      });
    }
    if (result.outcome === 'step_conflict') {
      return res.status(409).json({
        error: `Cannot transition remediation step ${result.stepIndex} from ${result.currentStatus} to ${parsed.report.status}`,
      });
    }
    if (result.outcome === 'invalid_evidence') {
      return res.status(400).json({ error: result.error });
    }
    return res.json({ updated: true, status: parsed.report.status });
  },
);
