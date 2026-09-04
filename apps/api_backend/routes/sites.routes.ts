import { Router } from 'express';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import { requireAuth, AuthedRequest } from '../middleware/auth';

export const sitesRouter = Router();
sitesRouter.use(requireAuth);

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

function isAgentAdmin(role: string) {
  return role === 'AGENT_ADMIN' || role === 'SUPER_ADMIN';
}

// GET /sites - list sites for current org (optional ?customerId= to filter by customer)
sitesRouter.get('/', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const customerId = typeof req.query.customerId === 'string' ? req.query.customerId.trim() : undefined;

  const where: Prisma.RmmSiteWhereInput = {
    customer: { organizationId: membership.organizationId }
  };
  if (customerId) {
    where.customer = { organizationId: membership.organizationId, id: customerId };
  }

  const sites = await prisma.rmmSite.findMany({
    where,
    include: {
      customer: { select: { id: true, name: true } },
      _count: { select: { devices: true } }
    },
    orderBy: [{ customer: { name: 'asc' } }, { name: 'asc' }]
  });

  res.json(
    sites.map((site) => ({
      id: site.id,
      customerId: site.customerId,
      customerName: site.customer.name,
      name: site.name,
      timezone: site.timezone,
      createdAt: site.createdAt,
      updatedAt: site.updatedAt,
      deviceCount: site._count.devices
    }))
  );
});

// GET /sites/:id - get single site for current org
sitesRouter.get('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const site = await prisma.rmmSite.findFirst({
    where: {
      id: req.params.id,
      customer: { organizationId: membership.organizationId }
    },
    include: {
      customer: { select: { id: true, name: true } },
      _count: { select: { devices: true } }
    }
  });

  if (!site) {
    return res.status(404).json({ error: 'Site not found' });
  }

  res.json({
    id: site.id,
    customerId: site.customerId,
    customerName: site.customer.name,
    name: site.name,
    timezone: site.timezone,
    createdAt: site.createdAt,
    updatedAt: site.updatedAt,
    deviceCount: site._count.devices
  });
});

// POST /sites - create site (AGENT_ADMIN or SUPER_ADMIN); site is under a customer
sitesRouter.post('/', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can create sites' });

  const customerId = String(req.body?.customerId ?? '').trim();
  const name = String(req.body?.name ?? '').trim();
  const timezone = req.body?.timezone != null ? String(req.body.timezone).trim() : null;

  if (!customerId) return res.status(400).json({ error: 'customerId is required' });
  if (!name || name.length < 2) return res.status(400).json({ error: 'Site name is required' });

  const customer = await prisma.customer.findFirst({
    where: { id: customerId, organizationId: membership.organizationId }
  });
  if (!customer) return res.status(404).json({ error: 'Customer not found' });

  const site = await prisma.rmmSite.create({
    data: {
      customerId: customer.id,
      name,
      timezone: timezone || null
    },
    include: { customer: { select: { id: true, name: true } } }
  });

  res.status(201).json({
    id: site.id,
    customerId: site.customerId,
    customerName: site.customer.name,
    name: site.name,
    timezone: site.timezone,
    createdAt: site.createdAt,
    updatedAt: site.updatedAt
  });
});

// PATCH /sites/:id - update site (AGENT_ADMIN or SUPER_ADMIN)
sitesRouter.patch('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can edit sites' });

  const site = await prisma.rmmSite.findFirst({
    where: {
      id: req.params.id,
      customer: { organizationId: membership.organizationId }
    },
    include: { customer: { select: { id: true, name: true } } }
  });

  if (!site) return res.status(404).json({ error: 'Site not found' });

  const name = req.body?.name != null ? String(req.body.name).trim() : undefined;
  const timezone = req.body?.timezone !== undefined ? (req.body.timezone == null ? null : String(req.body.timezone).trim()) : undefined;

  if (name !== undefined && name.length < 2) {
    return res.status(400).json({ error: 'Site name is required' });
  }

  const updated = await prisma.rmmSite.update({
    where: { id: site.id },
    data: {
      ...(name !== undefined ? { name } : {}),
      ...(timezone !== undefined ? { timezone } : {})
    },
    include: { customer: { select: { id: true, name: true } } }
  });

  res.json({
    id: updated.id,
    customerId: updated.customerId,
    customerName: updated.customer.name,
    name: updated.name,
    timezone: updated.timezone,
    createdAt: updated.createdAt,
    updatedAt: updated.updatedAt
  });
});

// DELETE /sites/:id - delete site; devices at this site get siteId set to null
sitesRouter.delete('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can delete sites' });

  const site = await prisma.rmmSite.findFirst({
    where: {
      id: req.params.id,
      customer: { organizationId: membership.organizationId }
    }
  });

  if (!site) return res.status(404).json({ error: 'Site not found' });

  await prisma.rmmDevice.updateMany({
    where: { siteId: site.id },
    data: { siteId: null }
  });
  await prisma.rmmSite.delete({ where: { id: site.id } });

  res.status(204).end();
});
