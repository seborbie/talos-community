ALTER TABLE "rmm_telemetry"."remediation_job"
  ADD COLUMN IF NOT EXISTS "command_id" TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS "remediation_job_command_id_key"
  ON "rmm_telemetry"."remediation_job"("command_id")
  WHERE "command_id" IS NOT NULL;

ALTER TABLE "public"."rmm_patch_action"
  ADD COLUMN IF NOT EXISTS "remediation_command_id" TEXT;

CREATE INDEX IF NOT EXISTS "rmm_patch_action_remediation_command_id_idx"
  ON "public"."rmm_patch_action"("remediation_command_id");
