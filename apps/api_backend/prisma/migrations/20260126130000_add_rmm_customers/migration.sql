-- Create customers table
CREATE TABLE "customers" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "description" TEXT,
  "is_unassigned" BOOLEAN NOT NULL DEFAULT false,
  "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "customers_pkey" PRIMARY KEY ("id")
);

-- Add customer column to rmm_devices
ALTER TABLE "rmm_devices"
ADD COLUMN "customer_id" TEXT;

-- Indexes
CREATE INDEX "customers_organization_id_idx" ON "customers"("organization_id");
CREATE INDEX "rmm_devices_customer_id_idx" ON "rmm_devices"("customer_id");

-- Foreign keys
ALTER TABLE "customers"
ADD CONSTRAINT "customers_organization_id_fkey"
FOREIGN KEY ("organization_id")
REFERENCES "Organization"("id")
ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "rmm_devices"
ADD CONSTRAINT "rmm_devices_customer_id_fkey"
FOREIGN KEY ("customer_id")
REFERENCES "customers"("id")
ON DELETE SET NULL ON UPDATE CASCADE;
