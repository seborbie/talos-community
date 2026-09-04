ALTER TABLE "public"."rmm_patch_action"
  ADD COLUMN IF NOT EXISTS "operation_id" TEXT,
  ADD COLUMN IF NOT EXISTS "phase" TEXT,
  ADD COLUMN IF NOT EXISTS "progress_jsonb" JSONB NOT NULL DEFAULT '{}',
  ADD COLUMN IF NOT EXISTS "evidence_jsonb" JSONB NOT NULL DEFAULT '{}',
  ADD COLUMN IF NOT EXISTS "error_message" TEXT,
  ADD COLUMN IF NOT EXISTS "started_at" TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS "finished_at" TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP;

UPDATE "public"."rmm_patch_action"
SET "operation_id" = COALESCE("operation_id", "id"),
    "updated_at" = COALESCE("updated_at", "created_at", CURRENT_TIMESTAMP);

ALTER TABLE "public"."rmm_patch_action"
  ALTER COLUMN "operation_id" SET NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_action_org_agent_operation_key"
  ON "public"."rmm_patch_action"("organization_id", "agent_id", "operation_id");

CREATE INDEX IF NOT EXISTS "rmm_patch_action_org_agent_status_updated_idx"
  ON "public"."rmm_patch_action"("organization_id", "agent_id", "status", "updated_at");

ALTER TABLE "public"."rmm_patch_action"
  DROP CONSTRAINT IF EXISTS "rmm_patch_action_type_check";

ALTER TABLE "public"."rmm_patch_action"
  ADD CONSTRAINT "rmm_patch_action_type_check"
  CHECK (
    "action_type" IN (
      'scan',
      'download',
      'install',
      'reboot',
      'control',
      'approval',
      'force_scan',
      'force_download',
      'force_install',
      'force_reboot',
      'defer',
      'defer_reboot',
      'block',
      'approve',
      'emergency_approve',
      'exclude_device',
      'maintenance_mode',
      'break_glass',
      'cancel'
    )
  );

ALTER TABLE "public"."rmm_patch_override"
  ADD COLUMN IF NOT EXISTS "operation_id" TEXT;

CREATE INDEX IF NOT EXISTS "rmm_patch_override_operation_id_idx"
  ON "public"."rmm_patch_override"("operation_id");
