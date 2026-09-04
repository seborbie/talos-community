import { describe, expect, test } from 'bun:test';
import {
  calculatePatchComplianceSummary,
  CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
  DEFAULT_PATCH_POLICY_PRIORITY,
  resolveEffectivePatchPolicy,
  type PatchPolicyForResolution
} from '../lib/patchManagement';
import {
  classifyPatchCategory,
  coercePatchPolicyConfig,
  defaultPatchPolicyConfig,
  evaluatePatchActionPlan,
  isWithinPatchWindow,
  type PatchPolicyConfig
} from '../lib/patchDecisionEngine';
import {
  inferPatchProgressActionType,
  projectPatchActionResultUpdates,
  selectActionablePatchUpdateTargetAgentIds
} from '../lib/patchDecisionService';
import {
  selectPostPatchRebootLoopFailureKeys,
  shouldClearRebootForFailedPendingUpdates
} from '../lib/patchRebootLoop';

const policies: PatchPolicyForResolution[] = [
  {
    id: 'org',
    scopeType: 'organization',
    scopeKey: 'org-1',
    approvalMode: 'manual',
    maintenanceWindowStart: '22:00',
    maintenanceWindowEnd: '04:00',
    maintenanceWindowTimezone: 'UTC',
    rebootBehavior: 'allow',
    deferralDays: 7,
    priority: CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
    enabled: true,
    updatedAt: '2026-05-01T00:00:00.000Z'
  },
  {
    id: 'customer',
    scopeType: 'customer',
    scopeKey: 'cust-1',
    approvalMode: 'auto_approve_security',
    maintenanceWindowStart: '21:00',
    maintenanceWindowEnd: '03:00',
    maintenanceWindowTimezone: 'UTC',
    rebootBehavior: 'suppress',
    deferralDays: 3,
    priority: CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
    enabled: true,
    updatedAt: '2026-05-02T00:00:00.000Z'
  },
  {
    id: 'site',
    scopeType: 'site',
    scopeKey: 'site-1',
    approvalMode: 'auto_approve_all',
    maintenanceWindowStart: null,
    maintenanceWindowEnd: null,
    maintenanceWindowTimezone: 'UTC',
    rebootBehavior: 'force',
    deferralDays: 0,
    priority: CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
    enabled: true,
    updatedAt: '2026-05-03T00:00:00.000Z'
  },
  {
    id: 'device',
    scopeType: 'device',
    scopeKey: 'agent-1',
    approvalMode: 'manual',
    maintenanceWindowStart: '23:00',
    maintenanceWindowEnd: '02:00',
    maintenanceWindowTimezone: 'UTC',
    rebootBehavior: 'allow',
    deferralDays: 1,
    priority: CUSTOM_PATCH_POLICY_DEFAULT_PRIORITY,
    enabled: true,
    updatedAt: '2026-05-04T00:00:00.000Z'
  }
];

const defaultPolicy: PatchPolicyForResolution = {
  id: 'default',
  scopeType: 'organization',
  scopeKey: '__talos_default_patch_policy__',
  approvalMode: 'auto_approve_all',
  maintenanceWindowStart: null,
  maintenanceWindowEnd: null,
  maintenanceWindowTimezone: 'UTC',
  rebootBehavior: 'allow',
  deferralDays: 0,
  priority: DEFAULT_PATCH_POLICY_PRIORITY,
  enabled: true,
  isDefault: true,
  updatedAt: '2026-05-05T00:00:00.000Z'
};

