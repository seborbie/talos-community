-- AlterTable
ALTER TABLE "rmm_telemetry"."device_event" ALTER COLUMN "created_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."fact_baseline" ALTER COLUMN "updated_at" DROP DEFAULT,
ALTER COLUMN "updated_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."fact_state_current" ALTER COLUMN "updated_at" DROP DEFAULT,
ALTER COLUMN "updated_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."processed_message_log" ALTER COLUMN "processed_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."remediation_job" ALTER COLUMN "requested_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."routing_decision" ALTER COLUMN "decided_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."routing_rule" ALTER COLUMN "created_at" SET DATA TYPE TIMESTAMP(3),
ALTER COLUMN "updated_at" DROP DEFAULT,
ALTER COLUMN "updated_at" SET DATA TYPE TIMESTAMP(3);

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."processed_message_log" ADD CONSTRAINT "processed_message_log_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

-- RenameIndex
ALTER INDEX "rmm_telemetry"."processed_message_log_source_topic_source_partition_source_offs" RENAME TO "processed_message_log_source_topic_source_partition_source__key";
