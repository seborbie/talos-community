ALTER TABLE "public"."rmm_devices"
  ADD COLUMN IF NOT EXISTS "websocket_status" TEXT NOT NULL DEFAULT 'unknown',
  ADD COLUMN IF NOT EXISTS "websocket_connected_at" TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS "websocket_disconnected_at" TIMESTAMPTZ(3);

CREATE INDEX IF NOT EXISTS "rmm_devices_websocket_status_idx"
  ON "public"."rmm_devices"("websocket_status");

CREATE TABLE IF NOT EXISTS "public"."rmm_agent_health_alert" (
  "id" BIGSERIAL PRIMARY KEY,
  "organization_id" TEXT NOT NULL,
  "agent_id" TEXT NOT NULL,
  "alert_key" TEXT NOT NULL,
  "severity" TEXT NOT NULL,
  "status" TEXT NOT NULL DEFAULT 'active',
  "reason" TEXT NOT NULL,
  "detail" TEXT,
  "first_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "last_seen_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "resolved_at" TIMESTAMPTZ(3),
  "occurrence_count" INTEGER NOT NULL DEFAULT 1,
  "context_jsonb" JSONB NOT NULL DEFAULT '{}',
  CONSTRAINT "rmm_agent_health_alert_agent_id_fkey"
    FOREIGN KEY ("agent_id") REFERENCES "public"."rmm_devices"("agent_id") ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT "rmm_agent_health_alert_organization_id_fkey"
    FOREIGN KEY ("organization_id") REFERENCES "public"."Organization"("id") ON DELETE RESTRICT ON UPDATE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_agent_health_alert_org_agent_key_key"
  ON "public"."rmm_agent_health_alert"("organization_id", "agent_id", "alert_key");

CREATE INDEX IF NOT EXISTS "rmm_agent_health_alert_organization_id_status_last_seen_at_idx"
  ON "public"."rmm_agent_health_alert"("organization_id", "status", "last_seen_at");

CREATE INDEX IF NOT EXISTS "rmm_agent_health_alert_agent_id_status_last_seen_at_idx"
  ON "public"."rmm_agent_health_alert"("agent_id", "status", "last_seen_at");
