import { test, expect } from 'bun:test';
import crypto from 'node:crypto';
import type { Server } from 'node:http';
import express from 'express';
import { prisma } from '../lib/prisma';
import { transitionRemediationStatus } from '../lib/remediationStatusTransitions';
import { rmmTelemetryRouter } from '../routes/rmmTelemetry.routes';

const databaseUrl = process.env.REMEDIATION_TEST_DATABASE_URL?.trim();

if (!databaseUrl) {
  test.skip('PostgreSQL remediation transition integration (set REMEDIATION_TEST_DATABASE_URL)', () => {});
} else {
  test('PostgreSQL serializes scoped remediation projection, replay, and conflicts', async () => {
    if (process.env.DATABASE_URL !== databaseUrl) {
      throw new Error('DATABASE_URL must equal REMEDIATION_TEST_DATABASE_URL for this integration test');
    }

    const suffix = crypto.randomUUID();
    const organizationA = `remediation-org-a-${suffix}`;
    const organizationB = `remediation-org-b-${suffix}`;
    const agentA = `remediation-agent-a-${suffix}`;
    const agentB = `remediation-agent-b-${suffix}`;
    const projectedCommand = `remediation-projected-${suffix}`;
    const projectedDedupe = `remediation-dedupe-${suffix}`;
    const multiStepCommand = `remediation-multi-${suffix}`;
    const concurrentCommand = `remediation-concurrent-${suffix}`;
    const batchCommandA = `remediation-batch-a-${suffix}`;
    const batchCommandB = `remediation-batch-b-${suffix}`;
    const patchCommand = `remediation-patch-${suffix}`;

    const app = express();
    app.use(express.json());
    app.use('/rmm/telemetry', rmmTelemetryRouter);
    app.use((error: any, _req: any, res: any, _next: any) => {
      res.status(error?.status || 500).json({ error: error?.message || 'Internal server error' });
    });
    const server = await new Promise<Server>((resolve) => {
      const listeningServer = app.listen(0, '127.0.0.1', () => resolve(listeningServer));
    });
    const address = server.address();
    if (!address || typeof address === 'string') {
      throw new Error('Failed to bind integration test server');
    }
    const baseUrl = `http://127.0.0.1:${address.port}/rmm/telemetry`;

    async function request(
      method: 'POST' | 'PATCH',
      path: string,
      body: unknown,
      credential: 'service' | 'rmm'
    ) {
      const response = await fetch(`${baseUrl}${path}`, {
        method,
        headers: {
          'content-type': 'application/json',
          ...(credential === 'service' ? { 'x-service-key': 'test-service-key' } : {}),
          ...(credential === 'rmm' ? { 'x-rmm-server-key': 'test-rmm-key' } : {})
        },
        body: JSON.stringify(body)
      });
      const text = await response.text();
      return { status: response.status, body: text ? JSON.parse(text) : null };
    }

    try {
      await prisma.organization.createMany({
        data: [
          { id: organizationA, name: 'Remediation integration A' },
          { id: organizationB, name: 'Remediation integration B' }
        ]
      });
      await prisma.rmmDevice.createMany({
        data: [
          {
            agentId: agentA,
            organizationId: organizationA,
            hostname: agentA,
            os: 'linux',
            ip: '127.0.0.1',
            lastSeen: new Date()
          },
          {
            agentId: agentB,
            organizationId: organizationB,
            hostname: agentB,
            os: 'linux',
            ip: '127.0.0.2',
            lastSeen: new Date()
          }
        ]
      });

      const projection = {
        commandId: projectedCommand,
        organizationId: organizationA,
        agentId: agentA,
        intentId: 'integration.generic',
        dedupeKey: projectedDedupe,
        requestedBy: 'integration-test',
        approvalState: 'approved',
        steps: [{ stepIndex: 0, command: 'echo projected' }]
      };
      const created = await request(
        'POST',
        '/remediation/commands/project',
        projection,
        'service'
      );
      const projectionReplay = await request(
        'POST',
        '/remediation/commands/project',
        projection,
        'service'
      );
      const projectionCollision = await request(
        'POST',
        '/remediation/commands/project',
        { ...projection, organizationId: organizationB, agentId: agentB },
        'service'
      );
      expect(created.status).toBe(202);
      expect(projectionReplay.status).toBe(202);
      expect(projectionCollision.status).toBe(409);

      const projectedOwner = await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
        where: { commandId: projectedCommand }
      });
      expect([projectedOwner.organizationId, projectedOwner.agentId])
        .toEqual([organizationA, agentA]);

      const claim = await request(
        'POST',
        `/remediation/agents/${agentA}/jobs/claim`,
        { limit: 10 },
        'rmm'
      );
      expect(claim.status).toBe(200);
      expect(claim.body.jobs.some((job: any) => job.commandId === projectedCommand)).toBe(true);

      const completedProjection = {
        statuses: [{
          commandId: projectedCommand,
          organizationId: organizationA,
          agentId: agentA,
          status: 'completed',
          stepIndex: 0,
          evidence: { exitCode: 0 }
        }]
      };
      const wrongScope = await request(
        'POST',
        '/remediation/commands/status',
        {
          statuses: [{
            ...completedProjection.statuses[0],
            organizationId: organizationB
          }]
        },
        'service'
      );
      const statusCompleted = await request(
        'POST',
        '/remediation/commands/status',
        completedProjection,
        'service'
      );
      const completedBeforeReplay = await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
        where: { commandId: projectedCommand }
      });
      const statusReplay = await request(
        'POST',
        '/remediation/commands/status',
        completedProjection,
        'service'
      );
      const statusRegression = await request(
        'POST',
        '/remediation/commands/status',
        {
          statuses: [{ ...completedProjection.statuses[0], status: 'running' }]
        },
        'service'
      );
      const completedAfterReplay = await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
        where: { commandId: projectedCommand }
      });
      expect(wrongScope.status).toBe(404);
      expect(statusCompleted.status).toBe(202);
      expect(statusReplay.status).toBe(202);
      expect(statusRegression.status).toBe(409);
      expect(completedAfterReplay.status).toBe('completed');
      expect(completedAfterReplay.finishedAt?.getTime())
        .toBe(completedBeforeReplay.finishedAt?.getTime());

      const multiStep = await prisma.rmmTelemetryRemediationJob.create({
        data: {
          commandId: multiStepCommand,
          organizationId: organizationA,
          agentId: agentA,
          intentId: 'integration.generic',
          status: 'running',
          requestedBy: 'integration-test',
          metadata: {},
          startedAt: new Date(),
          steps: {
            create: [0, 1, 2].map((stepIndex) => ({
              organizationId: organizationA,
              stepIndex,
              command: `echo ${stepIndex}`,
              status: 'pending'
            }))
          }
        }
      });
      const terminalEvidence = {
        steps: [0, 1, 2].map((stepIndex) => ({
          stepIndex,
          status: 'completed',
          exitCode: 0
        }))
      };
      const multiStepCompleted = await prisma.$transaction((tx) => transitionRemediationStatus(
        tx,
        {
          commandId: multiStepCommand,
          organizationId: organizationA,
          agentId: agentA,
          intentScope: 'generic'
        },
        {
          status: 'completed',
          stepIndex: 2,
          hasEvidence: true,
          evidence: terminalEvidence
        }
      ));
      expect(multiStepCompleted).toMatchObject({ outcome: 'updated', jobStatus: 'completed' });
      expect(
        await prisma.rmmTelemetryRemediationStep.findMany({
          where: { jobId: multiStep.id },
          orderBy: { stepIndex: 'asc' },
          select: { status: true }
        })
      ).toEqual([{ status: 'completed' }, { status: 'completed' }, { status: 'completed' }]);

      for (const commandId of [batchCommandA, batchCommandB]) {
        await prisma.rmmTelemetryRemediationJob.create({
          data: {
            commandId,
            organizationId: organizationA,
            agentId: agentA,
            intentId: 'integration.generic',
            status: 'running',
            requestedBy: 'integration-test',
            metadata: {},
            startedAt: new Date(),
            steps: {
              create: {
                organizationId: organizationA,
                stepIndex: 0,
                command: 'echo batch',
                status: 'running'
              }
            }
          }
        });
      }
      const rejectedBatch = await request(
        'POST',
        '/remediation/commands/status',
        {
          statuses: [
            {
              commandId: batchCommandA,
              organizationId: organizationA,
              agentId: agentA,
              status: 'completed',
              stepIndex: 0
            },
            {
              commandId: batchCommandB,
              organizationId: organizationB,
              agentId: agentA,
              status: 'completed',
              stepIndex: 0
            }
          ]
        },
        'service'
      );
      expect(rejectedBatch.status).toBe(404);
      expect(
        (await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
          where: { commandId: batchCommandA }
        })).status
      ).toBe('running');

      const concurrent = await prisma.rmmTelemetryRemediationJob.create({
        data: {
          commandId: concurrentCommand,
          organizationId: organizationA,
          agentId: agentA,
          intentId: 'integration.generic',
          status: 'running',
          requestedBy: 'integration-test',
          metadata: {},
          startedAt: new Date(),
          steps: {
            create: {
              organizationId: organizationA,
              stepIndex: 0,
              command: 'echo concurrent',
              status: 'running'
            }
          }
        }
      });
      const concurrentSelector = {
        commandId: concurrentCommand,
        organizationId: organizationA,
        agentId: agentA,
        intentScope: 'generic' as const
      };
      const [firstTerminal, secondTerminal] = await Promise.all([
        prisma.$transaction((tx) => transitionRemediationStatus(
          tx,
          concurrentSelector,
          { status: 'completed', stepIndex: 0, hasEvidence: true, evidence: { exitCode: 0 } }
        )),
        prisma.$transaction((tx) => transitionRemediationStatus(
          tx,
          concurrentSelector,
          { status: 'failed', stepIndex: 0, hasEvidence: true, evidence: { exitCode: 1 } }
        ))
      ]);
      expect([firstTerminal.outcome, secondTerminal.outcome].sort())
        .toEqual(['conflict', 'updated']);
      const concurrentDurable = await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
        where: { id: concurrent.id }
      });
      expect(['completed', 'failed']).toContain(concurrentDurable.status);
      expect(concurrentDurable.finishedAt).toBeInstanceOf(Date);

      const patch = await prisma.rmmTelemetryRemediationJob.create({
        data: {
          commandId: patchCommand,
          organizationId: organizationA,
          agentId: agentA,
          intentId: 'talos.patch.install',
          status: 'running',
          requestedBy: 'integration-test',
          metadata: {},
          startedAt: new Date(),
          steps: {
            create: {
              organizationId: organizationA,
              stepIndex: 0,
              command: 'talos-patch-install',
              status: 'pending'
            }
          }
        }
      });
      const patchCompleted = await request(
        'PATCH',
        `/remediation/agents/${agentA}/patch-jobs/${patch.id}/status`,
        { status: 'completed', stepIndex: 0, evidence: { installed: true } },
        'rmm'
      );
      const patchRegression = await request(
        'PATCH',
        `/remediation/agents/${agentA}/patch-jobs/${patch.id}/status`,
        { status: 'failed', stepIndex: 0 },
        'rmm'
      );
      expect(patchCompleted.status).toBe(200);
      expect(patchRegression.status).toBe(409);

      const retired = await request(
        'PATCH',
        `/remediation/jobs/${multiStep.id}/status`,
        { status: 'failed' },
        'service'
      );
      expect(retired.status).toBe(410);
      expect(
        (await prisma.rmmTelemetryRemediationJob.findUniqueOrThrow({
          where: { id: multiStep.id }
        })).status
      ).toBe('completed');
    } finally {
      await prisma.rmmTelemetryRemediationJob.deleteMany({
        where: { organizationId: { in: [organizationA, organizationB] } }
      });
      await prisma.rmmDevice.deleteMany({
        where: { agentId: { in: [agentA, agentB] } }
      });
      await prisma.organization.deleteMany({
        where: { id: { in: [organizationA, organizationB] } }
      });
      await new Promise<void>((resolve, reject) => {
        server.close((error) => error ? reject(error) : resolve());
      });
      await prisma.$disconnect();
    }
  }, 30_000);
}
