import { Router } from 'express';
import { prisma } from '../lib/prisma';
import { requireAuth, AuthedRequest } from '../middleware/auth';
import bcrypt from 'bcrypt';
import { decryptSecret, encryptSecret } from '../lib/crypto';
import { auditRequest, writeAuditEvent } from '../lib/audit';
import { ensureDefaultPatchPolicy } from '../lib/patchPolicies';

export const orgsRouter = Router();
orgsRouter.use(requireAuth);

// Helper: fetch current user's membership (first org)
async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { organization: true, user: { select: { id: true, email: true } } }
  });
}

function assertUser(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== 'user') {
    res.status(403).json({ error: 'Machine tokens are not allowed' });
    return false;
  }
  return true;
}

function isSuperAdmin(role: string) { return role === 'SUPER_ADMIN'; }
function isAgentAdmin(role: string) { return role === 'AGENT_ADMIN' || role === 'SUPER_ADMIN'; }

// GET /orgs/current - current user's organization and role
orgsRouter.get('/current', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  res.json({
    organization: { id: membership.organization.id, name: membership.organization.name, createdAt: membership.organization.createdAt },
    membership: { id: membership.id, role: membership.role, userId: membership.userId, organizationId: membership.organizationId },
    user: { id: membership.user.id, email: membership.user.email }
  });
});

// POST /orgs/onboard - create org for user who has none, optionally add members
orgsRouter.post('/onboard', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const userId = req.jwt!.sub;
  const existing = await getCurrentMembership(userId);
  if (existing) return res.status(400).json({ error: 'Already in an organization' });

  const { name, members } = req.body as { name: string; members?: Array<{ email: string; password?: string; role: 'AGENT_ADMIN' | 'VIEWER' | 'SUPER_ADMIN' }>; };
  if (!name || typeof name !== 'string' || name.trim().length < 2) {
    return res.status(400).json({ error: 'Organization name is required' });
  }

  const organization = await prisma.organization.create({ data: { name: name.trim() } });
  await ensureDefaultPatchPolicy(organization.id);

  // Add current user as SUPER_ADMIN
  await prisma.organizationMember.create({
    data: { userId, organizationId: organization.id, role: 'SUPER_ADMIN' }
  });

  // Twilio subaccounts removed; all numbers managed in main account

  // Optionally add other members
  if (Array.isArray(members)) {
    for (const m of members) {
      if (!m?.email || !m.role) continue;
      const role = m.role as any;
      // Do not allow creating another SUPER_ADMIN during onboarding unless explicitly specified
      const email = String(m.email).toLowerCase();
      let user = await prisma.user.findUnique({ where: { email } });
      if (!user) {
        if (!m.password) continue; // skip users without provided password for MVP
        const hash = await bcrypt.hash(m.password, 10);
        user = await prisma.user.create({ data: { email, password: hash } });
      }
      const exists = await prisma.organizationMember.findFirst({ where: { userId: user.id, organizationId: organization.id } });
      if (!exists) {
        await prisma.organizationMember.create({ data: { userId: user.id, organizationId: organization.id, role } });
      }
    }
  }

  res.status(201).json({ organization });
});

// ──────────────────────────────────────────────────────────────
// Ticketing System Integration (Halo) CRUD
// ──────────────────────────────────────────────────────────────

// GET /orgs/ticketing/halo - returns decrypted Halo config for current org (SUPER_ADMIN only)
orgsRouter.get('/ticketing/halo', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can view organization ticketing config' });

  const org = await prisma.organization.findUnique({ where: { id: requester.organizationId } });
  if (!org) return res.status(404).json({ error: 'Organization not found' });
  const config = {
    baseUrl: decryptSecret((org as any).haloBaseUrlEnc) || '',
    clientId: decryptSecret((org as any).haloClientIdEnc) || '',
    clientSecret: decryptSecret((org as any).haloClientSecretEnc) || '',
  };
  res.json(config);
});

// PUT /orgs/ticketing/halo - upsert Halo config (SUPER_ADMIN only)
orgsRouter.put('/ticketing/halo', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can update organization ticketing config' });

  const { baseUrl, clientId, clientSecret } = req.body || {};
  if (!baseUrl || !clientId || !clientSecret) {
    return res.status(400).json({ error: 'baseUrl, clientId and clientSecret are required' });
  }

  try {
    await prisma.organization.update({
      where: { id: requester.organizationId },
      data: {
        haloBaseUrlEnc: encryptSecret(String(baseUrl)),
        haloClientIdEnc: encryptSecret(String(clientId)),
        haloClientSecretEnc: encryptSecret(String(clientSecret)),
      } as any,
    });
    res.json({ success: true });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    res.status(500).json({ error: `Failed to save Halo configuration: ${message}` });
  }
});

// DELETE /orgs/ticketing/halo - clear halo config (SUPER_ADMIN only)
orgsRouter.delete('/ticketing/halo', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can remove organization ticketing config' });

  await prisma.organization.update({
    where: { id: requester.organizationId },
    data: { haloBaseUrlEnc: null, haloClientIdEnc: null, haloClientSecretEnc: null } as any,
  });
  res.status(204).end();
});

// GET /orgs/members - list members of current org
orgsRouter.get('/members', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  const members = await prisma.organizationMember.findMany({
    where: { organizationId: membership.organizationId },
    include: { user: { select: { id: true, email: true } } },
    orderBy: { createdAt: 'asc' }
  });
  res.json(members.map((m: any) => ({ id: m.id, role: m.role, userId: m.userId, organizationId: m.organizationId, email: m.user.email })));
});

