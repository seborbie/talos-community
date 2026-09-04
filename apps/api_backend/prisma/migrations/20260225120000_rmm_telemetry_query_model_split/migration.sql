-- AlterTable
ALTER TABLE "rmm_telemetry"."device_state" DROP COLUMN "device_details",
DROP COLUMN "last_inventory",
DROP COLUMN "snapshot",
ADD COLUMN     "agent_version" TEXT,
ADD COLUMN     "boot_session_id" TEXT,
ADD COLUMN     "cpu_base_mhz" DOUBLE PRECISION,
ADD COLUMN     "cpu_logical_cores" INTEGER,
ADD COLUMN     "cpu_model" TEXT,
ADD COLUMN     "cpu_physical_cores" INTEGER,
ADD COLUMN     "hostname" TEXT,
ADD COLUMN     "installed_apps_count" INTEGER,
ADD COLUMN     "inventory_data" JSONB,
ADD COLUMN     "memory_total_bytes" BIGINT,
ADD COLUMN     "os_name" TEXT,
ADD COLUMN     "os_version" TEXT,
ADD COLUMN     "pending_updates_count" INTEGER,
ADD COLUMN     "reboot_required" BOOLEAN;

-- AlterTable
ALTER TABLE "rmm_telemetry"."snapshot_ingest" DROP COLUMN "snapshot";

-- CreateTable
CREATE TABLE "rmm_telemetry"."device_installed_app" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "app_name" TEXT NOT NULL,
    "app_name_norm" TEXT NOT NULL,
    "publisher" TEXT,
    "publisher_norm" TEXT,
    "version" TEXT,
    "install_date" TEXT,
    "size_bytes" BIGINT,
    "source" TEXT,
    "location" TEXT,
    "uninstall_string" TEXT,
    "is_64_bit" BOOLEAN,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_installed_app_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE INDEX "device_installed_app_agent_id_idx" ON "rmm_telemetry"."device_installed_app"("agent_id");

-- CreateIndex
CREATE INDEX "device_installed_app_app_name_norm_idx" ON "rmm_telemetry"."device_installed_app"("app_name_norm");

-- CreateIndex
CREATE INDEX "device_installed_app_publisher_norm_idx" ON "rmm_telemetry"."device_installed_app"("publisher_norm");

-- CreateIndex
CREATE INDEX "device_installed_app_app_name_norm_version_idx" ON "rmm_telemetry"."device_installed_app"("app_name_norm", "version");

-- CreateIndex
CREATE UNIQUE INDEX "device_installed_app_agent_id_app_name_norm_version_key" ON "rmm_telemetry"."device_installed_app"("agent_id", "app_name_norm", "version");

-- CreateIndex
CREATE INDEX "device_state_hostname_idx" ON "rmm_telemetry"."device_state"("hostname");

-- CreateIndex
CREATE INDEX "device_state_os_name_idx" ON "rmm_telemetry"."device_state"("os_name");

-- CreateIndex
CREATE INDEX "device_state_agent_version_idx" ON "rmm_telemetry"."device_state"("agent_version");

-- CreateIndex
CREATE INDEX "device_state_reboot_required_idx" ON "rmm_telemetry"."device_state"("reboot_required");

-- CreateIndex
CREATE INDEX "device_state_pending_updates_count_idx" ON "rmm_telemetry"."device_state"("pending_updates_count");

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_installed_app" ADD CONSTRAINT "device_installed_app_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
