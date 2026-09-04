-- CreateTable
CREATE TABLE "rmm_telemetry"."snapshot_request" (
    "id" BIGSERIAL NOT NULL,
    "agent_id" TEXT NOT NULL,
    "request_id" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updated_at" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "snapshot_request_pkey" PRIMARY KEY ("id")
);

ALTER TABLE "rmm_telemetry"."snapshot_request"
ADD CONSTRAINT "snapshot_request_agent_id_fkey"
FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id")
ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "rmm_telemetry"."snapshot_request"
ADD CONSTRAINT "snapshot_request_status_check"
CHECK ("status" IN ('pending', 'completed', 'failed'));

-- CreateIndex
CREATE INDEX "snapshot_request_agent_id_idx" ON "rmm_telemetry"."snapshot_request"("agent_id");

-- CreateIndex
CREATE INDEX "snapshot_request_request_id_idx" ON "rmm_telemetry"."snapshot_request"("request_id");

-- CreateIndex
CREATE UNIQUE INDEX "snapshot_request_agent_id_request_id_key" ON "rmm_telemetry"."snapshot_request"("agent_id", "request_id");