describe('patch policy precedence', () => {
  test('uses device over site, customer, and organization', () => {
    const policy = resolveEffectivePatchPolicy(policies, {
      organizationId: 'org-1',
      customerId: 'cust-1',
      siteId: 'site-1',
      agentId: 'agent-1'
    });

    expect(policy?.id).toBe('device');
  });

  test('falls back from site to customer to organization', () => {
    const sitePolicy = resolveEffectivePatchPolicy(policies, {
      organizationId: 'org-1',
      customerId: 'cust-1',
      siteId: 'site-1',
      agentId: 'agent-2'
    });
    const customerPolicy = resolveEffectivePatchPolicy(policies, {
      organizationId: 'org-1',
      customerId: 'cust-1',
      siteId: null,
      agentId: 'agent-3'
    });
    const orgPolicy = resolveEffectivePatchPolicy(policies, {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-4'
    });

    expect(sitePolicy?.id).toBe('site');
    expect(customerPolicy?.id).toBe('customer');
    expect(orgPolicy?.id).toBe('org');
  });

  test('uses default policy only when no normal policy matches', () => {
    const fallback = resolveEffectivePatchPolicy([defaultPolicy], {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-5'
    });
    const overridden = resolveEffectivePatchPolicy([defaultPolicy, policies[0]], {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-6'
    });

    expect(fallback?.id).toBe('default');
    expect(overridden?.id).toBe('org');
  });

  test('uses the lowest numeric priority before scope specificity', () => {
    const policy = resolveEffectivePatchPolicy(
      [
        { ...policies[0], id: 'org-priority-10', priority: 10 },
        { ...policies[3], id: 'device-priority-100', priority: 100 }
      ],
      {
        organizationId: 'org-1',
        customerId: 'cust-1',
        siteId: 'site-1',
        agentId: 'agent-1'
      }
    );

    expect(policy?.id).toBe('org-priority-10');
  });

  test('ignores disabled policies even when their priority is higher precedence', () => {
    const policy = resolveEffectivePatchPolicy(
      [
        { ...policies[0], id: 'disabled-org-priority-1', priority: 1, enabled: false },
        { ...policies[3], id: 'device-priority-100', priority: 100 }
      ],
      {
        organizationId: 'org-1',
        customerId: 'cust-1',
        siteId: 'site-1',
        agentId: 'agent-1'
      }
    );

    expect(policy?.id).toBe('device-priority-100');
  });

  test('treats policies without an OS target as all devices', () => {
    const policy = resolveEffectivePatchPolicy(
      [{ ...policies[0], id: 'legacy-all', targetOsFamily: undefined }],
      {
        organizationId: 'org-1',
        customerId: null,
        siteId: null,
        agentId: 'agent-windows',
        osFamily: 'windows'
      }
    );

    expect(policy?.id).toBe('legacy-all');
  });

  test('matches Windows, Linux, and macOS policy targets only to matching devices', () => {
    const targetedPolicies: PatchPolicyForResolution[] = [
      { ...policies[0], id: 'all-fallback', targetOsFamily: 'all', priority: 100 },
      { ...policies[0], id: 'windows-target', targetOsFamily: 'windows', priority: 10 },
      { ...policies[0], id: 'linux-target', targetOsFamily: 'linux', priority: 10 },
      { ...policies[0], id: 'macos-target', targetOsFamily: 'macos', priority: 10 }
    ];

    const windowsPolicy = resolveEffectivePatchPolicy(targetedPolicies, {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-windows',
      osFamily: 'windows'
    });
    const linuxPolicy = resolveEffectivePatchPolicy(targetedPolicies, {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-linux',
      osFamily: 'linux'
    });
    const macPolicy = resolveEffectivePatchPolicy(targetedPolicies, {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-mac',
      osFamily: 'macos'
    });

    expect(windowsPolicy?.id).toBe('windows-target');
    expect(linuxPolicy?.id).toBe('linux-target');
    expect(macPolicy?.id).toBe('macos-target');
  });

  test('uses lower-priority OS-specific policy only when the OS matches', () => {
    const allPolicy = { ...policies[0], id: 'all-priority-50', targetOsFamily: 'all' as const, priority: 50 };
    const windowsPolicy = { ...policies[0], id: 'windows-priority-1', targetOsFamily: 'windows' as const, priority: 1 };

    const matched = resolveEffectivePatchPolicy([allPolicy, windowsPolicy], {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-windows',
      osFamily: 'windows'
    });
    const fallback = resolveEffectivePatchPolicy([allPolicy, windowsPolicy], {
      organizationId: 'org-1',
      customerId: null,
      siteId: null,
      agentId: 'agent-linux',
      osFamily: 'linux'
    });

    expect(matched?.id).toBe('windows-priority-1');
    expect(fallback?.id).toBe('all-priority-50');
  });

  test('uses specificity and then newest update when priorities are tied', () => {
    const scopedPolicy = resolveEffectivePatchPolicy(
      [
        { ...policies[0], id: 'org-tie', priority: 50 },
        { ...policies[2], id: 'site-tie', priority: 50 }
      ],
      {
        organizationId: 'org-1',
        customerId: 'cust-1',
        siteId: 'site-1',
        agentId: 'agent-2'
      }
    );
    const newestPolicy = resolveEffectivePatchPolicy(
      [
        { ...policies[0], id: 'org-older', priority: 50, updatedAt: '2026-05-01T00:00:00.000Z' },
        { ...policies[0], id: 'org-newer', priority: 50, updatedAt: '2026-05-02T00:00:00.000Z' }
      ],
      {
        organizationId: 'org-1',
        customerId: null,
        siteId: null,
        agentId: 'agent-2'
      }
    );

    expect(scopedPolicy?.id).toBe('site-tie');
    expect(newestPolicy?.id).toBe('org-newer');
  });
});

