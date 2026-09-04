-- Alert lifecycle, noise control, and notification routing
CREATE TABLE IF NOT EXISTS "rmm_telemetry"."alert_rule" (
    "id" BIGSERIAL NOT NULL,
    "organization_id" TEXT NOT NULL,
    "customer_id" TEXT,
    "site_id" TEXT,
    "agent_id" TEXT,
    "name" TEXT NOT NULL,
    "trigger_domain" TEXT NOT NULL,
    "trigger_key" TEXT NOT NULL,
    "match_operator" TEXT NOT NULL DEFAULT 'equals',
    "match_value" TEXT,
    "severity" TEXT NOT NULL DEFAULT 'medium',
    "min_severity" TEXT,
    "dedupe_window_seconds" INTEGER NOT NULL DEFAULT 300,
    "enabled" BOOLEAN NOT NULL DEFAULT true,
    "priority" INTEGER NOT NULL DEFAULT 100,
    "notification_channels_jsonb" JSONB NOT NULL DEFAULT '[]'::jsonb,
    "created_by" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "alert_rule_pkey" PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."alert" (
    "id" BIGSERIAL NOT NULL,
    "organization_id" TEXT NOT NULL,
    "customer_id" TEXT,
    "site_id" TEXT,
    "agent_id" TEXT NOT NULL,
    "rule_id" BIGINT,
    "status" TEXT NOT NULL DEFAULT 'open',
    "severity" TEXT NOT NULL,
    "source_domain" TEXT NOT NULL,
    "source_key" TEXT NOT NULL,
    "source_event_id" TEXT,
    "source_fact_key" TEXT,
    "source_decision_id" BIGINT,
    "title" TEXT NOT NULL,
    "summary" TEXT,
    "fingerprint" TEXT NOT NULL,
    "first_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "last_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "occurrence_count" INTEGER NOT NULL DEFAULT 1,
    "owner_user_id" TEXT,
    "acknowledged_by" TEXT,
    "acknowledged_at" TIMESTAMPTZ(3),
    "snoozed_until" TIMESTAMPTZ(3),
    "resolved_by" TEXT,
    "resolved_at" TIMESTAMPTZ(3),
    "suppressed_until" TIMESTAMPTZ(3),
    "metadata_jsonb" JSONB NOT NULL DEFAULT '{}'::jsonb,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "alert_pkey" PRIMARY KEY ("id")
);

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."alert_notification_delivery" (
    "id" BIGSERIAL NOT NULL,
    "alert_id" BIGINT NOT NULL,
    "channel" TEXT NOT NULL,
    "adapter" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "detail" TEXT,
    "external_ref" TEXT,
    "attempted_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "alert_notification_delivery_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "alert_organization_id_fingerprint_key"
  ON "rmm_telemetry"."alert"("organization_id", "fingerprint");

CREATE INDEX IF NOT EXISTS "alert_organization_id_status_severity_last_seen_at_idx"
  ON "rmm_telemetry"."alert"("organization_id", "status", "severity", "last_seen_at");

CREATE INDEX IF NOT EXISTS "alert_organization_id_agent_id_last_seen_at_idx"
  ON "rmm_telemetry"."alert"("organization_id", "agent_id", "last_seen_at");

CREATE INDEX IF NOT EXISTS "alert_organization_id_customer_id_site_id_idx"
  ON "rmm_telemetry"."alert"("organization_id", "customer_id", "site_id");

CREATE INDEX IF NOT EXISTS "alert_rule_id_idx"
  ON "rmm_telemetry"."alert"("rule_id");

CREATE INDEX IF NOT EXISTS "alert_rule_organization_id_enabled_trigger_domain_trigger_key_idx"
  ON "rmm_telemetry"."alert_rule"("organization_id", "enabled", "trigger_domain", "trigger_key");

CREATE INDEX IF NOT EXISTS "alert_rule_organization_id_customer_id_site_id_agent_id_idx"
  ON "rmm_telemetry"."alert_rule"("organization_id", "customer_id", "site_id", "agent_id");

CREATE INDEX IF NOT EXISTS "alert_notification_delivery_alert_id_attempted_at_idx"
  ON "rmm_telemetry"."alert_notification_delivery"("alert_id", "attempted_at");

ALTER TABLE "rmm_telemetry"."alert_rule"
  ADD CONSTRAINT "alert_rule_agent_id_fkey"
  FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert_rule"
  ADD CONSTRAINT "alert_rule_customer_id_fkey"
  FOREIGN KEY ("customer_id") REFERENCES "public"."customers"("id") ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert_rule"
  ADD CONSTRAINT "alert_rule_site_id_fkey"
  FOREIGN KEY ("site_id") REFERENCES "public"."rmm_sites"("id") ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert"
  ADD CONSTRAINT "alert_agent_id_fkey"
  FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert"
  ADD CONSTRAINT "alert_rule_id_fkey"
  FOREIGN KEY ("rule_id") REFERENCES "rmm_telemetry"."alert_rule"("id") ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert"
  ADD CONSTRAINT "alert_owner_user_id_fkey"
  FOREIGN KEY ("owner_user_id") REFERENCES "public"."User"("id") ON DELETE SET NULL ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."alert_notification_delivery"
  ADD CONSTRAINT "alert_notification_delivery_alert_id_fkey"
  FOREIGN KEY ("alert_id") REFERENCES "rmm_telemetry"."alert"("id") ON DELETE CASCADE ON UPDATE CASCADE;
