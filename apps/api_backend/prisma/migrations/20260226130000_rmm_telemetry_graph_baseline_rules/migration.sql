CREATE SCHEMA IF NOT EXISTS "rmm_telemetry";

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."processed_message_log" (
    "id" BIGSERIAL NOT NULL,
    "source_topic" TEXT NOT NULL,
    "source_partition" INTEGER NOT NULL,
    "source_offset" BIGINT NOT NULL,
    "source_ts" TIMESTAMPTZ(3) NOT NULL,
    "source_key" TEXT,
    "agent_id" TEXT NOT NULL,
    "message_type" TEXT NOT NULL,
    "payload_sha256" TEXT NOT NULL,
    "processed_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "processed_message_log_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "processed_message_log_source_topic_source_partition_source_offset_key"
    ON "rmm_telemetry"."processed_message_log"("source_topic", "source_partition", "source_offset");

CREATE UNIQUE INDEX IF NOT EXISTS "processed_message_log_agent_id_payload_sha256_key"
    ON "rmm_telemetry"."processed_message_log"("agent_id", "payload_sha256");

CREATE INDEX IF NOT EXISTS "processed_message_log_agent_id_source_ts_idx"
    ON "rmm_telemetry"."processed_message_log"("agent_id", "source_ts");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."device_event" (
    "id" BIGSERIAL NOT NULL,
    "event_id" TEXT NOT NULL,
    "agent_id" TEXT NOT NULL,
    "occurred_at" TIMESTAMPTZ(3) NOT NULL,
    "received_at" TIMESTAMPTZ(3) NOT NULL,
    "event_type" TEXT NOT NULL,
    "severity" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "service_name" TEXT,
    "process_name" TEXT,
    "code" TEXT,
    "message" TEXT,
    "attributes_jsonb" JSONB NOT NULL DEFAULT '{}'::jsonb,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "device_event_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "device_event_event_id_key" UNIQUE ("event_id")
);

CREATE INDEX IF NOT EXISTS "device_event_agent_id_occurred_at_idx"
    ON "rmm_telemetry"."device_event"("agent_id", "occurred_at");

CREATE INDEX IF NOT EXISTS "device_event_event_type_occurred_at_idx"
    ON "rmm_telemetry"."device_event"("event_type", "occurred_at");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."fact_state_current" (
    "agent_id" TEXT NOT NULL,
    "fact_key" TEXT NOT NULL,
    "fact_value" JSONB NOT NULL,
    "fact_value_text" TEXT NOT NULL,
    "stability_class" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "source_ts" TIMESTAMPTZ(3) NOT NULL,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "fact_state_current_pkey" PRIMARY KEY ("agent_id", "fact_key")
);

CREATE INDEX IF NOT EXISTS "fact_state_current_agent_id_updated_at_idx"
    ON "rmm_telemetry"."fact_state_current"("agent_id", "updated_at");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."fact_change_log" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "fact_key" TEXT NOT NULL,
    "prev_value" JSONB,
    "next_value" JSONB NOT NULL,
    "change_kind" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "source_ts" TIMESTAMPTZ(3) NOT NULL,
    "ts" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "fact_change_log_pkey" PRIMARY KEY ("id")
);

CREATE INDEX IF NOT EXISTS "fact_change_log_agent_id_ts_idx"
    ON "rmm_telemetry"."fact_change_log"("agent_id", "ts");

CREATE INDEX IF NOT EXISTS "fact_change_log_fact_key_ts_idx"
    ON "rmm_telemetry"."fact_change_log"("fact_key", "ts");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."fact_baseline" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "fact_key" TEXT NOT NULL,
    "promoted_value" JSONB,
    "candidate_value" JSONB,
    "candidate_count" INTEGER NOT NULL DEFAULT 0,
    "window_count" INTEGER NOT NULL DEFAULT 0,
    "last_changed_at" TIMESTAMPTZ(3),
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "fact_baseline_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "fact_baseline_agent_id_fact_key_key" UNIQUE ("agent_id", "fact_key")
);

