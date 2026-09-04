import { describe, expect, test } from 'bun:test';
import {
  evaluateFeatureUpgradePreflightChecks,
  featureUpgradeChecksForProfile,
  inferFeatureUpgradePreflightTarget,
  readFeatureUpgradeAgentIds,
  summarizeFeatureUpgradePreflightChecks
} from '../lib/featureUpgradePreflight';

describe('feature upgrade preflight manifest', () => {
  test('infers Windows 10 devices should target Windows 11 25H2', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 10 Pro 22H2');

    expect(target?.profile).toBe('windows10_to_11');
    expect(target?.targetProduct).toBe('Windows 11');
    expect(target?.targetVersion).toBe('25H2');
    expect(target?.checks.map((check) => check.id)).toContain('tpm_2_0');
    expect(target?.checks.map((check) => check.id)).toContain('secure_boot');
  });

  test('infers Windows 11 feature upgrades use the lighter Windows 11 manifest', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 11 Pro 23H2');
    const checkIds = target?.checks.map((check) => check.id) ?? [];

    expect(target?.profile).toBe('windows11_feature');
    expect(target?.targetBuildLabel).toBe('Windows 11 25H2');
    expect(checkIds).toContain('disk_space');
    expect(checkIds).not.toContain('tpm_2_0');
  });

  test('infers Windows Server devices should target Server 2025 with DC warning checks', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows Server 2019 Standard');
    const checkIds = target?.checks.map((check) => check.id) ?? [];

    expect(target?.profile).toBe('server_to_2025');
    expect(target?.targetBuildLabel).toBe('Windows Server 2025');
    expect(checkIds).toContain('domain_controller');
  });

  test('does not infer a target for non-Windows devices', () => {
    expect(inferFeatureUpgradePreflightTarget('Ubuntu 26.04')).toBeNull();
  });

  test('keeps warning checks visible without mixing them into required manifests', () => {
    const serverWarnings = featureUpgradeChecksForProfile('server_to_2025').filter((check) => check.severity === 'warning');
    const windows11Warnings = featureUpgradeChecksForProfile('windows11_feature').filter((check) => check.severity === 'warning');

    expect(serverWarnings.map((check) => check.id)).toEqual(['bitlocker', 'domain_controller']);
    expect(windows11Warnings.map((check) => check.id)).toEqual(['bitlocker']);
  });
});

describe('feature upgrade preflight request/result helpers', () => {
  test('deduplicates and trims selected agent ids', () => {
    expect(readFeatureUpgradeAgentIds([' agent-1 ', '', 'agent-1', 'agent-2', 7])).toEqual(['agent-1', 'agent-2']);
  });

  test('summarizes failed and warning check results for durable rows', () => {
    const checks = [
      { id: 'disk_space', label: 'Disk space', status: 'failed', message: 'Only 22 GB free' },
      { id: 'bitlocker', label: 'BitLocker', status: 'warning', message: 'Suspend before staging' },
      { id: 'pending_reboot', label: 'Pending reboot', status: 'passed', message: 'No reboot pending' }
    ];

    expect(summarizeFeatureUpgradePreflightChecks(checks, 'failed')).toEqual([
      { id: 'disk_space', label: 'Disk space', message: 'Only 22 GB free' }
    ]);
    expect(summarizeFeatureUpgradePreflightChecks(checks, 'warning')).toEqual([
      { id: 'bitlocker', label: 'BitLocker', message: 'Suspend before staging' }
    ]);
  });
});

