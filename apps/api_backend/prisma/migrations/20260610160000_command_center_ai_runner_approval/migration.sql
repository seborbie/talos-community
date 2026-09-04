CREATE SCHEMA IF NOT EXISTS command_center;

ALTER TABLE command_center.ai_runner_jobs
  ADD COLUMN IF NOT EXISTS job_type TEXT NOT NULL DEFAULT 'desktop_goal',
  ADD COLUMN IF NOT EXISTS approval_id TEXT,
  ADD COLUMN IF NOT EXISTS approval_chat_session_id TEXT,
  ADD COLUMN IF NOT EXISTS approval_requested_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS approval_responded_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS approval_expires_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS approval_window_expires_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS result_message_id TEXT;

ALTER TABLE command_center.ai_runner_jobs
  ALTER COLUMN job_type SET DEFAULT 'desktop_goal';

ALTER TABLE command_center.ai_runner_jobs
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_jobs_status_check,
  ADD CONSTRAINT command_center_ai_runner_jobs_status_check
    CHECK (
      status IN (
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
      )
    );

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_active_idx
  ON command_center.ai_runner_jobs (organization_id, user_id, conversation_id, status, updated_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_approval_idx
  ON command_center.ai_runner_jobs (approval_id);

CREATE TABLE IF NOT EXISTS command_center.ai_runner_approval_grants (
  id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  job_type TEXT NOT NULL,
  approval_id TEXT NOT NULL,
  job_id TEXT,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at TIMESTAMPTZ(3) NOT NULL,
  CONSTRAINT ai_runner_approval_grants_pkey PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_approval_grants_lookup_idx
  ON command_center.ai_runner_approval_grants (organization_id, user_id, agent_id, job_type, expires_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_approval_grants_approval_idx
  ON command_center.ai_runner_approval_grants (approval_id);
