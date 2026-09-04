-- Add indexes to improve analytics and org-scoped lookups.
CREATE INDEX IF NOT EXISTS "Agent_userId_idx" ON "Agent"("userId");
CREATE INDEX IF NOT EXISTS "Agent_organizationId_idx" ON "Agent"("organizationId");

CREATE INDEX IF NOT EXISTS "Call_agentId_idx" ON "Call"("agentId");
CREATE INDEX IF NOT EXISTS "Call_createdAt_idx" ON "Call"("createdAt");
CREATE INDEX IF NOT EXISTS "Call_status_idx" ON "Call"("status");
CREATE INDEX IF NOT EXISTS "Call_agentId_createdAt_idx" ON "Call"("agentId", "createdAt");
CREATE INDEX IF NOT EXISTS "Call_status_createdAt_idx" ON "Call"("status", "createdAt");

CREATE INDEX IF NOT EXISTS "OrganizationMember_userId_idx" ON "OrganizationMember"("userId");
