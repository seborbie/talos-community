import { describe, expect, test } from 'bun:test';
import { recordPatchActionResultInTransaction } from './patchDecisionService';

function sqlText(query: any): string {
  if (typeof query?.sql === 'string') return query.sql;
  if (Array.isArray(query?.strings)) return query.strings.join('?');
  return String(query);
}

describe('transactional patch action result projection', () => {
  test('uses only the supplied transaction client for terminal side effects', async () => {
    const queries: any[] = [];
    const transaction: any = {
      $queryRaw: async (query: any) => {
        queries.push(query);
        const text = sqlText(query);
        if (text.includes('FROM public.rmm_devices d')) {
          return [{
            organizationId: 'org-a',
            agentId: 'agent-a',
            hostname: 'agent-a',
            os: 'Windows',
            osVersion: null,
            customerId: null,
            customerName: null,
            siteId: null,
            siteName: null,
            lastSeen: new Date('2026-08-17T12:00:00Z'),
            collectedAt: null,
            rebootRequired: false,
            deviceType: null,
            patchRing: null,
            patchManaged: false,
            nativeWindowsUpdateControl: false,
            patchMaintenanceModeUntil: null,
            patchTags: []
          }];
        }
        if (text.includes('FROM public.rmm_patch_decision_log')) return [];
        throw new Error(`unexpected query: ${text}`);
      },
      $executeRaw: async (query: any) => {
        queries.push(query);
        return 1;
      }
    };

    await recordPatchActionResultInTransaction(transaction, {
      organizationId: 'org-a',
      agentId: 'agent-a',
      operationId: 'operation-a',
      action: 'install',
      status: 'completed',
      updateKeys: [],
      evidence: { summary: null, updates: [], currentUpdate: null }
    });

    expect(queries.some((query) => sqlText(query).includes('FROM public.rmm_devices d'))).toBe(true);
    const actionProjection = queries.find((query) =>
      sqlText(query).includes('UPDATE public.rmm_patch_action')
    );
    expect(sqlText(actionProjection)).toContain('evidence_jsonb');
    expect(sqlText(actionProjection)).not.toContain('error_message');
    expect(sqlText(actionProjection)).not.toContain('status =');
    expect(queries.some((query) => sqlText(query).includes('FROM public.rmm_patch_decision_log'))).toBe(true);
    expect(queries.some((query) => sqlText(query).includes('INSERT INTO public.rmm_patch_decision_log'))).toBe(false);
  });
});
