CREATE SCHEMA IF NOT EXISTS command_center;

ALTER TABLE command_center.ai_runner_jobs
  ADD COLUMN IF NOT EXISTS goal TEXT,
  ADD COLUMN IF NOT EXISTS dispatch_request_jsonb JSONB,
  ADD COLUMN IF NOT EXISTS lease_id TEXT,
  ADD COLUMN IF NOT EXISTS lease_owner_runner_id TEXT,
  ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS last_heartbeat_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS cancel_requested_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS resume_attempt INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS retryable BOOLEAN NOT NULL DEFAULT FALSE,
  ADD COLUMN IF NOT EXISTS retry_reason TEXT;

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_lease_expiry_idx
  ON command_center.ai_runner_jobs (lease_expires_at);

CREATE TABLE IF NOT EXISTS command_center.ai_runner_events (
  id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  conversation_id TEXT,
  agent_id TEXT NOT NULL,
  event_key TEXT NOT NULL,
  event_type TEXT NOT NULL,
  runner_id TEXT,
  lease_id TEXT,
  turn_index INTEGER,
  artifact_frame_id TEXT,
  command_approval_id TEXT,
  artifact_id TEXT,
  payload_jsonb JSONB,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT ai_runner_events_pkey PRIMARY KEY (id)
);

CREATE UNIQUE INDEX IF NOT EXISTS command_center_ai_runner_events_job_event_key
  ON command_center.ai_runner_events (job_id, event_key);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_events_job_created_idx
  ON command_center.ai_runner_events (job_id, created_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_events_org_user_created_idx
  ON command_center.ai_runner_events (organization_id, user_id, created_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_events_artifact_idx
  ON command_center.ai_runner_events (artifact_id);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_events_command_approval_idx
  ON command_center.ai_runner_events (command_approval_id);

ALTER TABLE command_center.ai_runner_events
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_events_job_id_fkey,
  ADD CONSTRAINT command_center_ai_runner_events_job_id_fkey
    FOREIGN KEY (job_id)
    REFERENCES command_center.ai_runner_jobs(id)
    ON DELETE CASCADE
    ON UPDATE CASCADE;

ALTER TABLE command_center.ai_runner_events
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_events_command_approval_id_fkey,
  ADD CONSTRAINT command_center_ai_runner_events_command_approval_id_fkey
    FOREIGN KEY (command_approval_id)
    REFERENCES command_center.ai_runner_command_approvals(id)
    ON DELETE SET NULL
    ON UPDATE CASCADE;
