ALTER TABLE "public"."rmm_patch_policy"
  ADD COLUMN IF NOT EXISTS "is_default" BOOLEAN NOT NULL DEFAULT false;

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_policy_one_default_per_org_idx"
  ON "public"."rmm_patch_policy"("organization_id")
  WHERE "is_default" = true;

INSERT INTO "public"."rmm_patch_policy"
  (
    "id", "organization_id", "scope_type", "scope_key", "customer_id", "site_id", "agent_id",
    "name", "approval_mode", "maintenance_window_start", "maintenance_window_end",
    "maintenance_window_timezone", "reboot_behavior", "deferral_days", "enabled",
    "is_default", "created_by", "created_at", "updated_at"
  )
SELECT
  'patch-default:' || o."id",
  o."id",
  'organization',
  '__talos_default_patch_policy__',
  NULL,
  NULL,
  NULL,
  'Default patch policy',
  'auto_approve_all',
  NULL,
  NULL,
  'UTC',
  'allow',
  0,
  true,
  true,
  'system',
  NOW(),
  NOW()
FROM "public"."Organization" o
ON CONFLICT ("organization_id", "scope_type", "scope_key")
DO UPDATE SET
  "name" = EXCLUDED."name",
  "approval_mode" = EXCLUDED."approval_mode",
  "maintenance_window_start" = EXCLUDED."maintenance_window_start",
  "maintenance_window_end" = EXCLUDED."maintenance_window_end",
  "maintenance_window_timezone" = EXCLUDED."maintenance_window_timezone",
  "reboot_behavior" = EXCLUDED."reboot_behavior",
  "deferral_days" = EXCLUDED."deferral_days",
  "enabled" = true,
  "is_default" = true,
  "updated_at" = NOW();