CREATE INDEX IF NOT EXISTS "fact_baseline_agent_id_updated_at_idx"
    ON "rmm_telemetry"."fact_baseline"("agent_id", "updated_at");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."routing_rule" (
    "id" BIGSERIAL NOT NULL,
    "organization_id" TEXT,
    "customer_id" TEXT,
    "agent_id" TEXT,
    "trigger_domain" TEXT NOT NULL,
    "trigger_key" TEXT NOT NULL,
    "match_operator" TEXT NOT NULL DEFAULT 'equals',
    "match_value" TEXT,
    "action" TEXT NOT NULL,
    "intent_id" TEXT,
    "cooldown_seconds" INTEGER NOT NULL DEFAULT 0,
    "enabled" BOOLEAN NOT NULL DEFAULT true,
    "priority" INTEGER NOT NULL DEFAULT 100,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "routing_rule_pkey" PRIMARY KEY ("id")
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_class idx
        JOIN pg_namespace ns ON ns.oid = idx.relnamespace
        WHERE idx.relkind = 'i'
          AND ns.nspname = 'rmm_telemetry'
          AND idx.relname = 'routing_rule_organization_customer_agent_idx'
    )
    AND NOT EXISTS (
        SELECT 1
        FROM pg_class idx
        JOIN pg_namespace ns ON ns.oid = idx.relnamespace
        WHERE idx.relkind = 'i'
          AND ns.nspname = 'rmm_telemetry'
          AND idx.relname = 'routing_rule_organization_id_customer_id_agent_id_idx'
    ) THEN
        CREATE INDEX "routing_rule_organization_customer_agent_idx"
            ON "rmm_telemetry"."routing_rule"("organization_id", "customer_id", "agent_id");
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS "routing_rule_enabled_trigger_domain_trigger_key_idx"
    ON "rmm_telemetry"."routing_rule"("enabled", "trigger_domain", "trigger_key");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."routing_decision" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "domain" TEXT NOT NULL,
    "trigger_key" TEXT NOT NULL,
    "trigger_value" JSONB,
    "action" TEXT NOT NULL,
    "matched_rule_id" BIGINT,
    "intent_id" TEXT,
    "reason" TEXT,
    "dedupe_key" TEXT,
    "source" TEXT NOT NULL,
    "source_ts" TIMESTAMPTZ(3) NOT NULL,
    "decided_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "routing_decision_pkey" PRIMARY KEY ("id")
);

CREATE INDEX IF NOT EXISTS "routing_decision_agent_id_source_ts_idx"
    ON "rmm_telemetry"."routing_decision"("agent_id", "source_ts");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."remediation_job" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "decision_id" BIGINT,
    "intent_id" TEXT NOT NULL,
    "status" TEXT NOT NULL DEFAULT 'queued',
    "dedupe_key" TEXT,
    "requested_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "started_at" TIMESTAMPTZ(3),
    "finished_at" TIMESTAMPTZ(3),
    "requested_by" TEXT NOT NULL DEFAULT 'consumer',
    "metadata_jsonb" JSONB NOT NULL DEFAULT '{}'::jsonb,

    CONSTRAINT "remediation_job_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "remediation_job_dedupe_key_key"
    ON "rmm_telemetry"."remediation_job"("dedupe_key");

CREATE INDEX IF NOT EXISTS "remediation_job_agent_id_requested_at_idx"
    ON "rmm_telemetry"."remediation_job"("agent_id", "requested_at");

CREATE TABLE IF NOT EXISTS "rmm_telemetry"."remediation_step" (
    "id" BIGSERIAL NOT NULL,
    "job_id" BIGINT NOT NULL,
    "step_index" INTEGER NOT NULL,
    "command" TEXT NOT NULL,
    "status" TEXT NOT NULL DEFAULT 'pending',
    "evidence_jsonb" JSONB,
    "started_at" TIMESTAMPTZ(3),
    "finished_at" TIMESTAMPTZ(3),

    CONSTRAINT "remediation_step_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "remediation_step_job_id_step_index_key" UNIQUE ("job_id", "step_index")
);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'device_event_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."device_event"
            ADD CONSTRAINT "device_event_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fact_state_current_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."fact_state_current"
            ADD CONSTRAINT "fact_state_current_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fact_change_log_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."fact_change_log"
            ADD CONSTRAINT "fact_change_log_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fact_baseline_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."fact_baseline"
            ADD CONSTRAINT "fact_baseline_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'routing_decision_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."routing_decision"
            ADD CONSTRAINT "routing_decision_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'routing_decision_matched_rule_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."routing_decision"
            ADD CONSTRAINT "routing_decision_matched_rule_id_fkey"
            FOREIGN KEY ("matched_rule_id") REFERENCES "rmm_telemetry"."routing_rule"("id")
            ON DELETE SET NULL ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'remediation_job_agent_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."remediation_job"
            ADD CONSTRAINT "remediation_job_agent_id_fkey"
            FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'remediation_job_decision_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."remediation_job"
            ADD CONSTRAINT "remediation_job_decision_id_fkey"
            FOREIGN KEY ("decision_id") REFERENCES "rmm_telemetry"."routing_decision"("id")
            ON DELETE SET NULL ON UPDATE CASCADE;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'remediation_step_job_id_fkey') THEN
        ALTER TABLE "rmm_telemetry"."remediation_step"
            ADD CONSTRAINT "remediation_step_job_id_fkey"
            FOREIGN KEY ("job_id") REFERENCES "rmm_telemetry"."remediation_job"("id")
            ON DELETE CASCADE ON UPDATE CASCADE;
    END IF;
END $$;
