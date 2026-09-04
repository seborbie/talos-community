ALTER TABLE "public"."rmm_devices"
  ADD COLUMN IF NOT EXISTS "device_type" TEXT NOT NULL DEFAULT 'unknown',
  ADD COLUMN IF NOT EXISTS "device_type_source" TEXT NOT NULL DEFAULT 'auto',
  ADD COLUMN IF NOT EXISTS "patch_ring" TEXT NOT NULL DEFAULT 'broad',
  ADD COLUMN IF NOT EXISTS "criticality_tier" TEXT,
  ADD COLUMN IF NOT EXISTS "patch_managed" BOOLEAN NOT NULL DEFAULT true,
  ADD COLUMN IF NOT EXISTS "native_windows_update_control" BOOLEAN NOT NULL DEFAULT true,
  ADD COLUMN IF NOT EXISTS "patch_maintenance_mode_until" TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS "patch_tags" JSONB NOT NULL DEFAULT '[]',
  ADD COLUMN IF NOT EXISTS "native_windows_update_backup_jsonb" JSONB;

ALTER TABLE "public"."rmm_devices"
  DROP CONSTRAINT IF EXISTS "rmm_devices_device_type_check",
  ADD CONSTRAINT "rmm_devices_device_type_check"
    CHECK ("device_type" IN ('server', 'workstation', 'laptop', 'unknown')),
  DROP CONSTRAINT IF EXISTS "rmm_devices_device_type_source_check",
  ADD CONSTRAINT "rmm_devices_device_type_source_check"
    CHECK ("device_type_source" IN ('auto', 'manual')),
  DROP CONSTRAINT IF EXISTS "rmm_devices_patch_ring_check",
  ADD CONSTRAINT "rmm_devices_patch_ring_check"
    CHECK ("patch_ring" IN ('pilot', 'early', 'broad', 'critical_servers', 'excluded'));

CREATE INDEX IF NOT EXISTS "rmm_devices_device_type_idx" ON "public"."rmm_devices"("device_type");
CREATE INDEX IF NOT EXISTS "rmm_devices_patch_ring_idx" ON "public"."rmm_devices"("patch_ring");
CREATE INDEX IF NOT EXISTS "rmm_devices_patch_managed_idx" ON "public"."rmm_devices"("patch_managed");

ALTER TABLE "public"."rmm_patch_policy"
  ADD COLUMN IF NOT EXISTS "managed_mode" BOOLEAN NOT NULL DEFAULT true,
  ADD COLUMN IF NOT EXISTS "native_windows_update_control" BOOLEAN NOT NULL DEFAULT true,
  ADD COLUMN IF NOT EXISTS "policy_config_jsonb" JSONB NOT NULL DEFAULT '{}';

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_update_catalog" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "update_key" TEXT NOT NULL,
  "title" TEXT NOT NULL,
  "title_norm" TEXT NOT NULL,
  "kb_article" TEXT,
  "category" TEXT NOT NULL DEFAULT 'other',
  "update_type" TEXT,
  "wua_identity" TEXT,
  "revision_number" INTEGER,
  "release_date" TIMESTAMPTZ(3),
  "release_date_source" TEXT,
  "superseded_by_jsonb" JSONB NOT NULL DEFAULT '[]',
  "supersedes_jsonb" JSONB NOT NULL DEFAULT '[]',
  "source" TEXT NOT NULL DEFAULT 'wua',
  "first_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "last_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_update_catalog_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_update_catalog_org_update_key_key"
  ON "public"."rmm_patch_update_catalog"("organization_id", "update_key");
CREATE INDEX IF NOT EXISTS "rmm_patch_update_catalog_org_category_idx"
  ON "public"."rmm_patch_update_catalog"("organization_id", "category");
CREATE INDEX IF NOT EXISTS "rmm_patch_update_catalog_org_kb_idx"
  ON "public"."rmm_patch_update_catalog"("organization_id", "kb_article");
CREATE INDEX IF NOT EXISTS "rmm_patch_update_catalog_org_release_idx"
  ON "public"."rmm_patch_update_catalog"("organization_id", "release_date");

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_device_update_state" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "update_key" TEXT NOT NULL,
  "catalog_id" TEXT,
  "title" TEXT NOT NULL,
  "title_norm" TEXT NOT NULL,
  "kb_article" TEXT,
  "category" TEXT NOT NULL DEFAULT 'other',
  "applicability_state" TEXT NOT NULL DEFAULT 'applicable',
  "approval_state" TEXT NOT NULL DEFAULT 'detected',
  "lifecycle_state" TEXT NOT NULL DEFAULT 'detected',
  "first_detected_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "last_detected_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "release_date" TIMESTAMPTZ(3),
  "eligible_at" TIMESTAMPTZ(3),
  "install_deadline_at" TIMESTAMPTZ(3),
  "reboot_deadline_at" TIMESTAMPTZ(3),
  "downloaded_at" TIMESTAMPTZ(3),
  "installed_at" TIMESTAMPTZ(3),
  "failed_at" TIMESTAMPTZ(3),
  "failure_code" TEXT,
  "failure_hresult" INTEGER,
  "failure_message" TEXT,
  "requires_reboot" BOOLEAN,
  "wua_identity" TEXT,
  "revision_number" INTEGER,
  "metadata_jsonb" JSONB NOT NULL DEFAULT '{}',
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_device_update_state_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_device_update_state_org_agent_update_key_key"
  ON "public"."rmm_patch_device_update_state"("organization_id", "agent_id", "update_key");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_update_state_org_lifecycle_idx"
  ON "public"."rmm_patch_device_update_state"("organization_id", "lifecycle_state");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_update_state_org_approval_idx"
  ON "public"."rmm_patch_device_update_state"("organization_id", "approval_state");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_update_state_agent_idx"
  ON "public"."rmm_patch_device_update_state"("agent_id");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_update_state_update_key_idx"
  ON "public"."rmm_patch_device_update_state"("update_key");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_update_state_install_deadline_idx"
  ON "public"."rmm_patch_device_update_state"("install_deadline_at");

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_override" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "scope_type" TEXT NOT NULL,
  "scope_key" TEXT NOT NULL,
  "action" TEXT NOT NULL,
  "update_key" TEXT,
  "kb_article" TEXT,
  "category" TEXT,
  "reason" TEXT,
  "defer_until" TIMESTAMPTZ(3),
  "expires_at" TIMESTAMPTZ(3),
  "enabled" BOOLEAN NOT NULL DEFAULT true,
  "created_by" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_override_pkey" PRIMARY KEY ("id")
);

