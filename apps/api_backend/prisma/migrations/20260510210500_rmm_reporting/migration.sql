CREATE TABLE IF NOT EXISTS "public"."rmm_report_run" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "report_id" TEXT NOT NULL,
  "format" TEXT NOT NULL,
  "filters_jsonb" JSONB NOT NULL DEFAULT '{}',
  "status" TEXT NOT NULL DEFAULT 'succeeded',
  "row_count" INTEGER NOT NULL DEFAULT 0,
  "generated_by" TEXT,
  "delivery_status" TEXT NOT NULL DEFAULT 'ready',
  "error_message" TEXT,
  "started_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "finished_at" TIMESTAMPTZ(3),
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "rmm_report_run_pkey" PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "public"."rmm_report_schedule" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "report_id" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "format" TEXT NOT NULL,
  "frequency" TEXT NOT NULL,
  "filters_jsonb" JSONB NOT NULL DEFAULT '{}',
  "email_to_jsonb" JSONB NOT NULL DEFAULT '[]',
  "email_delivery_status" TEXT NOT NULL DEFAULT 'stubbed',
  "is_enabled" BOOLEAN NOT NULL DEFAULT true,
  "last_run_at" TIMESTAMPTZ(3),
  "next_run_at" TIMESTAMPTZ(3),
  "created_by" TEXT,
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "rmm_report_schedule_pkey" PRIMARY KEY ("id")
);

CREATE INDEX IF NOT EXISTS "rmm_report_run_organization_id_created_at_idx"
  ON "public"."rmm_report_run"("organization_id", "created_at");

CREATE INDEX IF NOT EXISTS "rmm_report_run_organization_id_report_id_created_at_idx"
  ON "public"."rmm_report_run"("organization_id", "report_id", "created_at");

CREATE INDEX IF NOT EXISTS "rmm_report_schedule_organization_id_is_enabled_next_run_at_idx"
  ON "public"."rmm_report_schedule"("organization_id", "is_enabled", "next_run_at");

CREATE INDEX IF NOT EXISTS "rmm_report_schedule_organization_id_report_id_idx"
  ON "public"."rmm_report_schedule"("organization_id", "report_id");

ALTER TABLE "public"."rmm_report_run"
  DROP CONSTRAINT IF EXISTS "rmm_report_run_organization_id_fkey",
  ADD CONSTRAINT "rmm_report_run_organization_id_fkey"
  FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id")
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_report_run"
  DROP CONSTRAINT IF EXISTS "rmm_report_run_generated_by_fkey",
  ADD CONSTRAINT "rmm_report_run_generated_by_fkey"
  FOREIGN KEY ("generated_by") REFERENCES "public"."User"("id")
  ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_report_schedule"
  DROP CONSTRAINT IF EXISTS "rmm_report_schedule_organization_id_fkey",
  ADD CONSTRAINT "rmm_report_schedule_organization_id_fkey"
  FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id")
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_report_schedule"
  DROP CONSTRAINT IF EXISTS "rmm_report_schedule_created_by_fkey",
  ADD CONSTRAINT "rmm_report_schedule_created_by_fkey"
  FOREIGN KEY ("created_by") REFERENCES "public"."User"("id")
  ON DELETE SET NULL ON UPDATE CASCADE;
