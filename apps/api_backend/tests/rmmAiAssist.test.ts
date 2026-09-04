import { describe, expect, test } from 'bun:test';
import { buildShellAssistPrompt, buildTaskPrompt, parseTalosShellCommandArguments } from '../lib/rmmAiAssist';

describe('RMM AI Assist prompts', () => {
  test('desktop task prompt uses macOS guidance without Windows assumptions', () => {
    const prompt = buildTaskPrompt('open Safari', 'macos');

    expect(prompt).toContain('Target platform: macos.');
    expect(prompt).toContain('The target desktop is macOS.');
    expect(prompt).toContain('COMMAND/CMD');
    expect(prompt).toContain('OPTION/ALT');
    expect(prompt).toContain('Do not assume Windows-specific UI');
    expect(prompt).not.toContain('Windows remote desktop');
  });

  test('shell prompt keeps macOS target platform context', () => {
    const prompt = buildShellAssistPrompt(
      {
        prompt: 'show ip info',
        sessionId: 'session-1',
        sessionToken: 'token-1',
        rmmApiBase: 'https://rmm.example.test',
        platform: 'macos'
      },
      'root@mac ~ % ',
      []
    );

    expect(prompt).toContain('Target platform: macos.');
    expect(prompt).toContain('For Linux/macOS prefer POSIX shell syntax');
  });

  test('shell parser accepts wait actions with clamped waitMs', () => {
    const proposal = parseTalosShellCommandArguments(
      JSON.stringify({
        action: 'wait',
        command: '',
        explanation: 'Installer is still producing progress output.',
        risk: 'No command risk',
        notes: [],
        message: 'Waiting for the installer to finish.',
        waitMs: 120_000
      })
    );

    expect(proposal.action).toBe('wait');
    expect(proposal.command).toBe('');
    expect(proposal.waitMs).toBe(60_000);
  });

  test('shell parser accepts interrupt actions', () => {
    const proposal = parseTalosShellCommandArguments(
      JSON.stringify({
        action: 'interrupt',
        command: '',
        explanation: 'The active command is waiting for stdin and should be stopped.',
        risk: 'No command risk',
        notes: [],
        message: 'Stopping the active command before replanning.',
        waitMs: 0
      })
    );

    expect(proposal.action).toBe('interrupt');
    expect(proposal.command).toBe('');
    expect(proposal.waitMs).toBe(0);
  });

  test('shell parser requires non-wait waitMs to be zero', () => {
    const proposal = parseTalosShellCommandArguments(
      JSON.stringify({
        action: 'done',
        command: '',
        explanation: 'The requested output is present.',
        risk: 'No command risk',
        notes: [],
        message: 'Done.',
        waitMs: 0
      })
    );

    expect(proposal.action).toBe('done');
    expect(proposal.waitMs).toBe(0);

    expect(() =>
      parseTalosShellCommandArguments(
        JSON.stringify({
          action: 'done',
          command: '',
          explanation: 'Done',
          risk: 'No command risk',
          notes: [],
          message: 'Done.',
          waitMs: 10_000
        })
      )
    ).toThrow(/non-wait proposal must set waitMs to 0/);
  });

  test('shell parser rejects invalid wait actions', () => {
    expect(() =>
      parseTalosShellCommandArguments(
        JSON.stringify({
          action: 'wait',
          command: 'echo should-not-run',
          explanation: 'Wait',
          risk: 'No command risk',
          notes: [],
          message: 'Wait',
          waitMs: 10_000
        })
      )
    ).toThrow(/wait proposal must leave command empty/);

    expect(() =>
      parseTalosShellCommandArguments(
        JSON.stringify({
          action: 'wat',
          command: '',
          explanation: 'Wait',
          risk: 'No command risk',
          notes: [],
          message: 'Wait',
          waitMs: 10_000
        })
      )
    ).toThrow(/Unsupported Talos shell/);
  });

  test('shell parser rejects commands on non-command actions', () => {
    expect(() =>
      parseTalosShellCommandArguments(
        JSON.stringify({
          action: 'done',
          command: 'echo ignored',
          explanation: 'Done',
          risk: 'No command risk',
          notes: [],
          message: 'Done.',
          waitMs: 0
        })
      )
    ).toThrow(/non-command proposal must leave command empty/);

    expect(() =>
      parseTalosShellCommandArguments(
        JSON.stringify({
          action: 'interrupt',
          command: 'echo ignored',
          explanation: 'Interrupt',
          risk: 'No command risk',
          notes: [],
          message: 'Stopping.',
          waitMs: 0
        })
      )
    ).toThrow(/non-command proposal must leave command empty/);
  });

  test('shell prompt includes active command checkpoint context', () => {
    const prompt = buildShellAssistPrompt(
      {
        prompt: 'install TreeSize',
        sessionId: 'session-1',
        sessionToken: 'token-1',
        rmmApiBase: 'https://rmm.example.test',
        platform: 'windows',
        activeCommand: {
          command: 'Start-Sleep -Seconds 20',
          approvalId: 'approval-1',
          turnIndex: 1,
          elapsedMs: 10_000,
          checkpointCount: 1,
          recentOutput: 'Downloading installer...',
          remainingMs: 7_190_000
        }
      },
      'Downloading installer...',
      []
    );

    expect(prompt).toContain('action=wait');
    expect(prompt).toContain('action=interrupt');
    expect(prompt).toContain('Only use action=interrupt when an active command checkpoint is present');
    expect(prompt).toContain('For wait actions, choose waitMs');
    expect(prompt).toContain('Active approved command checkpoint follows:');
    expect(prompt).toContain('Approval: approval-1');
    expect(prompt).toContain('Command: Start-Sleep -Seconds 20');
    expect(prompt).toContain('Downloading installer...');
  });

  test('desktop task prompt includes compact device context and derives platform from it', () => {
    const prompt = buildTaskPrompt('check update status', 'unknown', {
      agentId: 'agent-win',
      hostname: 'win-ops-1',
      customerName: 'Acme',
      siteName: 'London',
      snapshot: {
        collectedAt: '2026-06-15T10:00:00.000Z',
        ageSeconds: 600
      },
      platform: {
        family: 'windows',
        osName: 'Windows 11 Pro',
        osVersion: '23H2',
        architecture: 'x64',
        timezone: 'Europe/London',
        locale: 'en-GB',
        domain: 'ACME.LOCAL'
      },
      agent: {
        version: '0.6.67',
        lastSeen: '2026-06-15T09:55:00.000Z'
      },
      hardware: {
        cpuModel: 'Intel Core i7',
        physicalCores: 8,
        logicalCores: 16,
        memoryTotalBytes: 17179869184
      },
      state: {
        pendingUpdatesCount: 3,
        rebootRequired: true
      },
      network: {
        primaryIp: '10.0.0.5'
      },
      shell: {
        runAs: 'system',
        account: 'NT AUTHORITY\\SYSTEM',
        elevated: true,
        description: 'AI shell commands run as the local Windows SYSTEM account, not the signed-in user.'
      },
      security: {
        firewallEnabled: true,
        secureBoot: true,
        tpmPresent: true,
        tpmEnabled: true,
        antivirusEnabled: true,
        bitlockerEnabled: false
      }
    });

    expect(prompt).toContain('Target platform: windows.');
    expect(prompt).toContain('Target device context:');
    expect(prompt).toContain('- Device: win-ops-1, Acme, London');
    expect(prompt).toContain('- Snapshot: 2026-06-15T10:00:00.000Z, 10 minutes old');
    expect(prompt).toContain('- OS: Windows 11 Pro, 23H2, x64, en-GB, Europe/London');
    expect(prompt).toContain('- Shell: AI shell commands run as the local Windows SYSTEM account, not the signed-in user.');
    expect(prompt).toContain('- Hardware: Intel Core i7, 8 physical / 16 logical cores, 16 GiB');
    expect(prompt).toContain('- State: 3 pending updates, reboot required: yes');
    expect(prompt).toContain('- Network: 10.0.0.5, ACME.LOCAL');
    expect(prompt).toContain('firewall: yes');
    expect(prompt).toContain('BitLocker: no');
  });

  test('shell prompt includes compact device context when supplied', () => {
    const prompt = buildShellAssistPrompt(
      {
        prompt: 'show pending update count',
        sessionId: 'session-1',
        sessionToken: 'token-1',
        rmmApiBase: 'https://rmm.example.test',
        platform: 'unknown',
        deviceContext: {
          agentId: 'agent-linux',
          hostname: 'linux-1',
          customerName: null,
          siteName: null,
          snapshot: { collectedAt: null, ageSeconds: null },
          platform: {
            family: 'linux',
            osName: 'Ubuntu',
            osVersion: '24.04',
            architecture: null,
            timezone: null,
            locale: null,
            domain: null
          },
          agent: { version: null, lastSeen: null },
          hardware: {
            cpuModel: null,
            physicalCores: null,
            logicalCores: null,
            memoryTotalBytes: null
          },
          state: { pendingUpdatesCount: null, rebootRequired: null },
          network: { primaryIp: null },
          shell: {
            runAs: 'configured_user',
            account: null,
            elevated: false,
            description: 'AI shell commands run as a configured Linux shell user, not root unless explicitly configured.'
          },
          security: {
            firewallEnabled: null,
            secureBoot: null,
            tpmPresent: null,
            tpmEnabled: null,
            antivirusEnabled: null,
            bitlockerEnabled: null
          }
        }
      },
      '$ ',
      []
    );

    expect(prompt).toContain('Target platform: linux.');
    expect(prompt).toContain('Target device context:');
    expect(prompt).toContain('- Device: linux-1');
    expect(prompt).toContain('- OS: Ubuntu, 24.04');
    expect(prompt).toContain('- Shell: AI shell commands run as a configured Linux shell user, not root unless explicitly configured.');
  });

  test('prompt formatter tolerates partial device context objects', () => {
    const prompt = buildShellAssistPrompt(
      {
        prompt: 'whoami',
        sessionId: 'session-1',
        sessionToken: 'token-1',
        rmmApiBase: 'https://rmm.example.test',
        platform: 'windows',
        deviceContext: {
          agentId: 'agent-partial',
          platform: { family: 'windows' }
        } as any
      },
      'PS>',
      []
    );

    expect(prompt).toContain('Target platform: windows.');
    expect(prompt).toContain('Target device context:');
    expect(prompt).toContain('- Device: agent-partial');
  });
});
