-- Add site model and scoped baseline storage (organization/customer/site).

CREATE TABLE IF NOT EXISTS "public"."rmm_sites" (
  "id" TEXT NOT NULL,
  "customer_id" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "timezone" TEXT,
  "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_sites_pkey" PRIMARY KEY ("id")
);

CREATE INDEX IF NOT EXISTS "rmm_sites_customer_id_idx"
  ON "public"."rmm_sites"("customer_id");

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_sites_customer_id_name_key"
  ON "public"."rmm_sites"("customer_id", "name");

-- Drop default on updated_at to match Prisma DateTime behavior (no default)
ALTER TABLE "public"."rmm_sites" ALTER COLUMN "updated_at" DROP DEFAULT;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'rmm_sites_customer_id_fkey') THEN
    ALTER TABLE "public"."rmm_sites"
      ADD CONSTRAINT "rmm_sites_customer_id_fkey"
      FOREIGN KEY ("customer_id") REFERENCES "public"."customers"("id")
      ON DELETE CASCADE ON UPDATE CASCADE;
  END IF;
END $$;

ALTER TABLE "public"."rmm_devices"
  ADD COLUMN IF NOT EXISTS "site_id" TEXT;

CREATE INDEX IF NOT EXISTS "rmm_devices_site_id_idx"
  ON "public"."rmm_devices"("site_id");

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'rmm_devices_site_id_fkey') THEN
    ALTER TABLE "public"."rmm_devices"
      ADD CONSTRAINT "rmm_devices_site_id_fkey"
      FOREIGN KEY ("site_id") REFERENCES "public"."rmm_sites"("id")
      ON DELETE SET NULL ON UPDATE CASCADE;
  END IF;
END $$;

-- Seed one default site per customer and auto-attach existing devices.
INSERT INTO "public"."rmm_sites"
  ("id", "customer_id", "name", "timezone", "created_at", "updated_at")
SELECT
  ('default-' || c.id) AS id,
  c.id,
  'Default Site',
  'UTC',
  CURRENT_TIMESTAMP,
  CURRENT_TIMESTAMP
FROM "public"."customers" c
ON CONFLICT ("id") DO NOTHING;

UPDATE "public"."rmm_devices" d
SET "site_id" = ('default-' || d."customer_id")
WHERE d."customer_id" IS NOT NULL
  AND d."site_id" IS NULL;

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."fact_baseline_scope" (
  "id" BIGSERIAL NOT NULL,
  "scope_type" TEXT NOT NULL CHECK ("scope_type" IN ('organization', 'customer', 'site')),
  "scope_key" TEXT NOT NULL,
  "organization_id" TEXT,
  "customer_id" TEXT,
  "site_id" TEXT,
  "agent_id" TEXT,
  "fact_key" TEXT NOT NULL,
  "promoted_value" JSONB,
  "candidate_value" JSONB,
  "candidate_count" INTEGER NOT NULL DEFAULT 0,
  "window_count" INTEGER NOT NULL DEFAULT 0,
  "support_count" INTEGER NOT NULL DEFAULT 0,
  "total_count" INTEGER NOT NULL DEFAULT 0,
  "support_ratio" DOUBLE PRECISION NOT NULL DEFAULT 0,
  "sample_size" INTEGER NOT NULL DEFAULT 0,
  "confidence_score" DOUBLE PRECISION NOT NULL DEFAULT 0,
  "is_stable" BOOLEAN NOT NULL DEFAULT FALSE,
  "last_changed_at" TIMESTAMPTZ(3),
  "updated_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "fact_baseline_scope_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "fact_baseline_scope_scope_type_scope_key_fact_key_key"
  ON "rmm_telemetry"."fact_baseline_scope"("scope_type", "scope_key", "fact_key");

CREATE INDEX IF NOT EXISTS "fact_baseline_scope_organization_id_scope_type_updated_at_idx"
  ON "rmm_telemetry"."fact_baseline_scope"("organization_id", "scope_type", "updated_at");

CREATE INDEX IF NOT EXISTS "fact_baseline_scope_customer_id_scope_type_updated_at_idx"
  ON "rmm_telemetry"."fact_baseline_scope"("customer_id", "scope_type", "updated_at");

CREATE INDEX IF NOT EXISTS "fact_baseline_scope_site_id_scope_type_updated_at_idx"
  ON "rmm_telemetry"."fact_baseline_scope"("site_id", "scope_type", "updated_at");

CREATE INDEX IF NOT EXISTS "fact_baseline_scope_scope_type_scope_key_updated_at_idx"
  ON "rmm_telemetry"."fact_baseline_scope"("scope_type", "scope_key", "updated_at");

CREATE INDEX IF NOT EXISTS "fact_baseline_scope_fact_key_updated_at_idx"
  ON "rmm_telemetry"."fact_baseline_scope"("fact_key", "updated_at");

-- Drop default on updated_at to match Prisma DateTime behavior
ALTER TABLE "rmm_telemetry"."fact_baseline_scope" ALTER COLUMN "updated_at" DROP DEFAULT;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fact_baseline_scope_agent_id_fkey') THEN
    ALTER TABLE "rmm_telemetry"."fact_baseline_scope"
      ADD CONSTRAINT "fact_baseline_scope_agent_id_fkey"
      FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
      ON DELETE CASCADE ON UPDATE CASCADE;
  END IF;
END $$;
