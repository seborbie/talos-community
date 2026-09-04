import { afterAll, beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import express from 'express';
import jwt, { type SignOptions } from 'jsonwebtoken';

process.env.JWT_SECRET ||= 'authorization-regression-test-secret';
process.env.TOKEN_TTL ||= '1h';
process.env.MACHINE_TOKEN_TTL ||= '30d';
process.env.RMM_SERVER_API_KEY ||= 'test-rmm-key';
process.env.RMM_SERVER_HTTP_URL = 'http://replica-local-cache.test';

type Role = 'SUPER_ADMIN' | 'AGENT_ADMIN' | 'VIEWER';
type OrgRecord = { id: string; name: string; createdAt: Date };
type UserRecord = { id: string; email: string; organizationId: string; role: Role };
type CustomerRecord = {
  id: string;
  organizationId: string;
  name: string;
  description: string | null;
  isUnassigned: boolean;
  createdAt: Date;
  updatedAt: Date;
};
type SiteRecord = {
  id: string;
  customerId: string;
  name: string;
  timezone: string | null;
  createdAt: Date;
  updatedAt: Date;
};
type DeviceRecord = {
  agentId: string;
  organizationId: string;
  hostname: string;
  os: string;
  ip: string;
  version: string | null;
  lastSeen: Date;
  customerId: string | null;
  siteId: string | null;
  aiRunnerAutoApprove: boolean;
};
type PolicyRecord = {
  id: bigint;
  commandName: string;
  scopeType: string;
  organizationId: string | null;
  customerId: string | null;
  roleScope: string | null;
  policyType: string;
  description: string | null;
  reason: string | null;
  createdBy: string | null;
};
type InstallerProfileRecord = {
  id: string;
  name: string;
  scopeType: 'ORGANIZATION' | 'CUSTOMER' | 'SITE';
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  expiresAt: Date | null;
  maxUses: number | null;
  revokedAt: Date | null;
  createdAt: Date;
  updatedAt: Date;
};

const now = new Date('2026-05-11T01:00:00.000Z');
const nativeFetch = globalThis.fetch;

let orgs: OrgRecord[];
let users: UserRecord[];
let customers: CustomerRecord[];
let sites: SiteRecord[];
let devices: DeviceRecord[];
let policies: PolicyRecord[];
let installerProfiles: InstallerProfileRecord[];
let snapshotRequests: Array<{ agentId: string; requestId: string; organizationId: string; status: string }>;
let calls: {
  commandPolicyFindMany: unknown[];
  rmmDeviceUpdateMany: unknown[];
  commandExecutionLogCreate: unknown[];
  queryRaw: unknown[];
  auditEventCreate: unknown[];
  liveProgressFetches: string[];
};

function resetState() {
  orgs = [
    { id: 'org-a', name: 'Org A', createdAt: now },
    { id: 'org-b', name: 'Org B', createdAt: now }
  ];
  users = [
    { id: 'super-a', email: 'super-a@example.test', organizationId: 'org-a', role: 'SUPER_ADMIN' },
    { id: 'admin-a', email: 'admin-a@example.test', organizationId: 'org-a', role: 'AGENT_ADMIN' },
    { id: 'viewer-a', email: 'viewer-a@example.test', organizationId: 'org-a', role: 'VIEWER' },
    { id: 'super-b', email: 'super-b@example.test', organizationId: 'org-b', role: 'SUPER_ADMIN' }
  ];
  customers = [
    { id: 'customer-a', organizationId: 'org-a', name: 'Customer A', description: null, isUnassigned: false, createdAt: now, updatedAt: now },
    { id: 'customer-a2', organizationId: 'org-a', name: 'Customer A2', description: null, isUnassigned: false, createdAt: now, updatedAt: now },
    { id: 'customer-b', organizationId: 'org-b', name: 'Customer B', description: null, isUnassigned: false, createdAt: now, updatedAt: now }
  ];
  sites = [
    { id: 'site-a', customerId: 'customer-a', name: 'Site A', timezone: null, createdAt: now, updatedAt: now },
    { id: 'site-a2', customerId: 'customer-a2', name: 'Site A2', timezone: null, createdAt: now, updatedAt: now },
    { id: 'site-b', customerId: 'customer-b', name: 'Site B', timezone: null, createdAt: now, updatedAt: now }
  ];
  devices = [
    { agentId: 'agent-a', organizationId: 'org-a', hostname: 'agent-a', os: 'Windows', ip: '10.0.0.10', version: '1.0.0', lastSeen: now, customerId: 'customer-a', siteId: 'site-a', aiRunnerAutoApprove: false },
    { agentId: 'agent-b', organizationId: 'org-b', hostname: 'agent-b', os: 'Windows', ip: '10.0.0.20', version: '1.0.0', lastSeen: now, customerId: 'customer-b', siteId: 'site-b', aiRunnerAutoApprove: false }
  ];
  policies = [
    { id: 1n, commandName: 'Get-Service', scopeType: 'organization', organizationId: 'org-a', customerId: null, roleScope: null, policyType: 'allow', description: null, reason: null, createdBy: 'super-a' },
    { id: 2n, commandName: 'Get-Process', scopeType: 'customer', organizationId: 'org-b', customerId: 'customer-b', roleScope: null, policyType: 'allow', description: null, reason: null, createdBy: 'super-b' }
  ];
  installerProfiles = [
    { id: 'profile-a', name: 'Profile A', scopeType: 'ORGANIZATION', organizationId: 'org-a', customerId: null, siteId: null, expiresAt: null, maxUses: null, revokedAt: null, createdAt: now, updatedAt: now },
    { id: 'profile-b', name: 'Profile B', scopeType: 'ORGANIZATION', organizationId: 'org-b', customerId: null, siteId: null, expiresAt: null, maxUses: null, revokedAt: null, createdAt: now, updatedAt: now }
  ];
  snapshotRequests = [
    { agentId: 'agent-a', requestId: 'request-a', organizationId: 'org-a', status: 'pending' },
    { agentId: 'agent-a', requestId: 'request-b', organizationId: 'org-b', status: 'completed' }
  ];
  calls = {
    commandPolicyFindMany: [],
    rmmDeviceUpdateMany: [],
    commandExecutionLogCreate: [],
    queryRaw: [],
    auditEventCreate: [],
    liveProgressFetches: []
  };
}

function pick<T extends Record<string, unknown>>(row: T, select?: Record<string, boolean>): Partial<T> {
  if (!select) return row;
  const selected: Partial<T> = {};
  for (const [key, enabled] of Object.entries(select)) {
    if (enabled) selected[key as keyof T] = row[key as keyof T];
  }
  return selected;
}

function organizationFor(id: string) {
  return orgs.find((org) => org.id === id) || null;
}

function customerFor(id: string | null | undefined) {
  return customers.find((customer) => customer.id === id) || null;
}

function siteFor(id: string | null | undefined) {
  return sites.find((site) => site.id === id) || null;
}

function siteOrganizationId(site: SiteRecord) {
  return customerFor(site.customerId)?.organizationId ?? null;
}

function membershipFor(userId: string) {
  const user = users.find((candidate) => candidate.id === userId);
  if (!user) return null;
  const organization = organizationFor(user.organizationId);
  return {
    id: `membership-${user.id}`,
    userId: user.id,
    organizationId: user.organizationId,
    role: user.role,
    createdAt: now,
    organization,
    user: { id: user.id, email: user.email }
  };
}

function customerMatchesWhere(customer: CustomerRecord, where: any) {
  if (where?.id !== undefined && customer.id !== where.id) return false;
  if (where?.organizationId !== undefined && customer.organizationId !== where.organizationId) return false;
  return true;
}

function siteMatchesWhere(site: SiteRecord, where: any) {
  if (where?.id !== undefined && site.id !== where.id) return false;
  if (where?.customerId !== undefined && site.customerId !== where.customerId) return false;
  if (where?.customer?.organizationId !== undefined && siteOrganizationId(site) !== where.customer.organizationId) return false;
  if (where?.customer?.id !== undefined && site.customerId !== where.customer.id) return false;
  return true;
}

function deviceMatchesWhere(device: DeviceRecord, where: any) {
  if (where?.agentId !== undefined) {
    if (typeof where.agentId === 'string' && device.agentId !== where.agentId) return false;
    if (where.agentId?.in && !where.agentId.in.includes(device.agentId)) return false;
  }
  if (where?.organizationId !== undefined && device.organizationId !== where.organizationId) return false;
  if (where?.customer?.organizationId !== undefined && device.organizationId !== where.customer.organizationId) return false;
  return true;
}

function policyMatchesWhere(policy: PolicyRecord, where: any) {
  if (where?.id !== undefined && policy.id !== where.id) return false;
  if (where?.organizationId !== undefined && policy.organizationId !== where.organizationId) return false;
  if (where?.commandName?.equals && policy.commandName.toLowerCase() !== String(where.commandName.equals).toLowerCase()) return false;
  if (Array.isArray(where?.OR)) {
    return where.OR.some((clause: any) => policyMatchesWhere(policy, clause));
  }
  if (where?.scopeType !== undefined && policy.scopeType !== where.scopeType) return false;
  if (where?.customerId !== undefined && policy.customerId !== where.customerId) return false;
  if (where?.roleScope !== undefined && policy.roleScope !== where.roleScope) return false;
  return true;
}

function profileWithRelations(profile: InstallerProfileRecord) {
  const customer = customerFor(profile.customerId);
  const site = siteFor(profile.siteId);
  return {
    ...profile,
    customer: customer ? { id: customer.id, name: customer.name } : null,
    site: site ? { id: site.id, name: site.name } : null,
    tokens: []
  };
}

function sqlText(query: any) {
  if (typeof query?.sql === 'string') return query.sql;
  if (Array.isArray(query?.strings)) return query.strings.join('?');
  return String(query);
}

function signUser(userId: string) {
  return jwt.sign(
    { sub: userId, type: 'user' },
    process.env.JWT_SECRET as string,
    { expiresIn: process.env.TOKEN_TTL ?? '1h' } as SignOptions
  );
}

function decodeTestBearer(token: string) {
  if (token.startsWith('machine:')) {
    return { sub: token.slice('machine:'.length), type: 'machine' as const };
  }

  try {
    const decoded = jwt.verify(token, process.env.JWT_SECRET as string) as {
      sub?: unknown;
      type?: unknown;
    };
    if (typeof decoded.sub === 'string') {
      return {
        sub: decoded.sub,
        type: decoded.type === 'machine' ? ('machine' as const) : ('user' as const)
      };
    }
  } catch {
    // Keep the mock compatible with literal test user ids.
  }

  return { sub: token, type: 'user' as const };
}

const prisma: any = {
  organizationMember: {
    findFirst: async ({ where }: any) => membershipFor(where.userId)
  },
  customer: {
    findUnique: async ({ where, include, select }: any) => {
      const customer = customers.find((candidate) => customerMatchesWhere(candidate, where));
      if (!customer) return null;
      const row = include?._count ? { ...customer, _count: { devices: devices.filter((device) => device.customerId === customer.id).length } } : customer;
      return pick(row, select);
    },
    findFirst: async ({ where, select }: any) => {
      const customer = customers.find((candidate) => customerMatchesWhere(candidate, where));
      return customer ? pick(customer, select) : null;
    },
    findMany: async ({ where }: any) => customers.filter((customer) => customerMatchesWhere(customer, where)),
    create: async ({ data }: any) => {
      const customer = {
        id: data.id || `customer-${customers.length + 1}`,
        organizationId: data.organizationId,
        name: data.name,
        description: data.description ?? null,
        isUnassigned: Boolean(data.isUnassigned),
        createdAt: now,
        updatedAt: now
      };
      customers.push(customer);
      return customer;
    },
    update: async ({ where, data }: any) => {
      const customer = customers.find((candidate) => candidate.id === where.id);
      if (!customer) throw new Error('customer not found');
      Object.assign(customer, data, { updatedAt: now });
      return customer;
    },
    delete: async ({ where }: any) => {
      const index = customers.findIndex((customer) => customer.id === where.id);
      return customers.splice(index, 1)[0] ?? null;
    }
  },
  rmmSite: {
    findFirst: async ({ where, include, select }: any) => {
      const site = sites.find((candidate) => siteMatchesWhere(candidate, where));
      if (!site) return null;
      const customer = customerFor(site.customerId);
      const row = {
        ...site,
        customer: customer ? { id: customer.id, name: customer.name, organizationId: customer.organizationId } : null,
        _count: { devices: devices.filter((device) => device.siteId === site.id).length }
      };
      if (include?.customer === true) return row;
      if (include?.customer?.select) row.customer = pick(row.customer as any, include.customer.select) as any;
      return pick(row, select);
    },
    findMany: async ({ where }: any) => sites.filter((site) => siteMatchesWhere(site, where)),
    create: async ({ data, include }: any) => {
      const site = {
        id: `site-${sites.length + 1}`,
        customerId: data.customerId,
        name: data.name,
        timezone: data.timezone ?? null,
        createdAt: now,
        updatedAt: now
      };
      sites.push(site);
      const customer = customerFor(site.customerId);
      return include?.customer ? { ...site, customer: { id: customer!.id, name: customer!.name } } : site;
    },
    update: async ({ where, data, include }: any) => {
      const site = sites.find((candidate) => candidate.id === where.id);
      if (!site) throw new Error('site not found');
      Object.assign(site, data, { updatedAt: now });
      const customer = customerFor(site.customerId);
      return include?.customer ? { ...site, customer: { id: customer!.id, name: customer!.name } } : site;
    },
    delete: async ({ where }: any) => {
      const index = sites.findIndex((site) => site.id === where.id);
      return sites.splice(index, 1)[0] ?? null;
    }
  },
  rmmDevice: {
    findFirst: async ({ where, include, select }: any) => {
      const device = devices.find((candidate) => deviceMatchesWhere(candidate, where));
      if (!device) return null;
      const row = {
        ...device,
        customer: customerFor(device.customerId),
        site: siteFor(device.siteId)
      };
      if (!include?.customer) row.customer = null;
      if (!include?.site) row.site = null;
      return pick(row, select);
    },
    findUnique: async ({ where, select }: any) => {
      const device = devices.find((candidate) => candidate.agentId === where.agentId);
      return device ? pick(device, select) : null;
    },
    findMany: async ({ where, take }: any) => devices.filter((device) => deviceMatchesWhere(device, where)).slice(0, take ?? devices.length),
    update: async ({ where, data, include, select }: any) => {
      const device = devices.find((candidate) => candidate.agentId === where.agentId);
      if (!device) throw new Error('device not found');
      Object.assign(device, data);
      const row = {
        ...device,
        customer: customerFor(device.customerId),
        site: siteFor(device.siteId)
      };
      if (!include?.customer) row.customer = null;
      if (!include?.site) row.site = null;
      return pick(row, select);
    },
    deleteMany: async ({ where }: any) => {
      const matches = devices.filter((device) => deviceMatchesWhere(device, where));
      devices = devices.filter((device) => !matches.includes(device));
      return { count: matches.length };
    },
    updateMany: async (args: any) => {
      calls.rmmDeviceUpdateMany.push(args);
      const matches = devices.filter((device) => deviceMatchesWhere(device, args.where));
      for (const device of matches) Object.assign(device, args.data);
      return { count: matches.length };
    }
  },
  rmmTelemetryDeviceState: {
    findUnique: async () => null
  },
  commandExecutionLog: {
    findMany: async () => [],
    create: async (args: any) => {
      calls.commandExecutionLogCreate.push(args);
      return { id: 1n };
    }
  },
  user: {
    findMany: async () => []
  },
  commandPolicy: {
    findMany: async (args: any) => {
      calls.commandPolicyFindMany.push(args);
      return policies.filter((policy) => policyMatchesWhere(policy, args.where));
    },
    findUnique: async ({ where }: any) => policies.find((policy) => policy.id === where.id) ?? null,
    create: async ({ data }: any) => {
      const policy = { id: BigInt(policies.length + 1), ...data };
      policies.push(policy);
      return policy;
    },
    update: async ({ where, data }: any) => {
      const policy = policies.find((candidate) => candidate.id === where.id);
      if (!policy) throw new Error('policy not found');
      Object.assign(policy, data);
      return policy;
    },
    delete: async ({ where }: any) => {
      const index = policies.findIndex((policy) => policy.id === where.id);
      return policies.splice(index, 1)[0] ?? null;
    }
  },
  rmmInstallerProfile: {
    findMany: async ({ where }: any) => installerProfiles.filter((profile) => {
      if (where.organizationId && profile.organizationId !== where.organizationId) return false;
      if (where.scopeType && profile.scopeType !== where.scopeType) return false;
      if (where.customerId && profile.customerId !== where.customerId) return false;
      if (where.siteId && profile.siteId !== where.siteId) return false;
      return true;
    }).map(profileWithRelations),
    findFirst: async ({ where }: any) => {
      const profile = installerProfiles.find((candidate) => {
        if (where.id && candidate.id !== where.id) return false;
        if (where.organizationId && candidate.organizationId !== where.organizationId) return false;
        return true;
      });
      return profile ? profileWithRelations(profile) : null;
    },
    create: async ({ data }: any) => {
      const profile = {
        id: `profile-${installerProfiles.length + 1}`,
        name: data.name,
        scopeType: data.scopeType,
        organizationId: data.organizationId,
        customerId: data.customerId,
        siteId: data.siteId,
        expiresAt: data.expiresAt ?? null,
        maxUses: data.maxUses ?? null,
        revokedAt: null,
        createdAt: now,
        updatedAt: now
      };
      installerProfiles.push(profile);
      return profileWithRelations(profile);
    },
    update: async ({ where, data }: any) => {
      const profile = installerProfiles.find((candidate) => candidate.id === where.id);
      if (!profile) throw new Error('profile not found');
      Object.assign(profile, data, { updatedAt: now });
      return profileWithRelations(profile);
    }
  },
  rmmInstallerEnrollmentToken: {
    create: async () => ({ id: 'token-1', tokenPrefix: 'prefix', expiresAt: null, maxUses: null, usedCount: 0, revokedAt: null, createdAt: now, lastUsedAt: null }),
    updateMany: async () => ({ count: 1 })
  },
  rmmInstallerDownloadAudit: {
    create: async () => ({ id: 1n })
  },
  auditEvent: {
    create: async (args: any) => {
      calls.auditEventCreate.push(args);
      return { id: BigInt(calls.auditEventCreate.length), ...args.data };
    }
  },
  rmmTelemetrySnapshotRequest: {
    findFirst: async ({ where }: any) => snapshotRequests.find((request) =>
      request.agentId === where.agentId &&
      request.requestId === where.requestId &&
      request.organizationId === where.organizationId
    ) ?? null
  },
  $transaction: async (work: any) => {
    if (typeof work === 'function') return work(prisma);
    return Promise.all(work);
  },
  $queryRaw: async (query: any) => {
    calls.queryRaw.push(query);
    const text = sqlText(query);
    if (text.includes('FROM rmm_telemetry.device_event') && text.includes('event_id')) {
      expect(text).toContain('organization_id');
      expect(query.values).toContain('org-a');
      return [{
        event_id: 'event-a',
        occurred_at: now,
        received_at: now,
        event_type: 'service',
        severity: 'info',
        source: 'agent',
        service_name: null,
        process_name: null,
        code: null,
        message: 'ok',
        attributes_jsonb: {},
        created_at: now
      }];
    }
    if (text.includes('FROM public.feature_upgrade_iso_stage_device s')) {
      return [{
        operationId: 'stage-operation',
        runId: 'stage-run',
        organizationId: 'org-a',
        agentId: 'agent-a',
        hostname: 'agent-a',
        isoMediaId: 'iso-1',
        isoDisplayName: 'Windows 11 ISO',
        sourceOs: 'Windows 10',
        targetProduct: 'Windows 11',
        targetVersion: '24H2',
        targetBuildLabel: '26100',
        status: 'running',
        phase: 'downloading',
        progress: {
          reportedAt: now.toISOString(),
          overallPercent: 37,
          phasePercent: 42,
          bytesDownloaded: 370,
          bytesTotal: 1000
        },
        evidence: { source: 'postgres' },
        errorMessage: null,
        sizeBytes: 1000n,
        sha256: 'stage-sha256',
        requestedBy: 'admin-a',
        claimedAt: now,
        startedAt: now,
        stagedAt: null,
        expiresAt: null,
        cleanedAt: null,
        finishedAt: null,
        createdAt: now,
        updatedAt: now
      }];
    }
    if (text.includes('FROM public.feature_upgrade_device u')) {
      return [{
        operationId: 'start-operation',
        runId: 'start-run',
        organizationId: 'org-a',
        agentId: 'agent-a',
        hostname: 'agent-a',
        preflightOperationId: 'preflight-operation',
        isoMediaId: 'iso-1',
        isoDisplayName: 'Windows 11 ISO',
        setupCommandMatrixId: 'setup-matrix-1',
        sourceOs: 'Windows 10',
        targetProduct: 'Windows 11',
        targetVersion: '24H2',
        targetBuildLabel: '26100',
        status: 'running',
        phase: 'setup',
        progress: {
          reportedAt: now.toISOString(),
          overallPercent: 48,
          phasePercent: 51
        },
        evidence: { source: 'postgres' },
        failureSummary: [],
        errorMessage: null,
        sizeBytes: 1000n,
        sha256: 'start-sha256',
        scheduledFor: null,
        requestedBy: 'admin-a',
        claimedAt: now,
        startedAt: now,
        finalSnapshotAt: null,
        setupStartedAt: now,
        rebootDetectedAt: null,
        verifiedAt: null,
        finishedAt: null,
        createdAt: now,
        updatedAt: now
      }];
    }
    return [];
  },
  $executeRaw: async () => 0
};

mock.module('../lib/prisma', () => ({ prisma }));
mock.module('../middleware/auth', () => ({
  requireAuth(req: any, res: any, next: any) {
    const header = req.header('authorization') || '';
    if (!header.startsWith('Bearer ')) {
      return res.status(401).json({ error: 'Missing Bearer token' });
    }
    const token = header.slice(7);
    req.jwt = decodeTestBearer(token);
    return next();
  }
}));
mock.module('../middleware/rmmServerKey', () => ({
  attachRmmServerAuth(req: any, _res: any, next: any) {
    if (req.header('x-rmm-server-key') === 'test-rmm-key') {
      req.rmmServer = { authenticated: true };
    }
    next();
  },
  requireRmmServer(req: any, res: any, next: any) {
    if (req.header('x-rmm-server-key') !== 'test-rmm-key') {
      return res.status(401).json({ error: 'Unauthorized' });
    }
    req.rmmServer = { authenticated: true };
    return next();
  }
}));

let makeApp: () => express.Express;

beforeAll(async () => {
  const [
    { customersRouter },
    { sitesRouter },
    { policiesRouter },
    { installersRouter },
    { rmmRouter },
    { rmmTelemetryRouter },
    { featureUpgradesRouter }
  ] = await Promise.all([
    import('../routes/customers.routes'),
    import('../routes/sites.routes'),
    import('../routes/policies.routes'),
    import('../routes/installers.routes'),
    import('../routes/rmm.routes'),
    import('../routes/rmmTelemetry.routes'),
    import('../routes/featureUpgrades.routes')
  ]);

  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (url.startsWith('http://replica-local-cache.test/')) {
      calls.liveProgressFetches.push(url);
      const operationId = url.includes('/stage-iso/') ? 'stage-operation' : 'start-operation';
      return new Response(JSON.stringify({
        items: [{
          operationId,
          status: 'failed',
          phase: 'replica-local-cache',
          overallPercent: 99
        }]
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' }
      });
    }
    return nativeFetch(input, init);
  };

  makeApp = () => {
    const app = express();
    app.use(express.json());
    app.use('/customers', customersRouter);
    app.use('/sites', sitesRouter);
    app.use('/policies', policiesRouter);
    app.use('/rmm/installers', installersRouter);
    app.use('/rmm/telemetry', rmmTelemetryRouter);
    app.use('/rmm/feature-upgrades', featureUpgradesRouter);
    app.use('/rmm', rmmRouter);
    app.use((err: any, _req: any, res: any, _next: any) => {
      res.status(err.status || 500).json({ error: err.message || 'Internal server error' });
    });
    return app;
  };
});

afterAll(() => {
  globalThis.fetch = nativeFetch;
});

beforeEach(resetState);

async function request(
  method: string,
  path: string,
  options: { user?: string; rmmServer?: boolean; body?: unknown } = {}
) {
  const app = makeApp();
  const server = app.listen(0);
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Failed to bind test server');
  }
  try {
    const headers: Record<string, string> = {};
    if (options.user) headers.authorization = `Bearer ${signUser(options.user)}`;
    if (options.rmmServer) headers['x-rmm-server-key'] = 'test-rmm-key';
    if (options.body !== undefined) headers['content-type'] = 'application/json';

    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method,
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body)
    });
    const text = await response.text();
    const body = text ? JSON.parse(text) : null;
    return { status: response.status, body };
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
}

