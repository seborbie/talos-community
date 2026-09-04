CREATE TABLE IF NOT EXISTS "public"."feature_upgrade_setup_command_matrix" (
  "id" TEXT NOT NULL,
  "iso_media_id" TEXT NOT NULL,
  "os_family" TEXT NOT NULL DEFAULT 'windows',
  "product" TEXT NOT NULL,
  "version" TEXT NOT NULL,
  "edition" TEXT,
  "architecture" TEXT NOT NULL,
  "language" TEXT,
  "setup_executable" TEXT NOT NULL DEFAULT '{mount_drive}\setup.exe',
  "arguments_jsonb" JSONB NOT NULL DEFAULT '[]',
  "dynamic_update_mode" TEXT NOT NULL DEFAULT 'disable',
  "requires_eula_accept" BOOLEAN NOT NULL DEFAULT false,
  "image_index_strategy" TEXT NOT NULL DEFAULT 'auto_match_current_edition',
  "supported" BOOLEAN NOT NULL DEFAULT true,
  "notes" TEXT,
  "active" BOOLEAN NOT NULL DEFAULT true,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  CONSTRAINT "feature_upgrade_setup_command_matrix_pkey" PRIMARY KEY ("id"),
  CONSTRAINT "feature_upgrade_setup_command_matrix_dynamic_update_mode_check"
    CHECK ("dynamic_update_mode" IN ('enable', 'disable')),
  CONSTRAINT "feature_upgrade_setup_command_matrix_image_index_strategy_check"
    CHECK ("image_index_strategy" IN ('auto_match_current_edition', 'none'))
);

CREATE UNIQUE INDEX IF NOT EXISTS "feature_upgrade_setup_command_matrix_iso_media_id_key"
  ON "public"."feature_upgrade_setup_command_matrix"("iso_media_id");

CREATE INDEX IF NOT EXISTS "feature_upgrade_setup_command_matrix_active_supported_idx"
  ON "public"."feature_upgrade_setup_command_matrix"("active", "supported");

CREATE INDEX IF NOT EXISTS "feature_upgrade_setup_command_matrix_product_version_idx"
  ON "public"."feature_upgrade_setup_command_matrix"("product", "version");

ALTER TABLE "public"."feature_upgrade_setup_command_matrix"
  DROP CONSTRAINT IF EXISTS "feature_upgrade_setup_command_matrix_iso_media_id_fkey",
  ADD CONSTRAINT "feature_upgrade_setup_command_matrix_iso_media_id_fkey"
    FOREIGN KEY ("iso_media_id") REFERENCES "public"."feature_upgrade_iso_media"("id") ON DELETE CASCADE ON UPDATE CASCADE;

CREATE TABLE IF NOT EXISTS "public"."feature_upgrade_run" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "requested_by" TEXT NOT NULL,
  "status" TEXT NOT NULL DEFAULT 'queued',
  "scheduled_for" TIMESTAMPTZ(3),
  "total_devices" INTEGER NOT NULL DEFAULT 0,
  "scheduled_devices" INTEGER NOT NULL DEFAULT 0,
  "queued_devices" INTEGER NOT NULL DEFAULT 0,
  "running_devices" INTEGER NOT NULL DEFAULT 0,
  "awaiting_devices" INTEGER NOT NULL DEFAULT 0,
  "verifying_devices" INTEGER NOT NULL DEFAULT 0,
  "succeeded_devices" INTEGER NOT NULL DEFAULT 0,
  "failed_devices" INTEGER NOT NULL DEFAULT 0,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "finished_at" TIMESTAMPTZ(3),
  CONSTRAINT "feature_upgrade_run_pkey" PRIMARY KEY ("id"),
  CONSTRAINT "feature_upgrade_run_status_check"
    CHECK ("status" IN ('scheduled', 'queued', 'running', 'completed', 'failed', 'cancelled'))
);

