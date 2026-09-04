<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { Permission, PermissionFlow } from '@veecore/tauri-plugin-permission-flow-api';
  import type { Permission as FlowPermission } from '@veecore/tauri-plugin-permission-flow-api';
  import './app.css';

  type PermissionState = {
    granted: boolean;
    probePath?: string | null;
    error?: string | null;
  };

  type Snapshot = {
    fullDiskAccess: PermissionState;
    screenRecording: PermissionState;
    accessibility: PermissionState;
    macosSoftwareUpdate: MacosUpdateAccountStatus;
    workerAppPath: string;
    workerHelperAppPath: string;
    checkedAtUnixMs: number;
  };

  type LaunchContext = {
    reason: string;
    fullDiskAccessRequired: boolean;
    screenRecordingRequired: boolean;
    accessibilityRequired: boolean;
    macosSoftwareUpdateRequired: boolean;
    afterInstall: boolean;
    loginCheck: boolean;
  };

  type MacosVolumeOwnerUser = {
    username?: string | null;
    fullName?: string | null;
    generatedUid?: string | null;
    volumeOwner: boolean;
  };

  type MacosUpdateAccountStatus = {
    schemaVersion: number;
    required: boolean;
    status: string;
    username: string;
    isAppleSilicon: boolean;
    accountPresent: boolean;
    isAdmin: boolean;
    isVolumeOwner: boolean;
    secureTokenEnabled: boolean;
    credentialAvailable: boolean;
    credentialVersion?: number | null;
    generatedUid?: string | null;
    expectedGeneratedUid?: string | null;
    discoveredVolumeOwners: MacosVolumeOwnerUser[];
    failureCode?: string | null;
    failureMessage?: string | null;
    checkedAt: string;
  };

  type MacosUpdateAccountIpcResponse = {
    ok: boolean;
    status?: MacosUpdateAccountStatus | null;
    sessionId?: string | null;
    errorCode?: string | null;
    errorMessage?: string | null;
  };

  type PermissionKey = 'fullDiskAccess' | 'screenRecording' | 'accessibility';
  type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error';

  type PermissionStep = {
    key: PermissionKey;
    label: string;
    permission: FlowPermission;
    targetName: string;
    appPath: string;
    detail: string;
  };

  const workerAppFallback = '/Library/Talos/Worker/Talos Worker.app';
  const workerHelperAppFallback = '/Library/Talos/Worker/Talos Worker Helper.app';
  const hasTauriBridge = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
  const permissionOrder: PermissionKey[] = ['fullDiskAccess', 'screenRecording', 'accessibility'];
  const backgroundRefreshIntervalMs = 6000;

  let snapshot = $state<Snapshot | null>(null);
  let launchContext = $state<LaunchContext | null>(null);
  let flow = $state<PermissionFlow | null>(null);
  let activePermission = $state<PermissionKey | null>(null);
  let lastError = $state<string | null>(null);
  let flowError = $state<string | null>(null);
  let busy = $state(false);
  let flowBusy = $state(false);
  let refreshInFlight = false;
  let restartRequested = $state(false);
  let softwareUpdateBusy = $state(false);
  let softwareUpdateError = $state<string | null>(null);

  let observedMissingRequired = false;
  let lastLoggedSoftwareUpdateStatusKey: string | null = null;

  const currentStep = $derived.by(() => (snapshot ? nextMissingStep(snapshot) : null));
  const selectedStep = $derived.by(() =>
    activePermission ? stepFor(activePermission) : currentStep
  );
  const statusDetail = $derived.by(() => {
    if (flowError) return flowError;
    if (softwareUpdateError) return softwareUpdateError;
    if (snapshot && !requiredPermissionsGranted(snapshot)) {
      return friendlyPermissionError(relevantPermissionError(snapshot));
    }
    if (snapshot && !softwareUpdateReady(snapshot)) {
      return softwareUpdateMessage(snapshot.macosSoftwareUpdate);
    }
    return friendlyError(lastError, null);
  });
  const requiredReady = $derived(
    snapshot ? requiredPermissionsGranted(snapshot) && softwareUpdateReady(snapshot) : false
  );
  const statusLabel = $derived(
    !snapshot ? 'Checking' : requiredReady ? 'Ready' : flowBusy ? 'Opening' : 'Action needed'
  );
  const remainingText = $derived.by(() => {
    if (!snapshot) return 'Checking approvals';
    const missing =
      requiredKeys().filter((key) => !isGranted(snapshot!, key)).length +
      (softwareUpdateReady(snapshot) ? 0 : 1);
    if (missing === 0) return 'All set';
    if (missing === 1) return '1 item left';
    return `${missing} items left`;
  });
  const helperCopy = $derived.by(() => {
    if (!snapshot) {
      return {
        title: 'Checking Talos setup.',
        body: 'This should only take a moment.'
      };
    }
    if (requiredReady) {
      return {
        title: 'Talos is ready.',
        body:
          launchContext?.reason === 'remote_desktop'
            ? 'Remote support is ready on this Mac.'
            : 'Required approvals are complete.'
      };
    }
    if (currentStep) {
      return {
        title: `Allow ${currentStep.label}.`,
        body: 'Open System Settings and approve this item.'
      };
    }
    if (!softwareUpdateReady(snapshot)) {
      if (softwareUpdateStarting(snapshot)) {
        return {
          title: 'Preparing Software Updates.',
          body: 'Talos is getting this ready. This can take a moment.'
        };
      }
      return {
        title: 'Approve Software Updates.',
        body: 'Approve this once with a Mac owner account.'
      };
    }
    return {
      title: 'Allow Talos Worker.',
      body: 'Open System Settings and approve the remaining item.'
    };
  });

  function requiredKeys(): PermissionKey[] {
    return permissionOrder;
  }

  function isRequired(key: PermissionKey): boolean {
    return requiredKeys().includes(key);
  }

  function isGranted(value: Snapshot, key: PermissionKey): boolean {
    if (key === 'fullDiskAccess') return value.fullDiskAccess.granted;
    if (key === 'screenRecording') return value.screenRecording.granted;
    return value.accessibility.granted;
  }

  function permissionState(value: Snapshot, key: PermissionKey): PermissionState {
    if (key === 'fullDiskAccess') return value.fullDiskAccess;
    if (key === 'screenRecording') return value.screenRecording;
    return value.accessibility;
  }

  function stepFor(key: PermissionKey, value: Snapshot | null = snapshot): PermissionStep {
    if (key === 'fullDiskAccess') {
      return {
        key,
        label: 'Full Disk Access',
        permission: Permission.FullDiskAccess,
        targetName: 'Talos Worker',
        appPath: value?.workerAppPath ?? workerAppFallback,
        detail: 'Lets Talos access protected files when needed.'
      };
    }
    if (key === 'screenRecording') {
      return {
        key,
        label: 'Screen Recording',
        permission: Permission.ScreenRecording,
        targetName: 'Talos Worker Helper',
        appPath: value?.workerHelperAppPath ?? workerHelperAppFallback,
        detail: 'Lets Talos show the screen during support sessions.'
      };
    }
    return {
      key,
      label: 'Accessibility',
      permission: Permission.Accessibility,
      targetName: 'Talos Worker Helper',
      appPath: value?.workerHelperAppPath ?? workerHelperAppFallback,
      detail: 'Lets Talos use mouse and keyboard control during support sessions.'
    };
  }

  function nextMissingStep(value: Snapshot): PermissionStep | null {
    const key = requiredKeys().find((candidate) => !isGranted(value, candidate));
    return key ? stepFor(key, value) : null;
  }

  function requiredPermissionsGranted(value: Snapshot): boolean {
    return requiredKeys().every((key) => isGranted(value, key));
  }

  function softwareUpdateRequired(value: Snapshot | null = snapshot): boolean {
    if (!value) return false;
    return Boolean(value.macosSoftwareUpdate.required || launchContext?.macosSoftwareUpdateRequired);
  }

  function softwareUpdateReady(value: Snapshot | null = snapshot): boolean {
    if (!value || !softwareUpdateRequired(value)) return true;
    return (
      value.macosSoftwareUpdate.status === 'ready' ||
      value.macosSoftwareUpdate.status === 'notRequired'
    );
  }

  function softwareUpdateStarting(value: Snapshot | null = snapshot): boolean {
    if (!value || !softwareUpdateRequired(value)) return false;
    const code = value.macosSoftwareUpdate.failureCode ?? '';
    return code === 'worker_unavailable';
  }

  function softwareUpdateStatusLabel(): string {
    if (!snapshot) return 'Checking';
    if (!softwareUpdateRequired()) return 'Optional';
    if (softwareUpdateStarting()) return 'Starting';
    const labels: Record<string, string> = {
      ready: 'Ready',
      needsEnrollment: 'Needed',
      missing: 'Needed',
      notRequired: 'Ready',
      error: 'Needed'
    };
    return labels[snapshot.macosSoftwareUpdate.status] ?? 'Needed';
  }

  function softwareUpdateDetail(): string {
    if (!snapshot) return 'Lets Talos install macOS updates.';
    if (!softwareUpdateRequired()) return 'No approval needed on this Mac.';
    if (softwareUpdateReady()) return 'Ready for macOS updates.';
    if (softwareUpdateStarting()) return 'Talos is getting this ready.';
    return softwareUpdateMessage(snapshot.macosSoftwareUpdate);
  }

  function canOpenSoftwareUpdate(): boolean {
    return Boolean(
      snapshot &&
        softwareUpdateRequired() &&
        !softwareUpdateReady() &&
        !softwareUpdateStarting() &&
        !softwareUpdateBusy
    );
  }

  function relevantPermissionError(value: Snapshot): string | null {
    const key = requiredKeys().find((candidate) => permissionState(value, candidate).error);
    return key ? permissionState(value, key).error ?? null : null;
  }

  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function friendlyError(
    message: string | null | undefined,
    fallback: string | null = 'Something went wrong. Try again.'
  ): string | null {
    if (!message) return fallback;
    const normalized = message.toLowerCase();
    if (
      normalized.includes('still starting') ||
      normalized.includes('worker_unavailable') ||
      normalized.includes('connection refused') ||
      normalized.includes('socket')
    ) {
      return 'Talos is still starting. Keep this window open.';
    }
    if (
      normalized.includes('permissionflow') ||
      normalized.includes('system settings') ||
      normalized.includes('start flow') ||
      normalized.includes('permission pane')
    ) {
      return 'System Settings did not open. Try again.';
    }
    if (normalized.includes('could not check software updates')) {
      return 'Talos could not check Software Updates.';
    }
    if (normalized.includes('software update') || normalized.includes('software updates')) {
      return 'Software Updates approval is not complete yet.';
    }
    if (
      normalized.includes('not found') ||
      normalized.includes('not installed') ||
      normalized.includes('missing')
    ) {
      return 'Talos is not fully installed. Contact support.';
    }
    if (normalized.includes('operation not permitted') || normalized.includes('permission denied')) {
      return 'macOS has not allowed this yet.';
    }
    return fallback;
  }

  function friendlyPermissionError(message: string | null): string | null {
    if (!message) return null;
    return friendlyError(message, 'Talos could not check permissions. Try again.');
  }

  function softwareUpdateMessage(status: MacosUpdateAccountStatus): string {
    const code = status.failureCode ?? '';
    if (code === 'worker_unavailable') {
      return 'Talos is still starting. Keep this window open.';
    }
    if (
      status.status === 'needsEnrollment' ||
      code === 'macos_update_account_needs_enrollment' ||
      code === 'macos_update_account_enrollment_failed'
    ) {
      return 'A Mac owner needs to approve Software Updates.';
    }
    if (status.status === 'missing' || code.includes('missing') || code.includes('recreated')) {
      return 'Talos needs to finish Software Updates setup.';
    }
    if (code === 'macos_update_account_admin') {
      return 'Software Updates setup needs support.';
    }
    return friendlyError(status.failureMessage, 'Software Updates approval is not complete yet.')!;
  }

  async function logUi(level: LogLevel, event: string, detail: Record<string, unknown> = {}) {
    if (!hasTauriBridge) return;
    try {
      await invoke('log_frontend_event', { level, event, detail });
    } catch (error) {
      console.warn('Talos Permissions Helper frontend log failed', error);
    }
  }

  function permissionStatusLabel(key: PermissionKey): string {
    if (!snapshot) return 'Checking';
    if (isGranted(snapshot, key)) return 'Ready';
    return isRequired(key) ? 'Needed' : 'Optional';
  }

  function canOpenPermission(key: PermissionKey): boolean {
    return Boolean(snapshot && flow && !flowBusy && !isGranted(snapshot, key));
  }

  async function startPermissionFlow(step: PermissionStep, useClickSourceFrame: boolean) {
    activePermission = step.key;
    flowError = null;
    void logUi('debug', 'permission_flow_start_requested', {
      key: step.key,
      permission: step.permission,
      appPath: step.appPath,
      targetName: step.targetName,
      useClickSourceFrame,
      hasTauriBridge,
      flowReady: Boolean(flow)
    });
    if (!hasTauriBridge) return;
    if (!flow) {
      flowError = 'System Settings is still getting ready.';
      void logUi('warn', 'permission_flow_start_blocked_no_controller', {
        key: step.key,
        permission: step.permission,
        appPath: step.appPath
      });
      return;
    }

    flowBusy = true;
    try {
      await flow.startFlow({
        permission: step.permission,
        appPath: step.appPath,
        useClickSourceFrame
      });
      void logUi('info', 'permission_flow_start_succeeded', {
        key: step.key,
        permission: step.permission,
        appPath: step.appPath,
        useClickSourceFrame
      });
    } catch (error) {
      const rawError = errorMessage(error);
      flowError = friendlyError(rawError, 'System Settings did not open. Try again.');
      void logUi('error', 'permission_flow_start_failed', {
        key: step.key,
        permission: step.permission,
        appPath: step.appPath,
        useClickSourceFrame,
        error: rawError
      });
    } finally {
      flowBusy = false;
    }
  }

  async function stopPermissionFlow(expectedPermission?: PermissionKey) {
    if (expectedPermission && activePermission !== expectedPermission) {
      void logUi('trace', 'permission_flow_stop_skipped_for_stale_active_permission', {
        expectedPermission,
        activePermission
      });
      return;
    }
    if (!hasTauriBridge || !flow || !activePermission) {
      activePermission = null;
      return;
    }
    const currentFlow = flow;
    activePermission = null;
    void logUi('trace', 'permission_flow_stop_requested', {});
    try {
      await currentFlow.stopCurrentFlow();
      void logUi('debug', 'permission_flow_stop_succeeded', {});
    } catch (error) {
      const rawError = errorMessage(error);
      lastError = rawError;
      void logUi('warn', 'permission_flow_stop_failed', { error: rawError });
    }
  }

  async function refresh() {
    if (refreshInFlight) {
      return;
    }
    refreshInFlight = true;
    try {
      const next = hasTauriBridge ? await invoke<Snapshot>('get_permission_snapshot') : mockSnapshot();
      const wasRequiredGranted = snapshot
        ? requiredPermissionsGranted(snapshot) && softwareUpdateReady(snapshot)
        : false;
      const activeKey = activePermission;
      snapshot = next;
      const softwareUpdateStatusKey = [
        next.macosSoftwareUpdate.status,
        next.macosSoftwareUpdate.failureCode ?? '',
        next.macosSoftwareUpdate.credentialVersion ?? ''
      ].join(':');
      if (softwareUpdateStatusKey !== lastLoggedSoftwareUpdateStatusKey) {
        lastLoggedSoftwareUpdateStatusKey = softwareUpdateStatusKey;
        void logUi('info', 'software_update_status_changed', {
          status: next.macosSoftwareUpdate.status,
          failureCode: next.macosSoftwareUpdate.failureCode,
          required: next.macosSoftwareUpdate.required,
          accountPresent: next.macosSoftwareUpdate.accountPresent,
          isVolumeOwner: next.macosSoftwareUpdate.isVolumeOwner,
          credentialAvailable: next.macosSoftwareUpdate.credentialAvailable
        });
      }
      lastError = relevantPermissionError(next) ?? (!softwareUpdateReady(next) ? next.macosSoftwareUpdate.failureMessage ?? null : null);
      const requiredGranted = requiredPermissionsGranted(next) && softwareUpdateReady(next);
      const activeGranted = activeKey ? isGranted(next, activeKey) : true;
      if (activeKey && activeGranted) {
        await stopPermissionFlow(activeKey);
      }
      if (requiredGranted) {
        if (!activeKey) {
          await stopPermissionFlow();
        } else if (!activeGranted) {
          void logUi('trace', 'permission_flow_kept_open_for_active_permission', {
            activePermission: activeKey,
            requiredGranted,
            activeGranted
          });
        }
        if (observedMissingRequired && !wasRequiredGranted && !restartRequested) {
          restartRequested = true;
          void logUi('info', 'request_worker_restart_from_ui', {
            observedMissingRequired,
            wasRequiredGranted
          });
          if (hasTauriBridge) void invoke('request_worker_restart');
        }
      } else {
        observedMissingRequired = true;
      }
    } catch (error) {
      const rawError = errorMessage(error);
      lastError = rawError;
      void logUi('error', 'permission_snapshot_refresh_failed', { error: rawError });
    } finally {
      refreshInFlight = false;
    }
  }

  async function openPermission(key: PermissionKey) {
    if (!canOpenPermission(key)) {
      void logUi('trace', 'permission_item_open_ignored', {
        key,
        granted: snapshot ? isGranted(snapshot, key) : null,
        flowReady: Boolean(flow),
        flowBusy
      });
      return;
    }
    const step = stepFor(key);
    void logUi('info', 'permission_item_clicked', {
      key,
      permission: step.permission,
      appPath: step.appPath,
      targetName: step.targetName,
      granted: snapshot ? isGranted(snapshot, key) : null,
      required: isRequired(key)
    });
    await startPermissionFlow(step, true);
    void refresh();
  }

  async function revealTarget() {
    const step = selectedStep;
    if (!step || !hasTauriBridge) return;
    busy = true;
    void logUi('debug', 'reveal_target_requested', {
      key: step.key,
      appPath: step.appPath,
      target: step.key === 'fullDiskAccess' ? 'worker' : 'helper'
    });
    try {
      await invoke('reveal_worker_app', {
        target: step.key === 'fullDiskAccess' ? 'worker' : 'helper'
      });
      void logUi('info', 'reveal_target_succeeded', {
        key: step.key,
        appPath: step.appPath
      });
    } catch (error) {
      const rawError = errorMessage(error);
      lastError = rawError;
      void logUi('warn', 'reveal_target_failed', {
        key: step.key,
        appPath: step.appPath,
        error: rawError
      });
    } finally {
      busy = false;
    }
  }

  async function startSoftwareUpdateEnrollment() {
    if (!canOpenSoftwareUpdate()) return;
    softwareUpdateBusy = true;
    softwareUpdateError = null;
    void logUi('info', 'software_update_enrollment_begin_requested', {
      status: snapshot?.macosSoftwareUpdate.status,
      failureCode: snapshot?.macosSoftwareUpdate.failureCode
    });
    try {
      const response = hasTauriBridge
        ? await invoke<MacosUpdateAccountIpcResponse>('approve_macos_software_update_enrollment')
        : mockApproveSoftwareUpdateEnrollment();
      if (response.status && snapshot) {
        snapshot = { ...snapshot, macosSoftwareUpdate: response.status };
      }
      if (!response.ok) {
        const rawError = response.errorMessage ?? response.status?.failureMessage ?? null;
        softwareUpdateError = response.status
          ? softwareUpdateMessage(response.status)
          : friendlyError(rawError, 'Software Updates approval is not complete yet.');
        void logUi('warn', 'software_update_enrollment_not_ready', { error: rawError });
        return;
      }
      await refresh();
      void logUi('info', 'software_update_enrollment_completed', {});
    } catch (error) {
      const rawError = errorMessage(error);
      softwareUpdateError = friendlyError(rawError, 'Software Updates approval is not complete yet.');
      void logUi('error', 'software_update_enrollment_begin_failed', { error: rawError });
    } finally {
      softwareUpdateBusy = false;
    }
  }

  onMount(() => {
    let cancelled = false;
    let refreshTimer: number | undefined;

    async function prepare() {
      void logUi('debug', 'frontend_prepare_started', { hasTauriBridge });
      try {
        if (hasTauriBridge) {
          const context = await invoke<LaunchContext>('get_launch_context');
          if (cancelled) {
            void logUi('debug', 'frontend_prepare_cancelled_after_context', {});
            return;
          }
          launchContext = context;
          void logUi('info', 'launch_context_loaded', { launchContext: context });
          try {
            void logUi('debug', 'permission_flow_create_requested', {});
            const createdFlow = await PermissionFlow.create();
            if (cancelled) {
              void logUi('debug', 'frontend_prepare_cancelled_after_flow_create', {});
              await createdFlow.close();
              return;
            }
            flow = createdFlow;
            void logUi('info', 'permission_flow_create_succeeded', {});
          } catch (error) {
            const rawError = errorMessage(error);
            flowError = friendlyError(rawError, 'System Settings did not open. Try again.');
            void logUi('error', 'permission_flow_create_failed', { error: rawError });
          }
        } else {
          launchContext = {
            reason: 'manual',
            fullDiskAccessRequired: true,
            screenRecordingRequired: true,
            accessibilityRequired: true,
            macosSoftwareUpdateRequired: true,
            afterInstall: false,
            loginCheck: false
          };
          void logUi('debug', 'mock_launch_context_loaded', { launchContext });
        }
        await refresh();
      } catch (error) {
        const rawError = errorMessage(error);
        flowError = friendlyError(rawError, 'Talos could not finish setup. Try again.');
        void logUi('error', 'frontend_prepare_failed', { error: rawError });
      } finally {
        if (!cancelled && refreshTimer === undefined) {
          refreshTimer = window.setInterval(() => void refresh(), backgroundRefreshIntervalMs);
          void logUi('debug', 'permission_snapshot_refresh_timer_started', {
            intervalMs: backgroundRefreshIntervalMs
          });
        }
      }
    }

    void prepare();

    return () => {
      cancelled = true;
      if (refreshTimer !== undefined) window.clearInterval(refreshTimer);
      const currentFlow = flow;
      flow = null;
      void logUi('debug', 'frontend_unmount', { hadFlow: Boolean(currentFlow) });
      if (currentFlow) {
        void currentFlow
          .close()
          .then(() => logUi('debug', 'permission_flow_close_succeeded', {}))
          .catch((error) =>
            logUi('warn', 'permission_flow_close_failed', { error: errorMessage(error) })
          );
      }
    };
  });

  function mockSnapshot(): Snapshot {
    return {
      fullDiskAccess: {
        granted: false,
        probePath: '/Users/example/Library/Application Support/com.apple.TCC/TCC.db',
        error: 'Operation not permitted'
      },
      screenRecording: {
        granted: false,
        probePath: workerHelperAppFallback,
        error: null
      },
      accessibility: {
        granted: false,
        probePath: workerHelperAppFallback,
        error: null
      },
      macosSoftwareUpdate: mockMacosSoftwareUpdateStatus('needsEnrollment'),
      workerAppPath: workerAppFallback,
      workerHelperAppPath: workerHelperAppFallback,
      checkedAtUnixMs: Date.now()
    };
  }

  function mockMacosSoftwareUpdateStatus(status: string): MacosUpdateAccountStatus {
    return {
      schemaVersion: 1,
      required: true,
      status,
      username: 'talos',
      isAppleSilicon: true,
      accountPresent: status !== 'missing',
      isAdmin: false,
      isVolumeOwner: status === 'ready',
      secureTokenEnabled: status === 'ready',
      credentialAvailable: true,
      credentialVersion: 1,
      generatedUid: 'MOCK-GENERATED-UID',
      expectedGeneratedUid: 'MOCK-GENERATED-UID',
      discoveredVolumeOwners: [
        {
          username: 'owner',
          fullName: 'Volume Owner',
          generatedUid: 'OWNER-GENERATED-UID',
          volumeOwner: true
        }
      ],
      failureCode: status === 'ready' ? null : 'macos_update_account_needs_enrollment',
      failureMessage:
        status === 'ready'
          ? null
          : 'Talos needs a volume-owner approval before macOS software updates can be installed.',
      checkedAt: new Date().toISOString()
    };
  }

  function mockApproveSoftwareUpdateEnrollment(): MacosUpdateAccountIpcResponse {
    return {
      ok: true,
      status: mockMacosSoftwareUpdateStatus('ready')
    };
  }
