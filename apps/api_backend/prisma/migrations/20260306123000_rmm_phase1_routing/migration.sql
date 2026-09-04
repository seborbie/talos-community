ALTER TABLE "rmm_telemetry"."routing_rule"
  ADD COLUMN IF NOT EXISTS "site_id" TEXT,
  ADD COLUMN IF NOT EXISTS "previous_match_operator" TEXT,
  ADD COLUMN IF NOT EXISTS "previous_match_value" TEXT,
  ADD COLUMN IF NOT EXISTS "min_support_ratio" DOUBLE PRECISION,
  ADD COLUMN IF NOT EXISTS "min_confidence_score" DOUBLE PRECISION,
  ADD COLUMN IF NOT EXISTS "scope_type_filter" TEXT;

DROP INDEX IF EXISTS "rmm_telemetry"."routing_rule_organization_id_customer_id_agent_id_idx";
CREATE INDEX IF NOT EXISTS "routing_rule_organization_id_customer_id_site_id_agent_id_idx"
  ON "rmm_telemetry"."routing_rule"("organization_id", "customer_id", "site_id", "agent_id");

ALTER TABLE "rmm_telemetry"."routing_decision"
  ADD COLUMN IF NOT EXISTS "execution_status" TEXT NOT NULL DEFAULT 'pending',
  ADD COLUMN IF NOT EXISTS "external_ref" TEXT,
  ADD COLUMN IF NOT EXISTS "outcome_message" TEXT;