describe('patch action result projection', () => {
  test('classifies failed finalizing install progress as install', () => {
    expect(inferPatchProgressActionType({
      eventType: 'patch.install.progress',
      phase: 'finalizing',
      status: 'failed',
      summary: { downloaded: 1, installed: 0 }
    })).toBe('install');
  });

  test('prefers existing action type over progress payload heuristics', () => {
    expect(inferPatchProgressActionType({
      eventType: 'patch.install.progress',
      phase: 'downloading',
      status: 'running',
      summary: { downloaded: 1, installed: 0 },
      existingActionType: 'install'
    })).toBe('install');
  });

  test('classifies completed download-only terminal progress as download', () => {
    expect(inferPatchProgressActionType({
      eventType: 'patch.install.progress',
      phase: 'finalizing',
      status: 'completed',
      summary: { downloaded: 2, installed: 0 }
    })).toBe('download');
  });

  test('uses per-update install evidence instead of marking every requested update installed', () => {
    const result = projectPatchActionResultUpdates({
      action: 'install',
      status: 'failed',
      updateKeys: ['openssl 3.0.13-0ubuntu3.6|', 'curl 8.5.0-2ubuntu10.6|'],
      evidence: {
        updates: [
          {
            updateKey: 'openssl 3.0.13-0ubuntu3.6|',
            matched: true,
            downloaded: true,
            installed: true,
            requiresReboot: true,
            result: 'installed'
          },
          {
            updateKey: 'curl 8.5.0-2ubuntu10.6|',
            matched: true,
            downloaded: true,
            installed: false,
            result: 'failed',
            error: 'apt failed'
          },
          {
            updateKey: 'missing 1.0|',
            matched: false,
            downloaded: false,
            installed: false,
            result: 'not_found'
          }
        ]
      }
    });

    expect(result.usedEvidence).toBe(true);
    expect(result.updates).toEqual([
      {
        updateKey: 'openssl 3.0.13-0ubuntu3.6|',
        lifecycleState: 'installed',
        requiresReboot: true,
        failureMessage: null
      },
      {
        updateKey: 'curl 8.5.0-2ubuntu10.6|',
        lifecycleState: 'failed',
        requiresReboot: null,
        failureMessage: expect.any(String)
      }
    ]);
  });

  test('ignores non-targeted macOS evidence rows when an install fails', () => {
    const result = projectPatchActionResultUpdates({
      action: 'install',
      status: 'failed',
      updateKeys: ['macos tahoe 26.5|', 'safari 18.5|'],
      evidence: {
        updates: [
          {
            updateKey: 'macos tahoe 26.5|',
            matched: true,
            selected: true,
            downloaded: false,
            installed: false,
            result: 'failed'
          },
          {
            updateKey: 'safari 18.5|',
            matched: false,
            selected: false,
            downloaded: false,
            installed: false,
            result: 'skipped'
          },
          {
            updateKey: 'xcode command line tools|',
            selected: false,
            downloaded: false,
            installed: false,
            result: 'skipped'
          },
          {
            updateKey: 'printer driver|',
            downloaded: false,
            installed: false,
            result: 'skipped'
          }
        ]
      }
    });

    expect(result.usedEvidence).toBe(true);
    expect(result.updates).toEqual([
      {
        updateKey: 'macos tahoe 26.5|',
        lifecycleState: 'failed',
        requiresReboot: null,
        failureMessage: expect.any(String)
      }
    ]);
  });

  test('falls back to bulk status mapping for older workers without update evidence', () => {
    const result = projectPatchActionResultUpdates({
      action: 'download',
      status: 'completed',
      updateKeys: ['openssl 3.0.13-0ubuntu3.6|', 'curl 8.5.0-2ubuntu10.6|'],
      evidence: { summary: { downloaded: 2 } }
    });

    expect(result.usedEvidence).toBe(false);
    expect(result.updates.map((update) => update.lifecycleState)).toEqual(['downloaded', 'downloaded']);
  });

  test('uses terminal progress state evidence even when update keys are omitted', () => {
    const result = projectPatchActionResultUpdates({
      action: 'download',
      status: 'completed',
      updateKeys: [],
      evidence: {
        updates: [
          {
            updateKey: 'networkmanager 1:1.54.3-2.fc43|',
            state: 'downloaded'
          },
          {
            updateKey: 'systemd 258.7-1.fc43|',
            state: 'downloaded',
            requiresReboot: true
          }
        ]
      }
    });

    expect(result.usedEvidence).toBe(true);
    expect(result.updates).toEqual([
      {
        updateKey: 'networkmanager 1:1.54.3-2.fc43|',
        lifecycleState: 'downloaded',
        requiresReboot: null,
        failureMessage: null
      },
      {
        updateKey: 'systemd 258.7-1.fc43|',
        lifecycleState: 'downloaded',
        requiresReboot: true,
        failureMessage: null
      }
    ]);
  });
});

