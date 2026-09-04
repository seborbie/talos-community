<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import {
    AlertTriangle,
    CalendarClock,
    CheckCircle2,
    ChevronLeft,
    ChevronRight,
    ClipboardCheck,
    Disc3,
    FileDown,
    HardDriveDownload,
    PlayCircle,
    RefreshCw,
    Search,
    Server,
    ShieldAlert,
    XCircle
  } from 'lucide-svelte';
  import Button from '$lib/ui/Button.svelte';
  import Dialog from '$lib/ui/Dialog.svelte';
  import FeatureUpgradePreflightChecklist from '$lib/components/FeatureUpgradePreflightChecklist.svelte';
  import { featureUpgradeApi, patchApi } from '$lib/api';
  import { topbarConfig } from '$lib/topbar';
  import { toast } from '$lib/toast';
  import type {
    FeatureUpgradeIsoMedia,
    FeatureUpgradeIsoStageDeviceProgress,
    FeatureUpgradeIsoStagePreviewResponse,
    FeatureUpgradePreflightCheckDefinition,
    FeatureUpgradePreflightCheckResult,
    FeatureUpgradePreflightDeviceProgress,
    FeatureUpgradePreflightPreviewDevice,
    FeatureUpgradePreflightPreviewResponse,
    FeatureUpgradeStartDeviceProgress,
    FeatureUpgradeStartPreviewResponse,
    PatchOverviewDevice,
    PatchOverviewResponse
  } from '$lib/types';

  type Readiness = 'eligible' | 'needs_review' | 'blocked' | 'unknown';
  type MediaStatus = 'assigned' | 'missing' | 'queued' | 'downloading' | 'staged' | 'failed' | 'expired' | 'upgrading' | 'upgraded';
  const PREFLIGHT_POLL_MS = 5_000;
  const SCHEDULE_WEEKDAYS = ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'];
  const MONTH_LABEL = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' });

  type UpgradeRow = {
    agentId: string;
    hostname: string;
    os: string;
    currentVersion: string;
    targetVersion: string;
    readiness: Readiness;
    readinessLabel: string;
    blockers: string[];
    warnings: string[];
    mediaStatus: MediaStatus;
    mediaLabel: string;
    phase: string;
    customerName: string | null;
    siteName: string | null;
    deviceType: PatchOverviewDevice['deviceType'];
    patchRing: PatchOverviewDevice['patchRing'];
  };

  let overview: PatchOverviewResponse | null = null;
  let isoMediaItems: FeatureUpgradeIsoMedia[] = [];
  let loading = true;
  let error: string | null = null;
  let search = '';
  let targetFilter = 'all';
  let readinessFilter: Readiness | 'all' = 'all';
  let mediaFilter: MediaStatus | 'all' = 'all';
  let selectedAgentIds = new Set<string>();
  let preflightPreview: FeatureUpgradePreflightPreviewResponse | null = null;
  let preflightDialogOpen = false;
  let preflightPreviewLoading = false;
  let preflightQueueing = false;
  let preflightProgressItems: FeatureUpgradePreflightDeviceProgress[] = [];
  let preflightPollTimer: ReturnType<typeof setInterval> | null = null;
  let preflightPollInFlight = false;
  let detailPreflight: FeatureUpgradePreflightDeviceProgress | null = null;
  let detailDialogOpen = false;
  let stagePreview: FeatureUpgradeIsoStagePreviewResponse | null = null;
  let stageDialogOpen = false;
  let stagePreviewLoading = false;
  let stageQueueing = false;
  let stageProgressItems: FeatureUpgradeIsoStageDeviceProgress[] = [];
  let stagePollInFlight = false;
  let startProgressItems: FeatureUpgradeStartDeviceProgress[] = [];
  let startPollInFlight = false;
  let startQueueing = false;
  let scheduleDialogOpen = false;
  let schedulePreview: FeatureUpgradeStartPreviewResponse | null = null;
  let schedulePreviewLoading = false;
  let scheduleQueueing = false;
  let scheduleLocalValue = '';
  let schedulePickerOpen = false;
  let scheduleCalendarMonth = new Date();

  $: devices = overview?.devices ?? [];
  $: preflightByAgentId = latestPreflightByAgentId(preflightProgressItems);
  $: stageByAgentId = latestStageByAgentId(stageProgressItems);
  $: startByAgentId = latestStartByAgentId(startProgressItems);
  $: rows = buildRows(devices, preflightByAgentId, stageByAgentId, startByAgentId, isoMediaItems);
  $: targets = uniqueSorted(rows.map((row) => row.targetVersion));
  $: visibleRows = rows.filter(matchesFilters);
  $: allVisibleSelected = visibleRows.length > 0 && visibleRows.every((row) => selectedAgentIds.has(row.agentId));
  $: selectedRows = rows.filter((row) => selectedAgentIds.has(row.agentId));
  $: selectedCanStart = selectedRows.length > 0 && selectedRows.every((row) => canStartUpgrade(row));
  $: selectedCanStage = selectedRows.length > 0 && selectedRows.some((row) => canStageIso(row));
  $: preflightDialogChecks = uniqueChecks(preflightPreview?.devices.flatMap((device) => device.checks) ?? []);
  $: stagePreviewReadyDevices = stagePreview?.devices.filter((device) => device.canStage) ?? [];
  $: schedulePreviewReadyDevices = schedulePreview?.devices.filter((device) => device.canStart) ?? [];
  $: totals = {
    windows: rows.length,
    eligible: rows.filter((row) => row.readiness === 'eligible').length,
    review: rows.filter((row) => row.readiness === 'needs_review').length,
    blocked: rows.filter((row) => row.readiness === 'blocked').length,
    mediaMissing: rows.filter((row) => row.mediaStatus === 'missing').length
  };

  $: topbarConfig.set({
    title: 'Feature Upgrade Center',
    action: {
      label: 'Refresh',
      disabled: loading,
      run: () => fetchData()
    }
  });

  function uniqueSorted(values: string[]) {
    return [...new Set(values)].sort((a, b) => a.localeCompare(b));
  }

  function latestPreflightByAgentId(items: FeatureUpgradePreflightDeviceProgress[]) {
    const latest = new Map<string, FeatureUpgradePreflightDeviceProgress>();
    for (const item of items) {
      const current = latest.get(item.agentId);
      const currentTime = current ? Date.parse(current.updatedAt) : 0;
      const itemTime = Date.parse(item.updatedAt);
      if (!current || (!Number.isNaN(itemTime) && itemTime >= currentTime)) {
        latest.set(item.agentId, item);
      }
    }
    return latest;
  }

  function latestStageByAgentId(items: FeatureUpgradeIsoStageDeviceProgress[]) {
    const latest = new Map<string, FeatureUpgradeIsoStageDeviceProgress>();
    for (const item of items) {
      const current = latest.get(item.agentId);
      const currentTime = current ? Date.parse(current.updatedAt) : 0;
      const itemTime = Date.parse(item.updatedAt);
      if (!current || (!Number.isNaN(itemTime) && itemTime >= currentTime)) {
        latest.set(item.agentId, item);
      }
    }
    return latest;
  }

  function latestStartByAgentId(items: FeatureUpgradeStartDeviceProgress[]) {
    const latest = new Map<string, FeatureUpgradeStartDeviceProgress>();
    for (const item of items) {
      const current = latest.get(item.agentId);
      const currentTime = current ? Date.parse(current.updatedAt) : 0;
      const itemTime = Date.parse(item.updatedAt);
      if (!current || (!Number.isNaN(itemTime) && itemTime >= currentTime)) {
        latest.set(item.agentId, item);
      }
    }
    return latest;
  }

  function isPreflightActive(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    return item?.status === 'queued' || item?.status === 'running';
  }

  function isStageActive(item: FeatureUpgradeIsoStageDeviceProgress | undefined) {
    return item?.status === 'queued' || item?.status === 'running';
  }

  function isStartActive(item: FeatureUpgradeStartDeviceProgress | undefined) {
    return item?.status === 'scheduled' || item?.status === 'queued' || item?.status === 'running' || item?.status === 'awaiting_reboot' || item?.status === 'verifying';
  }

  function canProceedAfterPreflight(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    return item?.status === 'passed';
  }

  function preflightStatusLabel(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    if (!item) return 'No preflight run';
    if (item.status === 'queued') return 'Queued';
    if (item.status === 'running') return 'Running preflight';
    if (item.status === 'passed') return 'Preflight passed';
    if (item.status === 'warning') return 'Passed with warnings';
    if (item.status === 'failed') return 'Preflight failed';
    return 'Preflight cancelled';
  }

  function preflightPhaseLabel(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    if (!item) return 'Not started';
    if (item.phase === 'checking') return 'Checking';
    if (item.phase === 'completed') return item.status === 'warning' ? 'Completed with warnings' : 'Completed';
    if (item.phase === 'failed') return 'Failed';
    if (item.phase === 'cancelled') return 'Cancelled';
    return 'Queued';
  }

  function preflightDetailLine(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    if (!item) return 'Run preflight before staging ISO';
    const pending = pendingChecksSummary(item.checks);
    if (pending) return pending;
    if (item.status === 'failed') return item.failureSummary[0]?.message ?? 'One or more required checks failed';
    if (item.status === 'warning') return item.warningSummary[0]?.message ?? 'Warnings require review before install planning';
    if (item.status === 'passed') return 'All required checks passed';
    return 'Waiting for worker result';
  }

  function stageStatusLabel(item: FeatureUpgradeIsoStageDeviceProgress | undefined) {
    if (!item) return 'Not staged';
    if (item.status === 'queued') return 'ISO staging queued';
    if (item.status === 'running') {
      if (item.phase === 'verifying') return 'Verifying ISO';
      if (item.phase === 'downloading') return `Downloading ISO ${Math.round(item.progress?.overallPercent ?? 0)}%`;
      return 'Preparing ISO download';
    }
    if (item.status === 'staged') return item.expiresAt ? `Staged until ${shortDateTime(item.expiresAt)}` : 'ISO staged';
    if (item.status === 'failed') return 'ISO staging failed';
    if (item.status === 'expired') return 'ISO expired and deleted';
    if (item.status === 'deleted') return 'ISO deleted';
    return 'ISO staging cancelled';
  }

  function stageDetailLine(item: FeatureUpgradeIsoStageDeviceProgress | undefined) {
    if (!item) return 'Stage ISO after preflight passes';
    if (item.status === 'failed') return item.errorMessage ?? item.progress?.error ?? 'Download failed';
    if (item.status === 'staged') return formatBytes(item.sizeBytes);
    if (item.status === 'expired' || item.status === 'deleted') return item.cleanedAt ? `Cleaned up ${snapshotAgeLabel(item.cleanedAt)}` : 'Cleaned up by worker';
    const downloaded = item.progress?.bytesDownloaded ?? null;
    const total = item.progress?.bytesTotal ?? item.sizeBytes;
    if (downloaded !== null && total) return `${formatBytes(downloaded)} of ${formatBytes(total)}`;
    return item.phase === 'verifying' ? 'Hash and size verification in progress' : 'Waiting for worker progress';
  }

  function stagePhaseLabel(item: FeatureUpgradeIsoStageDeviceProgress | undefined) {
    if (!item) return 'Not staged';
    if (item.phase === 'requesting_link') return 'Creating download link';
    if (item.phase === 'downloading') return 'Downloading ISO';
    if (item.phase === 'verifying') return 'Verifying ISO';
    if (item.phase === 'staged') return 'Staged';
    if (item.phase === 'deleted') return 'Deleted';
    if (item.phase === 'failed') return 'Failed';
    return item.phase === 'queued' ? 'Queued' : item.phase;
  }

  function startStatusLabel(item: FeatureUpgradeStartDeviceProgress | undefined) {
    if (!item) return 'Not started';
    if (item.status === 'scheduled') return item.scheduledFor ? `Scheduled for ${shortDateTime(item.scheduledFor)}` : 'Scheduled';
    if (item.status === 'queued') return 'Upgrade queued';
    if (item.status === 'running') return 'Running upgrade';
    if (item.status === 'awaiting_reboot') return 'Waiting for reboot';
    if (item.status === 'verifying') return 'Verifying upgrade';
    if (item.status === 'succeeded') return 'Upgrade succeeded';
    if (item.status === 'failed') return 'Upgrade failed';
    return 'Upgrade cancelled';
  }

  function startPhaseLabel(item: FeatureUpgradeStartDeviceProgress | undefined) {
    if (!item) return 'Not started';
    if (item.phase === 'scheduled') return 'Scheduled';
    if (item.phase === 'final_checks') return 'Final checks';
    if (item.phase === 'resolving_iso') return 'Resolving ISO';
    if (item.phase === 'downloading_iso') return 'Downloading ISO';
    if (item.phase === 'verifying_iso') return 'Verifying ISO';
    if (item.phase === 'mounting_iso') return 'Mounting ISO';
    if (item.phase === 'launching_setup') return 'Launching setup';
    if (item.phase === 'setup_running') return 'Setup running';
    if (item.phase === 'awaiting_reboot') return 'Waiting for reboot';
    if (item.phase === 'post_reboot_verifying') return 'Verifying';
    if (item.phase === 'completed') return 'Completed';
    if (item.phase === 'failed') return 'Failed';
    return item.phase === 'queued' ? 'Queued' : item.phase;
  }

  function startDetailLine(item: FeatureUpgradeStartDeviceProgress | undefined) {
    if (!item) return 'Start after preflight passes';
    if (item.status === 'failed') return item.errorMessage ?? item.progress?.error ?? item.failureSummary[0]?.message ?? 'Upgrade failed';
    if (item.status === 'succeeded') return item.verifiedAt ? `Verified ${snapshotAgeLabel(item.verifiedAt)}` : 'Windows target version verified';
    if (item.status === 'scheduled') return item.scheduledFor ? `Starts ${shortDateTime(item.scheduledFor)}` : 'Waiting for scheduled time';
    if (item.status === 'awaiting_reboot') return 'Worker will verify after Windows reboots and reconnects';
    if (item.status === 'verifying') return 'Collecting post-reboot snapshot';
    return item.progress?.error ?? 'Worker is running the upgrade workflow';
  }

  function shortDateTime(value: string) {
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return 'recorded expiry';
    return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(parsed));
  }

  function formatBytes(value: number | null | undefined) {
    if (value === null || value === undefined || !Number.isFinite(value)) return 'Unknown size';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let size = value;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    const digits = unit >= 3 ? 1 : 0;
    return `${size.toFixed(digits)} ${units[unit]}`;
  }

  function uniqueChecks(checks: Array<FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult>) {
    const byId = new Map<string, FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult>();
    for (const check of checks) {
      if (!byId.has(check.id)) byId.set(check.id, check);
    }
    return [...byId.values()];
  }

  function previewCachedReadiness(device: FeatureUpgradePreflightPreviewDevice) {
    if (device.checks.some((check) => check.severity === 'required' && check.status === 'failed')) return 'Blocked by cached evidence';
    const pending = pendingChecksSummary(device.checks);
    if (pending) return pending;
    if (device.checks.some((check) => check.status === 'warning')) return 'Cached warnings';
    return 'Cached checks ready';
  }

  function previewReadinessClass(device: FeatureUpgradePreflightPreviewDevice) {
    const label = previewCachedReadiness(device);
    if (label.includes('Blocked')) return 'blocked';
    if (label.includes('warning') || label.includes('pending')) return 'review';
    return 'eligible';
  }

  function pendingChecksSummary(checks: FeatureUpgradePreflightCheckResult[] | undefined) {
    const pendingNames = uniqueOrdered(
      (checks ?? [])
        .filter((check) => check.status === 'pending')
        .map((check) => pendingCheckName(check))
        .filter(Boolean)
    );
    if (pendingNames.length === 0) return '';
    if (pendingNames.length === 1) return `${pendingNames[0]} check pending`;
    if (pendingNames.length === 2) return `${pendingNames[0]} and ${pendingNames[1]} checks pending`;
    return `${pendingNames[0]}, ${pendingNames[1]} and other checks pending`;
  }

  function pendingCheckName(check: FeatureUpgradePreflightCheckResult) {
    const names: Record<string, string> = {
      os_supported: 'Upgrade path',
      architecture: 'Architecture',
      edition_language: 'Edition and language',
      disk_space: 'Disk space',
      pending_reboot: 'Pending reboot',
      tpm_2_0: 'TPM',
      secure_boot: 'Secure Boot',
      cpu_basic: 'CPU',
      memory: 'Memory',
      system_disk_size: 'System disk size',
      bitlocker: 'BitLocker',
      domain_controller: 'Domain controller'
    };
    return names[check.id] ?? check.label;
  }

  function uniqueOrdered(values: string[]) {
    const seen = new Set<string>();
    return values.filter((value) => {
      if (seen.has(value)) return false;
      seen.add(value);
      return true;
    });
  }

  function serverRoleSummary(device: PatchOverviewDevice) {
    const inventory = device.serverRoleInventory;
    if (!inventory?.evidencePresent) return 'Server role inventory missing from latest snapshot';
    if (inventory.roles.length === 0) return '';

    const roleText = inventory.roles.length === 1
      ? `${inventory.roles[0]} detected`
      : `${inventory.roles.slice(0, -1).join(', ')} and ${inventory.roles[inventory.roles.length - 1]} detected`;
    const detailParts: string[] = [];
    if (inventory.details.domainName) detailParts.push(inventory.details.domainName);
    if (inventory.details.dnsZones) detailParts.push(`${inventory.details.dnsZones} DNS zone${inventory.details.dnsZones === 1 ? '' : 's'}`);
    if (inventory.details.dhcpScopes) detailParts.push(`${inventory.details.dhcpScopes} DHCP scope${inventory.details.dhcpScopes === 1 ? '' : 's'}`);
    if (inventory.details.iisSites) detailParts.push(`${inventory.details.iisSites} IIS site${inventory.details.iisSites === 1 ? '' : 's'}`);
    return detailParts.length > 0 ? `${roleText}: ${detailParts.join(', ')}` : roleText;
  }

  function snapshotAgeLabel(value?: string | null) {
    if (!value) return 'No snapshot yet';
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return 'Unknown';
    const minutes = Math.max(0, Math.round((Date.now() - parsed) / 60000));
    if (minutes < 2) return 'Just now';
    if (minutes < 60) return `${minutes} min ago`;
    const hours = Math.round(minutes / 60);
    if (hours < 48) return `${hours} hr ago`;
    return `${Math.round(hours / 24)} days ago`;
  }

  function previewCurrentOs(device: FeatureUpgradePreflightPreviewDevice) {
    return inferWindowsVersion(device.os, device.osVersion);
  }

  function inferWindowsVersion(os: string, osVersion?: string | null) {
    const release = osVersion?.match(/\b(2[0-9]H[12])\b/i)?.[1].toUpperCase();
    if (release) {
      if (/Windows 10/i.test(os) && release > '22H2') return `Windows 11 ${release}`;
      if (/Windows 11/i.test(os)) return `Windows 11 ${release}`;
      if (/Windows 10/i.test(os)) return `Windows 10 ${release}`;
    }
    const buildMatch = os.match(/\b(2[0-9]H[12])\b/i);
    if (buildMatch) {
      const releaseFromOs = buildMatch[1].toUpperCase();
      if (/Windows 11/i.test(os)) return `Windows 11 ${releaseFromOs}`;
      if (/Windows 10/i.test(os)) return `Windows 10 ${releaseFromOs}`;
      return releaseFromOs;
    }
    const serverMatch = os.match(/Windows Server\s+(\d{4})/i);
    if (serverMatch) return `Server ${serverMatch[1]}`;
    if (/Windows 11/i.test(os)) return 'Windows 11';
    if (/Windows 10/i.test(os)) return 'Windows 10';
    return 'Windows';
  }

  function canonicalSourceOs(device: PatchOverviewDevice) {
    const preflight = preflightByAgentId.get(device.agentId);
    const stage = stageByAgentId.get(device.agentId);
    const start = startByAgentId.get(device.agentId);
    return preflight?.sourceOs || start?.sourceOs || stage?.sourceOs || device.os || 'Windows';
  }

  function primaryStartStatus(item: FeatureUpgradeStartDeviceProgress | undefined) {
    return item && item.status !== 'cancelled' ? item : undefined;
  }

  function primaryStageStatus(item: FeatureUpgradeIsoStageDeviceProgress | undefined) {
    return item && item.status !== 'cancelled' ? item : undefined;
  }

  function normalizedLanguage(value: string | null | undefined) {
    return value?.toLowerCase().replace(/_/g, '-') ?? null;
  }

  function preflightLanguage(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    const details = item?.checks.find((check) => check.id === 'edition_language')?.details ?? null;
    const raw = details?.locale ?? details?.language;
    return typeof raw === 'string' ? normalizedLanguage(raw) : null;
  }

  function matchingIsoMedia(targetVersion: string, deviceLanguage: string | null) {
    const isServer = targetVersion.includes('Server');
    const matches = isoMediaItems.filter((media) => {
      const productOk = isServer ? media.product.toLowerCase().includes('server') : media.product.toLowerCase().includes('windows 11');
      const versionOk = media.version.toLowerCase() === (isServer ? '2025' : '25h2');
      const archOk = /x64|amd64|64/i.test(media.architecture);
      return media.active && media.osFamily === 'windows' && productOk && versionOk && archOk;
    });
    return matches.sort((left, right) => {
      const leftExact = normalizedLanguage(left.language) === deviceLanguage ? 1 : 0;
      const rightExact = normalizedLanguage(right.language) === deviceLanguage ? 1 : 0;
      return rightExact - leftExact || left.displayName.localeCompare(right.displayName);
    })[0] ?? null;
  }

  function canStageIso(row: UpgradeRow) {
    const stage = stageByAgentId.get(row.agentId);
    return canProceedAfterPreflight(preflightByAgentId.get(row.agentId)) &&
      row.mediaStatus !== 'missing' &&
      !isStageActive(stage) &&
      !(stage?.status === 'staged');
  }

  function canStartUpgrade(row: UpgradeRow) {
    const start = startByAgentId.get(row.agentId);
    return canProceedAfterPreflight(preflightByAgentId.get(row.agentId)) &&
      row.mediaStatus !== 'missing' &&
      !isStartActive(start);
  }

  function buildRows(
    sourceDevices: PatchOverviewDevice[],
    _preflightByAgentId: Map<string, FeatureUpgradePreflightDeviceProgress>,
    _stageByAgentId: Map<string, FeatureUpgradeIsoStageDeviceProgress>,
    _startByAgentId: Map<string, FeatureUpgradeStartDeviceProgress>,
    _isoMediaItems: FeatureUpgradeIsoMedia[]
  ) {
    return sourceDevices.filter((device) => /\bwindows\b/i.test(device.os)).map(buildRow);
  }

  function buildRow(device: PatchOverviewDevice): UpgradeRow {
    const preflight = preflightByAgentId.get(device.agentId);
    const stage = primaryStageStatus(stageByAgentId.get(device.agentId));
    const start = startByAgentId.get(device.agentId);
    const primaryStart = primaryStartStatus(start);
    const os = canonicalSourceOs(device);
    const lower = os.toLowerCase();
    const isServer = lower.includes('server');
    const blockers: string[] = [];
    const warnings: string[] = [];
    const pendingSummary = pendingChecksSummary(preflight?.checks);

    if (device.rebootRequired || device.rebootPendingUpdates > 0) blockers.push('Pending reboot');
    if (pendingSummary) warnings.push(pendingSummary);
    if (isServer) {
      const roles = serverRoleSummary(device);
      if (roles) warnings.push(roles);
    }

    const readiness: Readiness = blockers.length > 0 ? 'blocked' : warnings.length > 0 ? 'needs_review' : lower.includes('windows') ? 'eligible' : 'unknown';
    const targetVersion = preflight?.targetBuildLabel || primaryStart?.targetBuildLabel || stage?.targetBuildLabel || (isServer ? 'Windows Server 2025' : 'Windows 11 25H2');
    const media = matchingIsoMedia(targetVersion, preflightLanguage(preflight));
    const mediaStatus: MediaStatus =
      primaryStart?.status === 'succeeded' ? 'upgraded'
      : primaryStart?.status === 'failed' ? 'failed'
      : isStartActive(primaryStart) ? 'upgrading'
      :
      stage?.status === 'staged' ? 'staged'
      : stage?.status === 'queued' ? 'queued'
      : stage?.status === 'running' ? 'downloading'
      : stage?.status === 'failed' ? 'failed'
      : stage?.status === 'expired' || stage?.status === 'deleted' ? 'expired'
      : media ? 'assigned'
      : 'missing';
    const mediaLabel =
      primaryStart ? startStatusLabel(primaryStart)
      :
      stage ? stageStatusLabel(stage)
      : media ? `${media.displayName} (${formatBytes(media.sizeBytes)})`
      : targetVersion.includes('Server') ? 'Server 2025 ISO not assigned' : 'Windows 11 25H2 ISO not assigned';

    return {
      agentId: device.agentId,
      hostname: device.hostname,
      os,
      currentVersion: inferWindowsVersion(os, device.osVersion),
      targetVersion,
      readiness,
      readinessLabel: readiness === 'needs_review' ? 'Needs review' : readiness === 'eligible' ? 'Ready for preflight' : readiness === 'blocked' ? 'Blocked' : 'Preflight required',
      blockers,
      warnings,
      mediaStatus,
      mediaLabel,
      phase: primaryStart ? startPhaseLabel(primaryStart) : stage ? stagePhaseLabel(stage) : 'Not started',
      customerName: device.customerName,
      siteName: device.siteName,
      deviceType: device.deviceType,
      patchRing: device.patchRing
    };
  }

  function matchesFilters(row: UpgradeRow) {
    const query = search.trim().toLowerCase();
    if (targetFilter !== 'all' && row.targetVersion !== targetFilter) return false;
    if (readinessFilter !== 'all' && row.readiness !== readinessFilter) return false;
    if (mediaFilter !== 'all' && row.mediaStatus !== mediaFilter) return false;
    if (!query) return true;
    return [
      row.hostname,
      row.agentId,
      row.os,
      row.currentVersion,
      row.targetVersion,
      row.readinessLabel,
      row.mediaLabel,
      row.phase,
      row.customerName,
      row.siteName,
      row.deviceType,
      row.patchRing,
      ...row.blockers,
      ...row.warnings
    ].filter(Boolean).some((value) => String(value).toLowerCase().includes(query));
  }

  function toggleRow(agentId: string) {
    const next = new Set(selectedAgentIds);
    next.has(agentId) ? next.delete(agentId) : next.add(agentId);
    selectedAgentIds = next;
  }

  function toggleAllVisible() {
    const next = new Set(selectedAgentIds);
    if (allVisibleSelected) visibleRows.forEach((row) => next.delete(row.agentId));
    else visibleRows.forEach((row) => next.add(row.agentId));
    selectedAgentIds = next;
  }

  async function openPreflightDialog(agentId?: string) {
    const agentIds = agentId ? [agentId] : [...selectedAgentIds];
    if (agentIds.length === 0) {
      toast({ title: 'Select at least one Windows device', variant: 'destructive' });
      return;
    }

    try {
      preflightPreviewLoading = true;
      preflightPreview = await featureUpgradeApi.previewPreflight(agentIds);
      preflightDialogOpen = true;
    } catch (err) {
      toast({
        title: 'Unable to prepare preflight',
        description: err instanceof Error ? err.message : 'Preflight preview failed',
        variant: 'destructive'
      });
    } finally {
      preflightPreviewLoading = false;
    }
  }

  async function queuePreflightRun() {
    const agentIds = preflightPreview?.devices.map((device) => device.agentId) ?? [];
    if (agentIds.length === 0) return;
    try {
      preflightQueueing = true;
      const response = await featureUpgradeApi.runPreflight(agentIds);
      mergePreflightProgress(response.devices);
      preflightDialogOpen = false;
      toast({
        title: 'Preflight queued',
        description: `${response.targetedDevices} device(s) will refresh snapshot-backed readiness checks.`
      });
      const pollAgentIds =
        overview?.devices.filter((device) => /\bwindows\b/i.test(device.os)).map((device) => device.agentId) ?? agentIds;
      await pollPreflightProgress(pollAgentIds);
    } catch (err) {
      toast({
        title: 'Unable to queue preflight',
        description: err instanceof Error ? err.message : 'Preflight queue request failed',
        variant: 'destructive'
      });
    } finally {
      preflightQueueing = false;
    }
  }

  function mergePreflightProgress(items: FeatureUpgradePreflightDeviceProgress[]) {
    const next = new Map(preflightProgressItems.map((item) => [item.operationId, item]));
    for (const item of items) next.set(item.operationId, item);
    preflightProgressItems = [...next.values()];
  }

  async function pollPreflightProgress(agentIds = rows.map((row) => row.agentId)) {
    if (preflightPollInFlight || agentIds.length === 0 || (typeof document !== 'undefined' && document.hidden)) return;
    preflightPollInFlight = true;
    try {
      const response = await featureUpgradeApi.queryPreflightProgress(agentIds);
      preflightProgressItems = response.items ?? [];
    } catch (err) {
      console.warn('Failed to check feature upgrade preflight progress:', err);
    } finally {
      preflightPollInFlight = false;
    }
  }

  function openPreflightDetails(item: FeatureUpgradePreflightDeviceProgress | undefined) {
    if (!item) return;
    detailPreflight = item;
    detailDialogOpen = true;
  }

  async function openStageIsoDialog(agentId?: string) {
    const agentIds = agentId ? [agentId] : [...selectedAgentIds];
    if (agentIds.length === 0) {
      toast({ title: 'Select at least one Windows device', variant: 'destructive' });
      return;
    }

    try {
      stagePreviewLoading = true;
      stagePreview = await featureUpgradeApi.previewStageIso(agentIds);
      stageDialogOpen = true;
    } catch (err) {
      toast({
        title: 'Unable to prepare ISO staging',
        description: err instanceof Error ? err.message : 'ISO staging preview failed',
        variant: 'destructive'
      });
    } finally {
      stagePreviewLoading = false;
    }
  }

  async function queueStageIsoRun() {
    const agentIds = stagePreviewReadyDevices.map((device) => device.agentId);
    if (agentIds.length === 0) return;
    try {
      stageQueueing = true;
      const response = await featureUpgradeApi.runStageIso(agentIds);
      mergeStageProgress(response.devices);
      stageDialogOpen = false;
      toast({
        title: 'ISO staging queued',
        description: `${response.targetedDevices} device(s) will download ISO media for 7 days.`
      });
      await pollStageIsoProgress();
    } catch (err) {
      toast({
        title: 'Unable to queue ISO staging',
        description: err instanceof Error ? err.message : 'ISO staging queue request failed',
        variant: 'destructive'
      });
    } finally {
      stageQueueing = false;
    }
  }

  function mergeStageProgress(items: FeatureUpgradeIsoStageDeviceProgress[]) {
    const next = new Map(stageProgressItems.map((item) => [item.operationId, item]));
    for (const item of items) next.set(item.operationId, item);
    stageProgressItems = [...next.values()];
  }

  async function pollStageIsoProgress(agentIds = rows.map((row) => row.agentId)) {
    if (stagePollInFlight || agentIds.length === 0 || (typeof document !== 'undefined' && document.hidden)) return;
    stagePollInFlight = true;
    try {
      const response = await featureUpgradeApi.queryStageIsoProgress(agentIds);
      stageProgressItems = response.items ?? [];
    } catch (err) {
      console.warn('Failed to check feature upgrade ISO staging progress:', err);
    } finally {
      stagePollInFlight = false;
    }
  }

  function mergeStartProgress(items: FeatureUpgradeStartDeviceProgress[]) {
    const next = new Map(startProgressItems.map((item) => [item.operationId, item]));
    for (const item of items) next.set(item.operationId, item);
    startProgressItems = [...next.values()];
  }

  async function pollStartProgress(agentIds = rows.map((row) => row.agentId)) {
    if (startPollInFlight || agentIds.length === 0 || (typeof document !== 'undefined' && document.hidden)) return;
    startPollInFlight = true;
    try {
      const response = await featureUpgradeApi.queryStartProgress(agentIds);
      startProgressItems = response.items ?? [];
    } catch (err) {
      console.warn('Failed to check feature upgrade start progress:', err);
    } finally {
      startPollInFlight = false;
    }
  }

  async function queueStartRun(agentId?: string, scheduledFor: string | null = null) {
    const agentIds = agentId ? [agentId] : [...selectedAgentIds];
    if (agentIds.length === 0) {
      toast({ title: 'Select at least one Windows device', variant: 'destructive' });
      return;
    }
    try {
      startQueueing = !scheduledFor;
      scheduleQueueing = Boolean(scheduledFor);
      const response = await featureUpgradeApi.runStart(agentIds, scheduledFor);
      mergeStartProgress(response.devices);
      scheduleDialogOpen = false;
      toast({
        title: scheduledFor ? 'Feature upgrade scheduled' : 'Feature upgrade queued',
        description: scheduledFor
          ? `${response.targetedDevices} device(s) will start ${shortDateTime(scheduledFor)}.`
          : `${response.targetedDevices} device(s) will start upgrade now.`
      });
      await pollStartProgress();
    } catch (err) {
      toast({
        title: scheduledFor ? 'Unable to schedule upgrade' : 'Unable to start upgrade',
        description: err instanceof Error ? err.message : 'Feature upgrade request failed',
        variant: 'destructive'
      });
    } finally {
      startQueueing = false;
      scheduleQueueing = false;
    }
  }

  async function openScheduleDialog(agentId?: string) {
    const agentIds = agentId ? [agentId] : [...selectedAgentIds];
    if (agentIds.length === 0) {
      toast({ title: 'Select at least one Windows device', variant: 'destructive' });
      return;
    }

    const soon = new Date(Date.now() + 60 * 60 * 1000);
    soon.setMinutes(Math.ceil(soon.getMinutes() / 5) * 5, 0, 0);
    scheduleLocalValue = toDatetimeLocalValue(soon);
    scheduleCalendarMonth = startOfMonth(soon);
    schedulePickerOpen = false;
    try {
      schedulePreviewLoading = true;
      schedulePreview = await featureUpgradeApi.previewStart(agentIds);
      scheduleDialogOpen = true;
    } catch (err) {
      toast({
        title: 'Unable to prepare schedule',
        description: err instanceof Error ? err.message : 'Schedule preview failed',
        variant: 'destructive'
      });
    } finally {
      schedulePreviewLoading = false;
    }
  }

  function queueScheduledRun() {
    const parsed = new Date(scheduleLocalValue);
    if (!scheduleLocalValue || Number.isNaN(parsed.getTime()) || parsed.getTime() <= Date.now()) {
      toast({ title: 'Choose a future date and time', variant: 'destructive' });
      return;
    }
    const agentIds = schedulePreviewReadyDevices.map((device) => device.agentId);
    void queueStartRunForAgentIds(agentIds, parsed.toISOString());
  }

  function parseDatetimeLocalValue(value: string) {
    const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value);
    if (!match) return null;
    const year = Number(match[1]);
    const month = Number(match[2]);
    const day = Number(match[3]);
    const hour = Number(match[4]);
    const minute = Number(match[5]);
    const parsed = new Date(year, month - 1, day, hour, minute, 0, 0);
    return Number.isNaN(parsed.getTime()) ? null : parsed;
  }

  function startOfMonth(date: Date) {
    return new Date(date.getFullYear(), date.getMonth(), 1);
  }

  function sameDay(left: Date, right: Date) {
    return left.getFullYear() === right.getFullYear() && left.getMonth() === right.getMonth() && left.getDate() === right.getDate();
  }

  function scheduleCalendarDays(monthDate: Date) {
    const firstOfMonth = startOfMonth(monthDate);
    const mondayOffset = (firstOfMonth.getDay() + 6) % 7;
    const firstGridDay = new Date(firstOfMonth);
    firstGridDay.setDate(firstOfMonth.getDate() - mondayOffset);
    const selected = parseDatetimeLocalValue(scheduleLocalValue);
    const today = new Date();
    const todayStart = new Date(today.getFullYear(), today.getMonth(), today.getDate());

    return Array.from({ length: 42 }, (_, index) => {
      const date = new Date(firstGridDay);
      date.setDate(firstGridDay.getDate() + index);
      return {
        date,
        day: date.getDate(),
        inMonth: date.getMonth() === monthDate.getMonth(),
        selected: selected ? sameDay(date, selected) : false,
        today: sameDay(date, today),
        disabled: date < todayStart
      };
    });
  }

  function changeScheduleMonth(delta: number) {
    scheduleCalendarMonth = new Date(scheduleCalendarMonth.getFullYear(), scheduleCalendarMonth.getMonth() + delta, 1);
  }

  function selectScheduleDate(date: Date) {
    if (date < new Date(new Date().getFullYear(), new Date().getMonth(), new Date().getDate())) return;
    const current = parseDatetimeLocalValue(scheduleLocalValue) ?? new Date(Date.now() + 60 * 60 * 1000);
    scheduleLocalValue = toDatetimeLocalValue(new Date(date.getFullYear(), date.getMonth(), date.getDate(), current.getHours(), current.getMinutes(), 0, 0));
    scheduleCalendarMonth = startOfMonth(date);
  }

  function setScheduleTime(part: 'hour' | 'minute', value: string) {
    const current = parseDatetimeLocalValue(scheduleLocalValue) ?? new Date(Date.now() + 60 * 60 * 1000);
    const next = new Date(current);
    if (part === 'hour') next.setHours(Number(value));
    if (part === 'minute') next.setMinutes(Number(value));
    next.setSeconds(0, 0);
    scheduleLocalValue = toDatetimeLocalValue(next);
  }

  function scheduleHour() {
    return String(parseDatetimeLocalValue(scheduleLocalValue)?.getHours() ?? 0).padStart(2, '0');
  }

  function scheduleMinute() {
    return String(parseDatetimeLocalValue(scheduleLocalValue)?.getMinutes() ?? 0).padStart(2, '0');
  }

  function scheduleDisplayValue() {
    const parsed = parseDatetimeLocalValue(scheduleLocalValue);
    if (!parsed) return 'Select start time';
    return new Intl.DateTimeFormat(undefined, {
      weekday: 'short',
      day: '2-digit',
      month: 'short',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    }).format(parsed);
  }

  async function queueStartRunForAgentIds(agentIds: string[], scheduledFor: string | null) {
    if (agentIds.length === 0) return;
    try {
      startQueueing = !scheduledFor;
      scheduleQueueing = Boolean(scheduledFor);
      const response = await featureUpgradeApi.runStart(agentIds, scheduledFor);
      mergeStartProgress(response.devices);
      scheduleDialogOpen = false;
      toast({
        title: scheduledFor ? 'Feature upgrade scheduled' : 'Feature upgrade queued',
        description: scheduledFor
          ? `${response.targetedDevices} device(s) will start ${shortDateTime(scheduledFor)}.`
          : `${response.targetedDevices} device(s) will start upgrade now.`
      });
      await pollStartProgress();
    } catch (err) {
      toast({
        title: scheduledFor ? 'Unable to schedule upgrade' : 'Unable to start upgrade',
        description: err instanceof Error ? err.message : 'Feature upgrade request failed',
        variant: 'destructive'
      });
    } finally {
      startQueueing = false;
      scheduleQueueing = false;
    }
  }

  function toDatetimeLocalValue(date: Date) {
    const pad = (value: number) => String(value).padStart(2, '0');
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  }

  function exportCsv() {
    const keys = Object.keys(visibleRows[0] ?? {});
    if (keys.length === 0) return;
    const escape = (value: unknown) => `"${String(value ?? '').replace(/"/g, '""')}"`;
    const csv = [keys.join(','), ...visibleRows.map((row) => keys.map((key) => escape((row as Record<string, unknown>)[key])).join(','))].join('\n');
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = 'feature-upgrades.csv';
    link.click();
    URL.revokeObjectURL(url);
  }

  async function fetchData() {
    try {
      loading = true;
      error = null;
      const [overviewResponse, mediaResponse] = await Promise.all([
        patchApi.getOverview(),
        featureUpgradeApi.listIsoMedia()
      ]);
      overview = overviewResponse;
      isoMediaItems = mediaResponse.items ?? [];
      const windowsAgentIds = overviewResponse.devices.filter((device) => /\bwindows\b/i.test(device.os)).map((device) => device.agentId);
      selectedAgentIds = new Set([...selectedAgentIds].filter((agentId) => windowsAgentIds.includes(agentId)));
      await Promise.all([pollPreflightProgress(windowsAgentIds), pollStageIsoProgress(windowsAgentIds), pollStartProgress(windowsAgentIds)]);

    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load feature upgrade data';
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void fetchData();
    preflightPollTimer = setInterval(() => {
      void pollPreflightProgress();
      void pollStageIsoProgress();
      void pollStartProgress();
    }, PREFLIGHT_POLL_MS);
    return () => {
      if (preflightPollTimer) clearInterval(preflightPollTimer);
    };
  });
  onDestroy(() => topbarConfig.set(null));
