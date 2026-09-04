-- CreateTable
CREATE TABLE "rmm_telemetry"."device_service" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "service_name" TEXT NOT NULL,
    "service_name_norm" TEXT NOT NULL,
    "display_name" TEXT NOT NULL,
    "display_name_norm" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "start_type" TEXT,
    "account" TEXT,
    "process_id" INTEGER,
    "can_stop" BOOLEAN,
    "can_pause" BOOLEAN,
    "is_critical" BOOLEAN,
    "description" TEXT,
    "path" TEXT,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_service_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "rmm_telemetry"."device_startup_item" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "item_name" TEXT NOT NULL,
    "item_name_norm" TEXT NOT NULL,
    "command" TEXT NOT NULL,
    "location" TEXT NOT NULL,
    "user_name" TEXT,
    "is_enabled" BOOLEAN,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_startup_item_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "rmm_telemetry"."device_windows_feature" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "feature_name" TEXT NOT NULL,
    "feature_name_norm" TEXT NOT NULL,
    "display_name" TEXT NOT NULL,
    "display_name_norm" TEXT NOT NULL,
    "install_state" TEXT,
    "enabled" BOOLEAN,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_windows_feature_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE INDEX "device_service_agent_id_idx" ON "rmm_telemetry"."device_service"("agent_id");

-- CreateIndex
CREATE INDEX "device_service_service_name_norm_idx" ON "rmm_telemetry"."device_service"("service_name_norm");

-- CreateIndex
CREATE INDEX "device_service_status_idx" ON "rmm_telemetry"."device_service"("status");

-- CreateIndex
CREATE INDEX "device_service_is_critical_idx" ON "rmm_telemetry"."device_service"("is_critical");

-- CreateIndex
CREATE UNIQUE INDEX "device_service_agent_id_service_name_norm_key" ON "rmm_telemetry"."device_service"("agent_id", "service_name_norm");

-- CreateIndex
CREATE INDEX "device_startup_item_agent_id_idx" ON "rmm_telemetry"."device_startup_item"("agent_id");

-- CreateIndex
CREATE INDEX "device_startup_item_item_name_norm_idx" ON "rmm_telemetry"."device_startup_item"("item_name_norm");

-- CreateIndex
CREATE INDEX "device_startup_item_is_enabled_idx" ON "rmm_telemetry"."device_startup_item"("is_enabled");

-- CreateIndex
CREATE UNIQUE INDEX "device_startup_item_agent_id_item_name_norm_command_locatio_key" ON "rmm_telemetry"."device_startup_item"("agent_id", "item_name_norm", "command", "location");

-- CreateIndex
CREATE INDEX "device_windows_feature_agent_id_idx" ON "rmm_telemetry"."device_windows_feature"("agent_id");

-- CreateIndex
CREATE INDEX "device_windows_feature_feature_name_norm_idx" ON "rmm_telemetry"."device_windows_feature"("feature_name_norm");

-- CreateIndex
CREATE INDEX "device_windows_feature_enabled_idx" ON "rmm_telemetry"."device_windows_feature"("enabled");

-- CreateIndex
CREATE UNIQUE INDEX "device_windows_feature_agent_id_feature_name_norm_key" ON "rmm_telemetry"."device_windows_feature"("agent_id", "feature_name_norm");

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_service" ADD CONSTRAINT "device_service_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_startup_item" ADD CONSTRAINT "device_startup_item_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_windows_feature" ADD CONSTRAINT "device_windows_feature_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
