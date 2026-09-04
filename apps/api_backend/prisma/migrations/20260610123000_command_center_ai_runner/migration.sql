CREATE SCHEMA IF NOT EXISTS command_center;

CREATE TABLE IF NOT EXISTS command_center.ai_runner_jobs (
  id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  conversation_id TEXT,
  agent_id TEXT NOT NULL,
  status TEXT NOT NULL,
  runner_id TEXT,
  result_jsonb JSONB,
  error TEXT,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  started_at TIMESTAMPTZ(3),
  finished_at TIMESTAMPTZ(3),
  CONSTRAINT ai_runner_jobs_pkey PRIMARY KEY (id),
  CONSTRAINT command_center_ai_runner_jobs_status_check
    CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'stopping', 'stopped'))
);

CREATE TABLE IF NOT EXISTS command_center.ai_runner_artifacts (
  id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  artifact_type TEXT NOT NULL,
  name TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  content_base64 TEXT NOT NULL,
  metadata_jsonb JSONB,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT ai_runner_artifacts_pkey PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_org_user_updated_idx
  ON command_center.ai_runner_jobs (organization_id, user_id, updated_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_org_agent_updated_idx
  ON command_center.ai_runner_jobs (organization_id, agent_id, updated_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_jobs_conversation_idx
  ON command_center.ai_runner_jobs (conversation_id);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_artifacts_org_user_created_idx
  ON command_center.ai_runner_artifacts (organization_id, user_id, created_at);

CREATE INDEX IF NOT EXISTS command_center_ai_runner_artifacts_job_created_idx
  ON command_center.ai_runner_artifacts (job_id, created_at);

ALTER TABLE command_center.ai_runner_artifacts
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_artifacts_job_id_fkey,
  ADD CONSTRAINT command_center_ai_runner_artifacts_job_id_fkey
    FOREIGN KEY (job_id)
    REFERENCES command_center.ai_runner_jobs(id)
    ON DELETE CASCADE
    ON UPDATE CASCADE;