</script>

<svelte:head>
  <title>Feature Upgrade Center | Talos</title>
</svelte:head>

<div class="upgrade-page">
  {#if error}<div class="error-band">{error}</div>{/if}

  <section class="summary" aria-label="Feature upgrade totals">
    <div><Server size={18} /><strong>{totals.windows}</strong><span>Windows devices</span></div>
    <div><CheckCircle2 size={18} /><strong>{totals.eligible}</strong><span>Eligible devices</span></div>
    <div><ClipboardCheck size={18} /><strong>{totals.review}</strong><span>Need preflight review</span></div>
    <div><ShieldAlert size={18} /><strong>{totals.blocked}</strong><span>Blocked</span></div>
    <div><Disc3 size={18} /><strong>{totals.mediaMissing}</strong><span>Missing ISO assignment</span></div>
  </section>

  {#if loading}
    <div class="empty-state">Loading feature upgrade data...</div>
  {:else}
    <section class="main">
      <section class="toolbar">
        <label class="search"><Search size={16} /> <input bind:value={search} placeholder="Search device, OS, target, customer, blocker" /></label>
        <label>Target <select bind:value={targetFilter}><option value="all">All targets</option>{#each targets as target}<option value={target}>{target}</option>{/each}</select></label>
        <label>Readiness <select bind:value={readinessFilter}><option value="all">All readiness</option><option value="eligible">Eligible</option><option value="needs_review">Needs review</option><option value="blocked">Blocked</option><option value="unknown">Unknown</option></select></label>
        <label>Media <select bind:value={mediaFilter}><option value="all">All media</option><option value="assigned">Assigned</option><option value="queued">Queued</option><option value="downloading">Downloading</option><option value="staged">Staged</option><option value="upgrading">Upgrading</option><option value="upgraded">Upgraded</option><option value="failed">Failed</option><option value="expired">Expired</option><option value="missing">Missing</option></select></label>
      </section>
      <section class="actions-row">
        <span>{selectedAgentIds.size} selected</span>
        <Button variant="secondary" size="sm" disabled={preflightPreviewLoading} on:click={() => openPreflightDialog()}><ClipboardCheck size={15} /> Run preflight</Button>
        <Button variant="secondary" size="sm" disabled={!selectedCanStage || stagePreviewLoading} on:click={() => openStageIsoDialog()}><HardDriveDownload size={15} /> Stage ISO</Button>
        <Button size="sm" disabled={!selectedCanStart || startQueueing} on:click={() => queueStartRun()}><PlayCircle size={15} /> {startQueueing ? 'Queueing...' : 'Start upgrade'}</Button>
        <Button variant="secondary" size="sm" disabled={!selectedCanStart || schedulePreviewLoading} on:click={() => openScheduleDialog()}><CalendarClock size={15} /> Schedule</Button>
        <Button variant="secondary" size="sm" on:click={exportCsv}><FileDown size={15} /> Export CSV</Button>
        <span>{visibleRows.length} of {rows.length} Windows devices</span>
      </section>

      <div class="table-wrap">
        <table>
          <thead><tr><th><input type="checkbox" checked={allVisibleSelected} on:change={toggleAllVisible} aria-label="Select visible devices" /></th><th>Device</th><th>Upgrade path</th><th>Readiness</th><th>ISO media</th><th>Phase</th><th>Actions</th></tr></thead>
          <tbody>
            {#each visibleRows as row (row.agentId)}
              {@const preflight = preflightByAgentId.get(row.agentId)}
              {@const stage = stageByAgentId.get(row.agentId)}
              {@const start = startByAgentId.get(row.agentId)}
              {@const displayStart = primaryStartStatus(start)}
              <tr>
                <td><input type="checkbox" checked={selectedAgentIds.has(row.agentId)} on:change={() => toggleRow(row.agentId)} aria-label={`Select ${row.hostname}`} /></td>
                <td>
                  <div class="device-name-row">
                    <button class="device-link" type="button" on:click={() => goto(`/dashboard/rmm/${row.agentId}`)}><strong>{row.hostname}</strong></button>
                    {#if isStartActive(displayStart)}
                      <span class="preflight-sync-icon" title="Feature upgrade in progress"><RefreshCw size={14} /></span>
                    {:else if isPreflightActive(preflight)}
                      <span class="preflight-sync-icon" title="Feature upgrade preflight in progress"><RefreshCw size={14} /></span>
                    {:else if isStageActive(stage)}
                      <span class="preflight-sync-icon" title="ISO staging in progress"><RefreshCw size={14} /></span>
                    {:else if preflight?.status === 'passed'}
                      <span class="preflight-passed-icon" title="Preflight passed"><CheckCircle2 size={14} /></span>
                    {:else if preflight?.status === 'warning'}
                      <button class="inline-status warning-icon" type="button" title="Preflight passed with warnings" on:click={() => openPreflightDetails(preflight)}><AlertTriangle size={14} /></button>
                    {:else if preflight?.status === 'failed'}
                      <button class="inline-status failed-icon" type="button" title="Preflight failed" on:click={() => openPreflightDetails(preflight)}><XCircle size={14} /></button>
                    {/if}
                  </div>
                  <small>{row.customerName ?? 'Unassigned'}{row.siteName ? ` / ${row.siteName}` : ''}</small><small>{row.deviceType} / {row.patchRing}</small>
                </td>
                <td><strong>{row.currentVersion} -> {row.targetVersion}</strong><small>{row.os}</small></td>
                <td>
                  {#if preflight}
                    <button class:eligible={preflight.status === 'passed'} class:review={preflight.status === 'warning' || preflight.status === 'running' || preflight.status === 'queued'} class:blocked={preflight.status === 'failed'} class="status status-button" type="button" on:click={() => openPreflightDetails(preflight)}>
                      {preflightStatusLabel(preflight)}
                    </button>
                    <small>{preflightDetailLine(preflight)}</small>
                  {:else}
                    <span class:eligible={row.readiness === 'eligible'} class:review={row.readiness === 'needs_review'} class:blocked={row.readiness === 'blocked'} class="status">{row.readinessLabel}</span><small>{row.blockers[0] ?? row.warnings.join(', ') ?? 'No blockers'}</small>
                  {/if}
                </td>
                <td>
                  <strong>{row.mediaStatus === 'missing' ? 'Missing' : row.mediaStatus}</strong>
                  <small>{row.mediaLabel}</small>
                  {#if isStartActive(displayStart)}
                    <div class="progress-track"><span style={`width: ${Math.max(4, Math.min(100, displayStart?.progress?.overallPercent ?? 0))}%`}></span></div>
                    <small>{startDetailLine(displayStart)}</small>
                  {:else if displayStart?.status === 'failed' || displayStart?.status === 'succeeded'}
                    <small>{startDetailLine(displayStart)}</small>
                  {:else if isStageActive(stage)}
                    <div class="progress-track"><span style={`width: ${Math.max(4, Math.min(100, stage?.progress?.overallPercent ?? 0))}%`}></span></div>
                    <small>{stageDetailLine(stage)}</small>
                  {:else if stage?.status === 'failed'}
                    <small>{stageDetailLine(stage)}</small>
                  {/if}
                </td>
                <td><strong>{displayStart ? startPhaseLabel(displayStart) : stage ? stagePhaseLabel(stage) : preflightPhaseLabel(preflight)}</strong><small>{displayStart ? startStatusLabel(displayStart) : stage ? stageStatusLabel(stage) : preflight ? preflightStatusLabel(preflight) : 'No preflight run'}</small></td>
                <td class="row-actions"><button title="Run preflight" on:click|stopPropagation={() => openPreflightDialog(row.agentId)}><ClipboardCheck size={15} /></button><button title="Stage ISO" disabled={!canStageIso(row)} on:click|stopPropagation={() => openStageIsoDialog(row.agentId)}><HardDriveDownload size={15} /></button><button title="Start upgrade" disabled={!canStartUpgrade(row)} on:click|stopPropagation={() => queueStartRun(row.agentId)}><PlayCircle size={15} /></button><button title="Schedule upgrade" disabled={!canStartUpgrade(row)} on:click|stopPropagation={() => openScheduleDialog(row.agentId)}><CalendarClock size={15} /></button></td>
              </tr>
            {:else}
              <tr><td colspan="7">No Windows devices match the current filters.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/if}

  <Dialog bind:open={preflightDialogOpen} className="preflight-dialog">
    <div class="dialog-content">
      <div>
        <h2>Run feature upgrade preflight</h2>
        <p>{preflightPreview?.devices.length ?? 0} Windows device(s) will refresh snapshot-backed readiness checks.</p>
      </div>

      {#if preflightPreview}
        <div class="dialog-grid">
          <section class="dialog-panel">
            <h3>Devices</h3>
            <div class="device-table-wrap">
              <table class="dialog-device-table">
                <thead>
                  <tr>
                    <th>Device</th>
                    <th>Current OS</th>
                    <th>Target</th>
                    <th>Cached readiness</th>
                    <th>Snapshot age</th>
                  </tr>
                </thead>
                <tbody>
                  {#each preflightPreview.devices as device}
                    <tr>
                      <td><strong>{device.hostname}</strong><small>{device.customerName ?? 'Unassigned'}{device.siteName ? ` / ${device.siteName}` : ''}</small></td>
                      <td><strong>{previewCurrentOs(device)}</strong><small>{device.os}</small></td>
                      <td><strong>{device.targetBuildLabel}</strong><small>{device.targetProduct}</small></td>
                      <td><span class={`status ${previewReadinessClass(device)}`}>{previewCachedReadiness(device)}</span></td>
                      <td><strong>{snapshotAgeLabel(device.snapshotCollectedAt)}</strong><small>{device.snapshotCollectedAt ? 'Latest snapshot' : 'Fresh snapshot will be requested'}</small></td>
                    </tr>
                  {:else}
                    <tr><td colspan="5">No eligible Windows devices selected.</td></tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </section>

          <FeatureUpgradePreflightChecklist title="Checks to run" checks={preflightDialogChecks} />
        </div>

        {#if preflightPreview.skipped.length > 0}
          <div class="warning-band">{preflightPreview.skipped.length} selected device(s) were skipped because they were not found or are not Windows devices.</div>
        {/if}
      {/if}

      <div class="dialog-actions">
        <Button variant="secondary" on:click={() => (preflightDialogOpen = false)}>Cancel</Button>
        <Button disabled={preflightQueueing || (preflightPreview?.devices.length ?? 0) === 0} on:click={queuePreflightRun}>
          <ClipboardCheck size={16} /> {preflightQueueing ? 'Queueing...' : 'Confirm preflight'}
        </Button>
      </div>
    </div>
  </Dialog>

  <Dialog bind:open={stageDialogOpen} className="preflight-dialog">
    <div class="dialog-content">
      <div>
        <h2>Stage feature upgrade ISO</h2>
        <p>{stagePreviewReadyDevices.length} of {stagePreview?.devices.length ?? 0} selected Windows device(s) are ready. ISO files are hidden on disk and deleted after 7 days.</p>
      </div>

      {#if stagePreview}
        <div class="stage-summary">
          <div><strong>{formatBytes(stagePreview.totalSizeBytes)}</strong><span>Total download across ready devices</span></div>
          <div><strong>{stagePreview.retentionDays} days</strong><span>Automatic worker cleanup</span></div>
          <div><strong>{shortDateTime(stagePreview.estimatedExpiresAt)}</strong><span>Estimated expiry if staged now</span></div>
        </div>

        <section class="dialog-panel">
          <h3>Devices</h3>
          <div class="device-table-wrap">
            <table class="dialog-device-table">
              <thead>
                <tr>
                  <th>Device</th>
                  <th>Target</th>
                  <th>ISO</th>
                  <th>Space</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {#each stagePreview.devices as device}
                  <tr>
                    <td><strong>{device.hostname}</strong><small>{device.customerName ?? 'Unassigned'}{device.siteName ? ` / ${device.siteName}` : ''}</small></td>
                    <td><strong>{device.targetBuildLabel}</strong><small>{device.os}</small></td>
                    <td><strong>{device.isoMedia?.displayName ?? 'No matching ISO'}</strong><small>{device.isoMedia?.sha256 ? `SHA-256 ${device.isoMedia.sha256.slice(0, 12)}...` : 'Hash metadata unavailable'}</small></td>
                    <td><strong>{formatBytes(device.expectedSizeBytes)}</strong><small>Held until worker cleanup</small></td>
                    <td>
                      <span class:eligible={device.canStage} class:blocked={!device.canStage} class="status">{device.canStage ? 'Ready to stage' : 'Blocked'}</span>
                      <small>{device.blockingReasons[0] ?? device.warnings[0] ?? 'Preflight passed and media matched'}</small>
                    </td>
                  </tr>
                {:else}
                  <tr><td colspan="5">No eligible Windows devices selected.</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>

        <div class="warning-band">The staged ISO is downloaded outside C:\Windows\Temp, hidden from normal browsing, and automatically deleted 7 days after staging. Devices need enough free space for the ISO plus Windows setup working space.</div>

        {#if stagePreview.skipped.length > 0}
          <div class="warning-band">{stagePreview.skipped.length} selected device(s) were skipped because they were not found or are not Windows devices.</div>
        {/if}
      {/if}

      <div class="dialog-actions">
        <Button variant="secondary" on:click={() => (stageDialogOpen = false)}>Cancel</Button>
        <Button disabled={stageQueueing || stagePreviewReadyDevices.length === 0} on:click={queueStageIsoRun}>
          <HardDriveDownload size={16} /> {stageQueueing ? 'Queueing...' : 'Confirm staging'}
        </Button>
      </div>
    </div>
  </Dialog>

  <Dialog bind:open={scheduleDialogOpen} className="preflight-dialog">
    <div class="dialog-content">
      <div>
        <h2>Schedule feature upgrade</h2>
        <p>{schedulePreviewReadyDevices.length} of {schedulePreview?.devices.length ?? 0} selected Windows device(s) are ready for a scheduled start.</p>
      </div>

      <div class="schedule-picker">
        <span class="schedule-picker-label">Start time</span>
        <div class="schedule-picker-shell">
          <button
            type="button"
            class="schedule-trigger"
            aria-expanded={schedulePickerOpen}
            on:click={() => (schedulePickerOpen = !schedulePickerOpen)}
          >
            <CalendarClock size={16} />
            <span>{scheduleDisplayValue()}</span>
          </button>

          {#if schedulePickerOpen}
            <div class="schedule-popover">
              <div class="schedule-calendar-header">
                <button type="button" aria-label="Previous month" on:click={() => changeScheduleMonth(-1)}>
                  <ChevronLeft size={16} />
                </button>
                <strong>{MONTH_LABEL.format(scheduleCalendarMonth)}</strong>
                <button type="button" aria-label="Next month" on:click={() => changeScheduleMonth(1)}>
                  <ChevronRight size={16} />
                </button>
              </div>

              <div class="schedule-weekdays" aria-hidden="true">
                {#each SCHEDULE_WEEKDAYS as weekday}
                  <span>{weekday}</span>
                {/each}
              </div>

              <div class="schedule-days">
                {#each scheduleCalendarDays(scheduleCalendarMonth) as day}
                  <button
                    type="button"
                    class:muted={!day.inMonth}
                    class:selected={day.selected}
                    class:today={day.today}
                    disabled={day.disabled}
                    on:click={() => selectScheduleDate(day.date)}
                    aria-label={`Select ${day.date.toLocaleDateString()}`}
                  >
                    {day.day}
                  </button>
                {/each}
              </div>

              <div class="schedule-time-row">
                <label>
                  Hour
                  <select value={scheduleHour()} on:change={(event) => setScheduleTime('hour', event.currentTarget.value)}>
                    {#each Array.from({ length: 24 }, (_, hour) => String(hour).padStart(2, '0')) as hour}
                      <option value={hour}>{hour}</option>
                    {/each}
                  </select>
                </label>
                <label>
                  Minute
                  <select value={scheduleMinute()} on:change={(event) => setScheduleTime('minute', event.currentTarget.value)}>
                    {#each Array.from({ length: 12 }, (_, index) => String(index * 5).padStart(2, '0')) as minute}
                      <option value={minute}>{minute}</option>
                    {/each}
                  </select>
                </label>
              </div>

            </div>
          {/if}
        </div>
      </div>

      {#if schedulePreview}
        <div class="stage-summary">
          <div><strong>{formatBytes(schedulePreview.totalDownloadBytes)}</strong><span>Download needed at start time</span></div>
          <div><strong>{formatBytes(schedulePreview.diskFreeBytesRequired)}</strong><span>Required free system-drive space</span></div>
          <div><strong>{schedulePreviewReadyDevices.length}</strong><span>Ready devices</span></div>
        </div>

        <section class="dialog-panel">
          <h3>Devices</h3>
          <div class="device-table-wrap">
            <table class="dialog-device-table">
              <thead>
                <tr>
                  <th>Device</th>
                  <th>Target</th>
                  <th>ISO</th>
                  <th>Preflight</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {#each schedulePreview.devices as device}
                  <tr>
                    <td><strong>{device.hostname}</strong><small>{device.customerName ?? 'Unassigned'}{device.siteName ? ` / ${device.siteName}` : ''}</small></td>
                    <td><strong>{device.targetBuildLabel}</strong><small>{device.os}</small></td>
                    <td><strong>{device.isoMedia?.displayName ?? 'No matching ISO'}</strong><small>{device.willDownloadIso ? 'Will download before setup' : 'Existing staged ISO will be reused'}</small></td>
                    <td><strong>{device.preflightStatus ?? 'Missing'}</strong><small>Strict pass required</small></td>
                    <td>
                      <span class:eligible={device.canStart} class:blocked={!device.canStart} class="status">{device.canStart ? 'Ready' : 'Blocked'}</span>
                      <small>{device.blockingReasons[0] ?? device.warnings[0] ?? 'Final checks run on the worker at start time'}</small>
                    </td>
                  </tr>
                {:else}
                  <tr><td colspan="5">No eligible Windows devices selected.</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>

        {#if schedulePreview.skipped.length > 0}
          <div class="warning-band">{schedulePreview.skipped.length} selected device(s) were skipped because they were not found or are not Windows devices.</div>
        {/if}
      {/if}

      <div class="dialog-actions">
        <Button variant="secondary" on:click={() => (scheduleDialogOpen = false)}>Cancel</Button>
        <Button disabled={scheduleQueueing || schedulePreviewReadyDevices.length === 0 || !scheduleLocalValue} on:click={queueScheduledRun}>
          <CalendarClock size={16} /> {scheduleQueueing ? 'Scheduling...' : 'Confirm schedule'}
        </Button>
      </div>
    </div>
  </Dialog>

  <Dialog bind:open={detailDialogOpen} className="preflight-dialog">
    {#if detailPreflight}
      <div class="dialog-content">
        <div>
          <h2>{detailPreflight.hostname ?? detailPreflight.agentId}</h2>
          <p>{detailPreflight.sourceOs} -> {detailPreflight.targetBuildLabel}</p>
        </div>
        <FeatureUpgradePreflightChecklist title={preflightStatusLabel(detailPreflight)} checks={detailPreflight.checks} />
        <div class="dialog-actions">
          <Button variant="secondary" on:click={() => (detailDialogOpen = false)}>Close</Button>
          <Button variant="secondary" on:click={() => openPreflightDialog(detailPreflight?.agentId)}><ClipboardCheck size={16} /> Re-run preflight</Button>
        </div>
      </div>
    {/if}
  </Dialog>
</div>

<style>
  .upgrade-page { display: flex; min-height: 100%; flex-direction: column; gap: 1.25rem; padding: 1.75rem; color: rgb(221 235 255); }
  .error-band, .empty-state { border: 1px solid rgba(255,255,255,.1); border-radius: 8px; padding: 1rem; background: rgba(255,255,255,.05); }
  .error-band { border-color: rgba(255,99,99,.35); color: rgb(255 190 190); }
  .toolbar, .actions-row { display: flex; min-width: 0; flex-wrap: wrap; gap: .75rem; align-items: center; }
  .summary { display: grid; grid-template-columns: repeat(5, minmax(0,1fr)); border: 1px solid rgba(105,135,180,.24); background: rgba(255,255,255,.025); }
  .summary div { display: grid; min-height: 5.4rem; grid-template-columns: auto 1fr; gap: .3rem .65rem; align-content: center; border-right: 1px solid rgba(105,135,180,.18); padding: 1rem; }
  .summary div:last-child { border-right: 0; }
  .summary :global(svg) { color: rgb(118 190 255); grid-row: span 2; }
  .summary strong { font-size: 1.25rem; line-height: 1; }
  .summary span, small { color: rgb(145 164 198); font-size: .78rem; }
  .main { display: flex; min-width: 0; flex-direction: column; gap: 1rem; }
  .main { overflow: hidden; }
  label { display: flex; align-items: center; gap: .45rem; color: rgb(145 164 198); font-size: .82rem; }
  input, select { min-height: 2.25rem; border: 1px solid rgba(118,142,190,.35); border-radius: 6px; background: rgba(255,255,255,.055); color: rgb(221 235 255); padding: .45rem .65rem; }
  .search { min-width: min(20rem, 100%); flex: 1 1 20rem; }
  .search input { width: 100%; }
  .table-wrap { overflow: auto; border: 1px solid rgba(105,135,180,.24); background: rgba(255,255,255,.025); }
  table { width: 100%; min-width: 1180px; border-collapse: collapse; }
  th, td { border-bottom: 1px solid rgba(105,135,180,.18); padding: .9rem 1rem; text-align: left; vertical-align: top; font-size: .86rem; }
  th { color: rgb(113 143 190); font-weight: 800; }
  td small { display: block; margin-top: .25rem; }
  .device-link { border: 0; background: transparent; color: rgb(148 198 255); padding: 0; text-align: left; }
  .device-name-row { display: flex; align-items: center; gap: .4rem; }
  .preflight-sync-icon,
  .preflight-passed-icon,
  .inline-status { display: inline-flex; flex: 0 0 auto; border: 0; background: transparent; padding: 0; }
  .preflight-sync-icon { color: rgb(125 200 255); }
  .preflight-sync-icon :global(svg) { animation: spin 1s linear infinite; }
  .preflight-passed-icon { color: rgb(98 230 170); }
  .warning-icon { color: rgb(255 205 92); }
  .failed-icon { color: rgb(255 118 118); }
  .status { display: inline-flex; width: fit-content; border: 1px solid rgba(145,164,198,.35); border-radius: 999px; padding: .18rem .55rem; background: rgba(145,164,198,.1); color: rgb(215 226 245); font-size: .75rem; font-weight: 800; }
  .status-button { cursor: pointer; }
  .eligible { border-color: rgba(98,230,170,.38); background: rgba(31,128,92,.22); color: rgb(160 245 205); }
  .review { border-color: rgba(255,205,92,.42); background: rgba(145,105,25,.24); color: rgb(255 222 145); }
  .blocked { border-color: rgba(255,99,99,.42); background: rgba(150,45,58,.22); color: rgb(255 184 184); }
  .row-actions { white-space: nowrap; }
  .row-actions button { margin-right: .35rem; border: 1px solid rgba(115,160,240,.35); border-radius: 7px; background: rgba(50,95,175,.22); color: rgb(210 228 255); padding: .45rem; }
  .row-actions button:disabled { cursor: not-allowed; opacity: .38; }
  .progress-track { height: .35rem; margin-top: .45rem; overflow: hidden; border-radius: 999px; background: rgba(105,135,180,.18); }
  .progress-track span { display: block; height: 100%; border-radius: inherit; background: rgb(118 190 255); }
  :global(.preflight-dialog) { width: min(980px, calc(100vw - 48px)) !important; max-width: min(980px, calc(100vw - 48px)) !important; max-height: calc(100vh - 56px); overflow: auto; }
  .dialog-content { display: grid; gap: 1rem; }
  .dialog-content h2, .dialog-content h3, .dialog-content p { margin: 0; }
  .dialog-content p { color: rgb(145 164 198); }
  .stage-summary { display: grid; grid-template-columns: repeat(3, minmax(0,1fr)); border: 1px solid rgba(105,135,180,.24); background: rgba(255,255,255,.025); }
  .stage-summary div { display: grid; gap: .25rem; border-right: 1px solid rgba(105,135,180,.18); padding: .8rem; }
  .stage-summary div:last-child { border-right: 0; }
  .stage-summary span { color: rgb(145 164 198); font-size: .78rem; }
  .dialog-grid { display: grid; grid-template-columns: minmax(30rem,1.2fr) minmax(24rem,.8fr); gap: 1rem; align-items: start; }
  .dialog-panel { border: 1px solid rgba(105,135,180,.24); border-radius: 8px; background: rgba(255,255,255,.03); padding: 1rem; }
  .schedule-picker { display: flex; justify-content: flex-start; align-items: flex-start; flex-direction: column; gap: .45rem; width: min(24rem, 100%); color: rgb(145 164 198); font-size: .82rem; }
  .schedule-picker-label { font-weight: 700; color: rgb(170 194 230); }
  .schedule-picker-shell { position: relative; width: 100%; }
  .schedule-trigger { display: flex; min-height: 2.5rem; width: 100%; align-items: center; gap: .55rem; border: 1px solid rgba(118,142,190,.35); border-radius: 7px; background: rgba(255,255,255,.055); color: rgb(221 235 255); padding: .55rem .7rem; text-align: left; font-weight: 700; box-shadow: inset 0 1px 0 rgba(255,255,255,.06); transition: border-color .16s ease, background .16s ease, box-shadow .16s ease; }
  .schedule-trigger:hover, .schedule-trigger:focus-visible { border-color: rgba(80,160,255,.55); background: rgba(255,255,255,.085); box-shadow: 0 0 0 3px rgba(60,140,255,.14), inset 0 1px 0 rgba(255,255,255,.08); outline: none; }
  .schedule-trigger :global(svg) { flex: 0 0 auto; color: rgb(118 190 255); }
  .schedule-trigger span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .schedule-popover { position: absolute; left: 0; top: calc(100% + .45rem); z-index: 5; width: min(23.5rem, calc(100vw - 4rem)); border: 1px solid rgba(70,140,255,.22); border-radius: 8px; background: rgba(8,18,52,.96); color: rgb(221 235 255); padding: .75rem; box-shadow: inset 0 1px 0 rgba(255,255,255,.07), 0 20px 55px rgba(0,0,0,.45); backdrop-filter: blur(24px) saturate(170%); -webkit-backdrop-filter: blur(24px) saturate(170%); }
  .schedule-calendar-header { display: grid; grid-template-columns: 2rem 1fr 2rem; align-items: center; gap: .5rem; margin-bottom: .65rem; }
  .schedule-calendar-header strong { text-align: center; color: rgb(221 235 255); font-size: .9rem; }
  .schedule-calendar-header button { border: 1px solid rgba(118,142,190,.24); border-radius: 6px; background: rgba(255,255,255,.055); color: rgb(190 215 248); min-height: 2rem; }
  .schedule-calendar-header button { display: grid; place-items: center; width: 2rem; padding: 0; }
  .schedule-calendar-header button:hover { border-color: rgba(80,160,255,.5); background: rgba(55,130,255,.18); color: rgb(235 246 255); }
  .schedule-weekdays, .schedule-days { display: grid; grid-template-columns: repeat(7, minmax(0,1fr)); gap: .25rem; }
  .schedule-weekdays { margin-bottom: .3rem; }
  .schedule-weekdays span { display: grid; min-height: 1.45rem; place-items: center; color: rgb(113 143 190); font-size: .72rem; font-weight: 800; }
  .schedule-days button { min-width: 0; min-height: 1.85rem; border: 1px solid transparent; border-radius: 6px; background: transparent; color: rgb(213 230 255); font-size: .8rem; font-weight: 700; transition: border-color .14s ease, background .14s ease, color .14s ease; }
  .schedule-days button:hover:not(:disabled), .schedule-days button:focus-visible { border-color: rgba(80,160,255,.45); background: rgba(55,130,255,.14); outline: none; }
  .schedule-days button.muted { color: rgb(104 128 166); }
  .schedule-days button.today { border-color: rgba(118,190,255,.42); }
  .schedule-days button.selected { border-color: rgba(85,165,255,.72); background: linear-gradient(180deg, rgba(55,130,255,.95), rgba(30,80,220,.95)); color: white; box-shadow: 0 8px 22px rgba(30,80,220,.32); }
  .schedule-days button:disabled { cursor: not-allowed; color: rgba(104,128,166,.42); text-decoration: line-through; }
  .schedule-time-row { display: grid; grid-template-columns: 1fr 1fr; gap: .65rem; margin-top: .8rem; padding-top: .75rem; border-top: 1px solid rgba(105,135,180,.2); }
  .schedule-time-row label { display: grid; gap: .35rem; color: rgb(145 164 198); font-size: .74rem; font-weight: 800; }
  .schedule-time-row select { width: 100%; min-height: 2.25rem; height: 2.25rem; }
  .device-table-wrap { max-height: 24rem; overflow: auto; border: 1px solid rgba(105,135,180,.18); background: rgba(255,255,255,.025); }
  .dialog-device-table { width: 100%; min-width: 720px; border-collapse: collapse; }
  .dialog-device-table th, .dialog-device-table td { padding: .7rem .75rem; border-bottom: 1px solid rgba(105,135,180,.16); font-size: .8rem; }
  .dialog-device-table th { position: sticky; top: 0; z-index: 1; background: rgb(9 18 42); color: rgb(113 143 190); }
  .dialog-device-table td strong { display: block; color: rgb(221 235 255); line-height: 1.3; }
  .warning-band { border: 1px solid rgba(255,205,92,.35); border-radius: 8px; background: rgba(145,105,25,.18); color: rgb(255 222 145); padding: .8rem; font-size: .84rem; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: .75rem; }
  @keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
  @media (max-width: 1100px) { .summary { grid-template-columns: repeat(2, minmax(0,1fr)); } }
  @media (max-width: 900px) { .dialog-grid { grid-template-columns: 1fr; } }
  :global(html.light) .schedule-picker { color: rgb(70 92 125); }
  :global(html.light) .schedule-picker-label { color: rgb(30 58 95); }
  :global(html.light) .schedule-trigger { border-color: rgba(100,158,220,.42); background: rgba(255,255,255,.72); color: rgb(10 22 40); box-shadow: inset 0 1px 0 rgba(255,255,255,.85); }
  :global(html.light) .schedule-trigger:hover, :global(html.light) .schedule-trigger:focus-visible { border-color: rgba(50,120,255,.5); background: rgba(255,255,255,.9); box-shadow: 0 0 0 3px rgba(50,120,255,.12), inset 0 1px 0 rgba(255,255,255,.9); }
  :global(html.light) .schedule-popover { border-color: rgba(100,158,220,.35); background: rgba(255,255,255,.94); color: rgb(10 22 40); box-shadow: inset 0 1px 0 rgba(255,255,255,.95), 0 20px 55px rgba(0,30,100,.18); }
  :global(html.light) .schedule-calendar-header strong { color: rgb(10 22 40); }
  :global(html.light) .schedule-calendar-header button { border-color: rgba(100,158,220,.28); background: rgba(235,245,255,.72); color: rgb(32 72 130); }
  :global(html.light) .schedule-calendar-header button:hover { border-color: rgba(50,120,255,.45); background: rgba(215,235,255,.9); color: rgb(14 56 150); }
  :global(html.light) .schedule-weekdays span { color: rgb(75 105 150); }
  :global(html.light) .schedule-days button { color: rgb(18 34 58); }
  :global(html.light) .schedule-days button.muted { color: rgb(135 155 180); }
  :global(html.light) .schedule-days button:hover:not(:disabled), :global(html.light) .schedule-days button:focus-visible { border-color: rgba(50,120,255,.42); background: rgba(50,120,255,.09); }
  :global(html.light) .schedule-days button.today { border-color: rgba(30,100,220,.36); }
  :global(html.light) .schedule-days button.selected { border-color: rgba(30,100,220,.68); background: linear-gradient(180deg, rgba(48,132,255,.96), rgba(24,88,220,.96)); color: white; box-shadow: 0 8px 22px rgba(30,100,220,.24); }
  :global(html.light) .schedule-days button:disabled { color: rgba(135,155,180,.55); }
  :global(html.light) .schedule-time-row { border-top-color: rgba(100,158,220,.24); }
  :global(html.light) .schedule-time-row label { color: rgb(70 92 125); }
  @media (max-width: 700px) { .upgrade-page { padding: 1rem; } .summary, .stage-summary { grid-template-columns: 1fr; } .search { min-width: 100%; } .schedule-popover { width: min(23.5rem, calc(100vw - 3rem)); } }
</style>
