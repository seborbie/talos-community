import { beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import express from 'express';

process.env.JWT_SECRET ||= 'patch-progress-regression-test-secret';
process.env.TOKEN_TTL ||= '1h';
process.env.MACHINE_TOKEN_TTL ||= '30d';
process.env.RMM_SERVER_API_KEY = 'test-rmm-key';

type PatchAction = {
  organizationId: string;
  agentId: string;
  operationId: string;
  actionType: string;
  status: string;
  phase: string;
  reportedAt: Date;
  progress: Record<string, unknown>;
  evidence: Record<string, unknown>;
};

const devices = [
  { agentId: 'agent-a', organizationId: 'org-a' },
  { agentId: 'agent-b', organizationId: 'org-b' }
];
let action: PatchAction | null;
let queries: any[];
let actionResultCalls: any[];
let failActionResultProjection: boolean;

function sqlText(query: any): string {
  if (typeof query?.sql === 'string') return query.sql;
  if (Array.isArray(query?.strings)) return query.strings.join('?');
  return String(query);
}

function resetState() {
  action = null;
  queries = [];
  actionResultCalls = [];
  failActionResultProjection = false;
}

const prisma: any = {
  $transaction: async (work: any) => {
    const actionBefore = action ? { ...action } : null;
    const actionResultCallCount = actionResultCalls.length;
    try {
      return await work(prisma);
    } catch (error) {
      action = actionBefore;
      actionResultCalls.length = actionResultCallCount;
      throw error;
    }
  },
  $queryRaw: async (query: any) => {
    queries.push(query);
    const text = sqlText(query);
    if (text.includes('FROM public.rmm_devices') && text.includes('FOR SHARE')) {
      const requested = new Set(query.values.map(String));
      return devices.filter((device) => requested.has(device.agentId));
    }
    if (text.includes('FROM public.rmm_patch_action') && text.includes('FOR UPDATE')) {
      const [organizationId, agentId, operationId] = query.values;
      if (
        action
        && action.organizationId === organizationId
        && action.agentId === agentId
        && action.operationId === operationId
      ) {
        return [{
          actionType: action.actionType,
          status: action.status,
          reportedAt: action.reportedAt
        }];
      }
      return [];
    }
    return [];
  },
  $executeRaw: async (query: any) => {
    queries.push(query);
    const text = sqlText(query);
    if (!text.includes('INSERT INTO public.rmm_patch_action')) return 0;

    const [
      , organizationId, agentId, operationId, actionType, status, phase,
      , progressJson, evidenceJson, , , reportedAt
    ] = query.values;
    if (
      action
      && (
        ['completed', 'failed', 'cancelled'].includes(action.status)
        || action.reportedAt.getTime() > (reportedAt as Date).getTime()
      )
    ) {
      return 0;
    }
    action = {
      organizationId,
      agentId,
      operationId,
      actionType,
      status,
      phase,
      reportedAt,
      progress: JSON.parse(progressJson),
      evidence: JSON.parse(evidenceJson)
    };
    return 1;
  }
};

mock.module('../lib/prisma', () => ({ prisma }));
mock.module('../lib/patchDecisionService', () => ({
  evaluateAndPersistPatchPlan: async () => ({ actions: [] }),
  inferPatchProgressActionType: ({ eventType, existingActionType }: any) =>
    existingActionType || (eventType === 'patch.scan.progress' ? 'scan' : 'install'),
  recordPatchActionResult: async (options: any) => {
    actionResultCalls.push(options);
  },
  recordPatchActionResultInTransaction: async (_transaction: any, options: any) => {
    actionResultCalls.push(options);
    if (failActionResultProjection) {
      throw new Error('simulated update-state projection failure');
    }
  }
}));

let makeApp: () => express.Express;

beforeAll(async () => {
  const { rmmTelemetryRouter } = await import('../routes/rmmTelemetry.routes');
  makeApp = () => {
    const app = express();
    app.use(express.json({ limit: '1mb' }));
    app.use('/rmm/telemetry', rmmTelemetryRouter);
    app.use((error: any, _req: any, res: any, _next: any) => {
      res.status(error.status || 500).json({ error: error.message || 'Internal server error' });
    });
    return app;
  };
});

beforeEach(resetState);

async function request(body: unknown, authenticated = true) {
  const server = makeApp().listen(0);
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Failed to bind test server');
  try {
    const response = await fetch(
      `http://127.0.0.1:${address.port}/rmm/telemetry/patch/progress`,
      {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          ...(authenticated ? { 'x-rmm-server-key': 'test-rmm-key' } : {})
        },
        body: JSON.stringify(body)
      }
    );
    return { status: response.status, body: await response.json() as any };
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
    });
  }
}

function progress(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    eventType: 'patch.install.progress',
    organizationId: 'org-a',
    agentId: 'agent-a',
    jobId: 'job-a',
    commandId: 'operation-a',
    status: 'running',
    phase: 'downloading',
    reportedAt: '2026-08-17T12:00:00Z',
    updates: [],
    summary: { matched: 1 },
    ...overrides
  };
}

