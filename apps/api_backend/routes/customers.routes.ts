import { Router } from 'express';
import { v5 as uuidV5 } from 'uuid';
import { prisma } from '../lib/prisma';
import { requireAuth, AuthedRequest } from '../middleware/auth';

/** Namespace for deterministic Unassigned customer IDs (must match talos_server). Each org gets a unique GUID. */
const UNASSIGNED_CUSTOMER_NAMESPACE = 'a7c9e2d1-4b3f-4a8e-9e1d-2c5b6a7d8e9f';

/** Default GUID for an organization's Unassigned customer (special type of customer). Unique per organization. */
export function unassignedCustomerId(organizationId: string): string {
  return uuidV5(organizationId, UNASSIGNED_CUSTOMER_NAMESPACE);
}

export const customersRouter = Router();
customersRouter.use(requireAuth);

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

async function getOrCreateUnassigned(organizationId: string) {
  const id = unassignedCustomerId(organizationId);
  const existing = await prisma.customer.findUnique({
    where: { id }
  });
  if (existing) return existing;

  return prisma.customer.create({
    data: {
      id,
      organizationId,
      name: 'Unassigned',
      description: 'Default holding customer for unassigned devices.',
      isUnassigned: true
    }
  });
}

// GET /customers - list customers for current org
customersRouter.get('/', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  await getOrCreateUnassigned(membership.organizationId);

  const customers = await prisma.customer.findMany({
    where: { organizationId: membership.organizationId },
    include: { _count: { select: { devices: true } } },
    orderBy: [{ isUnassigned: 'desc' }, { name: 'asc' }]
  });

  res.json(customers.map((customer) => ({
    id: customer.id,
    organizationId: customer.organizationId,
    name: customer.name,
    description: customer.description,
    isUnassigned: customer.isUnassigned,
    createdAt: customer.createdAt,
    updatedAt: customer.updatedAt,
    deviceCount: customer._count.devices
  })));
});

// GET /customers/:id - get single customer for current org
customersRouter.get('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const customer = await prisma.customer.findUnique({
    where: { id: req.params.id },
    include: { _count: { select: { devices: true } } }
  });

  if (!customer || customer.organizationId !== membership.organizationId) {
    return res.status(404).json({ error: 'Customer not found' });
  }

  res.json({
    id: customer.id,
    organizationId: customer.organizationId,
    name: customer.name,
    description: customer.description,
    isUnassigned: customer.isUnassigned,
    createdAt: customer.createdAt,
    updatedAt: customer.updatedAt,
    deviceCount: customer._count.devices
  });
});

// POST /customers - create customer (AGENT_ADMIN or SUPER_ADMIN)
customersRouter.post('/', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can create customers' });

  const name = String(req.body?.name || '').trim();
  const description = req.body?.description ? String(req.body.description).trim() : null;
  if (!name || name.length < 2) return res.status(400).json({ error: 'Customer name is required' });

  const customer = await prisma.customer.create({
    data: {
      organizationId: membership.organizationId,
      name,
      description: description || null
    }
  });

  res.status(201).json(customer);
});

// PATCH /customers/:id - update customer (AGENT_ADMIN or SUPER_ADMIN)
customersRouter.patch('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can edit customers' });

  const customer = await prisma.customer.findUnique({ where: { id: req.params.id } });
  if (!customer || customer.organizationId !== membership.organizationId) {
    return res.status(404).json({ error: 'Customer not found' });
  }
  if (customer.isUnassigned) {
    return res.status(400).json({ error: 'Unassigned customer cannot be edited' });
  }

  const name = req.body?.name ? String(req.body.name).trim() : undefined;
  const description = req.body?.description !== undefined ? String(req.body.description).trim() : undefined;

  if (name !== undefined && name.length < 2) {
    return res.status(400).json({ error: 'Customer name is required' });
  }

  const updated = await prisma.customer.update({
    where: { id: customer.id },
    data: {
      ...(name !== undefined ? { name } : {}),
      ...(description !== undefined ? { description: description || null } : {})
    }
  });

  res.json(updated);
});

// DELETE /customers/:id - delete customer (AGENT_ADMIN or SUPER_ADMIN)
customersRouter.delete('/:id', async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can delete customers' });

  const customer = await prisma.customer.findUnique({ where: { id: req.params.id } });
  if (!customer || customer.organizationId !== membership.organizationId) {
    return res.status(404).json({ error: 'Customer not found' });
  }
  if (customer.isUnassigned) {
    return res.status(400).json({ error: 'Unassigned customer cannot be deleted' });
  }

  const unassigned = await getOrCreateUnassigned(membership.organizationId);
  await prisma.rmmDevice.updateMany({
    where: { customerId: customer.id },
    data: { customerId: unassigned.id }
  });

  await prisma.customer.delete({ where: { id: customer.id } });
  res.status(204).end();
});