describe('feature upgrade preflight snapshot-aware evaluation', () => {
  test('preview uses cached facts but leaves disk and BitLocker pending for fresh snapshot', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 10 Pro 22H2')!;
    const checks = evaluateFeatureUpgradePreflightChecks({
      target,
      mode: 'preview',
      device: {
        os: 'Windows 10 Pro',
        osVersion: '22H2',
        state: {
          collectedAt: new Date('2026-05-24T10:00:00Z'),
          osName: 'Microsoft Windows 10 Pro',
          osVersion: '22H2',
          cpuPhysicalCores: 4,
          cpuBaseMhz: 2600,
          memoryTotalBytes: 8n * 1024n * 1024n * 1024n,
          rebootRequired: false,
          inventoryData: {
            operating_system: { system: { architecture: 'x64', edition: 'Professional', locale: 'en-GB' } },
            hardware: {
              cpu: { architecture: 'x64', cores: 4, frequency_mhz: 2600 },
              memory: { total_bytes: 8 * 1024 * 1024 * 1024 },
              disks: [{ size_bytes: 128 * 1024 * 1024 * 1024, volumes: [{ drive_letter: 'C:', total_bytes: 128 * 1024 * 1024 * 1024, free_bytes: 80 * 1024 * 1024 * 1024 }] }],
              tpm: { present: true, enabled: true, ready: true, version: '2.0' },
              secure_boot: true
            },
            security: { bitlocker: { enabled: false, volumes: [{ drive_letter: 'C:', protection_status: 'Unprotected' }] } }
          }
        },
        facts: new Map([
          ['security.tpm_present', { factKey: 'security.tpm_present', factValue: true, source: 'snapshot', sourceTs: new Date('2026-05-24T10:00:00Z') }],
          ['security.tpm_enabled', { factKey: 'security.tpm_enabled', factValue: true, source: 'snapshot', sourceTs: new Date('2026-05-24T10:00:00Z') }],
          ['security.tpm_version', { factKey: 'security.tpm_version', factValue: '2.0', source: 'snapshot', sourceTs: new Date('2026-05-24T10:00:00Z') }]
        ])
      }
    });

    expect(checks.find((check) => check.id === 'tpm_2_0')?.status).toBe('passed');
    expect(checks.find((check) => check.id === 'pending_reboot')?.status).toBe('passed');
    expect(checks.find((check) => check.id === 'disk_space')?.status).toBe('pending');
    expect(checks.find((check) => check.id === 'bitlocker')?.status).toBe('pending');
  });

  test('preview reads edition, language, and architecture from nested snapshot OS details', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 11 Pro 23H2')!;
    const checks = evaluateFeatureUpgradePreflightChecks({
      target,
      mode: 'preview',
      device: {
        os: 'Windows 11 Pro',
        osVersion: '23H2',
        state: {
          collectedAt: new Date('2026-05-24T10:00:00Z'),
          osName: 'Microsoft Windows 11 Pro',
          osVersion: '23H2',
          rebootRequired: false,
          inventoryData: {
            operating_system: {
              system: {
                os: {
                  architecture: '64-bit',
                  edition: 'Professional',
                  language: 'en-GB'
                }
              }
            }
          }
        }
      }
    });

    expect(checks.find((check) => check.id === 'architecture')?.status).toBe('passed');
    expect(checks.find((check) => check.id === 'edition_language')?.status).toBe('passed');
  });

  test('final evaluation fails required refreshed disk space and warns on protected BitLocker', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 11 Pro 23H2')!;
    const checks = evaluateFeatureUpgradePreflightChecks({
      target,
      mode: 'final',
      device: {
        os: 'Windows 11 Pro',
        osVersion: '23H2',
        state: {
          collectedAt: new Date('2026-05-24T10:05:00Z'),
          osName: 'Microsoft Windows 11 Pro',
          osVersion: '23H2',
          rebootRequired: false,
          inventoryData: {
            operating_system: { system: { architecture: 'x64', edition: 'Professional', locale: 'en-GB' } },
            hardware: {
              disks: [{ size_bytes: 128 * 1024 * 1024 * 1024, volumes: [{ drive_letter: 'C:', total_bytes: 128 * 1024 * 1024 * 1024, free_bytes: 20 * 1024 * 1024 * 1024 }] }]
            },
            security: { bitlocker: { enabled: true, volumes: [{ drive_letter: 'C:', protection_status: 'Protected' }] } }
          }
        }
      }
    });

    expect(checks.find((check) => check.id === 'disk_space')?.status).toBe('failed');
    expect(checks.find((check) => check.id === 'bitlocker')?.status).toBe('warning');
  });

  test('final evaluation reads BitLocker state from promoted facts when volume detail is missing', () => {
    const target = inferFeatureUpgradePreflightTarget('Windows 11 Pro 23H2')!;
    const checks = evaluateFeatureUpgradePreflightChecks({
      target,
      mode: 'final',
      device: {
        os: 'Windows 11 Pro',
        osVersion: '23H2',
        state: {
          collectedAt: new Date('2026-05-24T10:05:00Z'),
          osName: 'Microsoft Windows 11 Pro',
          osVersion: '23H2',
          rebootRequired: false,
          inventoryData: {
            operating_system: { system: { os: { architecture: '64-bit', edition: 'Professional', language: 'en-GB' } } },
            hardware: {
              disks: [{ size_bytes: 128 * 1024 * 1024 * 1024, volumes: [{ drive_letter: 'C:', total_bytes: 128 * 1024 * 1024 * 1024, free_bytes: 80 * 1024 * 1024 * 1024 }] }]
            }
          }
        },
        facts: new Map([
          ['security.bitlocker_enabled', { factKey: 'security.bitlocker_enabled', factValue: false, source: 'full_snapshot', sourceTs: new Date('2026-05-24T10:05:00Z') }],
          ['security.bitlocker_protection_status', { factKey: 'security.bitlocker_protection_status', factValue: 'Unprotected', source: 'full_snapshot', sourceTs: new Date('2026-05-24T10:05:00Z') }]
        ])
      }
    });

    expect(checks.find((check) => check.id === 'bitlocker')?.status).toBe('passed');
  });
});
