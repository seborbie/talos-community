import { beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import express from 'express';
import { persistedRemediationMetadata } from '../lib/remediationDispatch';

process.env.JWT_SECRET = 'remediation-test-jwt-secret-unique';
process.env.APP_ENCRYPTION_KEY = 'remediation-test-encryption-key-unique';
process.env.SERVICE_KEY = 'remediation-test-service-key-unique';
process.env.API_SERVICE_KEY = 'remediation-test-api-service-key-unique';
process.env.RMM_TELEMETRY_SERVICE_KEY = 'test-service-key';
process.env.RMM_SERVER_API_KEY = 'test-rmm-key';
process.env.TALOS_AI_RUNNER_SERVICE_KEY = 'remediation-test-ai-service-key-unique';
process.env.TALOS_AI_RUNNER_RMM_SERVER_KEY = 'remediation-test-ai-rmm-key-unique';
process.env.RMM_AGENT_TOKEN = 'remediation-test-agent-token-unique';
delete process.env.OPENAI_API_KEY;

type Job = {
  id: bigint;
  command_id: string | null;
  organization_id: string;
  agent_id: string;
  decision_id: bigint | null;
  intent_id: string;
  status: string;
  dedupe_key: string | null;
  requested_by: string;
  requested_at: Date;
  metadata_jsonb: unknown;
  started_at?: Date | null;
  finished_at?: Date | null;
};

type Step = {
  id: bigint;
  job_id: bigint;
  step_index: number;
  command: string;
  status: string;
  evidence_jsonb: unknown;
  started_at?: Date | null;
  finished_at?: Date | null;
};

let jobs: Job[];
let steps: Step[];
let queries: any[];

function sqlText(query: any) {
  if (typeof query?.sql === 'string') return query.sql;
  if (Array.isArray(query?.strings)) return query.strings.join('?');
  return String(query);
}

function resetState() {
  const metadata = persistedRemediationMetadata({
    metadata: { source: 'test' },
    execution: { maxRetries: 2, timeoutSeconds: 901, stopOnFailure: false },
    steps: [{ stepIndex: 0, command: 'echo frozen', timeoutSeconds: 17 }]
  });
  jobs = [
    { id: 1n, command_id: 'command-a', organization_id: 'org-a', agent_id: 'agent-a', decision_id: null, intent_id: 'generic.intent', status: 'queued', dedupe_key: 'dedupe-a', requested_by: 'operator', requested_at: new Date('2026-08-17T12:00:00Z'), metadata_jsonb: metadata },
    { id: 2n, command_id: 'patch-a', organization_id: 'org-a', agent_id: 'agent-a', decision_id: null, intent_id: 'talos.patch.install', status: 'queued', dedupe_key: null, requested_by: 'operator', requested_at: new Date('2026-08-17T12:01:00Z'), metadata_jsonb: {} },
    { id: 3n, command_id: 'pending-a', organization_id: 'org-a', agent_id: 'agent-a', decision_id: null, intent_id: 'generic.intent', status: 'pending_approval', dedupe_key: null, requested_by: 'operator', requested_at: new Date('2026-08-17T12:02:00Z'), metadata_jsonb: {} },
    { id: 4n, command_id: 'other-agent', organization_id: 'org-a', agent_id: 'agent-b', decision_id: null, intent_id: 'generic.intent', status: 'queued', dedupe_key: null, requested_by: 'operator', requested_at: new Date('2026-08-17T12:03:00Z'), metadata_jsonb: {} },
    { id: 5n, command_id: null, organization_id: 'org-a', agent_id: 'agent-a', decision_id: null, intent_id: 'generic.intent', status: 'queued', dedupe_key: null, requested_by: 'operator', requested_at: new Date('2026-08-17T12:04:00Z'), metadata_jsonb: {} },
    { id: 6n, command_id: 'step-less', organization_id: 'org-a', agent_id: 'agent-a', decision_id: null, intent_id: 'generic.intent', status: 'queued', dedupe_key: null, requested_by: 'operator', requested_at: new Date('2026-08-17T12:05:00Z'), metadata_jsonb: {} }
  ];
  steps = [
    { id: 10n, job_id: 1n, step_index: 0, command: 'echo durable', status: 'pending', evidence_jsonb: null },
    { id: 20n, job_id: 2n, step_index: 0, command: 'talos-patch-install', status: 'pending', evidence_jsonb: null }
  ];
  queries = [];
}

function addThreeStepJob() {
  jobs.push({
    id: 7n,
    command_id: 'command-three',
    organization_id: 'org-a',
    agent_id: 'agent-a',
    decision_id: null,
    intent_id: 'generic.intent',
    status: 'queued',
    dedupe_key: 'dedupe-three',
    requested_by: 'operator',
    requested_at: new Date('2026-08-17T12:06:00Z'),
    metadata_jsonb: {}
  });
  steps.push(
    { id: 70n, job_id: 7n, step_index: 0, command: 'echo zero', status: 'pending', evidence_jsonb: null },
    { id: 71n, job_id: 7n, step_index: 1, command: 'echo one', status: 'pending', evidence_jsonb: null },
    { id: 72n, job_id: 7n, step_index: 2, command: 'echo two', status: 'pending', evidence_jsonb: null }
  );
}

const prisma: any = {
  $transaction: async (work: any) => work(prisma),
  rmmDevice: {
    findUnique: async ({ where }: any) => {
      if (where.agentId === 'agent-a') {
        return {
          organizationId: 'org-a',
          customerId: null,
          siteId: null,
          hostname: 'agent-a'
        };
      }
      if (where.agentId === 'agent-b') {
        return {
          organizationId: 'org-b',
          customerId: null,
          siteId: null,
          hostname: 'agent-b'
        };
      }
      return null;
    },
    update: async () => ({})
  },
  $queryRaw: async (query: any) => {
    queries.push(query);
    const text = sqlText(query);
    if (text.includes('WITH candidates AS')) {
      const [agentId, patchIntentId, limit] = query.values;
      const claimed = jobs
        .filter((job) => job.status === 'queued')
        .filter((job) => job.agent_id === agentId)
        .filter((job) => job.intent_id !== patchIntentId)
        .filter((job) => job.command_id !== null)
        .filter((job) => steps.some((step) => step.job_id === job.id))
        .sort((left, right) => left.requested_at.getTime() - right.requested_at.getTime())
        .slice(0, limit);
      for (const job of claimed) {
        job.status = 'running';
        job.started_at ??= new Date('2026-08-17T12:30:00Z');
      }
      return claimed;
    }
    if (text.includes('FROM rmm_telemetry.remediation_step') && text.includes('WHERE job_id IN')) {
      const ids = new Set(query.values.map(String));
      return steps.filter((step) => ids.has(String(step.job_id)));
    }
    if (text.includes('FROM rmm_telemetry.remediation_job job') && text.includes('FOR UPDATE')) {
      const identity = query.values[0];
      const agentId = query.values[1];
      const hasOrganization = text.includes('job.organization_id =');
      const organizationId = hasOrganization ? query.values[2] : null;
      const patchIntentId = query.values[query.values.length - 1];
      const patchScope = text.includes('job.intent_id =') && !text.includes('job.intent_id <>');
      return jobs
        .filter((job) => typeof identity === 'bigint'
          ? job.id === identity
          : job.command_id === identity)
        .filter((job) => job.agent_id === agentId)
        .filter((job) => !organizationId || job.organization_id === organizationId)
        .filter((job) => patchScope
          ? job.intent_id === patchIntentId
          : job.intent_id !== patchIntentId)
        .map((job) => ({ ...job }));
    }
    if (text.includes('SELECT step_index, command, status')) {
      const [jobId] = query.values;
      return steps
        .filter((step) => step.job_id === jobId)
        .sort((left, right) => left.step_index - right.step_index)
        .map((step) => ({
          step_index: step.step_index,
          command: step.command,
          status: step.status
        }));
    }
    if (text.includes('INSERT INTO rmm_telemetry.remediation_job')) {
      const [
        commandId,
        organizationId,
        agentId,
        decisionId,
        intentId,
        status,
        dedupeKey,
        requestedBy,
        metadataJson
      ] = query.values;
      const conflictByDedupe = text.includes('ON CONFLICT (dedupe_key)');
      const existing = conflictByDedupe
        ? jobs.find((job) => job.dedupe_key === dedupeKey)
        : jobs.find((job) => job.command_id === commandId);
      if (existing) {
        if (
          existing.command_id !== commandId
          || existing.organization_id !== organizationId
          || existing.agent_id !== agentId
          || existing.intent_id !== intentId
        ) {
          return [];
        }
        if (!['running', 'completed', 'failed', 'cancelled'].includes(existing.status)) {
          existing.status = status;
          existing.requested_by = requestedBy;
          existing.metadata_jsonb = JSON.parse(metadataJson);
        }
        return [{ id: existing.id, status: existing.status }];
      }
      const id = jobs.reduce((highest, job) => job.id > highest ? job.id : highest, 0n) + 1n;
      const created: Job = {
        id,
        command_id: commandId,
        organization_id: organizationId,
        agent_id: agentId,
        decision_id: decisionId,
        intent_id: intentId,
        status,
        dedupe_key: dedupeKey,
        requested_by: requestedBy,
        requested_at: new Date('2026-08-17T12:40:00Z'),
        metadata_jsonb: JSON.parse(metadataJson),
        started_at: null,
        finished_at: null
      };
      jobs.push(created);
      return [{ id, status }];
    }
    return [];
  },
  $executeRaw: async (query: any) => {
    queries.push(query);
    const text = sqlText(query);
    if (text.includes('UPDATE rmm_telemetry.remediation_step')) {
      const values = query.values;
      const hasEvidence = text.includes('evidence_jsonb');
      const status = values[0];
      const jobId = values.find((value: unknown) => typeof value === 'bigint');
      const stepIndex = values[values.length - 1];
      const step = steps.find((candidate) => candidate.job_id === jobId && candidate.step_index === stepIndex);
      if (!step) return 0;
      step.status = status;
      if (hasEvidence) step.evidence_jsonb = JSON.parse(values[1]);
      step.started_at ??= new Date('2026-08-17T12:31:00Z');
      if (['completed', 'failed', 'cancelled'].includes(status)) {
        step.finished_at ??= new Date('2026-08-17T12:32:00Z');
      }
      return 1;
    }
    if (text.includes('UPDATE rmm_telemetry.remediation_job')) {
      const [status, _finished, jobId] = query.values;
      const job = jobs.find((candidate) => candidate.id === jobId);
      if (!job) return 0;
      job.status = status;
      job.started_at ??= new Date('2026-08-17T12:31:00Z');
      if (['completed', 'failed', 'cancelled'].includes(status)) {
        job.finished_at ??= new Date('2026-08-17T12:32:00Z');
      }
      return 1;
    }
    if (text.includes('INSERT INTO rmm_telemetry.remediation_step')) {
      const [organizationId, jobId, stepIndex, command, status, evidenceJson] = query.values;
      const existing = steps.find((step) => step.job_id === jobId && step.step_index === stepIndex);
      if (existing) return 1;
      steps.push({
        id: steps.reduce((highest, step) => step.id > highest ? step.id : highest, 0n) + 1n,
        job_id: jobId,
        step_index: stepIndex,
        command,
        status,
        evidence_jsonb: JSON.parse(evidenceJson),
        started_at: null,
        finished_at: null
      });
      expect(organizationId).toBe(jobs.find((job) => job.id === jobId)?.organization_id);
      return 1;
    }
    return 0;
  }
};

mock.module('../lib/prisma', () => ({ prisma }));
mock.module('../middleware/rmmServerKey', () => ({
  attachRmmServerAuth(req: any, _res: any, next: any) {
    req.rmmServer = req.header('x-rmm-server-key') === 'test-rmm-key';
    next();
  },
  requireRmmServer(req: any, res: any, next: any) {
    if (!req.rmmServer) return res.status(401).json({ error: 'Unauthorized' });
    next();
  }
}));

let makeApp: () => express.Express;
let makeCompatibilityApp: () => express.Express;

beforeAll(async () => {
  const { remediationDispatchRouter } = await import('../routes/remediationDispatch.routes');
  const { rmmTelemetryRouter } = await import('../routes/rmmTelemetry.routes');
  makeApp = () => {
    const app = express();
    app.use(express.json());
    app.use('/rmm/telemetry/remediation', remediationDispatchRouter);
    return app;
  };
  makeCompatibilityApp = () => {
    const app = express();
    app.use(express.json());
    app.use('/rmm/telemetry', rmmTelemetryRouter);
    app.use((error: any, _req: any, res: any, _next: any) => {
      res.status(error?.status || 500).json({ error: error?.message || 'Internal server error' });
    });
    return app;
  };
});

beforeEach(resetState);

async function request(method: string, path: string, body?: unknown, authenticated = true) {
  const server = makeApp().listen(0);
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Failed to bind test server');
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method,
      headers: {
        ...(authenticated ? { 'x-rmm-server-key': 'test-rmm-key' } : {}),
        ...(body === undefined ? {} : { 'content-type': 'application/json' })
      },
      body: body === undefined ? undefined : JSON.stringify(body)
    });
    return { status: response.status, body: await response.json() as any };
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
}

