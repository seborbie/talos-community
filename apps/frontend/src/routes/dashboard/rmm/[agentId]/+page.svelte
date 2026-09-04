<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { ArrowLeft, Copy, Database, Folder, KeyRound, Monitor, RefreshCw, Terminal, Trash2, Package, Server, Zap, Layers, AlertCircle, History, LayoutDashboard, ScrollText } from 'lucide-svelte';
  import { installerApi, patchApi, rmmApi } from '$lib/api';
  import type {
    RmmCommandExecutionLogEntry,
    RmmConnectResponse,
    RmmDevice,
    RmmDeviceTelemetry,
    LinuxShellCredential,
    PatchDeviceStateResponse,
    RmmTelemetryGraphEvent,
    RmmViewerConnectionSummary
  } from '$lib/types';
  import { toast } from '$lib/toast';
  import { detectViewerInstallerPlatform, isDesktopViewerLaunchSupported, launchViewerDeepLink } from '$lib/viewer-launcher';
  import {
    formatViewerSessionKind,
    sortViewerConnectionsForDisplay,
    VIEWER_CONNECTION_POLL_MS,
    waitForViewerSessionConnected
  } from '$lib/viewer-session-status';

  let device: RmmDevice | null = null;
  let loading = true;
  let error: string | null = null;
  let telemetry: RmmDeviceTelemetry | null = null;
  let linuxShellCredential: LinuxShellCredential | null = null;
  let linuxShellCredentialLoading = false;
  let linuxShellCredentialError: string | null = null;
  let telemetryLoading = false;
  let telemetryError: string | null = null;
  let patchState: PatchDeviceStateResponse | null = null;
  let patchStateLoading = false;
  let patchStateError: string | null = null;
  let snapshotRequesting = false;
  let snapshotPolling = false;
  let snapshotError: string | null = null;
  let snapshotProgressLabel = '';
  let snapshotCooldownUntil = 0;
  let snapshotCooldownSeconds = 0;
  let snapshotCooldownInterval: ReturnType<typeof setInterval> | null = null;
  let deleteLoading = false;
  let deleteError: string | null = null;
  let deviceSettingsSaving = false;
  let deviceSettingsError: string | null = null;
  let aiRunnerAutoApproveDraft = false;
  let commandInput = '';
  let commandLoading = false;
  let commandError: string | null = null;
  let commandHistory: Array<{
    command: string;
    output: string;
    exitCode: number | null;
    timestamp: Date;
  }> = [];

  type InventoryTabId = 'overview' | 'applications' | 'services' | 'startup' | 'features' | 'pending-updates' | 'downloaded-updates' | 'failed-updates' | 'update-history' | 'audit-trail';
  let inventoryTab: InventoryTabId = 'overview';

  const AUDIT_PAGE_SIZE = 50;
  let auditLoading = false;
  let auditError: string | null = null;
  let auditEntries: RmmCommandExecutionLogEntry[] = [];
  let auditNextCursor: string | null = null;
  let auditQuery = '';
  let auditAllowed: 'all' | 'allowed' | 'blocked' = 'all';
  let graphLoading = false;
  let graphError: string | null = null;
  let telemetryEventsCount = 0;
  let telemetryFactsCount = 0;
  let telemetryBaselinesCount = 0;
  let telemetryDecisionsCount = 0;
  let latestDecisionAction: string | null = null;
  let telemetryEvents: RmmTelemetryGraphEvent[] = [];
  let telemetryEventQuery = '';
  let telemetryEventSeverity: 'all' | 'critical' | 'high' | 'medium' | 'low' | 'info' = 'all';
  let viewerInstallerDownloading = false;
  let viewerLaunchOverlayOpen = false;
  let viewerLaunchOverlayLabel = 'Viewer';
  let viewerLaunchTimedOut = false;
  let cancelViewerLaunchWait: (() => void) | null = null;
  let viewerConnections: RmmViewerConnectionSummary[] = [];
  let viewerConnectionsPollTimer: ReturnType<typeof setInterval> | null = null;
  let snapshotAbortController: AbortController | null = null;
  let destroyed = false;

  type UnknownRecord = Record<string, unknown>;

  const asRecord = (value: unknown): UnknownRecord | null => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return null;
    }
    return value as UnknownRecord;
  };

  const pickString = (...values: Array<unknown>): string | null => {
    for (const value of values) {
      if (typeof value === 'string' && value.trim()) {
        return value;
      }
    }
    return null;
  };

  const pickNumber = (...values: Array<unknown>): number | null => {
    for (const value of values) {
      if (typeof value === 'number') {
        return value;
      }
    }
    return null;
  };

  const formatLastSeen = (value?: string | null) => {
    if (!value) return 'Unknown';
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return 'Unknown';
    return new Date(parsed).toLocaleString();
  };

  const healthStatus = () => device?.health?.status ?? 'unknown';

  const healthStatusLabel = () => {
    const status = healthStatus();
    if (status === 'healthy') return 'Healthy';
    if (status === 'warning') return 'Warning';
    if (status === 'critical') return 'Critical';
    if (status === 'offline') return 'Offline';
    return status ? status.charAt(0).toUpperCase() + status.slice(1) : 'Unknown';
  };

  const healthBadgeClass = () => {
    const status = healthStatus();
    if (status === 'healthy') return 'aero-badge-online';
    if (status === 'warning') return 'aero-severity-medium health-pill';
    if (status === 'critical') return 'aero-severity-critical health-pill';
    if (status === 'offline') return 'aero-badge-offline';
    return 'aero-badge-neutral';
  };

  const formatSignalDate = (value?: string | null) => (value ? formatLastSeen(value) : '—');

  const formatBytes = (value?: number | null) => {
    if (!value || value <= 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let size = value;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    return `${size.toFixed(1)} ${units[unit]}`;
  };

  const formatMaybeBytes = (value?: number | null) => {
    if (value === null || value === undefined) return '—';
    return formatBytes(value);
  };

  const formatPatchCategory = (category?: string | null) => {
    if (!category) return 'Other';
    const labels: Record<string, string> = {
      microsoft_product: 'Microsoft product',
      uwp_app: 'UWP app'
    };
    return labels[category] ?? category.replace(/_/g, ' ').replace(/\b\w/g, (char) => char.toUpperCase());
  };

  const formatPatchState = (state?: string | null) => {
    if (!state) return 'Unknown';
    return state.replace(/_/g, ' ').replace(/\b\w/g, (char) => char.toUpperCase());
  };

  const patchStateBadgeClass = (state?: string | null) => {
    if (state === 'installed') return 'aero-badge-online';
    if (state === 'failed') return 'aero-badge-offline';
    if (state === 'downloaded') return 'aero-badge-amber';
    return 'aero-badge-neutral';
  };

  const macosUpdateAccountLabel = (status?: string | null) => {
    if (!status) return 'Unknown';
    const labels: Record<string, string> = {
      ready: 'Ready',
      needsEnrollment: 'Needs enrollment',
      missing: 'Missing',
      notRequired: 'Not required',
      error: 'Error'
    };
    return labels[status] ?? formatPatchState(status);
  };

  const macosUpdateAccountBadgeClass = (status?: string | null) => {
    if (status === 'ready' || status === 'notRequired') return 'aero-badge-online';
    if (status === 'needsEnrollment') return 'aero-badge-amber';
    if (status === 'missing' || status === 'error') return 'aero-badge-offline';
    return 'aero-badge-neutral';
  };

  const shouldShowMacosUpdateEnrollmentPrompt = (status?: string | null, failureMessage?: string | null) => {
    if (status !== 'missing' && status !== 'needsEnrollment' && status !== 'error') {
      return false;
    }
    const normalizedFailureMessage = failureMessage?.toLowerCase() ?? '';
    return !normalizedFailureMessage.includes('talos permissions helper') && !normalizedFailureMessage.includes('software updates enrollment');
  };

  const patchLifecycleState = (update: { lifecycleState: string; downloadedAt?: string | null; failureMessage?: string | null }) => {
    if (update.lifecycleState === 'failed' && update.failureMessage?.trim().toLowerCase() === 'inprogress') {
      return 'detected';
    }
    if (
      update.downloadedAt &&
      update.lifecycleState !== 'installed' &&
      update.lifecycleState !== 'failed' &&
      update.lifecycleState !== 'superseded' &&
      update.lifecycleState !== 'reboot_pending'
    ) {
      return 'downloaded';
    }
    return update.lifecycleState;
  };

  const formatDurationMs = (value?: number | null) => {
    if (value === null || value === undefined) return '—';
    if (!Number.isFinite(value)) return '—';
    if (value < 1000) return `${Math.round(value)} ms`;
    return `${(value / 1000).toFixed(2)} s`;
  };

  const normalizeSeverity = (value?: string | null) => (value ?? 'info').toLowerCase();

  const severityPillClass = (value?: string | null) => {
    const normalized = normalizeSeverity(value);
    if (normalized === 'critical') return 'aero-severity-critical';
    if (normalized === 'high' || normalized === 'error') return 'aero-severity-high';
    if (normalized === 'medium' || normalized === 'warn' || normalized === 'warning') return 'aero-severity-medium';
    if (normalized === 'low') return 'aero-severity-low';
    return 'aero-severity-info';
  };

  const formatEventTitle = (entry: RmmTelemetryGraphEvent) =>
    entry.message?.trim() || entry.code?.trim() || entry.eventType || 'event';

  let filteredTelemetryEvents: RmmTelemetryGraphEvent[] = [];
  $: {
    const query = telemetryEventQuery.trim().toLowerCase();
    filteredTelemetryEvents = telemetryEvents.filter((event) => {
      const severity = normalizeSeverity(event.severity);
      const severityMatch = telemetryEventSeverity === 'all' || severity === telemetryEventSeverity;
      if (!severityMatch) return false;
      if (!query) return true;
      const haystack = [
        event.eventType,
        event.message,
        event.code,
        event.serviceName,
        event.processName,
        event.source
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase();
      return haystack.includes(query);
    });
  }

  $: patchSummary = patchState?.summary ?? {
    pending: telemetry?.pendingUpdates?.length ?? 0,
    downloaded: 0,
    failed: 0,
    installed: telemetry?.installedUpdates?.length ?? 0,
    blocked: 0,
    deferred: 0,
    rebootPending: 0
  };
  $: patchUpdates = patchState?.updates ?? [];
  $: pendingPatchUpdates = patchUpdates.filter((update) =>
    update.applicabilityState === 'applicable' &&
    update.lifecycleState !== 'installed' &&
    update.lifecycleState !== 'superseded'
  );
  $: downloadedPatchUpdates = patchUpdates.filter((update) =>
    update.applicabilityState === 'applicable' && patchLifecycleState(update) === 'downloaded'
  );
  $: failedPatchUpdates = patchUpdates.filter((update) => patchLifecycleState(update) === 'failed');
  $: installedPatchUpdates = patchUpdates.filter((update) => patchLifecycleState(update) === 'installed');
  $: patchTransactionFailures = patchState?.transactionFailures ?? [];
  $: failedPatchSignalCount = failedPatchUpdates.length + patchTransactionFailures.length;

  const formatPatchAction = (action?: string | null) => {
    if (!action) return 'Patch action';
    return action.replace(/_/g, ' ').replace(/\b\w/g, (char) => char.toUpperCase());
  };

  const formatAuditCommand = (entry: RmmCommandExecutionLogEntry): { title: string; isViewerAction: boolean } => {
    const raw = entry.command ?? '';
    if (raw.startsWith('viewer:connect:')) {
      const kind = raw.slice('viewer:connect:'.length);
      if (kind === 'desktop') return { title: 'Connect Remote Desktop', isViewerAction: true };
      if (kind === 'shell') return { title: 'Connect Remote Shell', isViewerAction: true };
      if (kind === 'file-transfer') return { title: 'Connect File Transfer', isViewerAction: true };
      if (kind === 'remote-registry') return { title: 'Connect Remote Registry', isViewerAction: true };
      return { title: `Viewer action: connect (${kind})`, isViewerAction: true };
    }
    return { title: raw, isViewerAction: false };
  };

  const formatUptime = (seconds?: number | null) => {
    if (!seconds || seconds <= 0) return 'Unknown';
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return days > 0 ? `${days}d ${hours}h ${minutes}m` : `${hours}h ${minutes}m`;
  };

  const normalizeInventorySource = (source: unknown): UnknownRecord | null => {
    const record = asRecord(source);
    if (!record) return null;
    return asRecord(record['inventory']) ?? record;
  };

  const getInventory = () => normalizeInventorySource(telemetry?.deviceState?.inventoryData ?? device?.lastInventory ?? null);
  const getDetails = () => device?.deviceDetails ?? telemetry?.deviceState?.inventoryData ?? null;
  const getDeviceState = () => telemetry?.deviceState ?? null;

  const meaningfulString = (value: unknown): string | null => {
    if (typeof value !== 'string') return null;
    const normalized = value.trim();
    if (!normalized) return null;
    const lower = normalized.toLowerCase();
    if (lower === 'unknown' || lower === 'n/a') return null;
    return normalized;
  };

  const isLinuxOsText = (value: unknown): boolean => {
    const normalized = meaningfulString(value)?.toLowerCase();
    return Boolean(normalized && /\b(linux|debian|ubuntu|fedora|centos|rhel|rocky|alma|suse|arch)\b/.test(normalized));
  };

  const isMacosOsText = (value: unknown): boolean => {
    const normalized = meaningfulString(value)?.toLowerCase();
    return Boolean(normalized && /\b(macos|mac os|mac os x|os x|darwin)\b/.test(normalized));
  };

  const getPrimaryIpFromInventory = (source: UnknownRecord | null): string | null => {
    const adapters = getNetworks(source);
    for (const adapter of adapters) {
      if (!Array.isArray(adapter.ips)) continue;
      for (const ip of adapter.ips) {
        const candidate = meaningfulString(ip?.address);
        if (candidate) return candidate;
      }
    }
    return null;
  };

  const getCpuSummary = (source: UnknownRecord | null) => {
    const cpu = source ? asRecord(source['cpu']) : null;
    if (!cpu) return null;
    const frequency = pickNumber(
      cpu['frequency_mhz'],
      cpu['frequencyMHz'],
      cpu['frequencyMhz'],
      cpu['frequency']
    );
    return {
      brand: pickString(cpu['brand']) ?? 'Unknown',
      cores: pickNumber(cpu['cores']) ?? '—',
      frequency,
    };
  };

  const getMemorySummary = (source: UnknownRecord | null) => {
    const memory = source ? asRecord(source['memory']) : null;
    if (!memory) return null;
    const total = pickNumber(memory['total_bytes'], memory['totalBytes']);
    const available = pickNumber(memory['available_bytes'], memory['availableBytes']);
    return { total, available };
  };

  const getDisks = (source: UnknownRecord | null) => {
    const disks = source ? source['disks'] : null;
    if (!Array.isArray(disks)) return [];
    return disks.map((disk) => {
      const record = asRecord(disk);
      return {
        name: pickString(record?.['name']) ?? 'Disk',
        mount: pickString(record?.['mount_point'], record?.['mountPoint']) ?? '—',
        total: pickNumber(record?.['total_bytes'], record?.['totalBytes']),
        available: pickNumber(record?.['available_bytes'], record?.['availableBytes']),
        fs: pickString(record?.['file_system'], record?.['fileSystem']) ?? '—',
      };
    });
  };

  const getNetworks = (source: UnknownRecord | null) => {
    const network = source ? asRecord(source['network']) : null;
    const adapters =
      network && Array.isArray(network['adapters'])
        ? network['adapters']
        : source && Array.isArray(source['networks'])
          ? source['networks']
          : null;
    if (!adapters || adapters.length === 0) return [];
    return adapters.map((adapter: unknown) => {
      const record = asRecord(adapter);
      if (!record) return { name: 'Adapter', ips: [], gateways: [], dnsServers: [] };
      const ipsRaw = Array.isArray(record['ips']) ? record['ips'] : [];
      const ips = ipsRaw.map((ip: unknown) => {
        const ipRecord = asRecord(ip);
        const address = pickString(ipRecord?.['address']) ?? '';
        const prefix = pickNumber(ipRecord?.['prefix']) ?? null;
        const mask = prefix != null ? `/${prefix}` : '';
        return { address, prefix, mask: address ? `${address}${mask}` : null };
      }).filter((x: { mask: string | null }) => x.mask);
      const gateways = Array.isArray(record['gateways']) ? record['gateways'].map((g: unknown) => typeof g === 'string' ? g.trim() : String(g)).filter(Boolean) : [];
      const dnsRaw = Array.isArray(record['dns_servers'])
        ? record['dns_servers']
        : Array.isArray(record['dnsServers'])
          ? record['dnsServers']
          : [];
      const dnsServers = dnsRaw.map((d: unknown) => typeof d === 'string' ? d.trim() : String(d)).filter(Boolean);
      return {
        name: pickString(record['name'], record['description']) ?? 'Adapter',
        ips,
        gateways,
        dnsServers
      };
    });
  };

  const getSystemSummary = (source: UnknownRecord | null) => {
    const system = source ? asRecord(source['system']) : null;
    if (!system) return null;
    return {
      name: pickString(system['name'], system['hostname'], system['os_name'], system['distro']) ?? 'Unknown',
      osVersion: pickString(system['osVersion'], system['os_version']) ?? 'Unknown',
      uptimeSeconds: pickNumber(system['uptimeSeconds'], system['uptime_seconds']),
      bootTime: pickNumber(system['bootTime'], system['boot_time']),
    };
  };

  const getProcesses = (source: UnknownRecord | null) => {
    const processes = source ? source['processes'] : null;
    if (!Array.isArray(processes)) return [];
    const records: UnknownRecord[] = [];
    for (const process of processes) {
      const record = asRecord(process);
      if (record) records.push(record);
    }
    return records;
  };

  const getProcessName = (process: UnknownRecord) => {
    const name = pickString(
      process['name'],
      process['processName'],
      process['command'],
      process['path']
    );
    if (name) return name;
    const nested = asRecord(process['name']);
    return (
      pickString(nested?.['name'], nested?.['value'], nested?.['process'], nested?.['command']) ??
      'Unknown'
    );
  };

  const getProcessMemoryBytes = (process: UnknownRecord): number | null => {
    return pickNumber(process['memory_bytes'], process['memoryBytes'], process['memory']);
  };

  const fetchCommandAudit = async (
    agentId: string,
    options: { reset?: boolean } = {}
  ) => {
    if (!agentId.trim()) return;
    if (auditLoading) return;

    const reset = options.reset ?? false;
    const cursor = reset ? null : auditNextCursor;

    try {
      auditLoading = true;
      auditError = null;

      const response = await rmmApi.getCommandExecutionLogs(agentId, {
        limit: AUDIT_PAGE_SIZE,
        cursor,
        q: auditQuery,
        allowed: auditAllowed
      });

      if (reset) {
        auditEntries = response.items;
      } else {
        const seen = new Set(auditEntries.map((entry) => entry.id));
        auditEntries = [...auditEntries, ...response.items.filter((entry) => !seen.has(entry.id))];
      }
      auditNextCursor = response.nextCursor;
    } catch (err) {
      console.error('Failed to fetch audit trail:', err);
      auditError = err instanceof Error ? err.message : 'Failed to fetch audit trail';
    } finally {
      auditLoading = false;
    }
  };

  const handleAuditKeyDown = (event: unknown) => {
    const keyboard = event as KeyboardEvent;
    if (keyboard?.key === 'Enter') {
      keyboard.preventDefault();
      if (device) void fetchCommandAudit(device.agentId, { reset: true });
    }
  };

  const fetchDevice = async (options: { refreshTelemetry?: boolean; refreshAudit?: boolean; silent?: boolean } = {}) => {
    const refreshTelemetry = options.refreshTelemetry ?? true;
    const refreshAudit = options.refreshAudit ?? true;
    const silent = options.silent ?? false;
    try {
      if (!silent) {
        loading = true;
      }
      error = null;
      const agentId = $page.params.agentId ? String($page.params.agentId) : '';
      if (!agentId.trim()) {
        throw new Error('Missing agent id');
      }
      device = await rmmApi.getDevice(agentId);
      syncDeviceSettingsDraft(device);
      linuxShellCredential = null;
      linuxShellCredentialError = null;
      void fetchViewerConnections(agentId);
      if (refreshAudit) void fetchCommandAudit(agentId, { reset: true });
      if (refreshTelemetry) void fetchTelemetry(agentId);
      void fetchPatchState(agentId);
      void fetchGraphTelemetry(agentId);
    } catch (err) {
      console.error('Failed to fetch device:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch device';
    } finally {
      if (!silent) {
        loading = false;
      }
    }
  };

  const syncDeviceSettingsDraft = (nextDevice: RmmDevice | null) => {
    aiRunnerAutoApproveDraft = Boolean(nextDevice?.aiRunnerAutoApprove);
    deviceSettingsError = null;
  };

  const fetchViewerConnections = async (agentId: string) => {
    if (!agentId.trim()) return;
    try {
      viewerConnections = sortViewerConnectionsForDisplay(await rmmApi.getViewerConnections(agentId));
    } catch (err) {
      console.error('Failed to fetch viewer connections:', err);
    }
  };

  const startViewerConnectionsPolling = (agentId: string) => {
    if (viewerConnectionsPollTimer) {
      clearInterval(viewerConnectionsPollTimer);
    }
    viewerConnectionsPollTimer = setInterval(() => {
      if (document.visibilityState !== 'visible') {
        return;
      }
      void fetchViewerConnections(agentId);
    }, VIEWER_CONNECTION_POLL_MS);
  };

  const fetchTelemetry = async (agentId: string) => {
    if (!agentId.trim()) return;
    try {
      telemetryLoading = true;
      telemetryError = null;
      telemetry = await rmmApi.getDeviceTelemetry(agentId);
    } catch (err) {
      console.error('Failed to fetch telemetry:', err);
      telemetryError = err instanceof Error ? err.message : 'Failed to load inventory data';
    } finally {
      telemetryLoading = false;
    }
  };

  const fetchGraphTelemetry = async (agentId: string) => {
    if (!agentId.trim()) return;
    try {
      graphLoading = true;
      graphError = null;
      const [eventsRes, factsRes, baselinesRes, decisionsRes] = await Promise.all([
        rmmApi.getTelemetryEvents(agentId, 100),
        rmmApi.getTelemetryFacts(agentId),
        rmmApi.getTelemetryBaselines(agentId),
        rmmApi.getTelemetryDecisions(agentId, 100)
      ]);
      telemetryEvents = [...eventsRes.items].sort((a, b) => Date.parse(b.occurredAt) - Date.parse(a.occurredAt));
      telemetryEventsCount = eventsRes.items.length;
      telemetryFactsCount = factsRes.items.length;
      telemetryBaselinesCount = baselinesRes.items.length;
      telemetryDecisionsCount = decisionsRes.items.length;
      latestDecisionAction = decisionsRes.items[0]?.action ?? null;
    } catch (err) {
      console.error('Failed to fetch graph telemetry:', err);
      graphError = err instanceof Error ? err.message : 'Failed to load graph telemetry';
      telemetryEvents = [];
    } finally {
      graphLoading = false;
    }
  };

  const SNAPSHOT_COOLDOWN_MS = 30_000;
  const SNAPSHOT_POLL_INTERVAL_MS = 2500;
  const SNAPSHOT_POLL_TIMEOUT_MS = 120_000;

  const isAbortError = (err: unknown) =>
    err instanceof DOMException && err.name === 'AbortError';

  const sleepMs = (ms: number, signal?: AbortSignal) =>
    new Promise<void>((resolve, reject) => {
      if (signal?.aborted) {
        reject(new DOMException('Aborted', 'AbortError'));
        return;
      }
      const timeout = setTimeout(resolve, ms);
      signal?.addEventListener(
        'abort',
        () => {
          clearTimeout(timeout);
          reject(new DOMException('Aborted', 'AbortError'));
        },
        { once: true }
      );
    });

  /** True when collectedAt is after requestStartedAt. Used only when no request id is available. */
  const isCollectedAfterRequest = (collectedAt: string | null, requestStartedAtMs: number) => {
    if (!collectedAt) return false;
    const ms = Date.parse(collectedAt);
    if (Number.isNaN(ms)) return false;
    return ms >= requestStartedAtMs;
  };

  const fetchPatchState = async (agentId: string) => {
    if (!agentId.trim()) return;
    try {
      patchStateLoading = true;
      patchStateError = null;
      patchState = await patchApi.getDeviceState(agentId);
    } catch (err) {
      console.error('Failed to fetch patch state:', err);
      patchStateError = err instanceof Error ? err.message : 'Failed to load patch state';
    } finally {
      patchStateLoading = false;
    }
  };

  const waitForInventoryUpdate = async (
    agentId: string,
    requestStartedAtMs: number,
    requestId: string | null,
    previousCollectedAt: string | null,
    signal: AbortSignal
  ): Promise<RmmDeviceTelemetry> => {
    const startedAt = Date.now();
    while (Date.now() - startedAt < SNAPSHOT_POLL_TIMEOUT_MS) {
      if (signal.aborted) throw new DOMException('Aborted', 'AbortError');
      try {
        const requestStatus = requestId
          ? await rmmApi.getSnapshotRequestStatus(agentId, requestId)
          : null;
        if (requestStatus?.status === 'failed') {
          throw new Error('Snapshot collection failed on the agent');
        }
        if (requestId && requestStatus?.status !== 'completed') {
          snapshotProgressLabel = 'Snapshot requested; waiting for agent collection...';
          await sleepMs(SNAPSHOT_POLL_INTERVAL_MS, signal);
          continue;
        }
        const telemetryPayload = await rmmApi.getDeviceTelemetry(agentId);
        const currentCollectedAt = telemetryPayload?.deviceState?.collectedAt ?? null;
        const isNewerThanPrevious =
          Boolean(previousCollectedAt && currentCollectedAt && currentCollectedAt !== previousCollectedAt);
        const isCompletedRequestTelemetry =
          requestStatus?.status === 'completed' &&
          Boolean(
            currentCollectedAt &&
              (!previousCollectedAt ||
                currentCollectedAt !== previousCollectedAt ||
                isCollectedAfterRequest(currentCollectedAt, requestStartedAtMs))
          );
        if (
          isCompletedRequestTelemetry ||
          isNewerThanPrevious ||
          (!requestId && isCollectedAfterRequest(currentCollectedAt, requestStartedAtMs))
        ) {
          return telemetryPayload;
        }
        if (requestStatus?.status === 'completed') {
          snapshotProgressLabel = 'Snapshot collected; waiting for inventory read model...';
        }
      } catch (err) {
        if (isAbortError(err)) throw err;
        const message = err instanceof Error ? err.message.toLowerCase() : '';
        if (message.includes('not found')) {
          await sleepMs(SNAPSHOT_POLL_INTERVAL_MS, signal);
          continue;
        }
        throw err;
      }
      await sleepMs(SNAPSHOT_POLL_INTERVAL_MS, signal);
    }
    throw new Error('Snapshot collection timed out (120s) before inventory updated');
  };

  const requestSnapshot = async () => {
    if (!device) return;
    if (snapshotRequesting || snapshotPolling || Date.now() < snapshotCooldownUntil) return;
    try {
      snapshotAbortController?.abort();
      snapshotAbortController = new AbortController();
      snapshotRequesting = true;
      snapshotError = null;
      snapshotProgressLabel = 'Waiting for agent/data collection...';
      const previousCollectedAt = telemetry?.deviceState?.collectedAt ?? null;
      const requestStartedAtMs = Date.now();
      const snapshotRequest = await rmmApi.requestSnapshot(device.agentId);
      snapshotCooldownUntil = Date.now() + SNAPSHOT_COOLDOWN_MS;
      snapshotCooldownSeconds = 30;
      if (snapshotCooldownInterval) clearInterval(snapshotCooldownInterval);
      snapshotCooldownInterval = setInterval(() => {
        snapshotCooldownSeconds = Math.max(0, Math.ceil((snapshotCooldownUntil - Date.now()) / 1000));
        if (snapshotCooldownSeconds <= 0 && snapshotCooldownInterval) {
          clearInterval(snapshotCooldownInterval);
          snapshotCooldownInterval = null;
        }
      }, 1000);
      snapshotPolling = true;
      const updatedTelemetry = await waitForInventoryUpdate(
        device.agentId,
        requestStartedAtMs,
        snapshotRequest.requestId ?? null,
        previousCollectedAt,
        snapshotAbortController.signal
      );
      if (destroyed || snapshotAbortController.signal.aborted) return;
      telemetry = updatedTelemetry;
      await fetchDevice({ refreshTelemetry: false, refreshAudit: false, silent: true });
      void fetchGraphTelemetry(device.agentId);
      toast({ title: 'Snapshot completed', description: 'Inventory has been refreshed from the latest telemetry.' });
    } catch (err) {
      if (isAbortError(err)) return;
      const msg = err instanceof Error ? err.message : 'Failed to request snapshot';
      snapshotError = msg;
      toast({ title: 'Snapshot request failed', description: msg, variant: 'destructive' });
    } finally {
      if (!destroyed) {
        snapshotRequesting = false;
        snapshotPolling = false;
        snapshotProgressLabel = '';
      }
    }
  };

  let lastInventoryLabel = '—';
  $: lastInventoryLabel =
    telemetry?.deviceState?.collectedAt
      ? formatLastSeen(telemetry.deviceState.collectedAt)
      : '—';

  let coreHostname = '—';
  let coreOs = '—';
  let coreIp = '—';
  let coreVersion = '—';
  let coreLastSeen = 'Unknown';
  let isLinuxAgent = false;
  let isMacosAgent = false;
  let isLimitedInteractiveAgent = true;
  let supportsRemoteDesktop = false;
  let supportsRemoteRegistry = false;
  let remoteDesktopDisabled = true;
  let remoteRegistryDisabled = true;
  let commandDescription = 'Execute PowerShell commands on the remote device.';
  let commandPlaceholder = 'Enter PowerShell command (e.g., Get-Service)';
  let featureTabLabel = 'Windows features';
  let featureEmptyLabel = 'No Windows features in telemetry. Request snapshot above or wait for telemetry.';
  let deviceSettingsChanged = false;
  $: deviceSettingsChanged = Boolean(device && aiRunnerAutoApproveDraft !== Boolean(device.aiRunnerAutoApprove));
  $: {
    const state = getDeviceState();
    const inventory = getInventory();
    const system = getSystemSummary(inventory);
    const deviceIp = meaningfulString(device?.ip);
    coreHostname =
      meaningfulString(device?.hostname) ??
      meaningfulString(state?.hostname) ??
      meaningfulString(system?.name) ??
      '—';
    coreOs =
      meaningfulString(device?.os) ??
      meaningfulString(state?.osName) ??
      '—';
    coreIp =
      (deviceIp && deviceIp !== '0.0.0.0' ? deviceIp : null) ??
      meaningfulString(getPrimaryIpFromInventory(inventory)) ??
      '—';
    coreVersion =
      meaningfulString(device?.version) ??
      meaningfulString(state?.agentVersion) ??
      '—';
    coreLastSeen = device?.lastSeen
      ? formatLastSeen(device.lastSeen)
      : state?.collectedAt
        ? formatLastSeen(state.collectedAt)
        : 'Unknown';
    isLinuxAgent =
      isLinuxOsText(device?.os) ||
      isLinuxOsText(state?.osName) ||
      isLinuxOsText(system?.name) ||
      isLinuxOsText(system?.osVersion);
    isMacosAgent =
      isMacosOsText(device?.os) ||
      isMacosOsText(state?.osName) ||
      isMacosOsText(system?.name) ||
      isMacosOsText(system?.osVersion);
    isLimitedInteractiveAgent = isLinuxAgent;
    supportsRemoteDesktop = Boolean(device) && !isLinuxAgent;
    supportsRemoteRegistry = Boolean(device) && !isLinuxAgent && !isMacosAgent;
    remoteDesktopDisabled = !supportsRemoteDesktop;
    remoteRegistryDisabled = !supportsRemoteRegistry;
    commandDescription = isMacosAgent
      ? 'Execute shell commands on the remote Mac.'
      : 'Execute PowerShell commands on the remote device.';
    commandPlaceholder = isMacosAgent
      ? 'Enter shell command (e.g., launchctl list)'
      : 'Enter PowerShell command (e.g., Get-Service)';
    featureTabLabel = isMacosAgent ? 'System features' : 'Windows features';
    featureEmptyLabel = isMacosAgent
      ? 'No system features in telemetry. Request snapshot above or wait for telemetry.'
      : 'No Windows features in telemetry. Request snapshot above or wait for telemetry.';
  }

  const connectViewer = async () => {
    if (!device) return;
    const agentId = device.agentId;
    await attemptViewerLaunch(
      'Viewer',
      agentId,
      async () => await rmmApi.connectDevice(agentId),
      'Failed to open viewer.'
    );
  };

  const connectShell = async () => {
    if (!device) return;
    const agentId = device.agentId;
    await attemptViewerLaunch(
      'Shell',
      agentId,
      async () => await rmmApi.connectShell(agentId),
      'Failed to open shell.'
    );
  };

  const connectFileTransfer = async () => {
    if (!device) return;
    const agentId = device.agentId;
    await attemptViewerLaunch(
      'File Transfer',
      agentId,
      async () => await rmmApi.connectFileTransfer(agentId),
      'Failed to open file transfer.'
    );
  };

  const connectRegistry = async () => {
    if (!device) return;
    const agentId = device.agentId;
    await attemptViewerLaunch(
      'Remote Registry',
      agentId,
      async () => await rmmApi.connectRegistry(agentId),
      'Failed to open remote registry.'
    );
  };

  const revealLinuxShellCredential = async () => {
    if (!device) return;
    linuxShellCredentialLoading = true;
    linuxShellCredentialError = null;
    try {
      linuxShellCredential = await rmmApi.getLinuxShellCredential(device.agentId);
    } catch (err) {
      linuxShellCredentialError = err instanceof Error ? err.message : 'Failed to reveal credential';
    } finally {
      linuxShellCredentialLoading = false;
    }
  };

  const hideLinuxShellCredential = () => {
    linuxShellCredential = null;
    linuxShellCredentialError = null;
  };

  const toggleLinuxShellCredential = async () => {
    if (linuxShellCredential) {
      hideLinuxShellCredential();
      return;
    }
    await revealLinuxShellCredential();
  };

  const copyLinuxCredentialValue = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast({ title: `${label} copied` });
    } catch {
      toast({ title: `Failed to copy ${label.toLowerCase()}`, variant: 'destructive' });
    }
  };

  const saveDeviceSettings = async () => {
    if (!device || !deviceSettingsChanged || deviceSettingsSaving) return;
    try {
      deviceSettingsSaving = true;
      deviceSettingsError = null;
      const updated = await rmmApi.updateDeviceSettings(device.agentId, {
        aiRunnerAutoApprove: aiRunnerAutoApproveDraft
      });
      device = updated;
      syncDeviceSettingsDraft(updated);
      toast({ title: 'Device settings saved' });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to save device settings';
      deviceSettingsError = message;
      toast({ title: 'Device settings failed', description: message, variant: 'destructive' });
    } finally {
      deviceSettingsSaving = false;
    }
  };

  const copyHostname = async () => {
    const hostname = coreHostname === '—' ? null : meaningfulString(coreHostname);
    if (!hostname) return;

    try {
      await navigator.clipboard.writeText(hostname);
      toast({ title: 'Hostname copied', description: hostname });
    } catch {
      toast({ title: 'Failed to copy hostname', variant: 'destructive' });
    }
  };

  const saveBlobFile = (filename: string, blob: Blob) => {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const waitForOverlayPaint = async () => {
    await tick();
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  };

  const resolveConnectSessionId = (response: RmmConnectResponse): string | null => {
    if (typeof response.sessionId === 'string' && response.sessionId.trim()) {
      return response.sessionId.trim();
    }
    try {
      const parsed = new URL(response.url);
      const fromUrl = parsed.searchParams.get('session');
      return fromUrl?.trim() ? fromUrl.trim() : null;
    } catch {
      return null;
    }
  };

  const downloadViewerInstaller = async () => {
    try {
      viewerInstallerDownloading = true;
      const result = await installerApi.downloadViewerInstaller(detectViewerInstallerPlatform());
      saveBlobFile(result.filename, result.blob);
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to download Talos Viewer.';
      toast({
        title: 'Viewer download failed',
        description: message,
      });
    } finally {
      viewerInstallerDownloading = false;
    }
  };

  const attemptViewerLaunch = async (
    actionLabel: string,
    agentId: string,
    requestConnect: () => Promise<RmmConnectResponse>,
    connectErrorMessage: string
  ) => {
    if (!isDesktopViewerLaunchSupported()) {
      toast({
        title: `${actionLabel} unavailable on this device`,
        description: 'Talos Viewer launch is supported on Windows and macOS browsers.',
      });
      return;
    }

    try {
      cancelViewerLaunchWait?.();
      const connectResponse = await requestConnect();
      viewerLaunchOverlayLabel = actionLabel;
      viewerLaunchOverlayOpen = true;
      viewerLaunchTimedOut = false;
      let cancelled = false;
      cancelViewerLaunchWait = () => {
        cancelled = true;
        viewerLaunchOverlayOpen = false;
        viewerLaunchTimedOut = false;
      };
      await waitForOverlayPaint();
      await launchViewerDeepLink(connectResponse.url);
      const sessionId = resolveConnectSessionId(connectResponse);
      if (!sessionId) {
        throw new Error('Connect response missing session id');
      }
      const status = await waitForViewerSessionConnected(sessionId, {
        agentId,
        onTimeout: () => {
          viewerLaunchTimedOut = true;
        },
        shouldCancel: () => cancelled,
      });
      cancelViewerLaunchWait = null;
      if (cancelled) {
        return;
      }
      viewerLaunchOverlayOpen = false;
      if (!status?.connected) {
        return;
      }
      viewerLaunchTimedOut = false;
      if (device) {
        await fetchViewerConnections(device.agentId);
      }
    } catch (err) {
      cancelViewerLaunchWait = null;
      viewerLaunchOverlayOpen = false;
      viewerLaunchTimedOut = false;
      const message = err instanceof Error ? err.message : connectErrorMessage;
      toast({
        title: `Unable to open ${actionLabel.toLowerCase()}`,
        description: message,
      });
    }
  };

  const executeCommand = async () => {
    if (!device || isLimitedInteractiveAgent || !commandInput.trim()) return;
    try {
      commandLoading = true;
      commandError = null;
      const result = await rmmApi.executeScript(device.agentId, commandInput.trim());
      commandHistory = [
        {
          command: commandInput.trim(),
          output: result.output,
          exitCode: result.exit_code,
          timestamp: new Date(),
        },
        ...commandHistory,
      ].slice(0, 10);
      commandInput = '';
      void fetchCommandAudit(device.agentId, { reset: true });
    } catch (err: unknown) {
      console.error('Failed to execute command:', err);
      const status = (err as { response?: { status?: number } })?.response?.status;
      commandError = status === 403
        ? "This command isn't allowed."
        : err instanceof Error ? err.message : 'Failed to execute command';
    } finally {
      commandLoading = false;
    }
  };

  const deleteDevice = async () => {
    if (!device) return;
    const confirmed = window.confirm(`Delete ${device.hostname}? This cannot be undone.`);
    if (!confirmed) return;

    try {
      deleteLoading = true;
      deleteError = null;
      await rmmApi.deleteDevice(device.agentId);
      await goto('/dashboard/devices');
    } catch (err) {
      console.error('Failed to delete device:', err);
      deleteError = err instanceof Error ? err.message : 'Failed to delete device';
    } finally {
      deleteLoading = false;
    }
  };

  const handleKeyDown = (event: unknown) => {
    const keyboard = event as KeyboardEvent;
    if (keyboard?.key === 'Enter' && !keyboard.shiftKey) {
      keyboard.preventDefault();
      executeCommand();
    }
  };

  onMount(() => {
    void fetchDevice();
    const agentId = $page.params.agentId ? String($page.params.agentId) : '';
    if (agentId.trim()) {
      startViewerConnectionsPolling(agentId);
      void fetchViewerConnections(agentId);
    }
  });

  onDestroy(() => {
    destroyed = true;
    snapshotAbortController?.abort();
    snapshotAbortController = null;
    if (snapshotCooldownInterval) clearInterval(snapshotCooldownInterval);
    if (viewerConnectionsPollTimer) {
      clearInterval(viewerConnectionsPollTimer);
      viewerConnectionsPollTimer = null;
    }
  });
</script>

<div class="space-y-6">
  <div class="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
    <div class="flex items-center gap-4">
      <a href="/dashboard/devices" class="text-muted-foreground hover:text-foreground transition-colors">
        <ArrowLeft class="h-5 w-5" />
      </a>
      <div>
        <h1 class="text-3xl font-bold aero-gradient-text">Device Details</h1>
        <p class="text-sm aero-muted mt-1">RMM agent details and inventory.</p>
      </div>
    </div>
    <div class="flex flex-wrap items-center gap-2">
      <Button variant="outline" onclick={fetchDevice} disabled={loading}>
        <RefreshCw class={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} />
        Refresh
      </Button>
      {#if supportsRemoteDesktop}
        <Button onclick={connectViewer} disabled={!device}>
          <Monitor class="h-4 w-4" />
          Open Viewer
        </Button>
      {/if}
      <Button variant="outline" onclick={connectShell} disabled={!device}>
        <Terminal class="h-4 w-4" />
        Shell
      </Button>
      <Button variant="outline" onclick={connectFileTransfer} disabled={!device}>
        <Folder class="h-4 w-4" />
        Files
      </Button>
      {#if supportsRemoteRegistry}
        <Button variant="outline" onclick={connectRegistry} disabled={!device}>
          <Database class="h-4 w-4" />
          Registry
        </Button>
      {/if}
      <Button variant="destructive" onclick={deleteDevice} disabled={deleteLoading || !device}>
        <Trash2 class="h-4 w-4" />
        {deleteLoading ? 'Deleting...' : 'Delete'}
      </Button>
    </div>
  </div>

  {#if loading}
    <div class="flex items-center justify-center h-32">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
    </div>
  {:else if error}
    <div class="aero-alert-error">{error}</div>
  {:else if device}
    {#if deleteError}
      <div class="aero-alert-error">{deleteError}</div>
    {/if}

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <Card>
        <CardHeader>
          <CardTitle>Core Details</CardTitle>
          <CardDescription>Basic device identity and status.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3 text-sm text-foreground">
          <div class="flex items-center justify-between gap-3">
            <span class="aero-muted">Hostname</span>
            <button
              class="inline-flex min-w-0 items-center justify-end gap-1.5 rounded-sm text-right font-medium transition-colors hover:text-sky-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300/70 focus-visible:ring-offset-2 focus-visible:ring-offset-transparent disabled:pointer-events-none disabled:opacity-70"
              type="button"
              title="Copy hostname"
              aria-label={`Copy hostname ${coreHostname}`}
              disabled={coreHostname === '—'}
              onclick={copyHostname}
            >
              <span class="truncate">{coreHostname}</span>
              <Copy class="h-3.5 w-3.5 opacity-70" aria-hidden="true" />
            </button>
          </div>
          <div class="flex items-center justify-between">
            <span class="aero-muted">OS</span>
            <span>{coreOs}</span>
          </div>
          <div class="flex items-center justify-between">
            <span class="aero-muted">IP</span>
            <span>{coreIp}</span>
          </div>
          <div class="flex items-center justify-between">
            <span class="aero-muted">Version</span>
            <span>{coreVersion}</span>
          </div>
          <div class="flex items-center justify-between">
            <span class="aero-muted">Last Seen</span>
            <span>{coreLastSeen}</span>
          </div>
          {#if isLinuxAgent}
            <div class="rounded-md border border-border/70 p-3 space-y-3">
              <div class="flex items-center justify-between gap-3">
                <div>
                  <div class="text-sm font-medium flex items-center gap-2">
                    <KeyRound class="h-4 w-4" />
                    Linux sudo credential
                  </div>
                  <div class="text-xs aero-muted">
                    {device.hasLinuxShellCredential ? `Managed user: ${device.linuxShellUsername ?? 'available'}` : 'No credential reported yet.'}
                  </div>
                </div>
                <Button
                  variant="outline"
                  onclick={toggleLinuxShellCredential}
                  disabled={linuxShellCredentialLoading || (!linuxShellCredential && !device.hasLinuxShellCredential)}
                >
                  {linuxShellCredentialLoading ? 'Revealing...' : linuxShellCredential ? 'Hide' : 'Reveal'}
                </Button>
              </div>
              {#if linuxShellCredentialError}
                <div class="aero-alert-error">{linuxShellCredentialError}</div>
              {/if}
              {#if linuxShellCredential}
                <div class="grid gap-2 text-sm">
                  <div class="flex items-center justify-between gap-2">
                    <span class="aero-muted">Username</span>
                    <button
                      class="font-mono text-xs hover:underline"
                      type="button"
                      onclick={() => copyLinuxCredentialValue(linuxShellCredential!.username, 'Username')}
                    >
                      {linuxShellCredential.username}
                    </button>
                  </div>
                  <div class="flex items-center justify-between gap-2">
                    <span class="aero-muted">Password</span>
                    <button
                      class="font-mono text-xs break-all text-right hover:underline"
                      type="button"
                      onclick={() => copyLinuxCredentialValue(linuxShellCredential!.password, 'Password')}
                    >
                      {linuxShellCredential.password}
                    </button>
                  </div>
                  <Button
                    variant="outline"
                    onclick={() => copyLinuxCredentialValue(`${linuxShellCredential!.username}:${linuxShellCredential!.password}`, 'Credential')}
                    className="w-full"
                  >
                    <Copy class="h-4 w-4" />
                    Copy Credential
                  </Button>
                </div>
              {/if}
            </div>
          {/if}
          {#if isMacosAgent}
            <div class="rounded-md border border-border/70 p-3 space-y-3">
              <div class="flex items-center justify-between gap-3">
                <div>
                  <div class="text-sm font-medium flex items-center gap-2">
                    <Package class="h-4 w-4" />
                    macOS software updates
                  </div>
                  <div class="text-xs aero-muted">
                    Managed user: {device.macosUpdateAccount?.username ?? 'talos'}
                  </div>
                </div>
                <span class={macosUpdateAccountBadgeClass(device.macosUpdateAccount?.status)}>
                  {macosUpdateAccountLabel(device.macosUpdateAccount?.status)}
                </span>
              </div>
              {#if device.macosUpdateAccount?.failureMessage}
                <div class={device.macosUpdateAccount.status === 'needsEnrollment' ? 'aero-alert-warning' : 'aero-alert-error'}>
                  {device.macosUpdateAccount.failureMessage}
                  {#if shouldShowMacosUpdateEnrollmentPrompt(device.macosUpdateAccount.status, device.macosUpdateAccount.failureMessage)}
                    Open Talos Permissions Helper on the Mac and complete Software Updates enrollment.
                  {/if}
                </div>
              {:else if !device.macosUpdateAccount?.status}
                <div class="aero-alert-warning">
                  Mac Software Updates readiness has not been reported yet. Wait for the worker to check in, then open Talos Permissions Helper if enrollment is needed.
                </div>
              {:else if device.macosUpdateAccount?.status === 'needsEnrollment' || device.macosUpdateAccount?.status === 'missing'}
                <div class="aero-alert-warning">
                  Open Talos Permissions Helper on the Mac and complete Software Updates enrollment.
                </div>
              {/if}
            </div>
          {/if}
        </CardContent>
		      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Health Signals</CardTitle>
          <CardDescription>Agent freshness, telemetry, updater, and remediation health.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4 text-sm text-foreground">
          <div class="flex flex-wrap items-center gap-2">
            <span class={healthBadgeClass()}>{healthStatusLabel()}</span>
            <span class="aero-muted">{device.health?.summary ?? 'No health data available'}</span>
          </div>

          <div class="health-signal-grid">
            <div>
              <span class="aero-muted">Websocket</span>
              <strong>{device.health?.signals.websocketStatus ?? device.websocketStatus ?? 'unknown'}</strong>
            </div>
            <div>
              <span class="aero-muted">Telemetry</span>
              <strong>{formatSignalDate(device.health?.signals.telemetryCollectedAt)}</strong>
            </div>
            <div>
              <span class="aero-muted">Target version</span>
              <strong>{device.health?.signals.targetVersion ?? '—'}</strong>
            </div>
            <div>
              <span class="aero-muted">Failures</span>
              <strong>
                {(device.health?.signals.commandFailureCount ?? 0)
                  + (device.health?.signals.updaterFailureCount ?? 0)
                  + (device.health?.signals.remediationFailureCount ?? 0)}
              </strong>
            </div>
          </div>

          {#if device.health?.reasons?.length}
            <div class="health-reason-list">
              {#each device.health.reasons as reason (reason.alertKey)}
                <div class="health-reason-row">
                  <span class={reason.severity === 'critical' ? 'aero-severity-critical health-pill' : 'aero-severity-medium health-pill'}>
                    {reason.severity}
                  </span>
                  <div>
                    <div class="font-medium">{reason.summary}</div>
                    {#if reason.detail}
                      <div class="aero-muted text-xs">{reason.detail}</div>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {:else}
            <div class="aero-empty-state">No active health reasons.</div>
          {/if}

          {#if device.activeHealthAlerts?.length}
            <div class="health-alert-footer">
              {device.activeHealthAlerts.length} active alert{device.activeHealthAlerts.length === 1 ? '' : 's'} de-duplicated for this endpoint.
            </div>
          {/if}
        </CardContent>
      </Card>

	      <Card>
	        <CardHeader>
	          <CardTitle>Remote Tools</CardTitle>
	          <CardDescription>
	            {supportsRemoteRegistry ? 'Remote desktop, shell, file transfer, and registry access.' : supportsRemoteDesktop ? 'Remote desktop, shell, and file transfer access.' : 'Shell and file transfer access.'}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {#if supportsRemoteDesktop}
            <Button
              onclick={connectViewer}
              disabled={remoteDesktopDisabled}
              className="w-full"
            >
              <Monitor class="h-4 w-4" />
              Open Remote Desktop
            </Button>
          {/if}
          <Button variant="outline" onclick={connectShell} disabled={!device} className="w-full">
            <Terminal class="h-4 w-4" />
            Open Interactive Shell
          </Button>
          <Button variant="outline" onclick={connectFileTransfer} disabled={!device} className="w-full">
            <Folder class="h-4 w-4" />
            Open File Transfer
          </Button>
          {#if supportsRemoteRegistry}
            <Button
              variant="outline"
              onclick={connectRegistry}
              disabled={remoteRegistryDisabled}
              className="w-full"
            >
              <Database class="h-4 w-4" />
              Open Remote Registry
            </Button>
          {/if}
          <div class="viewer-presence-panel">
            <div class="viewer-presence-heading">Active viewer connections</div>
            {#if viewerConnections.length === 0}
              <div class="viewer-presence-empty">No active viewer sessions for this device.</div>
            {:else}
              {#each viewerConnections as connection (connection.sessionId)}
                <div class="viewer-presence-row">
                  <span class="viewer-presence-user">{connection.userEmail ?? connection.userId ?? 'Unknown user'}</span>
                  <span class="viewer-presence-kind">{formatViewerSessionKind(connection.kind)}</span>
                </div>
              {/each}
            {/if}
          </div>
	        </CardContent>
	      </Card>

	      <Card>
	        <CardHeader>
	          <CardTitle>AI endpoint approval</CardTitle>
	          <CardDescription>Control whether Talos AI can start endpoint sessions without local approval.</CardDescription>
	        </CardHeader>
	        <CardContent className="space-y-3 text-sm text-foreground">
	          <label class="device-setting-row">
	            <span class="device-setting-copy">
	              <span class="device-setting-title">
	                <Zap class="h-4 w-4" />
	                Auto-approve AI endpoint connections
	              </span>
	              <span class="device-setting-description">
	                Talos AI can connect to this device without endpoint approval. Command approvals still apply.
	              </span>
	            </span>
	            <input
	              type="checkbox"
	              bind:checked={aiRunnerAutoApproveDraft}
	              class="aero-checkbox"
	              aria-label="Auto-approve AI endpoint connections"
	              disabled={deviceSettingsSaving}
	            />
	          </label>
	          {#if deviceSettingsError}
	            <div class="aero-alert-error">{deviceSettingsError}</div>
	          {/if}
	          <div class="flex justify-end">
	            <Button
	              onclick={saveDeviceSettings}
	              disabled={!deviceSettingsChanged || deviceSettingsSaving}
	            >
	              {deviceSettingsSaving ? 'Saving...' : 'Save'}
	            </Button>
	          </div>
	        </CardContent>
	      </Card>

	      {#if !isLimitedInteractiveAgent}
	        <Card>
          <CardHeader>
            <CardTitle>Command Execution</CardTitle>
            <CardDescription>{commandDescription}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <div class="flex gap-2">
              <Input
                type="text"
                placeholder={commandPlaceholder}
                bind:value={commandInput}
                on:keydown={handleKeyDown}
                disabled={commandLoading || !device}
                className="flex-1"
              />
              <Button
                onclick={executeCommand}
                disabled={commandLoading || !device || !commandInput.trim()}
              >
                {#if commandLoading}
                  <div
                    class="animate-spin rounded-full h-4 w-4 border-b-2"
                    style="border-color: rgba(55,130,255,0.8)"
                  ></div>
                {:else}
                  Execute
                {/if}
              </Button>
            </div>

            {#if commandError}
              <div class="aero-alert-error">{commandError}</div>
            {/if}

            {#if commandHistory.length > 0}
              <div class="space-y-2 max-h-96 overflow-y-auto">
                {#each commandHistory as entry (`${entry.timestamp.getTime()}-${entry.command}`)}
                  <div class="aero-terminal-entry">
                    <div class="flex items-center justify-between mb-2">
                      <span class="aero-terminal-prompt">$ {entry.command}</span>
                      <span class="text-xs aero-muted">{entry.timestamp.toLocaleTimeString()}</span>
                    </div>
                    <pre class="aero-terminal">{entry.output}</pre>
                    <div
                      class="mt-1.5 text-xs font-medium"
                      class:text-emerald-400={entry.exitCode === 0}
                      class:text-red-400={entry.exitCode !== 0}
                    >
                      Exit code: {entry.exitCode ?? 'N/A'}
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </CardContent>
        </Card>
      {/if}
    </div>

    <Card>
      <CardHeader className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <CardTitle>Telemetry events</CardTitle>
          <CardDescription>Recent events detected on this device, with severity and source context.</CardDescription>
        </div>
        <div class="flex flex-col gap-2 lg:flex-row lg:items-center">
          <Input
            type="text"
            placeholder="Search events (type, message, service...)"
            bind:value={telemetryEventQuery}
            className="w-full lg:w-80"
          />
          <select
            class="glass-input h-10"
            bind:value={telemetryEventSeverity}
          >
            <option value="all">All severities</option>
            <option value="critical">Critical</option>
            <option value="high">High</option>
            <option value="medium">Medium</option>
            <option value="low">Low</option>
            <option value="info">Info</option>
          </select>
          <Button variant="outline" onclick={() => device && fetchGraphTelemetry(device.agentId)} disabled={!device || graphLoading}>
            <RefreshCw class={`h-4 w-4 ${graphLoading ? 'animate-spin' : ''}`} />
            Refresh
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {#if graphError}
          <div class="aero-alert-warning">{graphError}</div>
        {/if}
        {#if graphLoading && telemetryEvents.length === 0}
          <div class="flex items-center justify-center h-20">
            <div class="animate-spin rounded-full h-7 w-7 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
          </div>
        {:else if filteredTelemetryEvents.length === 0}
          <div class="aero-empty-state">
            No telemetry events match the current filters.
          </div>
        {:else}
          <div class="aero-table-wrap">
            <Table className="text-sm">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[190px]">Time</TableHead>
                  <TableHead className="w-[110px]">Severity</TableHead>
                  <TableHead className="w-[220px]">Type</TableHead>
                  <TableHead>Details</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {#each filteredTelemetryEvents as event (event.eventId)}
                  <TableRow>
                    <TableCell className="whitespace-nowrap align-top text-xs text-muted-foreground">
                      {formatLastSeen(event.occurredAt)}
                    </TableCell>
                    <TableCell className="align-top">
                      <span class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${severityPillClass(event.severity)}`}>
                        {normalizeSeverity(event.severity)}
                      </span>
                    </TableCell>
                    <TableCell className="align-top">
                      <div class="font-medium text-foreground">{event.eventType}</div>
                      <div class="text-xs text-muted-foreground">{event.source}</div>
                    </TableCell>
                    <TableCell className="align-top">
                      <div class="text-sm text-foreground">{formatEventTitle(event)}</div>
                      <div class="mt-1 text-xs text-muted-foreground">
                        {#if event.serviceName}
                          Service: <span class="font-mono">{event.serviceName}</span>
                        {/if}
                        {#if event.processName}
                          {#if event.serviceName} • {/if}
                          Process: <span class="font-mono">{event.processName}</span>
                        {/if}
                        {#if event.code}
                          {(event.serviceName || event.processName) ? ' • ' : ''}Code: {event.code}
                        {/if}
                      </div>
                      {#if event.attributes}
                        <details class="mt-2">
                          <summary class="cursor-pointer text-xs aero-muted hover:text-foreground transition-colors">View raw attributes</summary>
                          <pre class="aero-terminal mt-1">{JSON.stringify(event.attributes, null, 2)}</pre>
                        </details>
                      {/if}
                    </TableCell>
                  </TableRow>
                {/each}
              </TableBody>
            </Table>
          </div>
        {/if}
      </CardContent>
    </Card>

    <Card>
      <CardHeader className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <CardTitle>Device inventory &amp; details</CardTitle>
          <CardDescription>Hardware, software, services, and updates from the latest telemetry snapshot.</CardDescription>
        </div>
        <div class="flex flex-col items-end gap-2">
          <div class="text-sm inventory-meta">
            Last inventory: {lastInventoryLabel}
          </div>
          <div class="flex flex-col items-end gap-1">
            <Button
              variant="outline"
              onclick={requestSnapshot}
              disabled={!device || snapshotRequesting || snapshotPolling || snapshotCooldownSeconds > 0}
            >
              <RefreshCw class={`h-4 w-4 ${(snapshotRequesting || snapshotPolling) ? 'animate-spin' : ''}`} />
              {snapshotRequesting
                ? 'Requesting...'
                : snapshotPolling
                  ? 'Collecting...'
                  : snapshotCooldownSeconds > 0
                    ? `Request snapshot (${snapshotCooldownSeconds} s)`
                    : 'Request snapshot'}
            </Button>
            {#if snapshotRequesting || snapshotPolling}
              <div class="w-56">
                <div class="text-[11px] snapshot-progress-label">
                  {snapshotProgressLabel || 'Waiting for agent/data collection...'}
                </div>
                <div class="snapshot-progress-track mt-1 h-1.5 w-full overflow-hidden rounded-full">
                  <div class="snapshot-indeterminate h-full w-[35%] rounded-full"></div>
                </div>
              </div>
            {/if}
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {#if snapshotError}
          <div class="aero-alert-error">{snapshotError}</div>
        {/if}
        {#if telemetryError && !telemetry}
          <div class="aero-alert-warning">{telemetryError}</div>
        {/if}

        <div class="inventory-tab-bar border-b">
          <nav class="flex flex-wrap gap-1 -mb-px" aria-label="Inventory tabs">
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'overview'}
              onclick={() => (inventoryTab = 'overview')}
            >
              <LayoutDashboard class="h-4 w-4" />
              Overview
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'applications'}
              onclick={() => (inventoryTab = 'applications')}
            >
              <Package class="h-4 w-4" />
              Applications
              {#if telemetry?.installedApps?.length != null}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{telemetry.installedApps.length}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'services'}
              onclick={() => (inventoryTab = 'services')}
            >
              <Server class="h-4 w-4" />
              Services
              {#if telemetry?.services?.length != null}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{telemetry.services.length}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'startup'}
              onclick={() => (inventoryTab = 'startup')}
            >
              <Zap class="h-4 w-4" />
              Startup
              {#if telemetry?.startupItems?.length != null}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{telemetry.startupItems.length}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'features'}
              onclick={() => (inventoryTab = 'features')}
            >
              <Layers class="h-4 w-4" />
              {featureTabLabel}
              {#if telemetry?.windowsFeatures?.length != null}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{telemetry.windowsFeatures.length}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'pending-updates'}
              onclick={() => (inventoryTab = 'pending-updates')}
            >
              <AlertCircle class="h-4 w-4" />
              Pending updates
              {#if patchSummary.pending > 0}
                <span class="aero-badge-amber text-xs rounded-full px-2 py-0.5">{patchSummary.pending}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'downloaded-updates'}
              onclick={() => (inventoryTab = 'downloaded-updates')}
            >
              <Database class="h-4 w-4" />
              Downloaded
              {#if patchSummary.downloaded > 0}
                <span class="aero-badge-amber text-xs rounded-full px-2 py-0.5">{patchSummary.downloaded}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'failed-updates'}
              onclick={() => (inventoryTab = 'failed-updates')}
            >
              <AlertCircle class="h-4 w-4" />
              Failed
              {#if failedPatchSignalCount > 0}
                <span class="aero-badge-offline text-xs rounded-full px-2 py-0.5">{failedPatchSignalCount}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'update-history'}
              onclick={() => (inventoryTab = 'update-history')}
            >
              <History class="h-4 w-4" />
              Update history
              {#if patchSummary.installed > 0 || telemetry?.installedUpdates?.length != null}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{patchSummary.installed || telemetry?.installedUpdates?.length || 0}</span>
              {/if}
            </button>
            <button
              type="button"
              class="inventory-tab px-3 py-2 text-sm font-medium rounded-t-md border-b-2 transition-colors flex items-center gap-2"
              class:inventory-tab-active={inventoryTab === 'audit-trail'}
              onclick={() => (inventoryTab = 'audit-trail')}
            >
              <ScrollText class="h-4 w-4" />
              Audit trail
              {#if auditEntries.length > 0}
                <span class="inventory-tab-badge text-xs rounded-full px-2 py-0.5">{auditEntries.length}{#if auditNextCursor}+{/if}</span>
              {/if}
            </button>
          </nav>
        </div>

        <div class="aero-table-wrap aero-table-wrap-lg p-4">
          {#if telemetryLoading && !telemetry}
          <div class="flex items-center justify-center h-32">
            <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
          </div>
        {:else if inventoryTab === 'overview'}
          {@const state = getDeviceState()}
          {@const inventory = getInventory()}
          {@const cpu = getCpuSummary(inventory)}
          {@const memory = getMemorySummary(inventory)}
          {@const disks = getDisks(inventory)}
          {@const networks = getNetworks(inventory)}
          <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
            <div class="inventory-card p-4">
              <div class="text-sm font-semibold inventory-card-title">System &amp; CPU</div>
              {#if device?.os || state?.osName || state?.cpuModel || cpu}
                <div class="mt-2 text-sm">{device?.os ?? state?.osName ?? 'Unknown'}</div>
                <div class="text-xs inventory-card-muted">{state?.osVersion ?? '—'}</div>
                <div class="mt-2 text-sm">{state?.cpuModel ?? cpu?.brand ?? '—'}</div>
                <div class="mt-1 text-xs inventory-card-muted">
                  {state?.cpuPhysicalCores ?? cpu?.cores ?? '—'} physical / {state?.cpuLogicalCores ?? '—'} logical
                  {#if state?.cpuBaseMhz ?? cpu?.frequency}
                    • {state?.cpuBaseMhz ?? cpu?.frequency} MHz
                  {/if}
                </div>
              {:else}
                <div class="mt-2 text-sm inventory-card-muted">No system data yet. Use "Request snapshot" or wait for telemetry.</div>
              {/if}
            </div>
            <div class="inventory-card p-4">
              <div class="text-sm font-semibold inventory-card-title">Memory</div>
              {#if state?.memoryTotalBytes != null || memory}
                <div class="mt-2 text-sm">
                  {#if state?.memoryTotalBytes != null}
                    {formatBytes(state.memoryTotalBytes)} total
                  {:else if memory}
                    {formatBytes(memory.available)} free of {formatBytes(memory.total)}
                  {:else}
                    —
                  {/if}
                </div>
              {:else}
                <div class="mt-2 text-sm inventory-card-muted">Unavailable</div>
              {/if}
            </div>
          </div>
          <div class="grid grid-cols-2 md:grid-cols-4 gap-3 mt-4">
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">
                {typeof (telemetry?.installedApps?.length ?? state?.installedAppsCount) === 'number'
                  ? (telemetry?.installedApps?.length ?? state?.installedAppsCount)
                  : '—'}
              </div>
              <div class="text-xs inventory-card-muted">Installed apps</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">
                {typeof (patchSummary.pending ?? telemetry?.pendingUpdates?.length ?? state?.pendingUpdatesCount) === 'number'
                  ? (patchSummary.pending ?? telemetry?.pendingUpdates?.length ?? state?.pendingUpdatesCount)
                  : '—'}
              </div>
              <div class="text-xs inventory-card-muted">Pending updates</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold" class:text-amber-400={state?.rebootRequired} class:inventory-card-title={!state?.rebootRequired}>
                {state?.rebootRequired === true ? 'Yes' : state?.rebootRequired === false ? 'No' : '—'}
              </div>
              <div class="text-xs inventory-card-muted">Reboot required</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-xs inventory-card-muted">Collected</div>
              <div class="text-sm font-medium inventory-card-title">{state?.collectedAt ? formatLastSeen(state.collectedAt) : '—'}</div>
            </div>
          </div>
          <div class="mt-3 text-xs inventory-meta">
            Graph facts: {telemetryFactsCount} • Baselines: {telemetryBaselinesCount} • Events: {telemetryEventsCount} • Decisions: {telemetryDecisionsCount}
            {#if latestDecisionAction}
              • Last action: <span class="font-medium">{latestDecisionAction}</span>
            {/if}
            {#if patchStateLoading}
              • Loading patch state...
            {:else if patchStateError}
              • Patch state: {patchStateError}
            {:else if patchState}
              • Patch state: {patchState.updates.length}
            {/if}
            {#if graphLoading}
              • Refreshing...
            {/if}
          </div>
          <div class="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-7 gap-3 mt-4">
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.pending}</div>
              <div class="text-xs inventory-card-muted">Pending</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.downloaded}</div>
              <div class="text-xs inventory-card-muted">Downloaded</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{failedPatchSignalCount}</div>
              <div class="text-xs inventory-card-muted">Failed</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.installed}</div>
              <div class="text-xs inventory-card-muted">Installed</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.blocked}</div>
              <div class="text-xs inventory-card-muted">Blocked</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.deferred}</div>
              <div class="text-xs inventory-card-muted">Deferred</div>
            </div>
            <div class="inventory-card p-3 text-center">
              <div class="text-2xl font-semibold inventory-card-title">{patchSummary.rebootPending}</div>
              <div class="text-xs inventory-card-muted">Reboot pending</div>
            </div>
          </div>
          {#if graphError}
            <div class="mt-2 aero-alert-warning text-xs">{graphError}</div>
          {/if}
          {#if disks.length > 0 || networks.length > 0}
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-4 mt-4">
              {#if disks.length > 0}
                <div>
                  <div class="text-sm font-semibold inventory-card-title">Disks</div>
                  <div class="mt-2 space-y-2">
                    {#each disks as disk (`${disk.name}-${disk.mount}`)}
                      <div class="inventory-card p-3 text-sm">
                        <div class="font-medium inventory-card-title">{disk.name}</div>
                        <div class="text-xs inventory-card-muted">{disk.mount} • {disk.fs}</div>
                        <div class="mt-1 text-xs inventory-card-muted">{formatBytes(disk.available)} free of {formatBytes(disk.total)}</div>
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
              {#if networks.length > 0}
                <div>
                  <div class="text-sm font-semibold inventory-card-title">Networks</div>
                  <div class="mt-2 space-y-2">
                    {#each networks as network (network.name)}
                      <div class="inventory-card p-3 text-xs">
                        <div class="font-medium inventory-card-title">{network.name}</div>
                        <div class="mt-1 inventory-card-muted">
                          {#if network.ips.length > 0}
                            <span class="font-mono">IP: {network.ips.map((ip) => ip.mask).join(', ')}</span>
                          {/if}
                          {#if network.gateways.length > 0}
                            {#if network.ips.length > 0}<br />{/if}
                            <span class="font-mono">Gateway: {network.gateways.join(', ')}</span>
                          {/if}
                          {#if network.dnsServers.length > 0}
                            {#if network.ips.length > 0 || network.gateways.length > 0}<br />{/if}
                            <span class="font-mono">DNS: {network.dnsServers.join(', ')}</span>
                          {/if}
                          {#if network.ips.length === 0 && network.gateways.length === 0 && network.dnsServers.length === 0}
                            <span class="opacity-40">No IP/gateway/DNS data</span>
                          {/if}
                        </div>
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            </div>
          {/if}
        {:else if inventoryTab === 'applications'}
          {#if telemetry?.installedApps?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[240px]">Name</TableHead>
                    <TableHead className="w-[160px]">Publisher</TableHead>
                    <TableHead className="w-[100px]">Version</TableHead>
                    <TableHead className="w-[100px]">Size</TableHead>
                    <TableHead>Source</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.installedApps as app (app.appName + (app.version ?? '') + (app.publisher ?? ''))}
                    <TableRow>
                      <TableCell className="font-medium align-top">{app.appName}</TableCell>
                      <TableCell className="text-muted-foreground align-top">{app.publisher ?? '—'}</TableCell>
                      <TableCell className="align-top">{app.version ?? '—'}</TableCell>
                      <TableCell className="align-top">{formatMaybeBytes(app.sizeBytes)}</TableCell>
                      <TableCell className="text-muted-foreground align-top">{app.source ?? '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No installed applications in telemetry. Request snapshot above or wait for telemetry.</div>
          {/if}
        {:else if inventoryTab === 'services'}
          {#if telemetry?.services?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[180px]">Service name</TableHead>
                    <TableHead className="w-[200px]">Display name</TableHead>
                    <TableHead className="w-[100px]">Status</TableHead>
                    <TableHead className="w-[100px]">Start type</TableHead>
                    <TableHead>Account</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.services as svc (svc.serviceName)}
                    <TableRow>
                      <TableCell className="font-mono text-xs font-medium align-top">{svc.serviceName}</TableCell>
                      <TableCell className="align-top">{svc.displayName}</TableCell>
                      <TableCell className="align-top">
                        {#if svc.status?.toLowerCase() === 'running'}
                          <span class="aero-badge-online">{svc.status}</span>
                        {:else}
                          <span class="aero-badge-neutral">{svc.status}</span>
                        {/if}
                      </TableCell>
                      <TableCell className="text-muted-foreground align-top">{svc.startType ?? '—'}</TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs">{svc.account ?? '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No services in telemetry. Request snapshot above or wait for telemetry.</div>
          {/if}
        {:else if inventoryTab === 'startup'}
          {#if telemetry?.startupItems?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[200px]">Name</TableHead>
                    <TableHead>Command</TableHead>
                    <TableHead className="w-[120px]">Location</TableHead>
                    <TableHead className="w-[100px]">User</TableHead>
                    <TableHead className="w-[80px]">Enabled</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.startupItems as item (item.itemName + item.command + item.location)}
                    <TableRow>
                      <TableCell className="font-medium align-top">{item.itemName}</TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground align-top max-w-md truncate" title={item.command}>{item.command}</TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs">{item.location}</TableCell>
                      <TableCell className="align-top">{item.userName ?? '—'}</TableCell>
                      <TableCell className="align-top">{item.isEnabled === true ? 'Yes' : item.isEnabled === false ? 'No' : '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No startup items in telemetry. Request snapshot above or wait for telemetry.</div>
          {/if}
        {:else if inventoryTab === 'features'}
          {#if telemetry?.windowsFeatures?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[200px]">Feature</TableHead>
                    <TableHead>Display name</TableHead>
                    <TableHead className="w-[100px]">State</TableHead>
                    <TableHead className="w-[80px]">Enabled</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.windowsFeatures as f (f.featureName)}
                    <TableRow>
                      <TableCell className="font-medium align-top">{f.featureName}</TableCell>
                      <TableCell className="text-muted-foreground align-top">{f.displayName}</TableCell>
                      <TableCell className="align-top">{f.installState ?? '—'}</TableCell>
                      <TableCell className="align-top">{f.enabled === true ? 'Yes' : f.enabled === false ? 'No' : '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">{featureEmptyLabel}</div>
          {/if}
        {:else if inventoryTab === 'pending-updates'}
          {#if pendingPatchUpdates.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[280px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[110px]">State</TableHead>
                    <TableHead className="w-[120px]">Category</TableHead>
                    <TableHead className="w-[140px]">Downloaded</TableHead>
                    <TableHead className="w-[80px]">Reboot</TableHead>
                    <TableHead>Policy</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each pendingPatchUpdates as u (u.updateKey)}
                    <TableRow>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="align-top"><span class={patchStateBadgeClass(patchLifecycleState(u))}>{formatPatchState(patchLifecycleState(u))}</span></TableCell>
                      <TableCell className="align-top">{formatPatchCategory(u.category)}</TableCell>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.downloadedAt ? formatLastSeen(u.downloadedAt) : '—'}</TableCell>
                      <TableCell className="align-top">{u.requiresReboot === true ? 'Yes' : u.requiresReboot === false ? 'No' : '—'}</TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs">{formatPatchState(u.approvalState)}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else if !patchState && telemetry?.pendingUpdates?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[280px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[80px]">Size</TableHead>
                    <TableHead className="w-[80px]">Reboot</TableHead>
                    <TableHead>Description</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.pendingUpdates as u (u.title + (u.kbArticle ?? ''))}
                    <TableRow>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="align-top">{formatMaybeBytes(u.sizeBytes)}</TableCell>
                      <TableCell className="align-top">{u.requiresReboot === true ? 'Yes' : u.requiresReboot === false ? 'No' : '—'}</TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs max-w-xs truncate" title={u.description ?? ''}>{u.description ?? '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No pending updates.</div>
          {/if}
        {:else if inventoryTab === 'downloaded-updates'}
          {#if downloadedPatchUpdates.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[280px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[120px]">Category</TableHead>
                    <TableHead className="w-[160px]">Downloaded</TableHead>
                    <TableHead className="w-[160px]">Install deadline</TableHead>
                    <TableHead className="w-[80px]">Reboot</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each downloadedPatchUpdates as u (u.updateKey)}
                    <TableRow>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="align-top">{formatPatchCategory(u.category)}</TableCell>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.downloadedAt ? formatLastSeen(u.downloadedAt) : '—'}</TableCell>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.installDeadlineAt ? formatLastSeen(u.installDeadlineAt) : '—'}</TableCell>
                      <TableCell className="align-top">{u.requiresReboot === true ? 'Yes' : u.requiresReboot === false ? 'No' : '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No downloaded updates waiting to install.</div>
          {/if}
        {:else if inventoryTab === 'failed-updates'}
          {#if patchTransactionFailures.length}
            <div class="aero-table-wrap mb-4">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[150px]">Failed</TableHead>
                    <TableHead className="w-[120px]">Action</TableHead>
                    <TableHead className="w-[120px]">Phase</TableHead>
                    <TableHead className="w-[140px]">Scope</TableHead>
                    <TableHead>Error</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each patchTransactionFailures as failure (failure.id)}
                    <TableRow>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{formatLastSeen(failure.decidedAt)}</TableCell>
                      <TableCell className="align-top">{formatPatchAction(failure.action)}</TableCell>
                      <TableCell className="align-top">{formatPatchState(failure.phase)}</TableCell>
                      <TableCell className="align-top text-muted-foreground">
                        {failure.transactionPackageCount ?? failure.updateKeyCount} package{(failure.transactionPackageCount ?? failure.updateKeyCount) === 1 ? '' : 's'}
                      </TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs">{failure.error ?? failure.reason}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {/if}
          {#if failedPatchUpdates.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[260px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[150px]">Failed</TableHead>
                    <TableHead className="w-[120px]">Code</TableHead>
                    <TableHead>Error</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each failedPatchUpdates as u (u.updateKey)}
                    <TableRow>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.failedAt ? formatLastSeen(u.failedAt) : '—'}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.failureCode ?? u.failureHresult ?? '—'}</TableCell>
                      <TableCell className="text-muted-foreground align-top text-xs">{u.failureMessage ?? '—'}</TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            {#if !patchTransactionFailures.length}
              <div class="aero-empty-state">No failed patch updates.</div>
            {/if}
          {/if}
        {:else if inventoryTab === 'update-history'}
          {#if installedPatchUpdates.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[160px]">Installed</TableHead>
                    <TableHead className="w-[260px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[120px]">Category</TableHead>
                    <TableHead className="w-[100px]">State</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each installedPatchUpdates as u (u.updateKey)}
                    <TableRow>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.installedAt ? formatLastSeen(u.installedAt) : '—'}</TableCell>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="align-top">{formatPatchCategory(u.category)}</TableCell>
                      <TableCell className="align-top"><span class={patchStateBadgeClass(patchLifecycleState(u))}>{formatPatchState(patchLifecycleState(u))}</span></TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else if telemetry?.installedUpdates?.length}
            <div class="aero-table-wrap">
              <Table className="text-sm">
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[160px]">Installed</TableHead>
                    <TableHead className="w-[260px]">Title</TableHead>
                    <TableHead className="w-[100px]">KB</TableHead>
                    <TableHead className="w-[100px]">Operation</TableHead>
                    <TableHead className="w-[100px]">Result</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each telemetry.installedUpdates as u (u.title + (u.installedAt ?? '') + (u.kbArticle ?? ''))}
                    <TableRow>
                      <TableCell className="whitespace-nowrap align-top text-muted-foreground">{u.installedAt ? formatLastSeen(u.installedAt) : '—'}</TableCell>
                      <TableCell className="font-medium align-top">{u.title}</TableCell>
                      <TableCell className="font-mono text-xs align-top">{u.kbArticle ?? '—'}</TableCell>
                      <TableCell className="align-top">{u.operation ?? '—'}</TableCell>
                      <TableCell className="align-top">
                        {#if u.result?.toLowerCase() === 'succeeded'}
                          <span class="aero-badge-online">{u.result}</span>
                        {:else if u.result?.toLowerCase() === 'failed'}
                          <span class="aero-badge-offline">{u.result}</span>
                        {:else}
                          <span class="aero-badge-neutral">{u.result ?? '—'}</span>
                        {/if}
                      </TableCell>
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {:else}
            <div class="aero-empty-state">No update history in telemetry. Request snapshot above or wait for telemetry.</div>
          {/if}
        {:else if inventoryTab === 'audit-trail'}
          <div class="space-y-3">
            <div class="flex flex-col gap-2 lg:flex-row lg:items-center">
              <Input
                type="text"
                placeholder="Search commands (e.g., Get-Service)"
                bind:value={auditQuery}
                on:keydown={handleAuditKeyDown}
                disabled={!device || auditLoading}
                className="flex-1"
              />
              <select
                class="glass-input h-10"
                bind:value={auditAllowed}
                disabled={!device || auditLoading}
              >
                <option value="all">All</option>
                <option value="allowed">Allowed</option>
                <option value="blocked">Blocked</option>
              </select>
              <Button
                variant="outline"
                onclick={() => device && fetchCommandAudit(device.agentId, { reset: true })}
                disabled={!device || auditLoading}
              >
                Apply
              </Button>
              <Button
                variant="outline"
                onclick={() => device && fetchCommandAudit(device.agentId, { reset: true })}
                disabled={!device || auditLoading}
              >
                <RefreshCw class={`h-4 w-4 ${auditLoading ? 'animate-spin' : ''}`} />
                Refresh
              </Button>
            </div>

            {#if auditError}
              <div class="aero-alert-error">{auditError}</div>
            {/if}

            {#if auditLoading && auditEntries.length === 0}
              <div class="flex items-center justify-center h-24">
                <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
              </div>
            {:else if auditEntries.length === 0}
              <div class="aero-empty-state">No audit events recorded yet.</div>
            {:else}
              <div class="aero-table-wrap">
                <Table className="text-sm">
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-[200px]">Time</TableHead>
                      <TableHead className="w-[220px]">User</TableHead>
                      <TableHead>Action</TableHead>
                      <TableHead className="w-[140px]">Result</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {#each auditEntries as entry (entry.id)}
                      {@const cmd = formatAuditCommand(entry)}
                      <TableRow>
                        <TableCell className="whitespace-nowrap align-top text-xs text-muted-foreground">
                          {new Date(entry.createdAt).toLocaleString()}
                        </TableCell>
                        <TableCell className="whitespace-nowrap align-top text-xs text-foreground">
                          {entry.userEmail ?? entry.userId}
                        </TableCell>
                        <TableCell className="align-top">
                          {#if cmd.isViewerAction}
                            <div class="text-sm font-medium text-foreground">{cmd.title}</div>
                          {:else}
                            <code class="aero-terminal-prompt break-all">$ {cmd.title}</code>
                          {/if}

                          {#if entry.matchedPolicyId}
                            <div class="mt-1 text-xs text-muted-foreground">Policy {entry.matchedPolicyId}</div>
                          {/if}
                          {#if !entry.wasAllowed && entry.denialReason}
                            <div class="mt-1 text-xs text-red-400">{entry.denialReason}</div>
                          {/if}
                        </TableCell>
                        <TableCell className="whitespace-nowrap align-top">
                          {#if entry.wasAllowed}
                            <span class="aero-badge-online">Allowed</span>
                          {:else}
                            <span class="aero-badge-offline">Blocked</span>
                          {/if}
                          <div class="mt-1 text-xs text-muted-foreground">
                            Exit {entry.exitCode ?? '—'}
                            {#if entry.executionTimeMs !== null || entry.outputLength !== null}
                              • {formatDurationMs(entry.executionTimeMs)} • {formatMaybeBytes(entry.outputLength)}
                            {/if}
                          </div>
                        </TableCell>
                      </TableRow>
                    {/each}
                  </TableBody>
                </Table>
              </div>
            {/if}

            {#if auditNextCursor}
              <div class="flex justify-center pt-2">
                <Button
                  variant="outline"
                  onclick={() => device && fetchCommandAudit(device.agentId)}
                  disabled={!device || auditLoading}
                >
                  {auditLoading ? 'Loading...' : 'Load more'}
                </Button>
              </div>
            {/if}
          </div>
        {/if}
        </div>
      </CardContent>
    </Card>
  {/if}
</div>

{#if viewerLaunchOverlayOpen}
  <div class="viewer-launch-overlay">
    <div class="viewer-launch-panel">
      {#if viewerLaunchTimedOut}
        <button
          type="button"
          class="viewer-launch-close"
          aria-label="Close viewer launch overlay"
          onclick={() => cancelViewerLaunchWait?.()}
        >
          ×
        </button>
      {/if}
      <div class="viewer-launch-spinner"></div>
      <div class="viewer-launch-title">Opening {viewerLaunchOverlayLabel}...</div>
      <div class="viewer-launch-copy">
        {#if viewerLaunchTimedOut}
          Talos Viewer still has not confirmed the session. Viewer not installed? Download and install it here. This page will keep waiting and will close automatically if the viewer connects.
        {:else}
          Waiting for Talos Viewer to confirm the session.
        {/if}
      </div>
      {#if viewerLaunchTimedOut}
        <div class="viewer-launch-timeout">
          <div class="viewer-launch-timeout-actions">
            <Button variant="ghost" on:click={() => cancelViewerLaunchWait?.()}>Cancel</Button>
            <Button on:click={downloadViewerInstaller} disabled={viewerInstallerDownloading}>
              {viewerInstallerDownloading ? 'Downloading...' : 'Download Viewer'}
            </Button>
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .health-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: capitalize;
  }
  .health-signal-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.75rem;
  }
  .health-signal-grid div {
    display: flex;
    min-height: 3.25rem;
    flex-direction: column;
    justify-content: center;
    gap: 0.2rem;
    border-radius: 0.5rem;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(255,255,255,0.04);
    padding: 0.55rem 0.65rem;
  }
  .health-signal-grid strong {
    overflow-wrap: anywhere;
    font-weight: 600;
  }
  .health-reason-list {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
  }
  .health-reason-row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 0.65rem;
    align-items: flex-start;
    border-top: 1px solid rgba(255,255,255,0.08);
    padding-top: 0.55rem;
  }
  .health-alert-footer {
    border-top: 1px solid rgba(255,255,255,0.08);
    padding-top: 0.65rem;
    color: rgba(200,225,255,0.7);
    font-size: 0.78rem;
  }
  :global(html.light) .health-signal-grid div {
    background: rgba(100,158,220,0.08);
    border-color: rgba(100,158,220,0.18);
  }
  :global(html.light) .health-reason-row,
  :global(html.light) .health-alert-footer {
    border-top-color: rgba(100,158,220,0.18);
  }
	  :global(html.light) .health-alert-footer {
	    color: rgba(10,30,90,0.7);
	  }
	  .device-setting-row {
	    display: flex;
	    align-items: flex-start;
	    justify-content: space-between;
	    gap: 1rem;
	    border-radius: 0.5rem;
	    border: 1px solid rgba(255, 255, 255, 0.08);
	    background: rgba(255, 255, 255, 0.04);
	    padding: 0.75rem;
	  }
	  .device-setting-copy {
	    display: flex;
	    min-width: 0;
	    flex-direction: column;
	    gap: 0.25rem;
	  }
	  .device-setting-title {
	    display: inline-flex;
	    align-items: center;
	    gap: 0.45rem;
	    font-weight: 600;
	  }
	  .device-setting-description {
	    color: rgba(200, 225, 255, 0.68);
	    font-size: 0.78rem;
	    line-height: 1.35;
	  }
	  :global(html.light) .device-setting-row {
	    background: rgba(100,158,220,0.08);
	    border-color: rgba(100,158,220,0.18);
	  }
	  :global(html.light) .device-setting-description {
	    color: rgba(10, 30, 90, 0.68);
	  }
	  .snapshot-progress-label {
	    color: rgba(120, 190, 255, 0.9);
	  }
  :global(html.light) .snapshot-progress-label {
    color: rgba(18, 68, 200, 0.9);
  }
  .snapshot-progress-track {
    background: rgba(255, 255, 255, 0.08);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  :global(html.light) .snapshot-progress-track {
    background: rgba(100, 158, 220, 0.12);
    border-color: rgba(100, 158, 220, 0.2);
  }
  .snapshot-indeterminate {
    background: linear-gradient(90deg, rgba(55, 130, 255, 0.85), rgba(90, 170, 255, 0.95));
    box-shadow: 0 0 10px rgba(55, 130, 255, 0.4);
    will-change: transform;
    animation: snapshot-indeterminate 1.2s ease-in-out infinite alternate;
  }
  :global(html.light) .snapshot-indeterminate {
    background: linear-gradient(90deg, rgba(55, 130, 255, 0.65), rgba(80, 150, 255, 0.85));
    box-shadow: 0 0 8px rgba(55, 130, 255, 0.25);
  }
  @keyframes snapshot-indeterminate {
    0% { transform: translateX(0%); }
    100% { transform: translateX(186%); }
  }
  .viewer-presence-panel {
    margin-top: 0.5rem;
    padding-top: 0.75rem;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }
  .viewer-presence-heading {
    font-size: 0.8rem;
    font-weight: 600;
    margin-bottom: 0.45rem;
  }
  .viewer-presence-empty {
    font-size: 0.8rem;
    color: rgba(200, 225, 255, 0.68);
  }
  .viewer-presence-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    font-size: 0.8rem;
    padding: 0.25rem 0;
  }
  .viewer-presence-user {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .viewer-presence-kind {
    color: rgba(200, 225, 255, 0.68);
    text-transform: capitalize;
  }
  :global(html.light) .viewer-presence-panel {
    border-top-color: rgba(100, 158, 220, 0.18);
  }
  :global(html.light) .viewer-presence-empty,
  :global(html.light) .viewer-presence-kind {
    color: rgba(10, 30, 90, 0.68);
  }
  .viewer-launch-overlay {
    position: fixed;
    inset: 0;
    z-index: 300;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(3, 9, 25, 0.62);
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
  }
  .viewer-launch-panel {
    position: relative;
    min-width: 18rem;
    padding: 1.5rem;
    border-radius: 1rem;
    border: 1px solid rgba(90, 150, 255, 0.24);
    background: rgba(10, 20, 55, 0.92);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.4);
    text-align: center;
  }
  .viewer-launch-spinner {
    width: 3rem;
    height: 3rem;
    margin: 0 auto 0.85rem;
    border-radius: 999px;
    border: 4px solid rgba(255, 255, 255, 0.12);
    border-top-color: rgba(103, 170, 255, 0.95);
    animation: viewer-launch-spin 0.9s linear infinite;
  }
  .viewer-launch-title {
    font-size: 1rem;
    font-weight: 600;
    color: white;
  }
  .viewer-launch-copy {
    margin-top: 0.35rem;
    font-size: 0.84rem;
    color: rgba(210, 230, 255, 0.78);
  }
  .viewer-launch-timeout {
    margin-top: 1rem;
    padding-top: 1rem;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }
  .viewer-launch-timeout-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.9rem;
  }
  .viewer-launch-close {
    position: absolute;
    top: 0.75rem;
    right: 0.75rem;
    width: 2rem;
    height: 2rem;
    border: none;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.86);
    font-size: 1.35rem;
    line-height: 1;
    cursor: pointer;
  }
  .viewer-launch-close:hover {
    background: rgba(255, 255, 255, 0.14);
  }
  :global(html.light) .viewer-launch-panel {
    background: rgba(245, 251, 255, 0.96);
    border-color: rgba(100, 158, 218, 0.28);
  }
  :global(html.light) .viewer-launch-title {
    color: #0a1628;
  }
  :global(html.light) .viewer-launch-copy {
    color: rgba(10, 30, 90, 0.72);
  }
  :global(html.light) .viewer-launch-timeout {
    border-top-color: rgba(100, 158, 220, 0.18);
  }
  :global(html.light) .viewer-launch-close {
    background: rgba(100, 158, 220, 0.12);
    color: rgba(10, 30, 90, 0.78);
  }
  @keyframes viewer-launch-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
