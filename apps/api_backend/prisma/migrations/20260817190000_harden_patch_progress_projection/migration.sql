ALTER TABLE "public"."rmm_patch_action"
  ADD COLUMN IF NOT EXISTS "reported_at" TIMESTAMPTZ(3);

COMMENT ON COLUMN "public"."rmm_patch_action"."reported_at" IS
  'Agent-reported RFC3339 event time used to reject stale patch progress projections.';
