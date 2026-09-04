-- Add encrypted Halo config fields to Organization
ALTER TABLE "public"."Organization" ADD COLUMN IF NOT EXISTS "haloBaseUrlEnc" TEXT;
ALTER TABLE "public"."Organization" ADD COLUMN IF NOT EXISTS "haloClientIdEnc" TEXT;
ALTER TABLE "public"."Organization" ADD COLUMN IF NOT EXISTS "haloClientSecretEnc" TEXT;