// POST /orgs/members - add member (SUPER_ADMIN only)
orgsRouter.post('/members', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can add members' });

  const { email, password, role } = req.body as { email: string; password?: string; role: 'SUPER_ADMIN' | 'AGENT_ADMIN' | 'VIEWER' };
  if (!email || !role) return res.status(400).json({ error: 'email and role are required' });
  const normalizedEmail = email.toLowerCase();
  let user = await prisma.user.findUnique({ where: { email: normalizedEmail } });
  if (!user) {
    if (!password || password.length < 8) return res.status(400).json({ error: 'Password (>=8 chars) required to create new user' });
    const hash = await bcrypt.hash(password, 10);
    user = await prisma.user.create({ data: { email: normalizedEmail, password: hash } });
  }
  const existing = await prisma.organizationMember.findFirst({ where: { userId: user.id, organizationId: requester.organizationId } });
  if (existing) return res.status(409).json({ error: 'User is already a member' });

  const member = await prisma.$transaction(async (tx) => {
    const created = await tx.organizationMember.create({ data: { userId: user.id, organizationId: requester.organizationId, role } });
    await writeAuditEvent(auditRequest(req, {
      organizationId: requester.organizationId,
      actorType: 'user',
      userId: requester.userId,
      userEmail: requester.user.email,
      actionType: 'user.role.add',
      targetType: 'organization_member',
      targetId: created.id,
      targetName: normalizedEmail,
      result: 'success',
      metadata: {
        targetUserId: user.id,
        role
      }
    }), tx);
    return created;
  });
  res.status(201).json(member);
});

// PATCH /orgs/members/:id - update member role (SUPER_ADMIN only)
orgsRouter.patch('/members/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can change roles' });

  const memberId = req.params.id;
  const member = await prisma.organizationMember.findUnique({
    where: { id: memberId },
    include: { user: { select: { id: true, email: true } } }
  });
  if (!member || member.organizationId !== requester.organizationId) return res.status(404).json({ error: 'Member not found' });

  const { role } = req.body as { role: 'SUPER_ADMIN' | 'AGENT_ADMIN' | 'VIEWER' };
  if (!role) return res.status(400).json({ error: 'role is required' });

  // Ensure not downgrading the last SUPER_ADMIN
  if (member.role === 'SUPER_ADMIN' && role !== 'SUPER_ADMIN') {
    const superAdmins = await prisma.organizationMember.count({ where: { organizationId: requester.organizationId, role: 'SUPER_ADMIN' } });
    if (superAdmins <= 1) {
      return res.status(400).json({ error: 'Cannot remove the last SUPER_ADMIN' });
    }
  }

  const updated = await prisma.$transaction(async (tx) => {
    const row = await tx.organizationMember.update({ where: { id: member.id }, data: { role } });
    await writeAuditEvent(auditRequest(req, {
      organizationId: requester.organizationId,
      actorType: 'user',
      userId: requester.userId,
      userEmail: requester.user.email,
      actionType: 'user.role.update',
      targetType: 'organization_member',
      targetId: member.id,
      targetName: member.user.email,
      result: 'success',
      metadata: {
        targetUserId: member.userId,
        previousRole: member.role,
        nextRole: role
      }
    }), tx);
    return row;
  });
  res.json(updated);
});

// DELETE /orgs/members/:id - remove a member (SUPER_ADMIN only)
orgsRouter.delete('/members/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can remove members' });

  const memberId = req.params.id;
  const member = await prisma.organizationMember.findUnique({
    where: { id: memberId },
    include: { user: { select: { id: true, email: true } } }
  });
  if (!member || member.organizationId !== requester.organizationId) return res.status(404).json({ error: 'Member not found' });

  // Prevent deleting the last SUPER_ADMIN
  if (member.role === 'SUPER_ADMIN') {
    const superAdmins = await prisma.organizationMember.count({ where: { organizationId: requester.organizationId, role: 'SUPER_ADMIN' } });
    if (superAdmins <= 1) {
      return res.status(400).json({ error: 'Cannot remove the last SUPER_ADMIN' });
    }
  }

  await prisma.$transaction(async (tx) => {
    await tx.organizationMember.delete({ where: { id: memberId } });
    await writeAuditEvent(auditRequest(req, {
      organizationId: requester.organizationId,
      actorType: 'user',
      userId: requester.userId,
      userEmail: requester.user.email,
      actionType: 'user.role.remove',
      targetType: 'organization_member',
      targetId: member.id,
      targetName: member.user.email,
      result: 'success',
      metadata: {
        targetUserId: member.userId,
        previousRole: member.role
      }
    }), tx);
  });
  res.status(204).end();
});

// DELETE /orgs/account - delete entire org (SUPER_ADMIN only)
orgsRouter.delete('/account', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const requester = await getCurrentMembership(req.jwt!.sub);
  if (!requester) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isSuperAdmin(requester.role)) return res.status(403).json({ error: 'Only SUPER_ADMIN can delete organization' });

  const orgId = requester.organizationId;

  // Delete org-linked data: memberships then org (customers/policies cascade or are unlinked as needed)
  await prisma.organizationMember.deleteMany({ where: { organizationId: orgId } });
  await prisma.organization.delete({ where: { id: orgId } });

  res.status(204).end();
});
