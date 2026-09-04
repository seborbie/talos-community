ALTER TABLE "public"."rmm_devices"
  ADD COLUMN IF NOT EXISTS "ai_runner_auto_approve" BOOLEAN NOT NULL DEFAULT FALSE;