CREATE TABLE IF NOT EXISTS "public"."feature_upgrade_device" (
  "operation_id" TEXT NOT NULL,
  "run_id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "preflight_operation_id" TEXT NOT NULL,
  "iso_media_id" TEXT NOT NULL,
  "setup_command_matrix_id" TEXT NOT NULL,
  "source_os" TEXT NOT NULL,
  "target_product" TEXT NOT NULL,
  "target_version" TEXT NOT NULL,
  "target_build_label" TEXT NOT NULL,
  "status" TEXT NOT NULL DEFAULT 'queued',
  "phase" TEXT NOT NULL DEFAULT 'queued',
  "progress_jsonb" JSONB NOT NULL DEFAULT '{}',
  "evidence_jsonb" JSONB NOT NULL DEFAULT '{}',
  "failure_summary_jsonb" JSONB NOT NULL DEFAULT '[]',
  "error_message" TEXT,
  "size_bytes" BIGINT,
  "sha256" TEXT,
  "scheduled_for" TIMESTAMPTZ(3),
  "requested_by" TEXT NOT NULL,
  "claimed_at" TIMESTAMPTZ(3),
  "started_at" TIMESTAMPTZ(3),
  "final_snapshot_at" TIMESTAMPTZ(3),
  "setup_started_at" TIMESTAMPTZ(3),
  "reboot_detected_at" TIMESTAMPTZ(3),
  "verified_at" TIMESTAMPTZ(3),
  "finished_at" TIMESTAMPTZ(3),
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  CONSTRAINT "feature_upgrade_device_pkey" PRIMARY KEY ("operation_id"),
  CONSTRAINT "feature_upgrade_device_status_check"
    CHECK ("status" IN ('scheduled', 'queued', 'running', 'awaiting_reboot', 'verifying', 'succeeded', 'failed', 'cancelled')),
  CONSTRAINT "feature_upgrade_device_phase_check"
    CHECK ("phase" IN ('scheduled', 'queued', 'final_checks', 'resolving_iso', 'downloading_iso', 'verifying_iso', 'mounting_iso', 'launching_setup', 'setup_running', 'awaiting_reboot', 'post_reboot_verifying', 'completed', 'failed', 'cancelled'))
);

