-- Add callSid and sessionId columns and indexes to Call table
ALTER TABLE "Call" ADD COLUMN "callSid" TEXT;
ALTER TABLE "Call" ADD COLUMN "sessionId" VARCHAR(64);

-- Unique constraint for callSid
CREATE UNIQUE INDEX IF NOT EXISTS "Call_callSid_key" ON "Call"("callSid");

-- Index for sessionId lookups
CREATE INDEX IF NOT EXISTS "Call_sessionId_idx" ON "Call"("sessionId");