describe('organization role boundaries', () => {
  test('SUPER_ADMIN and AGENT_ADMIN can create customers, VIEWER cannot', async () => {
    const superResponse = await request('POST', '/customers', {
      user: 'super-a',
      body: { name: 'Created by super' }
    });
    expect(superResponse.status).toBe(201);
    expect(superResponse.body.organizationId).toBe('org-a');

    const adminResponse = await request('POST', '/customers', {
      user: 'admin-a',
      body: { name: 'Created by admin' }
    });
    expect(adminResponse.status).toBe(201);
    expect(adminResponse.body.organizationId).toBe('org-a');

    const viewerResponse = await request('POST', '/customers', {
      user: 'viewer-a',
      body: { name: 'Created by viewer' }
    });
    expect(viewerResponse.status).toBe(403);
  });

  test('VIEWER cannot mutate devices', async () => {
    const response = await request('DELETE', '/rmm/devices/agent-a', { user: 'viewer-a' });

    expect(response.status).toBe(403);
    expect(devices.some((device) => device.agentId === 'agent-a')).toBe(true);
  });

  test('AGENT_ADMIN can update per-device AI endpoint auto-approval', async () => {
    const response = await request('PATCH', '/rmm/devices/agent-a/settings', {
      user: 'admin-a',
      body: { aiRunnerAutoApprove: true }
    });

    expect(response.status).toBe(200);
    expect(response.body.aiRunnerAutoApprove).toBe(true);
    expect(devices.find((device) => device.agentId === 'agent-a')?.aiRunnerAutoApprove).toBe(true);
    expect(calls.auditEventCreate).toHaveLength(1);
    expect((calls.auditEventCreate[0] as any).data).toMatchObject({
      organizationId: 'org-a',
      agentId: 'agent-a',
      actionType: 'device.settings.update',
      metadata: {
        aiRunnerAutoApprove: {
          previous: false,
          next: true
        }
      }
    });
  });

  test('VIEWER cannot update per-device AI endpoint auto-approval', async () => {
    const response = await request('PATCH', '/rmm/devices/agent-a/settings', {
      user: 'viewer-a',
      body: { aiRunnerAutoApprove: true }
    });

    expect(response.status).toBe(403);
    expect(devices.find((device) => device.agentId === 'agent-a')?.aiRunnerAutoApprove).toBe(false);
  });

  test('device settings reject invalid auto-approval payloads', async () => {
    const response = await request('PATCH', '/rmm/devices/agent-a/settings', {
      user: 'admin-a',
      body: { aiRunnerAutoApprove: 'true' }
    });

    expect(response.status).toBe(400);
    expect(devices.find((device) => device.agentId === 'agent-a')?.aiRunnerAutoApprove).toBe(false);
  });
});