async function compatibilityRequest(
  method: string,
  path: string,
  body?: unknown,
  credential: 'service' | 'rmm' | 'none' = 'service'
) {
  const server = makeCompatibilityApp().listen(0);
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Failed to bind test server');
  try {
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method,
      headers: {
        ...(credential === 'service' ? { 'x-service-key': 'test-service-key' } : {}),
        ...(credential === 'rmm' ? { 'x-rmm-server-key': 'test-rmm-key' } : {}),
        ...(body === undefined ? {} : { 'content-type': 'application/json' })
      },
      body: body === undefined ? undefined : JSON.stringify(body)
    });
    const text = await response.text();
    return { status: response.status, body: text ? JSON.parse(text) : null };
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
}

describe('durable remediation dispatch routes', () => {
  test('requires the RMM server key and atomically claims only eligible generic work once', async () => {
    const unauthorized = await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 },
      false
    );
    expect(unauthorized.status).toBe(401);
    expect(queries).toHaveLength(0);

    const first = await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );
    expect(first.status).toBe(200);
    expect(first.body.jobs).toEqual([{
      commandId: 'command-a',
      organizationId: 'org-a',
      agentId: 'agent-a',
      intentId: 'generic.intent',
      decisionId: null,
      dedupeKey: 'dedupe-a',
      requestedBy: 'operator',
      requestedAt: '2026-08-17T12:00:00.000Z',
      approvalState: 'approved',
      metadata: { source: 'test' },
      steps: [{
        stepIndex: 0,
        command: 'echo durable',
        timeoutSeconds: 17,
        status: 'pending',
        evidence: null
      }],
      execution: { maxRetries: 2, timeoutSeconds: 901, stopOnFailure: false }
    }]);
    expect(sqlText(queries[0])).toContain('FOR UPDATE SKIP LOCKED');
    expect(sqlText(queries[0])).toContain("job.status = 'queued'");
    expect(sqlText(queries[0])).toContain('job.command_id IS NOT NULL');

    const second = await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );
    expect(second.status).toBe(200);
    expect(second.body.jobs).toEqual([]);
    expect(jobs.find((job) => job.command_id === 'patch-a')?.status).toBe('queued');
    expect(jobs.find((job) => job.command_id === 'pending-a')?.status).toBe('pending_approval');
    expect(jobs.find((job) => job.command_id === 'other-agent')?.status).toBe('queued');
    expect(jobs.find((job) => job.command_id === 'step-less')?.status).toBe('queued');
  });

  test('scopes reports to the agent and enforces durable status transitions', async () => {
    const wrongAgent = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-b/jobs/command-a/status',
      { status: 'completed', stepIndex: 0 }
    );
    expect(wrongAgent.status).toBe(404);

    const beforeClaim = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'completed', stepIndex: 0 }
    );
    expect(beforeClaim.status).toBe(409);

    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 1 }
    );
    const completed = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'completed', stepIndex: 0, evidence: { exitCode: 0 } }
    );
    expect(completed).toEqual({ status: 200, body: { updated: true, status: 'completed' } });
    expect(steps[0]).toMatchObject({ status: 'completed', evidence_jsonb: { exitCode: 0 } });

    const repeated = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'completed', stepIndex: 0, evidence: { exitCode: 0 } }
    );
    expect(repeated.status).toBe(200);

    const conflict = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'failed', stepIndex: 0 }
    );
    expect(conflict.status).toBe(409);
  });

  test('rejects an unknown step without changing the running job', async () => {
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 1 }
    );
    const response = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'completed', stepIndex: 999 }
    );
    expect(response.status).toBe(404);
    expect(jobs.find((job) => job.command_id === 'command-a')?.status).toBe('running');
  });

  test('rejects oversized direct evidence without mutating durable state', async () => {
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 1 }
    );
    const response = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-a/status',
      { status: 'completed', stepIndex: 0, evidence: { output: 'x'.repeat(33 * 1024) } }
    );

    expect(response.status).toBe(413);
    expect(jobs.find((job) => job.command_id === 'command-a')?.status).toBe('running');
    expect(steps[0]).toMatchObject({ status: 'pending', evidence_jsonb: null });
  });

  test('atomically projects a three-step terminal outcome and completes the durable job coherently', async () => {
    addThreeStepJob();
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );

    for (const stepIndex of [0, 1, 2]) {
      const running = await request(
        'PATCH',
        '/rmm/telemetry/remediation/agents/agent-a/jobs/command-three/status',
        { status: 'running', stepIndex, evidence: { phase: 'running', stepIndex } }
      );
      expect(running).toEqual({ status: 200, body: { updated: true, status: 'running' } });
      expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('running');
    }

    const terminalSteps = [
      { stepIndex: 0, status: 'completed', exitCode: 0, output: 'zero\n' },
      { stepIndex: 1, status: 'completed', exitCode: 0, output: 'one\n' },
      { stepIndex: 2, status: 'completed', exitCode: 0, output: 'two\n' }
    ];
    const completed = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-three/status',
      {
        status: 'completed',
        stepIndex: 2,
        evidence: { phase: 'completed', steps: terminalSteps, error: null }
      }
    );

    expect(completed).toEqual({ status: 200, body: { updated: true, status: 'completed' } });
    expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('completed');
    expect(
      steps
        .filter((step) => step.job_id === 7n)
        .map((step) => ({ status: step.status, evidence: step.evidence_jsonb }))
    ).toEqual(terminalSteps.map((evidence) => ({ status: 'completed', evidence })));
  });

  test('projects a stopped three-step failure and marks the unexecuted step cancelled', async () => {
    addThreeStepJob();
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );

    const completedEvidence = {
      stepIndex: 0,
      status: 'completed',
      exitCode: 0,
      output: 'zero\n'
    };
    const failedEvidence = {
      stepIndex: 1,
      status: 'failed',
      exitCode: 1,
      output: 'failed\n'
    };
    const failed = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-three/status',
      {
        status: 'failed',
        stepIndex: 1,
        evidence: {
          phase: 'failed',
          steps: [completedEvidence, failedEvidence],
          error: 'One or more remediation steps failed'
        }
      }
    );

    expect(failed).toEqual({ status: 200, body: { updated: true, status: 'failed' } });
    expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('failed');
    expect(
      steps
        .filter((step) => step.job_id === 7n)
        .map((step) => ({ status: step.status, evidence: step.evidence_jsonb }))
    ).toEqual([
      { status: 'completed', evidence: completedEvidence },
      { status: 'failed', evidence: failedEvidence },
      {
        status: 'cancelled',
        evidence: {
          stepIndex: 2,
          status: 'cancelled',
          reason: 'not_executed_after_terminal_outcome'
        }
      }
    ]);
  });

  test('fails closed on malformed aggregate evidence and keeps missing aggregate evidence non-terminal', async () => {
    addThreeStepJob();
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );

    const malformed = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-three/status',
      { status: 'completed', stepIndex: 2, evidence: { steps: { invalid: true } } }
    );
    expect(malformed.status).toBe(400);
    expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('running');
    expect(steps.filter((step) => step.job_id === 7n).map((step) => step.status))
      .toEqual(['pending', 'pending', 'pending']);

    const direct = await request(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/command-three/status',
      { status: 'completed', stepIndex: 0, evidence: { exitCode: 0 } }
    );
    expect(direct.status).toBe(200);
    expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('running');
    expect(steps.filter((step) => step.job_id === 7n).map((step) => step.status))
      .toEqual(['completed', 'pending', 'pending']);
  });
});

