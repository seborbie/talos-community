ALTER TABLE command_center.ai_runner_jobs
  ADD COLUMN IF NOT EXISTS live_frame_message_id TEXT;
