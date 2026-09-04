import { describe, expect, test } from 'bun:test';
import {
  classifyPatchProgressTransition,
  MAX_PATCH_PROGRESS_BYTES,
  MAX_PATCH_PROGRESS_EVIDENCE_BYTES,
  MAX_PATCH_PROGRESS_FUTURE_SKEW_MS,
  parsePatchProgressBatch,
  PatchProgressValidationError,
} from './patchProgressProjection';

function validProgress(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    eventType: 'patch.install.progress',
    organizationId: 'org-a',
    agentId: 'agent-a',
    jobId: 'job-a',
    commandId: 'operation-a',
    status: 'running',
    phase: 'downloading',
    reportedAt: '2026-08-17T12:00:00.123456789+00:00',
    updates: [],
    summary: { matched: 1 },
    ...overrides,
  };
}

describe('patch progress projection validation', () => {
  test('normalizes the current worker payload without changing its wire fields', () => {
    const [parsed] = parsePatchProgressBatch({ progress: [validProgress()] });

    expect(parsed).toMatchObject({
      organizationId: 'org-a',
      agentId: 'agent-a',
      operationId: 'operation-a',
      eventType: 'patch.install.progress',
      status: 'running',
      phase: 'downloading',
      updateKeys: [],
    });
    expect(parsed?.reportedAt.toISOString()).toBe('2026-08-17T12:00:00.123Z');
    expect(parsed?.progress.reportedAt).toBe('2026-08-17T12:00:00.123Z');
    expect(parsed?.evidence).toEqual({
      summary: { matched: 1 },
      updates: [],
      currentUpdate: null,
    });
  });

  test.each([
    [{ status: 'queued' }, 'status must be running, completed, failed, or cancelled'],
    [{ phase: 'Download Updates' }, 'phase must match'],
    [{ phase: `a${'b'.repeat(64)}` }, 'phase must match'],
    [{ reportedAt: '2026-08-17 12:00:00' }, 'reportedAt must be an RFC3339 timestamp'],
    [{ reportedAt: '2026-02-31T12:00:00Z' }, 'reportedAt must be a valid timestamp'],
  ])('rejects an invalid bounded field: %p', (overrides, message) => {
    expect(() => parsePatchProgressBatch(validProgress(overrides))).toThrow(message);
  });

  test('enforces independent encoded limits for progress and projected evidence', () => {
    try {
      parsePatchProgressBatch(validProgress({ extra: 'x'.repeat(MAX_PATCH_PROGRESS_BYTES) }));
      throw new Error('expected oversized progress to fail');
    } catch (error) {
      expect(error).toBeInstanceOf(PatchProgressValidationError);
      expect((error as PatchProgressValidationError).httpStatus).toBe(413);
      expect((error as Error).message).toContain('progress must not exceed');
    }

    try {
      parsePatchProgressBatch(
        validProgress({
          updates: [{ title: 'x'.repeat(MAX_PATCH_PROGRESS_EVIDENCE_BYTES) }],
        }),
      );
      throw new Error('expected oversized evidence to fail');
    } catch (error) {
      expect(error).toBeInstanceOf(PatchProgressValidationError);
      expect((error as PatchProgressValidationError).httpStatus).toBe(413);
      expect((error as Error).message).toContain('progress evidence must not exceed');
    }
  });

  test('rejects a timestamp that could pin the operation in the future', () => {
    const serverNow = new Date('2026-08-17T12:00:00Z');
    const withinBound = new Date(
      serverNow.getTime() + MAX_PATCH_PROGRESS_FUTURE_SKEW_MS,
    ).toISOString();
    const beyondBound = new Date(
      serverNow.getTime() + MAX_PATCH_PROGRESS_FUTURE_SKEW_MS + 1,
    ).toISOString();

    expect(() =>
      parsePatchProgressBatch(validProgress({ reportedAt: withinBound }), serverNow),
    ).not.toThrow();
    expect(() =>
      parsePatchProgressBatch(validProgress({ reportedAt: beyondBound }), serverNow),
    ).toThrow('must not be more than 10 minutes in the future');
  });
});

describe('patch progress transition policy', () => {
  const earlier = { status: 'running' as const, reportedAt: new Date('2026-08-17T12:00:00Z') };
  const later = { status: 'running' as const, reportedAt: new Date('2026-08-17T12:01:00Z') };

  test('accepts a first report and an equal-or-newer nonterminal report', () => {
    expect(classifyPatchProgressTransition(null, earlier)).toBe('apply');
    expect(
      classifyPatchProgressTransition({ status: 'running', reportedAt: earlier.reportedAt }, later),
    ).toBe('apply');
  });

  test('rejects an older report', () => {
    expect(
      classifyPatchProgressTransition({ status: 'running', reportedAt: later.reportedAt }, earlier),
    ).toBe('stale');
  });

  test('makes terminal states immutable and recognizes same-state replays', () => {
    expect(
      classifyPatchProgressTransition(
        { status: 'completed', reportedAt: earlier.reportedAt },
        { status: 'running', reportedAt: later.reportedAt },
      ),
    ).toBe('terminal_conflict');
    expect(
      classifyPatchProgressTransition(
        { status: 'completed', reportedAt: earlier.reportedAt },
        { status: 'failed', reportedAt: later.reportedAt },
      ),
    ).toBe('terminal_conflict');
    expect(
      classifyPatchProgressTransition(
        { status: 'completed', reportedAt: earlier.reportedAt },
        { status: 'completed', reportedAt: later.reportedAt },
      ),
    ).toBe('duplicate_terminal');
  });
});
