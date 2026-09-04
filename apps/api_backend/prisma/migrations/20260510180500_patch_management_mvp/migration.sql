CREATE TABLE "public"."rmm_patch_policy" (
    "id" TEXT NOT NULL,
    "organization_id" TEXT NOT NULL,
    "scope_type" TEXT NOT NULL,
    "scope_key" TEXT NOT NULL,
    "customer_id" TEXT,
    "site_id" TEXT,
    "agent_id" TEXT,
    "name" TEXT NOT NULL,
    "approval_mode" TEXT NOT NULL DEFAULT 'manual',
    "maintenance_window_start" TEXT,
    "maintenance_window_end" TEXT,
    "maintenance_window_timezone" TEXT,
    "reboot_behavior" TEXT NOT NULL DEFAULT 'allow',
    "deferral_days" INTEGER NOT NULL DEFAULT 0,
    "enabled" BOOLEAN NOT NULL DEFAULT true,
    "created_by" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "rmm_patch_policy_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "rmm_patch_policy_scope_type_check" CHECK ("scope_type" IN ('organization', 'customer', 'site', 'device')),
    CONSTRAINT "rmm_patch_policy_approval_mode_check" CHECK ("approval_mode" IN ('manual', 'auto_approve_security', 'auto_approve_all')),
    CONSTRAINT "rmm_patch_policy_reboot_behavior_check" CHECK ("reboot_behavior" IN ('suppress', 'allow', 'force')),
    CONSTRAINT "rmm_patch_policy_deferral_days_check" CHECK ("deferral_days" >= 0 AND "deferral_days" <= 365),
    CONSTRAINT "rmm_patch_policy_window_start_check" CHECK ("maintenance_window_start" IS NULL OR "maintenance_window_start" ~ '^([01][0-9]|2[0-3]):[0-5][0-9]$'),
    CONSTRAINT "rmm_patch_policy_window_end_check" CHECK ("maintenance_window_end" IS NULL OR "maintenance_window_end" ~ '^([01][0-9]|2[0-3]):[0-5][0-9]$')
);

CREATE TABLE "public"."rmm_patch_approval" (
    "id" TEXT NOT NULL,
    "organization_id" TEXT NOT NULL,
    "agent_id" TEXT NOT NULL,
    "update_key" TEXT NOT NULL,
    "title_norm" TEXT NOT NULL,
    "kb_article" TEXT,
    "decision" TEXT NOT NULL,
    "reason" TEXT,
    "defer_until" TIMESTAMPTZ(3),
    "decided_by" TEXT NOT NULL,
    "decided_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "rmm_patch_approval_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "rmm_patch_approval_decision_check" CHECK ("decision" IN ('approved', 'denied', 'deferred'))
);

CREATE TABLE "public"."rmm_patch_action" (
    "id" TEXT NOT NULL,
    "organization_id" TEXT NOT NULL,
    "agent_id" TEXT NOT NULL,
    "action_type" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "update_keys_jsonb" JSONB NOT NULL DEFAULT '[]',
    "remediation_job_id" BIGINT,
    "requested_by" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "rmm_patch_action_pkey" PRIMARY KEY ("id"),
    CONSTRAINT "rmm_patch_action_type_check" CHECK ("action_type" IN ('scan', 'approval', 'install'))
);

CREATE UNIQUE INDEX "rmm_patch_policy_organization_id_scope_type_scope_key_key"
    ON "public"."rmm_patch_policy"("organization_id", "scope_type", "scope_key");
CREATE INDEX "rmm_patch_policy_organization_id_enabled_idx"
    ON "public"."rmm_patch_policy"("organization_id", "enabled");
CREATE INDEX "rmm_patch_policy_customer_id_idx" ON "public"."rmm_patch_policy"("customer_id");
CREATE INDEX "rmm_patch_policy_site_id_idx" ON "public"."rmm_patch_policy"("site_id");
CREATE INDEX "rmm_patch_policy_agent_id_idx" ON "public"."rmm_patch_policy"("agent_id");

CREATE UNIQUE INDEX "rmm_patch_approval_organization_id_agent_id_update_key_key"
    ON "public"."rmm_patch_approval"("organization_id", "agent_id", "update_key");
CREATE INDEX "rmm_patch_approval_organization_id_decision_idx"
    ON "public"."rmm_patch_approval"("organization_id", "decision");
CREATE INDEX "rmm_patch_approval_agent_id_idx" ON "public"."rmm_patch_approval"("agent_id");
CREATE INDEX "rmm_patch_approval_update_key_idx" ON "public"."rmm_patch_approval"("update_key");

CREATE INDEX "rmm_patch_action_organization_id_created_at_idx"
    ON "public"."rmm_patch_action"("organization_id", "created_at");
CREATE INDEX "rmm_patch_action_agent_id_created_at_idx"
    ON "public"."rmm_patch_action"("agent_id", "created_at");
CREATE INDEX "rmm_patch_action_status_idx" ON "public"."rmm_patch_action"("status");

ALTER TABLE "public"."rmm_patch_policy"
    ADD CONSTRAINT "rmm_patch_policy_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_policy"
    ADD CONSTRAINT "rmm_patch_policy_customer_id_fkey"
    FOREIGN KEY ("customer_id") REFERENCES "public"."customers"("id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_policy"
    ADD CONSTRAINT "rmm_patch_policy_site_id_fkey"
    FOREIGN KEY ("site_id") REFERENCES "public"."rmm_sites"("id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_policy"
    ADD CONSTRAINT "rmm_patch_policy_agent_id_fkey"
    FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_approval"
    ADD CONSTRAINT "rmm_patch_approval_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_approval"
    ADD CONSTRAINT "rmm_patch_approval_agent_id_fkey"
    FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "public"."rmm_patch_action"
    ADD CONSTRAINT "rmm_patch_action_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_action"
    ADD CONSTRAINT "rmm_patch_action_agent_id_fkey"
    FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
ALTER TABLE "public"."rmm_patch_action"
    ADD CONSTRAINT "rmm_patch_action_remediation_job_id_fkey"
    FOREIGN KEY ("remediation_job_id") REFERENCES "rmm_telemetry"."remediation_job"("id") ON DELETE SET NULL ON UPDATE CASCADE;
