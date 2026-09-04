-- Add device details field to rmm_devices
ALTER TABLE "rmm_devices"
ADD COLUMN "device_details" JSONB;
