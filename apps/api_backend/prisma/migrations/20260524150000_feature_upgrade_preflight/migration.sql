CREATE TABLE IF NOT EXISTS "public"."feature_upgrade_preflight_run" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "requested_by" TEXT NOT NULL,
  "status" TEXT NOT NULL DEFAULT 'queued',
  "total_devices" INTEGER NOT NULL DEFAULT 0,
  "queued_devices" INTEGER NOT NULL DEFAULT 0,
  "running_devices" INTEGER NOT NULL DEFAULT 0,
  "passed_devices" INTEGER NOT NULL DEFAULT 0,
  "warning_devices" INTEGER NOT NULL DEFAULT 0,
  "failed_devices" INTEGER NOT NULL DEFAULT 0,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "finished_at" TIMESTAMPTZ(3),
  CONSTRAINT "feature_upgrade_preflight_run_pkey" PRIMARY KEY ("id"),
  CONSTRAINT "feature_upgrade_preflight_run_status_check"
    CHECK ("status" IN ('queued', 'running', 'completed', 'failed', 'cancelled'))
);

CREATE TABLE IF NOT EXISTS "public"."feature_upgrade_preflight_device" (
  "operation_id" TEXT NOT NULL,
  "run_id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "source_os" TEXT NOT NULL,
  "target_product" TEXT NOT NULL,
  "target_version" TEXT NOT NULL,
  "target_build_label" TEXT NOT NULL,
  "status" TEXT NOT NULL DEFAULT 'queued',
  "phase" TEXT NOT NULL DEFAULT 'queued',
  "check_results_jsonb" JSONB NOT NULL DEFAULT '[]',
  "failure_summary_jsonb" JSONB NOT NULL DEFAULT '[]',
  "warning_summary_jsonb" JSONB NOT NULL DEFAULT '[]',
  "requested_by" TEXT NOT NULL,
  "claimed_at" TIMESTAMPTZ(3),
  "started_at" TIMESTAMPTZ(3),
  "finished_at" TIMESTAMPTZ(3),
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  CONSTRAINT "feature_upgrade_preflight_device_pkey" PRIMARY KEY ("operation_id"),
  CONSTRAINT "feature_upgrade_preflight_device_status_check"
    CHECK ("status" IN ('queued', 'running', 'passed', 'warning', 'failed', 'cancelled')),
  CONSTRAINT "feature_upgrade_preflight_device_phase_check"
    CHECK ("phase" IN ('queued', 'checking', 'completed', 'failed', 'cancelled'))
);

CREATE INDEX IF NOT EXISTS "feature_upgrade_preflight_run_org_created_idx"
  ON "public"."feature_upgrade_preflight_run"("organization_id", "created_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_preflight_device_org_agent_updated_idx"
  ON "public"."feature_upgrade_preflight_device"("organization_id", "agent_id", "updated_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_preflight_device_agent_status_created_idx"
  ON "public"."feature_upgrade_preflight_device"("agent_id", "status", "created_at");

CREATE INDEX IF NOT EXISTS "feature_upgrade_preflight_device_run_idx"
  ON "public"."feature_upgrade_preflight_device"("run_id");

ALTER TABLE "public"."feature_upgrade_preflight_run"
  DROP CONSTRAINT IF EXISTS "feature_upgrade_preflight_run_organization_id_fkey",
  ADD CONSTRAINT "feature_upgrade_preflight_run_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."feature_upgrade_preflight_device"
  DROP CONSTRAINT IF EXISTS "feature_upgrade_preflight_device_run_id_fkey",
  ADD CONSTRAINT "feature_upgrade_preflight_device_run_id_fkey"
    FOREIGN KEY ("run_id") REFERENCES "public"."feature_upgrade_preflight_run"("id") ON DELETE CASCADE ON UPDATE CASCADE,
  DROP CONSTRAINT IF EXISTS "feature_upgrade_preflight_device_org_agent_fkey",
  ADD CONSTRAINT "feature_upgrade_preflight_device_org_agent_fkey"
    FOREIGN KEY ("organization_id", "agent_id") REFERENCES "public"."rmm_devices"("organization_id", "agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
