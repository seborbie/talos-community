/*
  Warnings:

  - You are about to drop the column `haloBaseUrlEnc` on the `Organization` table. All the data in the column will be lost.
  - You are about to drop the column `haloClientIdEnc` on the `Organization` table. All the data in the column will be lost.
  - You are about to drop the column `haloClientSecretEnc` on the `Organization` table. All the data in the column will be lost.
  - You are about to drop the column `twilioSubaccountAuthToken` on the `Organization` table. All the data in the column will be lost.
  - You are about to drop the column `twilioSubaccountFriendlyName` on the `Organization` table. All the data in the column will be lost.
  - You are about to drop the column `twilioSubaccountSid` on the `Organization` table. All the data in the column will be lost.

*/
-- DropIndex
DROP INDEX IF EXISTS "public"."Organization_twilioSubaccountSid_key";

-- AlterTable
ALTER TABLE "public"."Organization"
DROP COLUMN IF EXISTS "haloBaseUrlEnc",
DROP COLUMN IF EXISTS "haloClientIdEnc",
DROP COLUMN IF EXISTS "haloClientSecretEnc",
DROP COLUMN IF EXISTS "twilioSubaccountAuthToken",
DROP COLUMN IF EXISTS "twilioSubaccountFriendlyName",
DROP COLUMN IF EXISTS "twilioSubaccountSid";