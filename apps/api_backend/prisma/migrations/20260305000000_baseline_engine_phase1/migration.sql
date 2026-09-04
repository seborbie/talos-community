-- CreateTable: fact_stability_override
CREATE TABLE "rmm_telemetry"."fact_stability_override" (
    "id" BIGSERIAL NOT NULL,
    "organization_id" TEXT NOT NULL,
    "fact_key_pattern" TEXT NOT NULL,
    "stability_class" TEXT NOT NULL,
    "reason" TEXT,
    "created_by" TEXT NOT NULL,
    "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "fact_stability_override_pkey" PRIMARY KEY ("id")
);

-- CreateTable: intent
CREATE TABLE "rmm_telemetry"."intent" (
    "id" TEXT NOT NULL,
    "organization_id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "description" TEXT,
    "type" TEXT NOT NULL DEFAULT 'hardcoded',
    "allow_list" JSONB,
    "steps" JSONB,
    "ai_prompt" TEXT,
    "trigger_domain" TEXT,
    "trigger_key" TEXT,
    "requires_approval" BOOLEAN NOT NULL DEFAULT true,
    "max_retries" INTEGER NOT NULL DEFAULT 1,
    "timeout_seconds" INTEGER NOT NULL DEFAULT 300,
    "enabled" BOOLEAN NOT NULL DEFAULT true,
    "created_by" TEXT NOT NULL,
    "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "intent_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE UNIQUE INDEX "fact_stability_override_organization_id_fact_key_pattern_key" ON "rmm_telemetry"."fact_stability_override"("organization_id", "fact_key_pattern");

-- CreateIndex
CREATE INDEX "fact_stability_override_organization_id_idx" ON "rmm_telemetry"."fact_stability_override"("organization_id");

-- CreateIndex
CREATE INDEX "intent_organization_id_enabled_idx" ON "rmm_telemetry"."intent"("organization_id", "enabled");

-- CreateIndex
CREATE INDEX "intent_organization_id_trigger_domain_trigger_key_idx" ON "rmm_telemetry"."intent"("organization_id", "trigger_domain", "trigger_key");
