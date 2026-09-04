import express, { Router } from 'express';
import { prisma } from '../lib/prisma';
import { requireAuth, AuthedRequest } from '../middleware/auth';
import { attachRmmServerAuth, RmmServerRequest } from '../middleware/rmmServerKey';
import { auditRequest, writeAuditEvent } from '../lib/audit';

export const policiesRouter = Router();

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

function policyToResponse(policy: any) {
  return {
    ...policy,
    id: policy.id.toString(),
    matchedPolicyId: policy.matchedPolicyId ? policy.matchedPolicyId.toString() : undefined
  };
}

function normalizeCommandName(value: unknown) {
  return String(value || '').trim();
}

function parseScopeType(value: unknown) {
  const scope = String(value || '').trim();
  if (!['organization', 'customer', 'role'].includes(scope)) {
    return null;
  }
  return scope as 'organization' | 'customer' | 'role';
}

function parsePolicyType(value: unknown) {
  const policy = String(value || '').trim();
  if (!['allow', 'deny'].includes(policy)) {
    return null;
  }
  return policy as 'allow' | 'deny';
}

function parseRoleScope(value: unknown) {
  const role = String(value || '').trim();
  if (!role) return null;
  if (!['SUPER_ADMIN', 'AGENT_ADMIN', 'VIEWER'].includes(role)) {
    return null;
  }
  return role as 'SUPER_ADMIN' | 'AGENT_ADMIN' | 'VIEWER';
}