describe('tenant data boundaries', () => {
  test('guessed customer, site, device, telemetry, installer, and audit ids from another org return 404', async () => {
    expect((await request('GET', '/customers/customer-b', { user: 'viewer-a' })).status).toBe(404);
    expect((await request('GET', '/sites/site-b', { user: 'viewer-a' })).status).toBe(404);
    expect((await request('GET', '/rmm/devices/agent-b', { user: 'viewer-a' })).status).toBe(404);
    expect((await request('GET', '/rmm/telemetry/read/events/agent-b', { user: 'viewer-a' })).status).toBe(404);
    expect((await request('POST', '/rmm/installers/profiles/profile-b/download', { user: 'admin-a', body: {} })).status).toBe(404);
    expect((await request('GET', '/rmm/devices/agent-b/command-log', { user: 'viewer-a' })).status).toBe(404);
    expect((await request('PATCH', '/rmm/devices/agent-b/settings', { user: 'admin-a', body: { aiRunnerAutoApprove: true } })).status).toBe(404);
  });

  test('policy updates cannot target another organization policy', async () => {
    const response = await request('PATCH', '/policies/2', {
      user: 'admin-a',
      body: { policyType: 'deny' }
    });

    expect(response.status).toBe(404);
    expect(policies.find((policy) => policy.id === 2n)?.policyType).toBe('allow');
  });

  test('telemetry event reads include the caller organization in the final read query', async () => {
    const response = await request('GET', '/rmm/telemetry/read/events/agent-a', { user: 'viewer-a' });

    expect(response.status).toBe(200);
    expect(response.body.items).toHaveLength(1);
    expect(calls.queryRaw).toHaveLength(1);
  });

  test('snapshot request polling does not return a request row from another organization', async () => {
    const response = await request('GET', '/rmm/devices/agent-a/snapshot-requests/request-b', {
      user: 'viewer-a'
    });

    expect(response.status).toBe(404);
  });
});

