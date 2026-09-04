/*
  Warnings:

  - A unique constraint covering the columns `[twilioPhoneNumber]` on the table `Agent` will be added. If there are existing duplicate values, this will fail.
  - A unique constraint covering the columns `[twilioPhoneNumberSid]` on the table `Agent` will be added. If there are existing duplicate values, this will fail.
  - A unique constraint covering the columns `[twilioSubaccountSid]` on the table `User` will be added. If there are existing duplicate values, this will fail.

*/
-- AlterTable
ALTER TABLE "Agent" ADD COLUMN     "twilioPhoneNumber" TEXT,
ADD COLUMN     "twilioPhoneNumberSid" TEXT;

-- AlterTable
ALTER TABLE "User" ADD COLUMN     "twilioSubaccountAuthToken" TEXT,
ADD COLUMN     "twilioSubaccountFriendlyName" TEXT,
ADD COLUMN     "twilioSubaccountSid" TEXT;

-- CreateIndex
CREATE UNIQUE INDEX "Agent_twilioPhoneNumber_key" ON "Agent"("twilioPhoneNumber");

-- CreateIndex
CREATE UNIQUE INDEX "Agent_twilioPhoneNumberSid_key" ON "Agent"("twilioPhoneNumberSid");

-- CreateIndex
CREATE UNIQUE INDEX "User_twilioSubaccountSid_key" ON "User"("twilioSubaccountSid");