function checkDangerousPatterns(command: string): string | null {
  const patterns: Array<{ regex: RegExp; reason: string }> = [
    { regex: /[;`]/, reason: 'Command chaining not allowed' },
    { regex: /&/, reason: 'Call operator not allowed' },
    { regex: /&&|\|\|/, reason: 'Command operators not allowed' },
    {
      regex: /\|\s*(?!Select-Object|Where-Object|Format-Table|Format-List|Measure-Object|Sort-Object)/i,
      reason: 'Piping to non-whitelisted cmdlets not allowed'
    },
    { regex: />>|>|</, reason: 'File redirection not allowed' },
    { regex: /\$\([^)]*\)/, reason: 'Command substitution not allowed' },
    { regex: /&\s*\(/, reason: 'Subexpression invocation not allowed' },
    { regex: /-EncodedCommand/i, reason: 'Encoded commands not allowed' },
    { regex: /-WindowStyle\s+Hidden/i, reason: 'Hidden execution not allowed' },
    { regex: /-ExecutionPolicy/i, reason: 'Execution policy override not allowed' },
    { regex: /\{[^}]*\}/, reason: 'Script blocks not allowed' }
  ];

  for (const { regex, reason } of patterns) {
    if (regex.test(command)) {
      return reason;
    }
  }
  return null;
}

function extractCommandName(command: string): string | null {
  const trimmed = command.trim();
  if (!trimmed) return null;
  const first = trimmed.split(/\s+/)[0];
  return first || null;
}

function extractAllowedClassNames(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  for (const item of value) {
    if (!item || typeof item !== 'object') continue;
    const name = (item as any).name;
    if (name !== 'ClassName') continue;
    const allowedValues = (item as any).allowed_values;
    if (!Array.isArray(allowedValues)) continue;
    const classes = allowedValues.filter((v) => typeof v === 'string') as string[];
    if (classes.length > 0) return classes;
  }
  return null;
}

function extractWmiClassName(command: string): string | null {
  const paramRe = /-(ClassName|Class)\s+("([^"]+)"|'([^']+)'|([^\s]+))/i;
  const match = paramRe.exec(command);
  if (match) {
    return (match[3] || match[4] || match[5] || '').trim() || null;
  }
  const parts = command.trim().split(/\s+/);
  if (parts.length < 2) return null;
  const candidate = parts[1];
  if (candidate.startsWith('-')) return null;
  return candidate.replace(/^['"]|['"]$/g, '').trim() || null;
}

function validateParameters(
  command: string,
  commandName: string,
  policy: { allowedParameters?: unknown }
): string | null {
  const normalized = commandName.toLowerCase();
  if (normalized !== 'get-ciminstance' && normalized !== 'get-wmiobject') {
    return null;
  }

  const allowed = extractAllowedClassNames(policy.allowedParameters);
  if (!allowed) return null;

  const className = extractWmiClassName(command);
  if (!className) {
    return 'WMI/CIM ClassName is required';
  }

  const match = allowed.find((value) => value.toLowerCase() === className.toLowerCase());
  if (!match) {
    return `WMI/CIM class '${className}' is not allowed`;
  }

  return null;
}

function requireAuthOrRmmServer(
  req: RmmServerRequest & AuthedRequest,
  res: express.Response,
  next: express.NextFunction
) {
  if (req.rmmServer) {
    return next();
  }
  return requireAuth(req, res, next);
}

// GET /policies - list policies for current org + global
policiesRouter.get('/', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const policies = await prisma.commandPolicy.findMany({
    where: {
      OR: [
        { scopeType: 'global' },
        { organizationId: membership.organizationId }
      ]
    },
    orderBy: [
      { scopeType: 'asc' },
      { commandName: 'asc' }
    ]
  });

  const response = policies.map((policy) => ({
    ...policy,
    id: policy.id.toString()
  }));

  res.json(response);
});

// POST /policies - create policy (AGENT_ADMIN or SUPER_ADMIN)
policiesRouter.post('/', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can create policies' });

  const commandName = normalizeCommandName(req.body?.commandName);
  const scopeType = parseScopeType(req.body?.scopeType);
  const policyType = parsePolicyType(req.body?.policyType);
  const customerId = req.body?.customerId ? String(req.body.customerId) : null;
  const roleScope = parseRoleScope(req.body?.roleScope);
  const description = req.body?.description ? String(req.body.description).trim() : null;
  const reason = req.body?.reason ? String(req.body.reason).trim() : null;

  if (!commandName) return res.status(400).json({ error: 'commandName is required' });
  if (!scopeType) return res.status(400).json({ error: 'scopeType is invalid' });
  if (!policyType) return res.status(400).json({ error: 'policyType is invalid' });

  if (scopeType === 'customer') {
    if (!customerId) return res.status(400).json({ error: 'customerId is required for customer scope' });
    const customer = await prisma.customer.findUnique({ where: { id: customerId } });
    if (!customer || customer.organizationId !== membership.organizationId) {
      return res.status(404).json({ error: 'Customer not found' });
    }
  }

  if (scopeType === 'role' && !roleScope) {
    return res.status(400).json({ error: 'roleScope is required for role scope' });
  }

  const policy = await prisma.$transaction(async (tx) => {
    const created = await tx.commandPolicy.create({
      data: {
        commandName,
        scopeType,
        organizationId: membership.organizationId,
        customerId: scopeType === 'customer' ? customerId : null,
        roleScope: scopeType === 'role' ? roleScope : null,
        policyType,
        description: description || null,
        reason: reason || null,
        createdBy: req.jwt!.sub
      }
    });

    await writeAuditEvent(auditRequest(req, {
      organizationId: membership.organizationId,
      customerId: created.customerId,
      actorType: 'user',
      userId: req.jwt!.sub,
      userEmail: membership.user.email,
      actionType: 'policy.create',
      targetType: 'command_policy',
      targetId: created.id.toString(),
      targetName: created.commandName,
      result: 'success',
      metadata: {
        scopeType: created.scopeType,
        roleScope: created.roleScope,
        policyType: created.policyType
      }
    }), tx);

    return created;
  });

  res.status(201).json(policyToResponse(policy));
});

// PATCH /policies/:id - update policy (AGENT_ADMIN or SUPER_ADMIN)
policiesRouter.patch('/:id', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can edit policies' });

  const policy = await prisma.commandPolicy.findUnique({
    where: { id: BigInt(req.params.id) }
  });
  if (!policy || policy.organizationId !== membership.organizationId) {
    return res.status(404).json({ error: 'Policy not found' });
  }
  if (policy.scopeType === 'global') {
    return res.status(403).json({ error: 'Cannot modify global policies' });
  }

  const policyType = req.body?.policyType ? parsePolicyType(req.body.policyType) : null;
  const description = req.body?.description !== undefined ? String(req.body.description).trim() : undefined;
  const reason = req.body?.reason !== undefined ? String(req.body.reason).trim() : undefined;

  if (req.body?.policyType && !policyType) {
    return res.status(400).json({ error: 'policyType is invalid' });
  }

  const updated = await prisma.$transaction(async (tx) => {
    const row = await tx.commandPolicy.update({
      where: { id: policy.id },
      data: {
        ...(policyType ? { policyType } : {}),
        ...(description !== undefined ? { description: description || null } : {}),
        ...(reason !== undefined ? { reason: reason || null } : {})
      }
    });

    await writeAuditEvent(auditRequest(req, {
      organizationId: membership.organizationId,
      customerId: row.customerId,
      actorType: 'user',
      userId: req.jwt!.sub,
      userEmail: membership.user.email,
      actionType: 'policy.update',
      targetType: 'command_policy',
      targetId: row.id.toString(),
      targetName: row.commandName,
      result: 'success',
      metadata: {
        previous: {
          policyType: policy.policyType,
          description: policy.description,
          reason: policy.reason
        },
        next: {
          policyType: row.policyType,
          description: row.description,
          reason: row.reason
        }
      }
    }), tx);

    return row;
  });

  res.json(policyToResponse(updated));
});

// DELETE /policies/:id - delete policy (AGENT_ADMIN or SUPER_ADMIN)
policiesRouter.delete('/:id', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });
  if (!isAgentAdmin(membership.role)) return res.status(403).json({ error: 'Only admins can delete policies' });

  const policy = await prisma.commandPolicy.findUnique({
    where: { id: BigInt(req.params.id) }
  });
  if (!policy || policy.organizationId !== membership.organizationId) {
    return res.status(404).json({ error: 'Policy not found' });
  }
  if (policy.scopeType === 'global') {
    return res.status(403).json({ error: 'Cannot delete global policies' });
  }

  await prisma.$transaction(async (tx) => {
    await tx.commandPolicy.delete({ where: { id: policy.id } });
    await writeAuditEvent(auditRequest(req, {
      organizationId: membership.organizationId,
      customerId: policy.customerId,
      actorType: 'user',
      userId: req.jwt!.sub,
      userEmail: membership.user.email,
      actionType: 'policy.delete',
      targetType: 'command_policy',
      targetId: policy.id.toString(),
      targetName: policy.commandName,
      result: 'success',
      metadata: {
        scopeType: policy.scopeType,
        roleScope: policy.roleScope,
        policyType: policy.policyType
      }
    }), tx);
  });
  res.status(204).end();
});

// POST /policies/validate - validate command execution against policies
policiesRouter.post(
  '/validate',
  attachRmmServerAuth,
  requireAuthOrRmmServer,
  async (req: RmmServerRequest & AuthedRequest, res) => {
    const command = String(req.body?.command || '').trim();
    if (!command) {
      return res.status(400).json({ error: 'command is required' });
    }

    let organizationId: string | null = null;
    let role: string | null = null;
    let customerId: string | null = req.body?.customerId ? String(req.body.customerId) : null;

    if (req.rmmServer) {
      organizationId = req.body?.organizationId ? String(req.body.organizationId) : null;
      role = req.body?.role ? String(req.body.role) : null;
      if (!organizationId || !role) {
        return res.status(400).json({ error: 'organizationId and role are required' });
      }
    } else {
      if (!assertUser(req, res)) return;
      const membership = await getCurrentMembership(req.jwt!.sub);
      if (!membership) {
        return res.status(404).json({ error: 'No organization', needsOnboarding: true });
      }
      organizationId = membership.organizationId;
      role = membership.role;
    }

    if (customerId) {
      if (!organizationId) {
        return res.status(400).json({ error: 'organizationId is required' });
      }
      const customer = await prisma.customer.findFirst({
        where: { id: customerId, organizationId }
      });
      if (!customer) {
        return res.status(404).json({ error: 'Customer not found' });
      }
    }

    const dangerousReason = checkDangerousPatterns(command);
    if (dangerousReason) {
      return res.json({ allowed: false, reason: dangerousReason, matchedPolicyId: null });
    }

    const commandName = extractCommandName(command);
    if (!commandName) {
      return res.status(400).json({ error: 'Command is empty' });
    }

    const scopeFilters: Array<
      | { scopeType: 'global' }
      | { scopeType: 'organization'; organizationId: string }
      | { scopeType: 'role'; organizationId: string; roleScope: string | null }
      | { scopeType: 'customer'; customerId: string }
    > = [{ scopeType: 'global' }];
    if (organizationId) {
      scopeFilters.push({ scopeType: 'organization', organizationId });
      scopeFilters.push({ scopeType: 'role', organizationId, roleScope: role });
    }
    if (customerId) {
      scopeFilters.push({ scopeType: 'customer', customerId });
    }

    const policies = await prisma.commandPolicy.findMany({
      where: {
        commandName: {
          equals: commandName,
          mode: 'insensitive'
        },
        OR: scopeFilters
      }
    });

    const scopeOrder: Record<string, number> = {
      role: 1,
      customer: 2,
      organization: 3,
      global: 4
    };
    policies.sort((a, b) => (scopeOrder[a.scopeType] ?? 99) - (scopeOrder[b.scopeType] ?? 99));

    for (const policy of policies) {
      if (policy.policyType === 'deny') {
        return res.json({
          allowed: false,
          reason: `Denied by ${policy.scopeType} policy: ${policy.reason || 'No reason provided'}`,
          matchedPolicyId: policy.id.toString()
        });
      }
    }

    for (const policy of policies) {
      if (policy.policyType === 'allow') {
        const validationReason = validateParameters(command, commandName, policy);
        if (validationReason) {
          return res.json({
            allowed: false,
            reason: validationReason,
            matchedPolicyId: policy.id.toString()
          });
        }

        return res.json({
          allowed: true,
          reason: `Allowed by ${policy.scopeType} policy`,
          matchedPolicyId: policy.id.toString()
        });
      }
    }

    return res.json({
      allowed: false,
      reason: 'Command not in allowlist',
      matchedPolicyId: null
    });
  }
);