describe('durable feature-upgrade progress', () => {
  test('stage ISO progress returns the Postgres projection without a replica-local overlay', async () => {
    const response = await request('POST', '/rmm/feature-upgrades/stage-iso/progress/query', {
      user: 'viewer-a',
      body: { agentIds: ['agent-a'] }
    });

    expect(response.status).toBe(200);
    expect(response.body.items).toHaveLength(1);
    expect(response.body.items[0]).toMatchObject({
      operationId: 'stage-operation',
      status: 'running',
      phase: 'downloading',
      evidence: { source: 'postgres' },
      progress: {
        operationId: 'stage-operation',
        status: 'running',
        phase: 'downloading',
        overallPercent: 37,
        phasePercent: 42
      }
    });
    expect(calls.liveProgressFetches).toEqual([]);
  });

  test('start-upgrade progress returns the Postgres projection without a replica-local overlay', async () => {
    const response = await request('POST', '/rmm/feature-upgrades/start/progress/query', {
      user: 'viewer-a',
      body: { agentIds: ['agent-a'] }
    });

    expect(response.status).toBe(200);
    expect(response.body.items).toHaveLength(1);
    expect(response.body.items[0]).toMatchObject({
      operationId: 'start-operation',
      status: 'running',
      phase: 'setup',
      evidence: { source: 'postgres' },
      progress: {
        operationId: 'start-operation',
        status: 'running',
        phase: 'setup',
        overallPercent: 48,
        phasePercent: 51
      }
    });
    expect(calls.liveProgressFetches).toEqual([]);
  });
});