describe('patch update action targeting', () => {
  test('selects only applicable non-installed update targets', () => {
    const result = selectActionablePatchUpdateTargetAgentIds(
      [
        {
          agentId: 'agent-1',
          updateKey: 'openssl|kb1',
          applicabilityState: 'applicable',
          lifecycleState: 'detected'
        },
        {
          agentId: 'agent-1',
          updateKey: 'openssl|kb1',
          applicabilityState: 'applicable',
          lifecycleState: 'downloaded'
        },
        {
          agentId: 'agent-2',
          updateKey: 'openssl|kb1',
          applicabilityState: 'applicable',
          lifecycleState: 'installed'
        },
        {
          agentId: 'agent-3',
          updateKey: 'openssl|kb1',
          applicabilityState: 'not_applicable',
          lifecycleState: 'detected'
        },
        {
          agentId: 'agent-4',
          updateKey: 'curl|kb2',
          applicabilityState: 'applicable',
          lifecycleState: 'detected'
        },
        {
          agentId: 'agent-5',
          updateKey: 'openssl|kb1',
          applicabilityState: 'applicable',
          lifecycleState: 'superseded'
        }
      ],
      ['openssl|kb1']
    );

    expect(result).toEqual(['agent-1']);
  });
});

describe('patch compliance summary', () => {
  test('counts critical/security updates, reboots, unknown scans, and compliant devices', () => {
    const summary = calculatePatchComplianceSummary(
      [
        {
          agentId: 'agent-1',
          hostname: 'win-critical',
          os: 'Windows 11',
          customerId: 'cust-1',
          siteId: 'site-1',
          lastScanAt: '2026-05-10T10:00:00.000Z',
          rebootRequired: false,
          installStatus: 'queued',
          pendingUpdates: [
            {
              title: 'Critical Update for Windows 11 (KB5000001)',
              titleNorm: 'critical update for windows 11',
              kbArticle: 'KB5000001',
              requiresReboot: true
            },
            {
              title: 'Security Intelligence Update for Microsoft Defender',
              titleNorm: 'security intelligence update for microsoft defender',
              kbArticle: 'KB2267602'
            }
          ]
        },
        {
          agentId: 'agent-2',
          hostname: 'win-clean',
          os: 'Windows 11',
          customerId: 'cust-1',
          siteId: 'site-1',
          lastScanAt: '2026-05-10T10:00:00.000Z',
          rebootRequired: false,
          pendingUpdates: []
        },
        {
          agentId: 'agent-3',
          hostname: 'win-unknown',
          os: 'Windows 11',
          customerId: 'cust-1',
          siteId: null,
          lastScanAt: null,
          rebootRequired: false,
          pendingUpdates: []
        }
      ],
      policies,
      [
        {
          agentId: 'agent-1',
          updateKey: 'security intelligence update for microsoft defender|kb2267602',
          decision: 'approved'
        }
      ],
      'org-1'
    );

    expect(summary.totals.devices).toBe(3);
    expect(summary.totals.critical).toBe(1);
    expect(summary.totals.compliant).toBe(1);
    expect(summary.totals.unknown).toBe(1);
    expect(summary.totals.missingCritical).toBe(1);
    expect(summary.totals.missingSecurity).toBe(1);
    expect(summary.totals.rebootRequired).toBe(1);
    expect(summary.items[0].installStatus).toBe('queued');
    expect(summary.items[0].updates[1].approvalDecision).toBe('approved');
  });

  test('clears completed install status when a newer scan finds pending updates', () => {
    const summary = calculatePatchComplianceSummary(
      [
        {
          agentId: 'agent-1',
          hostname: 'win-new-patch',
          os: 'Windows 11',
          lastScanAt: '2026-05-20T11:20:00.000Z',
          rebootRequired: false,
          installStatus: 'completed',
          installStatusAt: '2026-05-20T10:45:00.000Z',
          pendingUpdates: [
            {
              title: 'Security Intelligence Update for Microsoft Defender',
              kbArticle: 'KB2267602'
            }
          ]
        }
      ],
      policies,
      [],
      'org-1'
    );

    expect(summary.items[0].installStatus).toBe('not_requested');
    expect(summary.items[0].pendingUpdatesCount).toBe(1);
    expect(summary.items[0].complianceStatus).toBe('security');
  });

  test('keeps completed install status when pending update data is older than the install', () => {
    const summary = calculatePatchComplianceSummary(
      [
        {
          agentId: 'agent-1',
          hostname: 'win-old-scan',
          os: 'Windows 11',
          lastScanAt: '2026-05-20T10:00:00.000Z',
          rebootRequired: false,
          installStatus: 'completed',
          installStatusAt: '2026-05-20T10:45:00.000Z',
          pendingUpdates: [
            {
              title: 'Security Intelligence Update for Microsoft Defender',
              kbArticle: 'KB2267602'
            }
          ]
        }
      ],
      policies,
      [],
      'org-1'
    );

    expect(summary.items[0].installStatus).toBe('completed');
  });
});

