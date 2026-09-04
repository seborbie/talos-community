-- DropForeignKey
ALTER TABLE "Call" DROP CONSTRAINT IF EXISTS "Call_agentId_fkey";

-- DropTable
DROP TABLE IF EXISTS "Call";

-- DropTable
DROP TABLE IF EXISTS "Agent";

-- AlterTable
ALTER TABLE "User" DROP COLUMN IF EXISTS "twilioSubaccountSid",
DROP COLUMN IF EXISTS "twilioSubaccountAuthToken",
DROP COLUMN IF EXISTS "twilioSubaccountFriendlyName";