describe('patch progress projection route', () => {
  test('requires the RMM server key', async () => {
    const response = await request(progress(), false);
    expect(response.status).toBe(401);
    expect(queries).toHaveLength(0);
  });

  test('rejects a mixed-scope batch before projecting any member', async () => {
    const response = await request({
      progress: [
        progress(),
        progress({ organizationId: 'org-b', reportedAt: '2026-08-17T12:01:00Z' })
      ]
    });

    expect(response).toEqual({
      status: 400,
      body: { error: 'agentId does not belong to organizationId', itemIndex: 1 }
    });
    expect(action).toBeNull();
    expect(queries).toHaveLength(1);
    expect(sqlText(queries[0])).toContain('FOR SHARE');
  });

  test('rejects unbounded fields, invalid timestamps, and oversized evidence', async () => {
    for (const invalid of [
      progress({ status: 'arbitrary' }),
      progress({ phase: 'x'.repeat(65) }),
      progress({ reportedAt: 'not-a-time' })
    ]) {
      const response = await request(invalid);
      expect(response.status).toBe(400);
    }

    const oversized = await request(progress({
      updates: [{ title: 'x'.repeat(128 * 1024) }]
    }));
    expect(oversized.status).toBe(413);
    expect(oversized.body.error).toContain('progress evidence must not exceed');
    expect(action).toBeNull();
    expect(queries).toHaveLength(0);
  });

  test('rejects a far-future heartbeat before it can pin later progress as stale', async () => {
    const response = await request(progress({
      reportedAt: new Date(Date.now() + 60 * 60 * 1000).toISOString()
    }));

    expect(response.status).toBe(400);
    expect(response.body.error).toContain('must not be more than 10 minutes in the future');
    expect(action).toBeNull();
    expect(queries).toHaveLength(0);
  });

  test('keeps terminal state immutable across delayed heartbeats and terminal replays', async () => {
    const running = await request(progress());
    expect(running).toEqual({
      status: 202,
      body: { accepted: true, updated: 1, ignored: 0 }
    });

    const completed = await request({
      progress: [progress({
        status: 'completed',
        phase: 'finalizing',
        reportedAt: '2026-08-17T12:01:00Z'
      })]
    });
    expect(completed.body).toEqual({ accepted: true, updated: 1, ignored: 0 });
    expect(action).toMatchObject({ status: 'completed', phase: 'finalizing' });
    expect(actionResultCalls).toHaveLength(1);

    const delayedHeartbeat = await request(progress({
      phase: 'installing',
      reportedAt: '2026-08-17T12:02:00Z'
    }));
    expect(delayedHeartbeat.body).toEqual({ accepted: true, updated: 0, ignored: 1 });

    const conflictingTerminal = await request(progress({
      status: 'failed',
      phase: 'failed',
      reportedAt: '2026-08-17T12:03:00Z'
    }));
    expect(conflictingTerminal.body).toEqual({ accepted: true, updated: 0, ignored: 1 });

    const replay = await request(progress({
      status: 'completed',
      phase: 'finalizing',
      reportedAt: '2026-08-17T12:01:00Z'
    }));
    expect(replay.body).toEqual({ accepted: true, updated: 0, ignored: 1 });
    expect(action).toMatchObject({ status: 'completed', phase: 'finalizing' });
    expect(actionResultCalls).toHaveLength(1);

    const upsert = queries.find((query) => sqlText(query).includes('INSERT INTO public.rmm_patch_action'));
    expect(sqlText(upsert)).toContain("status NOT IN ('completed', 'failed', 'cancelled')");
    expect(sqlText(upsert)).toContain('reported_at <= EXCLUDED.reported_at');
  });

  test('ignores an older nonterminal report using the parsed producer timestamp', async () => {
    await request(progress({
      phase: 'installing',
      reportedAt: '2026-08-17T12:02:00Z'
    }));
    const stale = await request(progress({
      phase: 'downloading',
      reportedAt: '2026-08-17T12:01:00Z'
    }));

    expect(stale.body).toEqual({ accepted: true, updated: 0, ignored: 1 });
    expect(action).toMatchObject({
      status: 'running',
      phase: 'installing',
      reportedAt: new Date('2026-08-17T12:02:00Z')
    });
  });

  test('rolls back a terminal event when its update-state projection fails so retry is not lost', async () => {
    await request(progress());
    failActionResultProjection = true;
    const failedAttempt = await request(progress({
      status: 'completed',
      phase: 'finalizing',
      reportedAt: '2026-08-17T12:01:00Z'
    }));

    expect(failedAttempt.status).toBe(500);
    expect(action).toMatchObject({ status: 'running', reportedAt: new Date('2026-08-17T12:00:00Z') });
    expect(actionResultCalls).toHaveLength(0);

    failActionResultProjection = false;
    const retry = await request(progress({
      status: 'completed',
      phase: 'finalizing',
      reportedAt: '2026-08-17T12:01:00Z'
    }));
    expect(retry.body).toEqual({ accepted: true, updated: 1, ignored: 0 });
    expect(action).toMatchObject({ status: 'completed' });
    expect(actionResultCalls).toHaveLength(1);
  });
});
