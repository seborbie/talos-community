ALTER TABLE "public"."rmm_patch_policy"
  ADD COLUMN IF NOT EXISTS "priority" INTEGER NOT NULL DEFAULT 100;

ALTER TABLE "public"."rmm_patch_policy"
  DROP CONSTRAINT IF EXISTS "rmm_patch_policy_priority_check",
  ADD CONSTRAINT "rmm_patch_policy_priority_check"
    CHECK ("priority" >= 0 AND "priority" <= 10000);

DROP INDEX IF EXISTS "public"."rmm_patch_policy_organization_id_scope_type_scope_key_key";

CREATE INDEX IF NOT EXISTS "rmm_patch_policy_organization_id_scope_type_scope_key_idx"
  ON "public"."rmm_patch_policy"("organization_id", "scope_type", "scope_key");

CREATE INDEX IF NOT EXISTS "rmm_patch_policy_organization_id_priority_idx"
  ON "public"."rmm_patch_policy"("organization_id", "priority");

UPDATE "public"."rmm_patch_policy"
SET
  "scope_type" = 'organization',
  "scope_key" = '__talos_default_patch_policy__',
  "customer_id" = NULL,
  "site_id" = NULL,
  "agent_id" = NULL,
  "name" = 'Default patch policy',
  "approval_mode" = 'auto_approve_all',
  "maintenance_window_start" = NULL,
  "maintenance_window_end" = NULL,
  "maintenance_window_timezone" = 'UTC',
  "reboot_behavior" = 'allow',
  "deferral_days" = 0,
  "managed_mode" = true,
  "native_windows_update_control" = true,
  "policy_config_jsonb" = '{
    "managedMode": true,
    "nativeWindowsUpdateControl": true,
    "checkInIntervalMinutes": 60,
    "windows": {
      "scan": { "enabled": true, "start": null, "end": null, "timezone": "UTC" },
      "download": { "enabled": true, "start": null, "end": null, "timezone": "UTC" },
      "install": { "enabled": true, "start": null, "end": null, "timezone": "UTC" },
      "reboot": { "enabled": true, "start": null, "end": null, "timezone": "UTC" }
    },
    "categories": {
      "security": { "approval": "auto", "installAfterDays": 0, "forceInstallByDays": 21, "forceRebootByDays": 24 },
      "critical": { "approval": "auto", "installAfterDays": 0, "forceInstallByDays": 14, "forceRebootByDays": 17 },
      "cumulative": { "approval": "auto", "installAfterDays": 0, "forceInstallByDays": 21, "forceRebootByDays": 24 },
      "definition": { "approval": "auto", "installAfterDays": 0, "forceInstallByDays": 2, "forceRebootByDays": 3 },
      "microsoft_product": { "approval": "auto", "installAfterDays": 0, "forceInstallByDays": 21, "forceRebootByDays": 24 },
      "uwp_app": { "approval": "manual", "installAfterDays": 0, "forceInstallByDays": null, "forceRebootByDays": null },
      "feature": { "approval": "manual", "installAfterDays": 30, "forceInstallByDays": 90, "forceRebootByDays": 97 },
      "driver": { "approval": "manual", "installAfterDays": 14, "forceInstallByDays": null, "forceRebootByDays": null },
      "firmware": { "approval": "manual", "installAfterDays": 30, "forceInstallByDays": null, "forceRebootByDays": null },
      "optional": { "approval": "manual", "installAfterDays": 14, "forceInstallByDays": null, "forceRebootByDays": null },
      "preview": { "approval": "blocked", "installAfterDays": 365, "forceInstallByDays": null, "forceRebootByDays": null },
      "other": { "approval": "manual", "installAfterDays": 0, "forceInstallByDays": null, "forceRebootByDays": null }
    },
    "reboot": {
      "allowAutomaticReboot": true,
      "forceRebootAfterDeadline": true,
      "warningMinutes": 60,
      "maxUserDeferrals": 3,
      "activeHoursProtection": true,
      "serverBehavior": "window_only",
      "workstationBehavior": "window_or_deadline"
    }
  }'::jsonb,
  "priority" = 10000,
  "enabled" = true,
  "is_default" = true,
  "updated_at" = NOW()
WHERE "is_default" = true;