CREATE INDEX IF NOT EXISTS "feature_upgrade_run_org_created_idx"
  ON "public"."feature_upgrade_run"("organization_id", "created_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_run_status_scheduled_idx"
  ON "public"."feature_upgrade_run"("status", "scheduled_for");

CREATE INDEX IF NOT EXISTS "feature_upgrade_device_org_agent_updated_idx"
  ON "public"."feature_upgrade_device"("organization_id", "agent_id", "updated_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_device_agent_status_scheduled_created_idx"
  ON "public"."feature_upgrade_device"("agent_id", "status", "scheduled_for", "created_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_device_run_idx"
  ON "public"."feature_upgrade_device"("run_id");

CREATE INDEX IF NOT EXISTS "feature_upgrade_device_iso_media_idx"
  ON "public"."feature_upgrade_device"("iso_media_id");

CREATE INDEX IF NOT EXISTS "feature_upgrade_device_setup_command_matrix_idx"
  ON "public"."feature_upgrade_device"("setup_command_matrix_id");

ALTER TABLE "public"."feature_upgrade_run"
  DROP CONSTRAINT IF EXISTS "feature_upgrade_run_organization_id_fkey",
  ADD CONSTRAINT "feature_upgrade_run_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."feature_upgrade_device"
  DROP CONSTRAINT IF EXISTS "feature_upgrade_device_run_id_fkey",
  ADD CONSTRAINT "feature_upgrade_device_run_id_fkey"
    FOREIGN KEY ("run_id") REFERENCES "public"."feature_upgrade_run"("id") ON DELETE CASCADE ON UPDATE CASCADE,
  DROP CONSTRAINT IF EXISTS "feature_upgrade_device_org_agent_fkey",
  ADD CONSTRAINT "feature_upgrade_device_org_agent_fkey"
    FOREIGN KEY ("organization_id", "agent_id") REFERENCES "public"."rmm_devices"("organization_id", "agent_id") ON DELETE CASCADE ON UPDATE CASCADE,
  DROP CONSTRAINT IF EXISTS "feature_upgrade_device_iso_media_id_fkey",
  ADD CONSTRAINT "feature_upgrade_device_iso_media_id_fkey"
    FOREIGN KEY ("iso_media_id") REFERENCES "public"."feature_upgrade_iso_media"("id") ON DELETE RESTRICT ON UPDATE CASCADE,
  DROP CONSTRAINT IF EXISTS "feature_upgrade_device_setup_command_matrix_id_fkey",
  ADD CONSTRAINT "feature_upgrade_device_setup_command_matrix_id_fkey"
    FOREIGN KEY ("setup_command_matrix_id") REFERENCES "public"."feature_upgrade_setup_command_matrix"("id") ON DELETE RESTRICT ON UPDATE CASCADE;

INSERT INTO "public"."feature_upgrade_setup_command_matrix" (
  "id",
  "iso_media_id",
  "os_family",
  "product",
  "version",
  "edition",
  "architecture",
  "language",
  "setup_executable",
  "arguments_jsonb",
  "dynamic_update_mode",
  "requires_eula_accept",
  "image_index_strategy",
  "supported",
  "notes",
  "active",
  "created_at",
  "updated_at"
)
SELECT
  CONCAT('matrix-', media."id"),
  media."id",
  media."os_family",
  media."product",
  media."version",
  media."edition",
  media."architecture",
  media."language",
  '{mount_drive}\setup.exe',
  CASE
    WHEN media."product" ILIKE '%windows server%' AND media."version" ILIKE '%2008%' THEN '[]'::jsonb
    WHEN media."product" ILIKE '%windows server%' AND media."version" = '2025' THEN
      '["/auto","upgrade","/quiet","/eula","accept","/pkey","{target_server_gvlk}","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
    WHEN media."product" ILIKE '%windows 11%' THEN
      '["/auto","upgrade","/quiet","/eula","accept","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
    ELSE
      '["/auto","upgrade","/quiet","/dynamicupdate","disable","/showoobe","none","/compat","ignorewarning","/migratedrivers","all","/copylogs","{log_dir}"]'::jsonb
  END,
  'disable',
  CASE
    WHEN media."product" ILIKE '%windows 11%' OR (media."product" ILIKE '%windows server%' AND media."version" = '2025') THEN true
    ELSE false
  END,
  'auto_match_current_edition',
  CASE
    WHEN media."product" ILIKE '%windows server%' AND media."version" ILIKE '%2008%' THEN false
    ELSE true
  END,
  CASE
    WHEN media."product" ILIKE '%windows server%' AND media."version" ILIKE '%2008%' THEN 'Unsupported by the v1 automated in-place feature upgrade flow.'
    ELSE 'Seeded from Microsoft Windows Setup command-line options for silent in-place upgrades.'
  END,
  true,
  NOW(),
  NOW()
FROM "public"."feature_upgrade_iso_media" media
WHERE media."os_family" = 'windows'
ON CONFLICT ("iso_media_id") DO UPDATE SET
  "os_family" = EXCLUDED."os_family",
  "product" = EXCLUDED."product",
  "version" = EXCLUDED."version",
  "edition" = EXCLUDED."edition",
  "architecture" = EXCLUDED."architecture",
  "language" = EXCLUDED."language",
  "arguments_jsonb" = EXCLUDED."arguments_jsonb",
  "dynamic_update_mode" = EXCLUDED."dynamic_update_mode",
  "requires_eula_accept" = EXCLUDED."requires_eula_accept",
  "image_index_strategy" = EXCLUDED."image_index_strategy",
  "supported" = EXCLUDED."supported",
  "notes" = EXCLUDED."notes",
  "active" = true,
  "updated_at" = NOW();
