import { describe, expect, test } from 'bun:test';
import { buildAuditCreateData } from '../lib/audit';

describe('audit event payloads', () => {
  test('records command execution scope and result', () => {
    const data = buildAuditCreateData({
      organizationId: 'org-1',
      customerId: 'cust-1',
      agentId: 'agent-1',
      userId: 'user-1',
      userEmail: 'operator@example.com',
      actionType: 'command.execute',
      targetType: 'rmm_device',
      targetId: 'agent-1',
      result: 'success',
      correlationId: 'request-1',
      metadata: {
        command: 'Get-Service',
        matchedPolicyId: '42',
        exitCode: 0
      }
    });

    expect(data.organizationId).toBe('org-1');
    expect(data.customerId).toBe('cust-1');
    expect(data.agentId).toBe('agent-1');
    expect(data.actionType).toBe('command.execute');
    expect(data.result).toBe('success');
    expect(data.correlationId).toBe('request-1');
    expect(data.metadata).toEqual({
      command: 'Get-Service',
      matchedPolicyId: '42',
      exitCode: 0
    });
  });

  test('records policy update before and after values', () => {
    const data = buildAuditCreateData({
      organizationId: 'org-1',
      userId: 'admin-1',
      actionType: 'policy.update',
      targetType: 'command_policy',
      targetId: '7',
      targetName: 'Get-CimInstance',
      result: 'success',
      metadata: {
        previous: { policyType: 'allow', reason: null },
        next: { policyType: 'deny', reason: 'maintenance window only' }
      }
    });

    expect(data.actionType).toBe('policy.update');
    expect(data.targetType).toBe('command_policy');
    expect(data.targetId).toBe('7');
    expect(data.metadata).toEqual({
      previous: { policyType: 'allow', reason: null },
      next: { policyType: 'deny', reason: 'maintenance window only' }
    });
  });

  test('records remote session start and end correlation', () => {
    const start = buildAuditCreateData({
      organizationId: 'org-1',
      siteId: 'site-1',
      agentId: 'agent-1',
      userId: 'viewer-1',
      actionType: 'remote_desktop.start',
      targetType: 'rmm_device',
      targetId: 'agent-1',
      sessionId: 'session-1',
      metadata: { transports: ['relay'] }
    });
    const end = buildAuditCreateData({
      organizationId: 'org-1',
      siteId: 'site-1',
      agentId: 'agent-1',
      userId: 'viewer-1',
      actionType: 'remote_desktop.end',
      targetType: 'rmm_device',
      targetId: 'agent-1',
      sessionId: 'session-1',
      metadata: { endedBy: 'viewer' }
    });

    expect(start.sessionId).toBe('session-1');
    expect(end.sessionId).toBe('session-1');
    expect(start.actionType).toBe('remote_desktop.start');
    expect(end.actionType).toBe('remote_desktop.end');
  });
});
