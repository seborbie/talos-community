-- CreateTable
CREATE TABLE "rmm_telemetry"."device_pending_update" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "title" TEXT NOT NULL,
    "title_norm" TEXT NOT NULL,
    "description" TEXT,
    "kb_article" TEXT,
    "is_mandatory" BOOLEAN,
    "size_bytes" BIGINT,
    "requires_reboot" BOOLEAN,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_pending_update_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "rmm_telemetry"."device_installed_update" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "collected_at" TIMESTAMPTZ(3) NOT NULL,
    "installed_at" TIMESTAMPTZ(3),
    "title" TEXT NOT NULL,
    "title_norm" TEXT NOT NULL,
    "kb_article" TEXT,
    "operation" TEXT,
    "result" TEXT,
    "hresult" INTEGER,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "device_installed_update_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE INDEX "device_pending_update_agent_id_idx" ON "rmm_telemetry"."device_pending_update"("agent_id");

-- CreateIndex
CREATE INDEX "device_pending_update_title_norm_idx" ON "rmm_telemetry"."device_pending_update"("title_norm");

-- CreateIndex
CREATE INDEX "device_pending_update_kb_article_idx" ON "rmm_telemetry"."device_pending_update"("kb_article");

-- CreateIndex
CREATE INDEX "device_pending_update_requires_reboot_idx" ON "rmm_telemetry"."device_pending_update"("requires_reboot");

-- CreateIndex
CREATE UNIQUE INDEX "device_pending_update_agent_id_title_norm_kb_article_key" ON "rmm_telemetry"."device_pending_update"("agent_id", "title_norm", "kb_article");

-- CreateIndex
CREATE INDEX "device_installed_update_agent_id_idx" ON "rmm_telemetry"."device_installed_update"("agent_id");

-- CreateIndex
CREATE INDEX "device_installed_update_installed_at_idx" ON "rmm_telemetry"."device_installed_update"("installed_at");

-- CreateIndex
CREATE INDEX "device_installed_update_title_norm_idx" ON "rmm_telemetry"."device_installed_update"("title_norm");

-- CreateIndex
CREATE INDEX "device_installed_update_kb_article_idx" ON "rmm_telemetry"."device_installed_update"("kb_article");

-- CreateIndex
CREATE INDEX "device_installed_update_result_idx" ON "rmm_telemetry"."device_installed_update"("result");

-- CreateIndex
CREATE UNIQUE INDEX "device_installed_update_agent_id_title_norm_installed_at_kb_key" ON "rmm_telemetry"."device_installed_update"("agent_id", "title_norm", "installed_at", "kb_article");

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_pending_update" ADD CONSTRAINT "device_pending_update_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "rmm_telemetry"."device_installed_update" ADD CONSTRAINT "device_installed_update_agent_id_fkey" FOREIGN KEY ("agent_id") REFERENCES "rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE;
