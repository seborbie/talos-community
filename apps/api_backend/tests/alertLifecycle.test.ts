import { describe, expect, test } from 'bun:test';
import {
  filterAlertsBySeverity,
  planAlertLifecycle,
  type AlertLifecycleState
} from '../lib/alertLifecycle';

describe('alert lifecycle noise controls', () => {
  test('suppresses duplicate occurrences inside the dedupe window', () => {
    const firstSeenAt = new Date('2026-05-10T10:00:00.000Z');
    const existing: AlertLifecycleState = {
      status: 'open',
      firstSeenAt,
      lastSeenAt: new Date('2026-05-10T10:01:00.000Z'),
      occurrenceCount: 1
    };

    const plan = planAlertLifecycle(existing, new Date('2026-05-10T10:02:00.000Z'), {
      dedupeWindowSeconds: 300
    });

    expect(plan.status).toBe('open');
    expect(plan.occurrenceCount).toBe(2);
    expect(plan.duplicateSuppressed).toBe(true);
    expect(plan.notificationSuggested).toBe(false);
    expect(plan.reason).toBe('duplicate_suppressed');
  });

  test('re-opens a resolved alert when the condition recurs', () => {
    const existing: AlertLifecycleState = {
      status: 'resolved',
      firstSeenAt: new Date('2026-05-10T09:00:00.000Z'),
      lastSeenAt: new Date('2026-05-10T09:15:00.000Z'),
      occurrenceCount: 2,
      resolvedAt: new Date('2026-05-10T09:20:00.000Z')
    };

    const plan = planAlertLifecycle(existing, new Date('2026-05-10T10:00:00.000Z'), {
      dedupeWindowSeconds: 300
    });

    expect(plan.status).toBe('open');
    expect(plan.reopened).toBe(true);
    expect(plan.resolvedAt).toBeNull();
    expect(plan.occurrenceCount).toBe(3);
    expect(plan.notificationSuggested).toBe(true);
  });

  test('expires snooze state on recurrence after the snooze deadline', () => {
    const existing: AlertLifecycleState = {
      status: 'snoozed',
      firstSeenAt: new Date('2026-05-10T09:00:00.000Z'),
      lastSeenAt: new Date('2026-05-10T09:10:00.000Z'),
      occurrenceCount: 1,
      snoozedUntil: new Date('2026-05-10T09:30:00.000Z')
    };

    const plan = planAlertLifecycle(existing, new Date('2026-05-10T10:00:00.000Z'), {
      dedupeWindowSeconds: 300
    });

    expect(plan.status).toBe('open');
    expect(plan.snoozeExpired).toBe(true);
    expect(plan.snoozedUntil).toBeNull();
    expect(plan.notificationSuggested).toBe(true);
  });

  test('filters alert collections by normalized severity', () => {
    const alerts = [
      { id: '1', severity: 'critical' },
      { id: '2', severity: 'warning' },
      { id: '3', severity: 'low' }
    ];

    expect(filterAlertsBySeverity(alerts, 'medium').map((alert) => alert.id)).toEqual(['2']);
    expect(filterAlertsBySeverity(alerts, 'all')).toHaveLength(3);
  });
});
