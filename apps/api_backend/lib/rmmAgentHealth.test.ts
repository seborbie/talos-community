import { describe, expect, test } from 'bun:test';
import {
  buildAgentHealth,
  compareVersions,
  reconcileHealthAlerts,
  type AgentHealthThresholds
} from './rmmAgentHealth';

const now = new Date('2026-05-11T00:00:00.000Z');
const thresholds: AgentHealthThresholds = {
  staleAgentMs: 10 * 60 * 1000,
  offlineAgentMs: 30 * 60 * 1000,
  staleTelemetryMs: 2 * 60 * 60 * 1000,
  rebootRequiredMs: 24 * 60 * 60 * 1000,
  repeatedUpdaterFailureCount: 2
};

describe('rmm agent health', () => {
  test('keeps a fresh connected agent healthy', () => {
    const health = buildAgentHealth({
      now,
      websocketStatus: 'connected',
      lastSeenAt: '2026-05-10T23:58:00.000Z',
      telemetryCollectedAt: '2026-05-10T23:45:00.000Z',
      agentVersion: '0.6.94',
      targetAgentVersion: '0.6.94'
    }, thresholds);

    expect(health.status).toBe('healthy');
    expect(health.reasons).toHaveLength(0);
  });

  test('marks stale and offline agents at separate thresholds', () => {
    const stale = buildAgentHealth({
      now,
      lastSeenAt: '2026-05-10T23:45:00.000Z',
      telemetryCollectedAt: '2026-05-10T23:45:00.000Z'
    }, thresholds);
    const offline = buildAgentHealth({
      now,
      lastSeenAt: '2026-05-10T23:00:00.000Z',
      telemetryCollectedAt: '2026-05-10T23:45:00.000Z'
    }, thresholds);

    expect(stale.status).toBe('warning');
    expect(stale.reasons.map((reason) => reason.code)).toContain('agent_stale');
    expect(offline.status).toBe('offline');
    expect(offline.reasons.map((reason) => reason.code)).toContain('agent_offline');
  });

  test('combines version drift, updater failures, reboot age, and command failures', () => {
    const health = buildAgentHealth({
      now,
      websocketStatus: 'disconnected',
      lastSeenAt: '2026-05-10T23:58:00.000Z',
      telemetryCollectedAt: '2026-05-09T23:00:00.000Z',
      telemetryAgentVersion: '0.6.90',
      targetAgentVersion: '0.6.94',
      rebootRequired: true,
      commandFailureCount: 1,
      updaterFailureCount: 2,
      remediationFailureCount: 1
    }, thresholds);

    expect(health.status).toBe('critical');
    expect(health.reasons.map((reason) => reason.code)).toEqual(expect.arrayContaining([
      'websocket_disconnected',
      'telemetry_stale',
      'agent_version_drift',
      'updater_repeated_failures',
      'recent_command_failures',
      'recent_remediation_failures',
      'reboot_required_aged'
    ]));
  });

  test('compares three-part service versions numerically', () => {
    expect(compareVersions('0.6.94', '0.6.100')).toBe(-1);
    expect(compareVersions('1.0.0', '0.9.99')).toBe(1);
    expect(compareVersions('0.6.94', '0.6.94')).toBe(0);
  });

  test('reconciles alerts without duplicating active issues', () => {
    const health = buildAgentHealth({
      now,
      lastSeenAt: '2026-05-10T23:00:00.000Z',
      telemetryCollectedAt: '2026-05-10T23:45:00.000Z',
      commandFailureCount: 1
    }, thresholds);
    const reconciliation = reconcileHealthAlerts(
      [
        { alertKey: 'agent_offline', status: 'active' },
        { alertKey: 'telemetry_stale', status: 'active' },
        { alertKey: 'recent_command_failures', status: 'resolved' }
      ],
      health.reasons
    );

    expect(reconciliation.newKeys).toEqual([]);
    expect(reconciliation.recurringKeys).toEqual(['recent_command_failures']);
    expect(reconciliation.resolveKeys).toEqual(['telemetry_stale']);
  });
});
