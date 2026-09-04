import { describe, expect, test } from 'bun:test';
import {
  buildRemediationDispatchJobs,
  persistedRemediationMetadata,
  REMEDIATION_DISPATCH_SNAPSHOT_KEY,
  remediationDispatchSnapshot,
} from './remediationDispatch';

describe('remediation dispatch snapshots', () => {
  test('round-trips the frozen execution contract without exposing reserved metadata', () => {
    const persisted = persistedRemediationMetadata({
      metadata: {
        source: 'routing-engine',
        [REMEDIATION_DISPATCH_SNAPSHOT_KEY]: { execution: { maxRetries: 999 } },
      },
      execution: {
        maxRetries: 2,
        timeoutSeconds: 901,
        stopOnFailure: false,
      },
      steps: [
        { stepIndex: 0, command: 'original zero', timeoutSeconds: 17 },
        { stepIndex: 1, command: 'original one', timeoutSeconds: 33 },
      ],
    });

    expect(remediationDispatchSnapshot(persisted)).toEqual({
      metadata: { source: 'routing-engine' },
      execution: {
        maxRetries: 2,
        timeoutSeconds: 901,
        stopOnFailure: false,
      },
      steps: [
        { stepIndex: 0, command: 'original zero', timeoutSeconds: 17 },
        { stepIndex: 1, command: 'original one', timeoutSeconds: 33 },
      ],
    });

    const jobs = buildRemediationDispatchJobs(
      [
        {
          id: 10n,
          command_id: 'command-1',
          organization_id: 'org-1',
          agent_id: 'agent-1',
          decision_id: 42n,
          intent_id: 'generic.intent',
          dedupe_key: 'dedupe-1',
          requested_by: 'operator',
          requested_at: new Date('2026-08-17T12:00:00.000Z'),
          metadata_jsonb: persisted,
        },
      ],
      [
        {
          id: 101n,
          job_id: 10n,
          step_index: 1,
          command: 'durable one',
          status: 'pending',
          evidence_jsonb: null,
        },
        {
          id: 100n,
          job_id: 10n,
          step_index: 0,
          command: 'durable zero',
          status: 'running',
          evidence_jsonb: { pid: 7 },
        },
      ],
    );

    expect(jobs).toEqual([
      {
        commandId: 'command-1',
        organizationId: 'org-1',
        agentId: 'agent-1',
        intentId: 'generic.intent',
        decisionId: '42',
        dedupeKey: 'dedupe-1',
        requestedBy: 'operator',
        requestedAt: '2026-08-17T12:00:00.000Z',
        approvalState: 'approved',
        metadata: { source: 'routing-engine' },
        steps: [
          {
            stepIndex: 0,
            command: 'durable zero',
            timeoutSeconds: 17,
            status: 'running',
            evidence: { pid: 7 },
          },
          {
            stepIndex: 1,
            command: 'durable one',
            timeoutSeconds: 33,
            status: 'pending',
            evidence: null,
          },
        ],
        execution: {
          maxRetries: 2,
          timeoutSeconds: 901,
          stopOnFailure: false,
        },
      },
    ]);
  });
});