describe('legacy remediation compatibility routes', () => {
  test('keeps command projection identity immutable across organization and agent replays', async () => {
    const replay = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/project',
      {
        commandId: 'command-a',
        organizationId: 'org-a',
        agentId: 'agent-a',
        intentId: 'generic.intent',
        dedupeKey: 'dedupe-a',
        requestedBy: 'consumer',
        approvalState: 'approved',
        metadata: { replay: true },
        steps: [{ stepIndex: 0, command: 'echo durable' }]
      }
    );
    expect(replay.status).toBe(202);
    expect(replay.body).toMatchObject({ accepted: true, commandId: 'command-a', status: 'queued' });

    const wrongOwner = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/project',
      {
        commandId: 'command-a',
        organizationId: 'org-b',
        agentId: 'agent-b',
        intentId: 'generic.intent',
        dedupeKey: 'dedupe-a',
        approvalState: 'approved',
        steps: [{ stepIndex: 0, command: 'echo stolen' }]
      }
    );
    expect(wrongOwner.status).toBe(409);
    expect(jobs.find((job) => job.command_id === 'command-a')).toMatchObject({
      organization_id: 'org-a',
      agent_id: 'agent-a',
      intent_id: 'generic.intent'
    });
    expect(steps.find((step) => step.job_id === 1n)?.command).toBe('echo durable');

    const mismatchedDeviceScope = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/project',
      {
        commandId: 'new-command',
        organizationId: 'org-b',
        agentId: 'agent-a',
        intentId: 'generic.intent',
        approvalState: 'approved',
        steps: [{ stepIndex: 0, command: 'echo rejected' }]
      }
    );
    expect(mismatchedDeviceScope.status).toBe(404);
    expect(jobs.some((job) => job.command_id === 'new-command')).toBe(false);
  });

  test('projects the active consumer payload through scoped guarded transitions', async () => {
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 1 }
    );

    const wrongOrganization = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      {
        statuses: [{
          commandId: 'command-a',
          organizationId: 'org-b',
          agentId: 'agent-a',
          status: 'completed',
          stepIndex: 0
        }]
      }
    );
    expect(wrongOrganization.status).toBe(404);
    expect(jobs.find((job) => job.command_id === 'command-a')?.status).toBe('running');

    const wrongAgent = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      {
        statuses: [{
          commandId: 'command-a',
          organizationId: 'org-a',
          agentId: 'agent-b',
          status: 'completed',
          stepIndex: 0
        }]
      }
    );
    expect(wrongAgent.status).toBe(404);

    const completedPayload = {
      statuses: [{
        commandId: 'command-a',
        organizationId: 'org-a',
        agentId: 'agent-a',
        status: 'completed',
        stepIndex: 0,
        evidence: { exitCode: 0 }
      }]
    };
    const completed = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      completedPayload
    );
    expect(completed).toEqual({ status: 202, body: { accepted: true, updated: 1 } });
    const terminalFinishedAt = jobs.find((job) => job.command_id === 'command-a')?.finished_at;
    expect(terminalFinishedAt).toBeInstanceOf(Date);

    const replay = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      completedPayload
    );
    expect(replay.status).toBe(202);
    expect(jobs.find((job) => job.command_id === 'command-a')?.finished_at)
      .toBe(terminalFinishedAt);

    const regression = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      {
        statuses: [{
          commandId: 'command-a',
          organizationId: 'org-a',
          agentId: 'agent-a',
          status: 'running',
          stepIndex: 0
        }]
      }
    );
    expect(regression.status).toBe(409);
    expect(jobs.find((job) => job.command_id === 'command-a')).toMatchObject({
      status: 'completed',
      finished_at: terminalFinishedAt
    });
  });

  test('projects a multi-step terminal consumer event atomically', async () => {
    addThreeStepJob();
    await request(
      'POST',
      '/rmm/telemetry/remediation/agents/agent-a/jobs/claim',
      { limit: 10 }
    );
    const terminalSteps = [
      { stepIndex: 0, status: 'completed', exitCode: 0 },
      { stepIndex: 1, status: 'completed', exitCode: 0 },
      { stepIndex: 2, status: 'completed', exitCode: 0 }
    ];

    const response = await compatibilityRequest(
      'POST',
      '/rmm/telemetry/remediation/commands/status',
      {
        statuses: [{
          commandId: 'command-three',
          organizationId: 'org-a',
          agentId: 'agent-a',
          status: 'completed',
          stepIndex: 2,
          evidence: { steps: terminalSteps }
        }]
      }
    );
    expect(response.status).toBe(202);
    expect(jobs.find((job) => job.command_id === 'command-three')?.status).toBe('completed');
    expect(steps.filter((step) => step.job_id === 7n).map((step) => step.status))
      .toEqual(['completed', 'completed', 'completed']);
  });

  test('routes patch status through the same transition guard', async () => {
    const patchJob = jobs.find((job) => job.command_id === 'patch-a')!;
    patchJob.status = 'running';
    patchJob.started_at = new Date('2026-08-17T12:20:00Z');

    const completed = await compatibilityRequest(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/patch-jobs/2/status',
      { status: 'completed', stepIndex: 0, evidence: { installed: true } },
      'rmm'
    );
    expect(completed).toEqual({ status: 200, body: { updated: true, status: 'completed' } });
    const terminalFinishedAt = patchJob.finished_at;

    const regression = await compatibilityRequest(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-a/patch-jobs/2/status',
      { status: 'failed', stepIndex: 0 },
      'rmm'
    );
    expect(regression.status).toBe(409);
    expect(patchJob).toMatchObject({ status: 'completed', finished_at: terminalFinishedAt });

    const wrongAgent = await compatibilityRequest(
      'PATCH',
      '/rmm/telemetry/remediation/agents/agent-b/patch-jobs/2/status',
      { status: 'completed', stepIndex: 0 },
      'rmm'
    );
    expect(wrongAgent.status).toBe(404);
  });

  test('retires the unscoped numeric job status mutation without touching state', async () => {
    const response = await compatibilityRequest(
      'PATCH',
      '/rmm/telemetry/remediation/jobs/1/status',
      { status: 'completed' }
    );
    expect(response.status).toBe(410);
    expect(response.body.replacement).toContain('/agents/{agentId}/jobs/{commandId}/status');
    expect(jobs.find((job) => job.id === 1n)?.status).toBe('queued');
  });
});
