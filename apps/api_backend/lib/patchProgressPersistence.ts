import crypto from 'crypto';
import { Prisma } from '@prisma/client';
import {
  inferPatchProgressActionType,
  recordPatchActionResultInTransaction,
} from './patchDecisionService';
import {
  classifyPatchProgressTransition,
  PATCH_PROGRESS_TERMINAL_STATUSES,
  PatchProgressValidationError,
  type NormalizedPatchProgress,
} from './patchProgressProjection';

export async function projectPatchProgressBatch(
  transaction: Prisma.TransactionClient,
  progressItems: NormalizedPatchProgress[],
): Promise<{ updated: number; ignored: number }> {
  const agentIds = [...new Set(progressItems.map((item) => item.agentId))].sort();
  const deviceRows = await transaction.$queryRaw<
    Array<{ agentId: string; organizationId: string }>
  >(Prisma.sql`
    SELECT agent_id AS "agentId", organization_id AS "organizationId"
    FROM public.rmm_devices
    WHERE agent_id IN (${Prisma.join(agentIds)})
    ORDER BY agent_id ASC
    FOR SHARE
  `);
  const organizationByAgent = new Map(
    deviceRows.map((device) => [device.agentId, device.organizationId] as const),
  );
  const invalidScopeIndex = progressItems.findIndex(
    (item) => organizationByAgent.get(item.agentId) !== item.organizationId,
  );
  if (invalidScopeIndex >= 0) {
    throw new PatchProgressValidationError(
      'agentId does not belong to organizationId',
      400,
      invalidScopeIndex,
    );
  }

  let updated = 0;
  let ignored = 0;
  for (const progress of progressItems) {
    const existingActions = await transaction.$queryRaw<
      Array<{ actionType: string; status: string; reportedAt: Date | null }>
    >(Prisma.sql`
      SELECT
        action_type AS "actionType",
        status,
        reported_at AS "reportedAt"
      FROM public.rmm_patch_action
      WHERE organization_id = ${progress.organizationId}
        AND agent_id = ${progress.agentId}
        AND operation_id = ${progress.operationId}
      FOR UPDATE
    `);
    const existingAction = existingActions[0] ?? null;
    const transition = classifyPatchProgressTransition(existingAction, progress);
    if (transition !== 'apply') {
      ignored += 1;
      continue;
    }

    const actionType = inferPatchProgressActionType({
      eventType: progress.eventType,
      phase: progress.phase,
      status: progress.status,
      summary: progress.summary,
      existingActionType: existingAction?.actionType ?? null,
    });
    const finished = PATCH_PROGRESS_TERMINAL_STATUSES.has(progress.status);
    const changed = await transaction.$executeRaw(Prisma.sql`
      INSERT INTO public.rmm_patch_action
        (
          id, organization_id, agent_id, operation_id, action_type, status, phase,
          update_keys_jsonb, progress_jsonb, evidence_jsonb, error_message,
          requested_by, reported_at, started_at, finished_at, created_at, updated_at
        )
      VALUES
        (
          ${crypto.randomUUID()}, ${progress.organizationId}, ${progress.agentId},
          ${progress.operationId}, ${actionType}, ${progress.status}, ${progress.phase},
          ${JSON.stringify(progress.updateKeys)}::jsonb, ${progress.progressJson}::jsonb,
          ${progress.evidenceJson}::jsonb, ${progress.error}, ${'agent'}, ${progress.reportedAt},
          CASE WHEN ${progress.status} = 'running' THEN ${progress.reportedAt} ELSE NULL END,
          CASE WHEN ${finished} THEN ${progress.reportedAt} ELSE NULL END, NOW(), NOW()
        )
      ON CONFLICT (organization_id, agent_id, operation_id)
      DO UPDATE SET
        action_type = CASE
          WHEN public.rmm_patch_action.action_type IN ('scan', 'download', 'install', 'reboot')
            THEN public.rmm_patch_action.action_type
          ELSE EXCLUDED.action_type
        END,
        status = EXCLUDED.status,
        phase = EXCLUDED.phase,
        update_keys_jsonb = CASE
          WHEN jsonb_array_length(EXCLUDED.update_keys_jsonb) > 0 THEN EXCLUDED.update_keys_jsonb
          ELSE public.rmm_patch_action.update_keys_jsonb
        END,
        progress_jsonb = EXCLUDED.progress_jsonb,
        evidence_jsonb = EXCLUDED.evidence_jsonb,
        error_message = EXCLUDED.error_message,
        reported_at = EXCLUDED.reported_at,
        started_at = COALESCE(
          public.rmm_patch_action.started_at,
          EXCLUDED.started_at,
          EXCLUDED.reported_at
        ),
        finished_at = CASE
          WHEN ${finished} THEN COALESCE(public.rmm_patch_action.finished_at, EXCLUDED.reported_at)
          ELSE public.rmm_patch_action.finished_at
        END,
        updated_at = NOW()
      WHERE public.rmm_patch_action.status NOT IN ('completed', 'failed', 'cancelled')
        AND (
          public.rmm_patch_action.reported_at IS NULL
          OR public.rmm_patch_action.reported_at <= EXCLUDED.reported_at
        )
    `);
    if (changed === 0) {
      ignored += 1;
      continue;
    }

    updated += 1;
    if (!finished) continue;

    await transaction.$executeRaw(Prisma.sql`
      UPDATE public.rmm_patch_override
      SET enabled = false, updated_at = NOW()
      WHERE organization_id = ${progress.organizationId}
        AND scope_type = 'device'
        AND scope_key = ${progress.agentId}
        AND operation_id = ${progress.operationId}
        AND action IN ('force_scan', 'force_download', 'force_install', 'force_reboot')
        AND enabled = true
    `);
    if (
      (progress.status === 'completed' || progress.status === 'failed') &&
      (actionType === 'download' || actionType === 'install')
    ) {
      await recordPatchActionResultInTransaction(transaction, {
        organizationId: progress.organizationId,
        agentId: progress.agentId,
        operationId: progress.operationId,
        action: actionType,
        status: progress.status,
        updateKeys: progress.updateKeys,
        evidence: progress.evidence,
      });
    }
  }

  return { updated, ignored };
}
