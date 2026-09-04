<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import './app.css';

  type DeliveryState = 'sending' | 'sent' | 'failed';
  type Row = { id: string; fromViewer: boolean; text: string; state?: DeliveryState };
  type AppMode = 'loading' | 'remote-chat' | 'update-reboot' | 'ai-approval';
  type RebootNotice = {
    noticeId: string;
    deadlineUnixMs: number;
    deferralsUsed: number;
    maxDeferrals: number;
    delayMinutes: number;
    actionSent: boolean;
  };
  type AiApprovalRequest = {
    approvalId: string;
    requesterLabel: string;
    requesterEmail: string | null;
    organizationName: string | null;
    deviceLabel: string;
    reason: string;
    expiresAtUnixMs: number;
    approvalWindowExpiresAtUnixMs: number;
    actionSent: boolean;
  };

  let appMode = $state<AppMode>('loading');
  let messages = $state<Row[]>([]);
  let draft = $state('');
  let bridgeOk = $state(false);
  let rootEl = $state<HTMLDivElement | undefined>();
  let rebootNotice = $state<RebootNotice>({
    noticeId: '',
    deadlineUnixMs: Date.now() + 15 * 60 * 1000,
    deferralsUsed: 0,
    maxDeferrals: 4,
    delayMinutes: 15,
    actionSent: false,
  });
  let aiApproval = $state<AiApprovalRequest>({
    approvalId: '',
    requesterLabel: 'A Talos operator',
    requesterEmail: null,
    organizationName: null,
    deviceLabel: 'this device',
    reason: 'View the current screen',
    expiresAtUnixMs: Date.now() + 5 * 60 * 1000,
    approvalWindowExpiresAtUnixMs: Date.now() + 15 * 60 * 1000,
    actionSent: false,
  });
  let countdownMs = $state(15 * 60 * 1000);
  let rebootError = $state('');
  let approvalError = $state('');
  let expiredApprovalCloseSent = $state(false);

  function errorText(error: unknown) {
    return error instanceof Error ? error.message : String(error);
  }

  function logUiEvent(event: string, data: Record<string, unknown> = {}) {
    void invoke('log_ui_event', { event, data }).catch((error) => {
      console.debug('failed to write Talos Worker Chat UI log', error);
    });
  }

  function upsertMessage(row: Row) {
    if (messages.some((m) => m.id === row.id)) return;
    messages = [...messages, row];
  }

  function markMessage(id: string, state: DeliveryState) {
    messages = messages.map((m) => (m.id === id ? { ...m, state } : m));
  }

  function rowFromPayload(p: Record<string, unknown>): Row | null {
    if (
      p.kind !== 'message' ||
      typeof p.text !== 'string' ||
      typeof p.id !== 'string'
    ) {
      return null;
    }
    const fromViewer = !!(p.fromViewer ?? p.from_viewer);
    return {
      id: p.id,
      fromViewer,
      text: p.text,
      state: 'sent',
    };
  }

  function numberValue(value: unknown, fallback: number) {
    return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
  }

  function boolValue(value: unknown, fallback = false) {
    return typeof value === 'boolean' ? value : fallback;
  }

  function stringValue(raw: Record<string, unknown>, camel: string, snake: string, fallback: string) {
    const value = raw[camel] ?? raw[snake];
    return typeof value === 'string' && value.trim() ? value.trim() : fallback;
  }

  function optionalStringValue(raw: Record<string, unknown>, camel: string, snake: string) {
    const value = raw[camel] ?? raw[snake];
    return typeof value === 'string' && value.trim() ? value.trim() : null;
  }

  function setRebootNotice(raw: Record<string, unknown>) {
    logUiEvent('set_reboot_notice', {
      noticeId: raw.noticeId ?? raw.notice_id ?? null,
      connected: raw.connected ?? null,
      actionSent: raw.actionSent ?? raw.action_sent ?? null,
    });
    rebootNotice = {
      noticeId: typeof raw.noticeId === 'string' ? raw.noticeId : rebootNotice.noticeId,
      deadlineUnixMs: numberValue(raw.deadlineUnixMs, rebootNotice.deadlineUnixMs),
      deferralsUsed: numberValue(raw.deferralsUsed, rebootNotice.deferralsUsed),
      maxDeferrals: numberValue(raw.maxDeferrals, rebootNotice.maxDeferrals),
      delayMinutes: numberValue(raw.delayMinutes, rebootNotice.delayMinutes),
      actionSent: boolValue(raw.actionSent, rebootNotice.actionSent),
    };
    bridgeOk = boolValue(raw.connected, bridgeOk);
    updateCountdown();
  }

  function setAiApproval(raw: Record<string, unknown>) {
    logUiEvent('set_ai_approval', {
      approvalId: raw.approvalId ?? raw.approval_id ?? null,
      requesterLabel: raw.requesterLabel ?? raw.requester_label ?? null,
      deviceLabel: raw.deviceLabel ?? raw.device_label ?? null,
      actionSent: raw.actionSent ?? raw.action_sent ?? null,
    });
    aiApproval = {
      approvalId: stringValue(raw, 'approvalId', 'approval_id', aiApproval.approvalId),
      requesterLabel: stringValue(raw, 'requesterLabel', 'requester_label', aiApproval.requesterLabel),
      requesterEmail: optionalStringValue(raw, 'requesterEmail', 'requester_email'),
      organizationName: optionalStringValue(raw, 'organizationName', 'organization_name'),
      deviceLabel: stringValue(raw, 'deviceLabel', 'device_label', aiApproval.deviceLabel),
      reason: stringValue(raw, 'reason', 'reason', aiApproval.reason),
      expiresAtUnixMs: numberValue(raw.expiresAtUnixMs ?? raw.expires_at_unix_ms, aiApproval.expiresAtUnixMs),
      approvalWindowExpiresAtUnixMs: numberValue(
        raw.approvalWindowExpiresAtUnixMs ?? raw.approval_window_expires_at_unix_ms,
        aiApproval.approvalWindowExpiresAtUnixMs
      ),
      actionSent: boolValue(raw.actionSent ?? raw.action_sent, aiApproval.actionSent),
    };
    expiredApprovalCloseSent = false;
    appMode = 'ai-approval';
    updateCountdown();
  }

  async function loadSnapshot() {
    logUiEvent('load_snapshot.begin');
    try {
      const snapshot = await invoke<Record<string, unknown>>('get_chat_snapshot');
      logUiEvent('load_snapshot.result', {
        connected: snapshot.connected ?? null,
        messageCount: Array.isArray(snapshot.messages) ? snapshot.messages.length : null,
      });
      if (typeof snapshot.connected === 'boolean') {
        bridgeOk = snapshot.connected;
      }
      if (Array.isArray(snapshot.messages)) {
        for (const item of snapshot.messages) {
          if (!item || typeof item !== 'object') continue;
          const row = rowFromPayload(item as Record<string, unknown>);
          if (row) upsertMessage(row);
        }
      }
    } catch (e) {
      logUiEvent('load_snapshot.error', { error: errorText(e) });
      console.error(e);
    }
  }

  async function loadAppState() {
    logUiEvent('load_app_state.begin');
    try {
      const state = await invoke<Record<string, unknown>>('get_app_state');
      logUiEvent('load_app_state.result', {
        mode: state.mode ?? null,
        chatConnected: state.chatConnected ?? state.chat_connected ?? null,
        hasAiApproval: !!state.aiApproval,
        hasRebootNotice: !!state.rebootNotice,
      });
      if (typeof state.chatConnected === 'boolean') {
        bridgeOk = state.chatConnected;
      } else if (typeof state.chat_connected === 'boolean') {
        bridgeOk = state.chat_connected;
      }
      if (state.mode === 'update-reboot') {
        const reboot = state.rebootNotice;
        if (reboot && typeof reboot === 'object') {
          setRebootNotice(reboot as Record<string, unknown>);
        }
        appMode = 'update-reboot';
        return;
      }
      const approval = state.aiApproval;
      if (approval && typeof approval === 'object') {
        logUiEvent('load_app_state.ai_approval_found');
        setAiApproval(approval as Record<string, unknown>);
        return;
      }
    } catch (e) {
      logUiEvent('load_app_state.error', { error: errorText(e) });
      console.error(e);
    }
    logUiEvent('load_app_state.fallback_remote_chat');
    appMode = 'remote-chat';
    await loadSnapshot();
  }

  onMount(() => {
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const timer = window.setInterval(updateCountdown, 1000);

    async function setupListenersAndState() {
      logUiEvent('startup.begin');
      try {
        const registered = await Promise.all([
          listen<Record<string, unknown>>('chat/inbound', (ev) => {
            const p = ev.payload;
            logUiEvent('event.chat_inbound', {
              hasPayload: !!p,
              kind: p && typeof p === 'object' ? (p as Record<string, unknown>).kind ?? null : null,
            });
            if (!p || typeof p !== 'object') return;
            const row = rowFromPayload(p as Record<string, unknown>);
            if (row) upsertMessage(row);
          }),
          listen<Record<string, unknown>>('chat/status', (ev) => {
            const p = ev.payload;
            logUiEvent('event.chat_status', {
              connected: p && typeof p === 'object' ? (p as Record<string, unknown>).connected ?? null : null,
              error: p && typeof p === 'object' ? (p as Record<string, unknown>).error ?? null : null,
            });
            if (!p || typeof p !== 'object' || typeof p.connected !== 'boolean') return;
            bridgeOk = p.connected;
            if (!p.connected) {
              messages = messages.map((m) => (m.state === 'sending' ? { ...m, state: 'failed' } : m));
            }
          }),
          listen<Record<string, unknown>>('chat/ack', (ev) => {
            const p = ev.payload;
            const id = (p as { messageId?: unknown; message_id?: unknown } | null)?.messageId ??
              (p as { messageId?: unknown; message_id?: unknown } | null)?.message_id;
            logUiEvent('event.chat_ack', { messageId: typeof id === 'string' ? id : null });
            if (typeof id === 'string') markMessage(id, 'sent');
          }),
          listen<Record<string, unknown>>('reboot/status', (ev) => {
            const p = ev.payload;
            logUiEvent('event.reboot_status', {
              connected: p && typeof p === 'object' ? (p as Record<string, unknown>).connected ?? null : null,
              error: p && typeof p === 'object' ? (p as Record<string, unknown>).error ?? null : null,
            });
            if (!p || typeof p !== 'object') return;
            if (typeof p.connected === 'boolean') bridgeOk = p.connected;
            if (typeof p.error === 'string') rebootError = p.error;
          }),
          listen<Record<string, unknown>>('approval/request', (ev) => {
            const p = ev.payload;
            logUiEvent('event.approval_request', {
              hasPayload: !!p,
              approvalId:
                p && typeof p === 'object'
                  ? (p as Record<string, unknown>).approvalId ??
                    (p as Record<string, unknown>).approval_id ??
                    null
                  : null,
            });
            if (!p || typeof p !== 'object') return;
            bridgeOk = true;
            setAiApproval(p as Record<string, unknown>);
          }),
          listen<Record<string, unknown>>('approval/status', (ev) => {
            const p = ev.payload;
            logUiEvent('event.approval_status', {
              sent: p && typeof p === 'object' ? (p as Record<string, unknown>).sent ?? null : null,
              error: p && typeof p === 'object' ? (p as Record<string, unknown>).error ?? null : null,
            });
            if (!p || typeof p !== 'object') return;
            if (typeof p.error === 'string') approvalError = p.error;
            if ((p as { sent?: unknown }).sent === true) {
              aiApproval = { ...aiApproval, actionSent: true };
            }
          }),
        ]);

        if (disposed) {
          for (const unlisten of registered) unlisten();
          return;
        }

        unlisteners.push(...registered);
        logUiEvent('startup.listeners_registered', { count: registered.length });
        await loadAppState();
      } catch (e) {
        logUiEvent('startup.error', { error: errorText(e) });
        console.error(e);
        if (!disposed) {
          appMode = 'remote-chat';
          await loadSnapshot();
        }
      }
    }

    void setupListenersAndState();

    return () => {
      disposed = true;
      window.clearInterval(timer);
      for (const unlisten of unlisteners) unlisten();
      logUiEvent('startup.cleanup');
    };
  });

  async function send() {
    const t = draft.trim();
    if (!t) return;
    draft = '';
    try {
      const sent = await invoke<Record<string, unknown>>('send_chat_message', { text: t });
      if (typeof sent.id === 'string' && typeof sent.text === 'string') {
        upsertMessage({
          id: sent.id,
          fromViewer: false,
          text: sent.text,
          state: 'sending',
        });
      }
    } catch (e) {
      draft = t;
      console.error(e);
    }
  }

  async function sendRebootAction(action: 'defer' | 'reboot_now') {
    if (rebootNotice.actionSent) return;
    rebootError = '';
    rebootNotice = { ...rebootNotice, actionSent: true };
    try {
      await invoke('send_reboot_notice_action', { action });
    } catch (e) {
      rebootNotice = { ...rebootNotice, actionSent: false };
      rebootError = e instanceof Error ? e.message : String(e);
    }
  }

  async function sendAiApprovalDecision(approved: boolean) {
    if (aiApproval.actionSent || !aiApproval.approvalId) return;
    approvalError = '';
    aiApproval = { ...aiApproval, actionSent: true };
    try {
      await invoke('send_ai_runner_approval_decision', {
        approvalId: aiApproval.approvalId,
        approved,
      });
    } catch (e) {
      aiApproval = { ...aiApproval, actionSent: false };
      approvalError = e instanceof Error ? e.message : String(e);
    }
  }

  async function closeExpiredAiApprovalWindow() {
    if (
      expiredApprovalCloseSent ||
      appMode !== 'ai-approval' ||
      aiApproval.actionSent ||
      !aiApproval.approvalId ||
      Date.now() < aiApproval.expiresAtUnixMs
    ) {
      return;
    }

    expiredApprovalCloseSent = true;
    logUiEvent('approval.expired_auto_close', { approvalId: aiApproval.approvalId });
    try {
      const result = await invoke<Record<string, unknown>>('close_expired_ai_runner_approval', {
        approvalId: aiApproval.approvalId,
      });
      if (result.closed !== true) {
        expiredApprovalCloseSent = false;
        logUiEvent('approval.expired_auto_close_skipped', {
          approvalId: aiApproval.approvalId,
          reason: result.reason ?? null,
        });
      }
    } catch (e) {
      expiredApprovalCloseSent = false;
      approvalError = e instanceof Error ? e.message : String(e);
      logUiEvent('approval.expired_auto_close_error', { error: approvalError });
    }
  }

  function keydown(ev: KeyboardEvent) {
    if (ev.key === 'Enter' && !ev.shiftKey) {
      ev.preventDefault();
      void send();
    }
  }

  function updateCountdown() {
    const remainingMs =
      appMode === 'ai-approval' ? aiApproval.expiresAtUnixMs - Date.now() : rebootNotice.deadlineUnixMs - Date.now();
    countdownMs = Math.max(0, remainingMs);
    if (appMode === 'ai-approval' && remainingMs <= 0) {
      void closeExpiredAiApprovalWindow();
    }
  }

  function countdownLabel() {
    const totalSeconds = Math.max(0, Math.ceil(countdownMs / 1000));
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = String(totalSeconds % 60).padStart(2, '0');
    return `${minutes}:${seconds}`;
  }

  function deferralsRemaining() {
    return Math.max(0, rebootNotice.maxDeferrals - rebootNotice.deferralsUsed);
  }

  function deferralStatusLabel() {
    const remaining = deferralsRemaining();
    if (remaining <= 0) return 'Delay limit reached';
    return `${remaining} ${remaining === 1 ? 'delay' : 'delays'} remaining`;
  }

  function approvalWindowLabel() {
    const totalSeconds = Math.max(0, Math.ceil((aiApproval.approvalWindowExpiresAtUnixMs - Date.now()) / 1000));
    const minutes = Math.max(1, Math.ceil(totalSeconds / 60));
    return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`;
  }

  $effect(() => {
    if (appMode !== 'remote-chat') return;
    void messages;
    queueMicrotask(() => rootEl?.scrollTo({ top: rootEl.scrollHeight, behavior: 'smooth' }));
  });
</script>

{#if appMode === 'update-reboot'}
  <main class="notice-shell">
    <section class="notice-panel" aria-labelledby="reboot-title">
      <div class="notice-brand">
        <span class="brand-mark" aria-hidden="true"><span></span><span></span><span></span></span>
        <div>
          <p>Talos</p>
          <strong>Software updates</strong>
        </div>
      </div>

      <div class="notice-copy">
        <p class="notice-eyebrow">Reboot required</p>
        <h1 id="reboot-title">Your device will reboot in 15 minutes</h1>
        <p>Updates are ready to finish installing. Save your work before the countdown ends.</p>
      </div>

      <div class="countdown" aria-live="polite">
        <span>{countdownLabel()}</span>
        <small>{deferralStatusLabel()}</small>
      </div>

      {#if rebootError}
        <p class="notice-error">{rebootError}</p>
      {/if}

      <div class="notice-actions">
        <button
          type="button"
          class="secondary"
          onclick={() => void sendRebootAction('defer')}
          disabled={!bridgeOk || rebootNotice.actionSent || deferralsRemaining() <= 0}
        >
          Delay {rebootNotice.delayMinutes} minutes
        </button>
        <button
          type="button"
          onclick={() => void sendRebootAction('reboot_now')}
          disabled={!bridgeOk || rebootNotice.actionSent}
        >
          Reboot now
        </button>
      </div>
    </section>
  </main>
{:else if appMode === 'ai-approval'}
  <main class="notice-shell approval-shell">
    <section class="notice-panel approval-panel" aria-labelledby="approval-title">
      <div class="notice-brand">
        <span class="brand-mark" aria-hidden="true"><span></span><span></span><span></span></span>
        <div>
          <p>Talos</p>
          <strong>AI screen approval</strong>
        </div>
      </div>

      <div class="notice-copy">
        <p class="notice-eyebrow">Approval requested</p>
        <h1 id="approval-title">{aiApproval.requesterLabel} wants Talos AI to view this device screen.</h1>
        <p>
          Approving lets Talos AI capture screenshots from {aiApproval.deviceLabel} for {approvalWindowLabel()}.
          Deny if you do not expect this request.
        </p>
      </div>

      <div class="approval-details">
        {#if aiApproval.organizationName}
          <span>Organization</span>
          <strong>{aiApproval.organizationName}</strong>
        {/if}
        {#if aiApproval.requesterEmail}
          <span>Requester</span>
          <strong>{aiApproval.requesterEmail}</strong>
        {/if}
        <span>Reason</span>
        <strong>{aiApproval.reason}</strong>
      </div>

      <div class="countdown" aria-live="polite">
        <span>{countdownLabel()}</span>
        <small>Request expires</small>
      </div>

      {#if approvalError}
        <p class="notice-error">{approvalError}</p>
      {/if}

      <div class="notice-actions">
        <button
          type="button"
          class="secondary"
          onclick={() => void sendAiApprovalDecision(false)}
          disabled={!bridgeOk || aiApproval.actionSent || countdownMs <= 0}
        >
          Deny
        </button>
        <button
          type="button"
          onclick={() => void sendAiApprovalDecision(true)}
          disabled={!bridgeOk || aiApproval.actionSent || countdownMs <= 0}
        >
          Approve
        </button>
      </div>
    </section>
  </main>
{:else if appMode === 'remote-chat'}
  <div class="shell">
    <header>
      Talos remote chat
      <div class="sub">
        {bridgeOk ? 'Connected to technician session' : 'Connecting...'}
      </div>
    </header>

    <div class="messages" bind:this={rootEl}>
      {#each messages as m (m.id)}
        <div class="row" class:remote={m.fromViewer} class:local={!m.fromViewer}>
          <div class="bubble">
            <span>{m.text}</span>
            {#if !m.fromViewer && m.state === 'failed'}
              <small>Failed</small>
            {:else if !m.fromViewer && m.state === 'sending'}
              <small>Sending</small>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    <div class="composer">
      <textarea
        bind:value={draft}
        placeholder="Message technician..."
        rows="2"
        onkeydown={keydown}
      ></textarea>
      <button type="button" onclick={() => void send()} disabled={!draft.trim()}> Send </button>
    </div>
  </div>
{:else}
  <main class="loading-shell"></main>
{/if}
