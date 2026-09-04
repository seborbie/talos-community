-- Dev-only destructive telemetry schema split.
-- Rebuilds device telemetry storage under rmm_telemetry schema.

CREATE SCHEMA IF NOT EXISTS rmm_telemetry;

CREATE TABLE IF NOT EXISTS rmm_telemetry.snapshot_ingest (
  id BIGSERIAL PRIMARY KEY,
  agent_id TEXT NOT NULL,
  collected_at TIMESTAMPTZ(3) NOT NULL,
  received_at TIMESTAMPTZ(3) NOT NULL,
  snapshot JSONB NOT NULL,
  blob_container TEXT NOT NULL,
  blob_name TEXT NOT NULL,
  blob_content_encoding TEXT NULL,
  blob_size_bytes BIGINT NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  CONSTRAINT snapshot_ingest_agent_id_fkey
    FOREIGN KEY (agent_id) REFERENCES public.rmm_devices(agent_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS snapshot_ingest_agent_id_collected_at_key
  ON rmm_telemetry.snapshot_ingest(agent_id, collected_at);

CREATE INDEX IF NOT EXISTS snapshot_ingest_agent_id_collected_at_idx
  ON rmm_telemetry.snapshot_ingest(agent_id, collected_at);

CREATE TABLE IF NOT EXISTS rmm_telemetry.device_state (
  agent_id TEXT PRIMARY KEY,
  collected_at TIMESTAMPTZ(3) NOT NULL,
  snapshot JSONB NOT NULL,
  last_inventory JSONB NULL,
  device_details JSONB NULL,
  blob_container TEXT NOT NULL,
  blob_name TEXT NOT NULL,
  blob_content_encoding TEXT NULL,
  blob_size_bytes BIGINT NULL,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
  CONSTRAINT device_state_agent_id_fkey
    FOREIGN KEY (agent_id) REFERENCES public.rmm_devices(agent_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS device_state_collected_at_idx
  ON rmm_telemetry.device_state(collected_at);

DROP TABLE IF EXISTS public.rmm_inventory_snapshots;

ALTER TABLE public.rmm_devices
  DROP COLUMN IF EXISTS last_inventory,
  DROP COLUMN IF EXISTS device_details;