ALTER TABLE "public"."rmm_patch_override"
  DROP CONSTRAINT IF EXISTS "rmm_patch_override_scope_type_check",
  ADD CONSTRAINT "rmm_patch_override_scope_type_check"
    CHECK ("scope_type" IN ('global', 'organization', 'customer', 'site', 'group', 'tag', 'ring', 'device')),
  DROP CONSTRAINT IF EXISTS "rmm_patch_override_action_check",
  ADD CONSTRAINT "rmm_patch_override_action_check"
    CHECK ("action" IN (
      'approve', 'block', 'defer', 'force_install', 'force_scan', 'force_download',
      'force_reboot', 'defer_reboot', 'exclude_device', 'maintenance_mode',
      'break_glass', 'emergency_approve', 'cancel'
    ));

CREATE INDEX IF NOT EXISTS "rmm_patch_override_org_scope_idx"
  ON "public"."rmm_patch_override"("organization_id", "scope_type", "scope_key");
CREATE INDEX IF NOT EXISTS "rmm_patch_override_org_action_idx"
  ON "public"."rmm_patch_override"("organization_id", "action");
CREATE INDEX IF NOT EXISTS "rmm_patch_override_org_update_key_idx"
  ON "public"."rmm_patch_override"("organization_id", "update_key");
CREATE INDEX IF NOT EXISTS "rmm_patch_override_expires_at_idx"
  ON "public"."rmm_patch_override"("expires_at");

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_device_group" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "description" TEXT,
  "created_by" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_device_group_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_device_group_org_name_key"
  ON "public"."rmm_patch_device_group"("organization_id", "name");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_group_org_idx"
  ON "public"."rmm_patch_device_group"("organization_id");

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_device_group_member" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "group_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_device_group_member_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_patch_device_group_member_group_agent_key"
  ON "public"."rmm_patch_device_group_member"("group_id", "agent_id");
CREATE INDEX IF NOT EXISTS "rmm_patch_device_group_member_org_agent_idx"
  ON "public"."rmm_patch_device_group_member"("organization_id", "agent_id");

CREATE TABLE IF NOT EXISTS "public"."rmm_patch_decision_log" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "policy_id" TEXT,
  "operation_id" TEXT NOT NULL,
  "action" TEXT NOT NULL,
  "update_keys_jsonb" JSONB NOT NULL DEFAULT '[]',
  "decision" TEXT NOT NULL,
  "reason" TEXT NOT NULL,
  "details_jsonb" JSONB NOT NULL DEFAULT '{}',
  "decided_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_patch_decision_log_pkey" PRIMARY KEY ("id")
);

CREATE INDEX IF NOT EXISTS "rmm_patch_decision_log_org_decided_idx"
  ON "public"."rmm_patch_decision_log"("organization_id", "decided_at");
CREATE INDEX IF NOT EXISTS "rmm_patch_decision_log_agent_decided_idx"
  ON "public"."rmm_patch_decision_log"("agent_id", "decided_at");
CREATE INDEX IF NOT EXISTS "rmm_patch_decision_log_operation_idx"
  ON "public"."rmm_patch_decision_log"("operation_id");

ALTER TABLE "public"."rmm_patch_update_catalog"
  DROP CONSTRAINT IF EXISTS "rmm_patch_update_catalog_organization_id_fkey",
  ADD CONSTRAINT "rmm_patch_update_catalog_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_device_update_state"
  DROP CONSTRAINT IF EXISTS "rmm_patch_device_update_state_agent_id_fkey",
  ADD CONSTRAINT "rmm_patch_device_update_state_agent_id_fkey"
    FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE,
  DROP CONSTRAINT IF EXISTS "rmm_patch_device_update_state_catalog_id_fkey",
  ADD CONSTRAINT "rmm_patch_device_update_state_catalog_id_fkey"
    FOREIGN KEY ("catalog_id") REFERENCES "public"."rmm_patch_update_catalog"("id") ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_override"
  DROP CONSTRAINT IF EXISTS "rmm_patch_override_organization_id_fkey",
  ADD CONSTRAINT "rmm_patch_override_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_device_group"
  DROP CONSTRAINT IF EXISTS "rmm_patch_device_group_organization_id_fkey",
  ADD CONSTRAINT "rmm_patch_device_group_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_device_group_member"
  DROP CONSTRAINT IF EXISTS "rmm_patch_device_group_member_group_id_fkey",
  ADD CONSTRAINT "rmm_patch_device_group_member_group_id_fkey"
    FOREIGN KEY ("group_id") REFERENCES "public"."rmm_patch_device_group"("id") ON DELETE CASCADE ON UPDATE CASCADE;
