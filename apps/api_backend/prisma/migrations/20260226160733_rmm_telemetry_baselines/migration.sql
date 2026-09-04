-- DropIndex
DROP INDEX "rmm_telemetry"."device_event_agent_id_occurred_at_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."device_event_event_type_occurred_at_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."fact_baseline_agent_id_updated_at_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."fact_change_log_agent_id_ts_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."fact_change_log_fact_key_ts_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."fact_state_current_agent_id_updated_at_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."processed_message_log_agent_id_source_ts_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."remediation_job_agent_id_requested_at_idx";

-- DropIndex
DROP INDEX "rmm_telemetry"."routing_decision_agent_id_source_ts_idx";

-- CreateIndex
CREATE INDEX "device_event_agent_id_occurred_at_idx" ON "rmm_telemetry"."device_event"("agent_id", "occurred_at");

-- CreateIndex
CREATE INDEX "device_event_event_type_occurred_at_idx" ON "rmm_telemetry"."device_event"("event_type", "occurred_at");

-- CreateIndex
CREATE INDEX "fact_baseline_agent_id_updated_at_idx" ON "rmm_telemetry"."fact_baseline"("agent_id", "updated_at");

-- CreateIndex
CREATE INDEX "fact_change_log_agent_id_ts_idx" ON "rmm_telemetry"."fact_change_log"("agent_id", "ts");

-- CreateIndex
CREATE INDEX "fact_change_log_fact_key_ts_idx" ON "rmm_telemetry"."fact_change_log"("fact_key", "ts");

-- CreateIndex
CREATE INDEX "fact_state_current_agent_id_updated_at_idx" ON "rmm_telemetry"."fact_state_current"("agent_id", "updated_at");

-- CreateIndex
CREATE INDEX "processed_message_log_agent_id_source_ts_idx" ON "rmm_telemetry"."processed_message_log"("agent_id", "source_ts");

-- CreateIndex
CREATE INDEX "remediation_job_agent_id_requested_at_idx" ON "rmm_telemetry"."remediation_job"("agent_id", "requested_at");

-- CreateIndex
CREATE INDEX "routing_decision_agent_id_source_ts_idx" ON "rmm_telemetry"."routing_decision"("agent_id", "source_ts");

-- RenameIndex
ALTER INDEX "rmm_telemetry"."routing_rule_organization_customer_agent_idx" RENAME TO "routing_rule_organization_id_customer_id_agent_id_idx";