describe('patch reboot loop guard', () => {
  test('selects same reboot-required updates after a Talos patch reboot', () => {
    const failedKeys = selectPostPatchRebootLoopFailureKeys({
      previousBootSessionId: 'boot-a',
      currentBootSessionId: 'boot-b',
      previousRebootRequired: true,
      currentRebootRequired: true,
      hadPatchRebootIntent: true,
      pendingRebootUpdateKeys: ['macos tahoe 26.5|', 'other reboot update|'],
      previousRebootUpdateKeys: ['macos tahoe 26.5|']
    });

    expect(failedKeys).toEqual(['macos tahoe 26.5|']);
  });

  test('does not select failures without a boot change or Talos reboot intent', () => {
    expect(selectPostPatchRebootLoopFailureKeys({
      previousBootSessionId: 'boot-a',
      currentBootSessionId: 'boot-a',
      previousRebootRequired: true,
      currentRebootRequired: true,
      hadPatchRebootIntent: true,
      pendingRebootUpdateKeys: ['macos tahoe 26.5|'],
      previousRebootUpdateKeys: ['macos tahoe 26.5|']
    })).toEqual([]);

    expect(selectPostPatchRebootLoopFailureKeys({
      previousBootSessionId: 'boot-a',
      currentBootSessionId: 'boot-b',
      previousRebootRequired: true,
      currentRebootRequired: true,
      hadPatchRebootIntent: false,
      pendingRebootUpdateKeys: ['macos tahoe 26.5|'],
      previousRebootUpdateKeys: ['macos tahoe 26.5|']
    })).toEqual([]);
  });

  test('does not require the previous snapshot device reboot flag', () => {
    expect(selectPostPatchRebootLoopFailureKeys({
      previousBootSessionId: 'boot-a',
      currentBootSessionId: 'boot-b',
      previousRebootRequired: false,
      currentRebootRequired: true,
      hadPatchRebootIntent: true,
      pendingRebootUpdateKeys: ['macos tahoe 26.5|'],
      previousRebootUpdateKeys: ['macos tahoe 26.5|']
    })).toEqual(['macos tahoe 26.5|']);
  });

  test('clears reboot only when all current reboot-required updates are guarded failures', () => {
    expect(shouldClearRebootForFailedPendingUpdates(
      ['macos tahoe 26.5|'],
      ['macos tahoe 26.5|']
    )).toBe(true);
    expect(shouldClearRebootForFailedPendingUpdates(
      ['macos tahoe 26.5|', 'firmware 1.2|'],
      ['macos tahoe 26.5|']
    )).toBe(false);
  });
});

