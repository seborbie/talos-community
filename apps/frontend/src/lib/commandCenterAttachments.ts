import type {
  CommandCenterAiRunnerEvidence,
  CommandCenterAiRunnerJobStatus,
  CommandCenterCommandApproval,
  CommandCenterCommandApprovalStatus,
  CommandCenterMessageAttachment
} from './types';

const finiteNumber = (value: unknown): number | undefined => {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
};

const stringValue = (value: unknown): string | undefined =>
  typeof value === 'string' && value.trim() ? value.trim() : undefined;

function parseCursor(value: unknown): CommandCenterMessageAttachment['cursor'] | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const record = value as Record<string, unknown>;
  const width = finiteNumber(record.width);
  const height = finiteNumber(record.height);
  if (width === undefined || height === undefined) return undefined;
  const cursor: NonNullable<CommandCenterMessageAttachment['cursor']> = {
    visible: record.visible === true,
    width,
    height
  };
  const x = finiteNumber(record.x);
  const y = finiteNumber(record.y);
  if (x !== undefined) cursor.x = x;
  if (y !== undefined) cursor.y = y;
  return cursor;
}

export function commandCenterMessageAttachments(metadata: unknown): CommandCenterMessageAttachment[] {
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) return [];
  const raw = (metadata as { attachments?: unknown }).attachments;
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object' || Array.isArray(item)) return null;
      const record = item as Record<string, unknown>;
      if (
        record.type !== 'image' ||
        typeof record.artifactId !== 'string' ||
        typeof record.mimeType !== 'string' ||
        typeof record.name !== 'string'
      ) {
        return null;
      }
      return {
        id: stringValue(record.id) ?? record.artifactId,
        type: 'image' as const,
        artifactId: record.artifactId,
        mimeType: record.mimeType,
        name: record.name,
        width: finiteNumber(record.width),
        height: finiteNumber(record.height),
        presentation: record.presentation === 'live_frame' ? 'live_frame' : 'inline',
        jobId: stringValue(record.jobId),
        frameSeq: finiteNumber(record.frameSeq),
        cursor: parseCursor(record.cursor)
      };
    })
    .filter(Boolean) as CommandCenterMessageAttachment[];
}

const aiRunnerJobStatuses = new Set<CommandCenterAiRunnerJobStatus>([
  'queued',
  'approval_pending',
  'approval_granted',
  'approval_denied',
  'approval_expired',
  'running',
  'succeeded',
  'failed',
  'stopping',
  'stopped'
]);

export function commandCenterMessageAiRunnerEvidence(metadata: unknown): CommandCenterAiRunnerEvidence | null {
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) return null;
  const raw = (metadata as { aiRunnerJob?: unknown }).aiRunnerJob;
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
  const record = raw as Record<string, unknown>;
  const jobId = stringValue(record.jobId);
  const jobType = stringValue(record.jobType);
  const status =
    typeof record.status === 'string' && aiRunnerJobStatuses.has(record.status as CommandCenterAiRunnerJobStatus)
      ? (record.status as CommandCenterAiRunnerJobStatus)
      : null;
  if (!jobId || !jobType || !status) return null;
  const shellTranscriptAvailable = record.shellTranscriptAvailable === true;
  const desktopReplayAvailable = record.desktopReplayAvailable === true;
  const replayFrameCount = finiteNumber(record.replayFrameCount) ?? 0;
  if (!shellTranscriptAvailable && !desktopReplayAvailable) return null;
  return {
    jobId,
    jobType,
    status,
    shellTranscriptAvailable,
    desktopReplayAvailable,
    replayFrameCount
  };
}

const commandApprovalStatuses = new Set<CommandCenterCommandApprovalStatus>([
  'pending',
  'approved',
  'denied',
  'desktop_control_requested',
  'executing',
  'executed',
  'failed',
  'expired',
  'policy_blocked'
]);

export function commandCenterMessageCommandApproval(metadata: unknown): CommandCenterCommandApproval | null {
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) return null;
  const raw = (metadata as { commandApproval?: unknown }).commandApproval;
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
  const record = raw as Record<string, unknown>;
  const id = stringValue(record.id);
  const jobId = stringValue(record.jobId);
  const command = stringValue(record.command);
  const explanation = stringValue(record.explanation);
  const risk = stringValue(record.risk);
  const status = typeof record.status === 'string' && commandApprovalStatuses.has(record.status as CommandCenterCommandApprovalStatus)
    ? (record.status as CommandCenterCommandApprovalStatus)
    : null;
  if (!id || !jobId || !command || !explanation || !risk || !status) return null;
  return {
    id,
    jobId,
    turnIndex: finiteNumber(record.turnIndex) ?? 0,
    status,
    command,
    explanation,
    risk,
    notes: Array.isArray(record.notes) ? record.notes.filter((item): item is string => typeof item === 'string') : [],
    message: stringValue(record.message) ?? null,
    policyAllowed: typeof record.policyAllowed === 'boolean' ? record.policyAllowed : null,
    policyReason: stringValue(record.policyReason) ?? null,
    output: stringValue(record.output) ?? null,
    outputLength: finiteNumber(record.outputLength) ?? null,
    exitCode: finiteNumber(record.exitCode) ?? null,
    error: stringValue(record.error) ?? null,
    updatedAt: stringValue(record.updatedAt) ?? new Date().toISOString()
  };
}
