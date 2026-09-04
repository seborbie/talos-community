-- DropForeignKey
ALTER TABLE "rmm_telemetry"."device_state" DROP CONSTRAINT "device_state_agent_id_fkey";

-- DropForeignKey
ALTER TABLE "rmm_telemetry"."snapshot_ingest" DROP CONSTRAINT "snapshot_ingest_agent_id_fkey";

-- AlterTable
ALTER TABLE "rmm_telemetry"."device_state" ALTER COLUMN "updated_at" DROP DEFAULT,
ALTER COLUMN "updated_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "rmm_telemetry"."snapshot_ingest" ALTER COLUMN "created_at" SET DATA TYPE TIMESTAMP(3);

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."snapshot_ingest" ADD CONSTRAINT "snapshot_ingest_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_state" ADD CONSTRAINT "device_state_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