describe('patch decision engine', () => {
  const decisionDevice = {
    organizationId: 'org-1',
    agentId: 'agent-1',
    hostname: 'DESKTOP-1',
    os: 'Windows 11 Pro',
    customerId: 'cust-1',
    siteId: 'site-1',
    deviceType: 'workstation' as const,
    patchRing: 'broad' as const,
    patchManaged: true,
    nativeWindowsUpdateControl: true,
    lastScanAt: '2026-05-20T10:00:00.000Z'
  };

  test('classifies macOS OS-version updates as feature after severity checks', () => {
    expect(classifyPatchCategory({ title: 'macOS Tahoe 26.5' })).toBe('feature');
    expect(classifyPatchCategory({ title: 'macOS Sequoia 15.5' })).toBe('feature');
    expect(classifyPatchCategory({ title: 'Mac OS Sonoma 14.5' })).toBe('feature');
    expect(classifyPatchCategory({ title: 'macOS Security Response 14.5' })).toBe('security');
    expect(classifyPatchCategory({ title: 'Critical Update for macOS Sonoma 14.5' })).toBe('critical');
    expect(classifyPatchCategory({ title: 'Safari for macOS 18.5' })).toBe('other');
  });

  test('handles cross-midnight maintenance windows', () => {
    expect(isWithinPatchWindow({
      enabled: true,
      start: '22:00',
      end: '04:00',
      timezone: 'UTC'
    }, '2026-05-20T23:30:00.000Z')).toBe(true);
    expect(isWithinPatchWindow({
      enabled: true,
      start: '22:00',
      end: '04:00',
      timezone: 'UTC'
    }, '2026-05-20T12:30:00.000Z')).toBe(false);
  });

  test('blocks a deferred KB until its release-age eligibility date', () => {
    const config: PatchPolicyConfig = coercePatchPolicyConfig({
      ...policies[0],
      policyConfig: {
        categories: {
          security: {
            approval: 'auto',
            installAfterDays: 14,
            forceInstallByDays: 21,
            forceRebootByDays: 24
          }
        }
      }
    });

    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: decisionDevice,
      policy: { ...policies[0], policyConfig: config },
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-05-19T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ]
    });

    const deferAction = plan.actions.find((action) => action.action === 'defer' && action.updateKeys.includes('security update|kb5030000'));
    expect(deferAction?.notBefore).toBe('2026-06-02T00:00:00.000Z');
    expect(deferAction?.reason).toContain('2026-06-02');
  });

  test('blocks preview updates by default', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: decisionDevice,
      policy: policies[0],
      updates: [
        {
          updateKey: 'preview update|kb5031111',
          title: 'Preview Cumulative Update for Windows (KB5031111)',
          kbArticle: 'KB5031111',
          releaseDate: '2026-04-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ]
    });

    expect(plan.actions.some((action) => action.action === 'blocked' && action.reason.includes('preview'))).toBe(true);
  });

  test('does not emit Windows Update native control actions for macOS devices', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        os: 'macOS Tahoe',
        nativeWindowsUpdateControl: true
      },
      policy: defaultPolicy,
      updates: []
    });

    expect(plan.nativeWindowsUpdateControl).toBe(false);
    expect(plan.actions.some((action) => action.action === 'applyNativeControl')).toBe(false);
  });

  test('schedules macOS softwareupdate installs without Windows native control', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        os: 'macOS Tahoe',
        nativeWindowsUpdateControl: true
      },
      policy: {
        ...policies[0],
        targetOsFamily: 'macos',
        nativeWindowsUpdateControl: false,
        policyConfig: {
          ...defaultPatchPolicyConfig(0),
          nativeWindowsUpdateControl: false
        }
      },
      updates: [
        {
          updateKey: 'safari 18.5|',
          title: 'Safari 18.5',
          category: 'security',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ]
    });

    expect(plan.nativeWindowsUpdateControl).toBe(false);
    expect(plan.actions.some((action) => action.action === 'applyNativeControl')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'install' && action.updateKeys.includes('safari 18.5|'))).toBe(true);
  });

  test('emergency approval bypasses deferral and schedules install', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: decisionDevice,
      policy: {
        ...policies[0],
        policyConfig: {
          categories: {
            security: {
              approval: 'auto',
              installAfterDays: 14,
              forceInstallByDays: 21,
              forceRebootByDays: 24
            }
          }
        }
      },
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-05-19T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'override-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'emergency_approve',
          updateKey: 'security update|kb5030000',
          reason: 'Emergency out-of-band patch'
        }
      ]
    });

    expect(plan.actions.some((action) => action.action === 'install' && action.updateKeys.includes('security update|kb5030000'))).toBe(true);
  });

  test('manual download override schedules install without reboot', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        rebootRequired: true
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-04-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'download-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_download',
          reason: 'Download and install'
        }
      ]
    });

    expect(plan.actions.some((action) =>
      action.action === 'install' &&
      action.updateKeys.includes('security update|kb5030000') &&
      action.metadata?.rebootBehavior === 'suppress'
    )).toBe(true);
    expect(plan.actions.some((action) => action.action === 'reboot')).toBe(false);
  });

  test('broad macOS download override installs only the latest OS update', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        os: 'macOS Ventura 13.7.7',
        nativeWindowsUpdateControl: false
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'macos tahoe 26.5.1|',
          title: 'macOS Tahoe 26.5.1',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        },
        {
          updateKey: 'macos ventura 13.7.8|',
          title: 'macOS Ventura 13.7.8',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        },
        {
          updateKey: 'safari|',
          title: 'Safari',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'download-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_download',
          reason: 'Download and install'
        }
      ]
    });

    const installAction = plan.actions.find((action) => action.action === 'install');
    expect(installAction?.updateKeys).toContain('macos tahoe 26.5.1|');
    expect(installAction?.updateKeys).toContain('safari|');
    expect(installAction?.updateKeys).not.toContain('macos ventura 13.7.8|');
    expect(plan.actions.some((action) =>
      action.action === 'blocked' &&
      action.updateKeys.includes('macos ventura 13.7.8|') &&
      action.reason.includes('multiple macOS OS-version updates')
    )).toBe(true);
  });

  test('broad macOS install override installs the latest OS update when OS version is unknown', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        os: 'macOS',
        nativeWindowsUpdateControl: false
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'macos tahoe 26.5.1|',
          title: 'macOS Tahoe 26.5.1',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        },
        {
          updateKey: 'macos ventura 13.7.8|',
          title: 'macOS Ventura 13.7.8',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        },
        {
          updateKey: 'safari|',
          title: 'Safari',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'install-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_install',
          reason: 'Install'
        }
      ]
    });

    const installAction = plan.actions.find((action) => action.action === 'install');
    expect(installAction?.updateKeys).toContain('macos tahoe 26.5.1|');
    expect(installAction?.updateKeys).toContain('safari|');
    expect(installAction?.updateKeys).not.toContain('macos ventura 13.7.8|');
    expect(plan.actions.filter((action) => action.action === 'blocked')).toHaveLength(1);
  });

  test('specific macOS install override keeps the requested OS update', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        os: 'macOS Ventura 13.7.7',
        nativeWindowsUpdateControl: false
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'macos tahoe 26.5.1|',
          title: 'macOS Tahoe 26.5.1',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        },
        {
          updateKey: 'macos ventura 13.7.8|',
          title: 'macOS Ventura 13.7.8',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'install-tahoe',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_install',
          updateKey: 'macos tahoe 26.5.1|',
          reason: 'Install Tahoe'
        }
      ]
    });

    const installAction = plan.actions.find((action) => action.action === 'install');
    expect(installAction?.updateKeys).toEqual(['macos tahoe 26.5.1|']);
  });

  test('manual scan override preserves queued operation id', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        lastScanAt: '2026-05-20T11:59:00.000Z'
      },
      policy: defaultPolicy,
      updates: [],
      overrides: [
        {
          id: 'scan-1',
          operationId: 'operation-scan-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_scan',
          reason: 'Scan now'
        }
      ]
    });

    const scanAction = plan.actions.find((action) => action.action === 'scan');
    expect(scanAction?.operationId).toBe('operation-scan-1');
    expect(scanAction?.metadata.overrideIds).toContain('scan-1');
  });

  test('manual scan override does not schedule download, install, or reboot', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        rebootRequired: true,
        lastScanAt: '2026-05-20T11:59:00.000Z'
      },
      policy: {
        ...defaultPolicy,
        policyConfig: {
          windows: {
            scan: { enabled: true, start: null, end: null, timezone: 'UTC' },
            download: { enabled: true, start: null, end: null, timezone: 'UTC' },
            install: { enabled: true, start: '00:00', end: '00:01', timezone: 'UTC' },
            reboot: { enabled: true, start: '00:00', end: '00:01', timezone: 'UTC' }
          }
        }
      },
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-04-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'scan-1',
          operationId: 'operation-scan-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_scan',
          reason: 'Scan now'
        }
      ]
    });

    expect(plan.actions.some((action) => action.action === 'scan')).toBe(true);
    expect(plan.actions.some((action) => action.action === 'download')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'install')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'reboot')).toBe(false);
  });

  test('restricted install window constrains legacy open download window', () => {
    const config = coercePatchPolicyConfig({
      ...defaultPolicy,
      policyConfig: {
        windows: {
          download: { enabled: true, start: null, end: null, timezone: 'UTC' },
          install: { enabled: true, start: '00:00', end: '00:01', timezone: 'UTC' }
        }
      }
    });

    expect(config.windows.download.start).toBe('00:00');
    expect(config.windows.download.end).toBe('00:01');
  });

  test('preserves explicit scan, download, install, and reboot windows', () => {
    const config = coercePatchPolicyConfig({
      ...defaultPolicy,
      policyConfig: {
        windows: {
          scan: { enabled: true, start: '01:00', end: '02:00', timezone: 'UTC' },
          download: { enabled: true, start: '03:00', end: '04:00', timezone: 'UTC' },
          install: { enabled: true, start: '03:00', end: '04:00', timezone: 'UTC' },
          reboot: { enabled: true, start: '05:00', end: '06:00', timezone: 'UTC' }
        }
      }
    });

    expect(config.windows.scan.start).toBe('01:00');
    expect(config.windows.scan.end).toBe('02:00');
    expect(config.windows.download.start).toBe('03:00');
    expect(config.windows.download.end).toBe('04:00');
    expect(config.windows.install.start).toBe('03:00');
    expect(config.windows.install.end).toBe('04:00');
    expect(config.windows.reboot.start).toBe('05:00');
    expect(config.windows.reboot.end).toBe('06:00');
  });

  test('manual install override does not schedule scan, download, or reboot', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        rebootRequired: true
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-04-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'install-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_install',
          reason: 'Install only'
        }
      ]
    });

    const installAction = plan.actions.find((action) => action.action === 'install');
    expect(installAction?.updateKeys).toContain('security update|kb5030000');
    expect(installAction?.metadata.rebootBehavior).toBe('allow');
    expect(plan.actions.some((action) => action.action === 'scan')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'download')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'reboot')).toBe(false);
  });

  test('failed updates are not retried automatically', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: decisionDevice,
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'macos tahoe 26.5|',
          title: 'macOS Tahoe 26.5',
          category: 'other',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'failed'
        }
      ]
    });

    expect(plan.actions.some((action) => action.action === 'install')).toBe(false);
  });

  test('failed updates can still be retried by explicit install override', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: decisionDevice,
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'macos tahoe 26.5|',
          title: 'macOS Tahoe 26.5',
          category: 'other',
          releaseDate: '2026-05-01T00:00:00.000Z',
          lifecycleState: 'failed'
        }
      ],
      overrides: [
        {
          id: 'retry-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_install',
          reason: 'Retry failed update'
        }
      ]
    });

    expect(plan.actions.some((action) =>
      action.action === 'install' && action.updateKeys.includes('macos tahoe 26.5|')
    )).toBe(true);
  });

  test('manual reboot override does not schedule scan, download, or install', () => {
    const plan = evaluatePatchActionPlan({
      now: '2026-05-20T12:00:00.000Z',
      device: {
        ...decisionDevice,
        rebootRequired: false
      },
      policy: defaultPolicy,
      updates: [
        {
          updateKey: 'security update|kb5030000',
          title: 'Security Update for Windows (KB5030000)',
          kbArticle: 'KB5030000',
          category: 'security',
          releaseDate: '2026-04-01T00:00:00.000Z',
          lifecycleState: 'detected'
        }
      ],
      overrides: [
        {
          id: 'reboot-1',
          scopeType: 'device',
          scopeKey: 'agent-1',
          action: 'force_reboot',
          reason: 'Reboot only'
        }
      ]
    });

    expect(plan.actions.some((action) => action.action === 'reboot')).toBe(true);
    expect(plan.actions.some((action) => action.action === 'scan')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'download')).toBe(false);
    expect(plan.actions.some((action) => action.action === 'install')).toBe(false);
  });
});