describe('mixed-organization payloads', () => {
  test('bulk customer reassignment rejects a customer outside the caller organization', async () => {
    const response = await request('POST', '/rmm/devices/bulk-update-customer', {
      user: 'admin-a',
      body: { deviceIds: ['agent-a'], customerId: 'customer-b' }
    });

    expect(response.status).toBe(404);
    expect(calls.rmmDeviceUpdateMany).toHaveLength(0);
  });

  test('command policy validation rejects an out-of-org customer scope before evaluating policies', async () => {
    const response = await request('POST', '/policies/validate', {
      user: 'viewer-a',
      body: { command: 'Get-Process', customerId: 'customer-b' }
    });

    expect(response.status).toBe(404);
    expect(calls.commandPolicyFindMany).toHaveLength(0);
  });

  test('installer profile creation rejects an out-of-org site scope', async () => {
    const response = await request('POST', '/rmm/installers/profiles', {
      user: 'admin-a',
      body: { scopeType: 'site', siteId: 'site-b', name: 'Wrong site' }
    });

    expect(response.status).toBe(404);
  });

  test('internal command audit writes reject organization and customer mismatches', async () => {
    const orgMismatch = await request('POST', '/rmm/command-log', {
      rmmServer: true,
      body: { organizationId: 'org-b', userId: 'super-a', agentId: 'agent-a', command: 'Get-Service', wasAllowed: true }
    });
    expect(orgMismatch.status).toBe(409);

    const customerMismatch = await request('POST', '/rmm/command-log', {
      rmmServer: true,
      body: { organizationId: 'org-a', customerId: 'customer-a2', userId: 'super-a', agentId: 'agent-a', command: 'Get-Service', wasAllowed: true }
    });
    expect(customerMismatch.status).toBe(409);
    expect(calls.commandExecutionLogCreate).toHaveLength(0);
  });
});