</script>

<main class="shell">
  <section class="panel" class:ready={requiredReady}>
    <div class="topline">
      <div class="brand-lockup" aria-label="Talos">
        <span class="brand-mark" aria-hidden="true">
          <span></span>
          <span></span>
          <span></span>
        </span>
        <div>
          <p class="eyebrow">Talos setup</p>
          <strong>Permissions</strong>
        </div>
      </div>
      <span class="status-pill">{statusLabel}</span>
    </div>

    <h1>{helperCopy.title}</h1>
    <p class="summary">{helperCopy.body}</p>

    {#if selectedStep}
      <div class="target-box">
        <span>{selectedStep.label}</span>
        <strong>{selectedStep.targetName}</strong>
      </div>

      <div class="actions">
        <button
          type="button"
          onclick={() => openPermission(selectedStep!.key)}
          disabled={!canOpenPermission(selectedStep!.key)}
        >
          {flowBusy ? 'Opening...' : 'Open Settings'}
        </button>
        <button class="secondary" type="button" onclick={revealTarget} disabled={busy}>
          Show App
        </button>
      </div>
    {:else}
      <div class="target-box complete">
        <span>Complete</span>
        <strong>{remainingText}</strong>
      </div>
    {/if}

    <div class="permission-list">
      {#each permissionOrder as key}
        {@const step = stepFor(key)}
        {@const granted = snapshot ? isGranted(snapshot, key) : false}
        <button
          type="button"
          class="permission-item"
          class:active={currentStep?.key === key || activePermission === key}
          class:granted
          class:required={isRequired(key)}
          onclick={() => openPermission(key)}
          disabled={!canOpenPermission(key)}
        >
          <span>{permissionStatusLabel(key)}</span>
          <strong>{step.label}</strong>
          <p>{step.detail}</p>
        </button>
      {/each}
      <button
        type="button"
        class="permission-item"
        class:active={softwareUpdateBusy}
        class:granted={softwareUpdateReady()}
        class:required={softwareUpdateRequired()}
        onclick={startSoftwareUpdateEnrollment}
        disabled={!canOpenSoftwareUpdate()}
      >
        <span>{softwareUpdateStatusLabel()}</span>
        <strong>Software Updates</strong>
        <p>{softwareUpdateDetail()}</p>
      </button>
    </div>

    {#if statusDetail && !requiredReady}
      <p class="status-detail">{statusDetail}</p>
    {/if}

    <footer>
      <span>{remainingText}</span>
      <span>{snapshot ? new Date(snapshot.checkedAtUnixMs).toLocaleTimeString() : 'Pending'}</span>
    </footer>
  </section>
</main>
