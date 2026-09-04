CREATE SCHEMA IF NOT EXISTS command_center;

CREATE TABLE IF NOT EXISTS command_center.ai_runner_command_approvals (
  id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  conversation_id TEXT,
  agent_id TEXT NOT NULL,
  turn_index INTEGER NOT NULL,
  status TEXT NOT NULL,
  command TEXT NOT NULL,
  explanation TEXT NOT NULL,
  risk TEXT NOT NULL,
  notes_jsonb JSONB,
  message TEXT,
  model_response_id TEXT,
  policy_allowed BOOLEAN,
  policy_reason TEXT,
  matched_policy_id BIGINT,
  decided_by_user_id TEXT,
  decided_at TIMESTAMPTZ(3),
  output TEXT,
  output_length INTEGER,
  exit_code INTEGER,
  error TEXT,
  message_id TEXT,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  expires_at TIMESTAMPTZ(3),
  executed_at TIMESTAMPTZ(3),
  CONSTRAINT ai_runner_command_approvals_pkey PRIMARY KEY (id),
  CONSTRAINT command_center_ai_runner_command_approvals_status_check
    CHECK (
      status IN (
        'pending',
        'approved',
        'denied',
        'executing',
        'executed',
        'failed',
        'expired',
        'policy_blocked'
      )
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS command_center_ai_runner_command_approvals_job_turn_key
  ON command_center.ai_runner_command_approvals (job_id, turn_index);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_command_approvals_org_user_status_idx
  ON command_center.ai_runner_command_approvals (organization_id, user_id, status, updated_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_command_approvals_job_status_idx
  ON command_center.ai_runner_command_approvals (job_id, status, updated_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_command_approvals_message_idx
  ON command_center.ai_runner_command_approvals (message_id);

ALTER TABLE command_center.ai_runner_command_approvals
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_command_approvals_job_id_fkey,
  ADD CONSTRAINT command_center_ai_runner_command_approvals_job_id_fkey
    FOREIGN KEY (job_id)
    REFERENCES command_center.ai_runner_jobs(id)
    ON DELETE CASCADE
    ON UPDATE CASCADE;
