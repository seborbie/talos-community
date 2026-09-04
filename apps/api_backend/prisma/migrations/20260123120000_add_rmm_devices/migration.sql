-- Create RMM devices table
CREATE TABLE "rmm_devices" (
  "agent_id" TEXT NOT NULL,
  "hostname" TEXT NOT NULL,
  "os" TEXT NOT NULL,
  "ip" TEXT NOT NULL,
  "version" TEXT,
  "last_seen" TIMESTAMP(3) NOT NULL,
  "last_inventory" JSONB,
  "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "rmm_devices_pkey" PRIMARY KEY ("agent_id")
);

-- Inventory snapshot history
CREATE TABLE "rmm_inventory_snapshots" (
  "id" BIGSERIAL NOT NULL,
  "agent_id" TEXT NOT NULL,
  "inventory" JSONB NOT NULL,
  "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "rmm_inventory_snapshots_pkey" PRIMARY KEY ("id")
);

CREATE INDEX "rmm_devices_last_seen_idx" ON "rmm_devices"("last_seen");
CREATE INDEX "rmm_inventory_snapshots_agent_id_idx" ON "rmm_inventory_snapshots"("agent_id");
CREATE INDEX "rmm_inventory_snapshots_created_at_idx" ON "rmm_inventory_snapshots"("created_at");

ALTER TABLE "rmm_inventory_snapshots"
ADD CONSTRAINT "rmm_inventory_snapshots_agent_id_fkey"
FOREIGN KEY ("agent_id")
REFERENCES "rmm_devices"("agent_id")
ON DELETE CASCADE ON UPDATE CASCADE;
