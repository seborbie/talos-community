<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import {
    Ban,
    CalendarClock,
    CheckCircle2,
    ClipboardCheck,
    Disc3,
    FileDown,
    HardDriveDownload,
    Info,
    Plus,
    PlayCircle,
    RefreshCw,
    Save,
    Search,
    Server,
    ShieldAlert,
    Trash2,
    Zap
  } from 'lucide-svelte';
  import Button from '$lib/ui/Button.svelte';
  import { customerApi, patchApi, siteApi } from '$lib/api';
  import { topbarConfig } from '$lib/topbar';
  import { toast } from '$lib/toast';
  import type {
    Customer,
    PatchDecisionLog,
    PatchOverviewDevice,
    PatchOverviewResponse,
    PatchOverviewUpdate,
    PatchOverride,
    PatchPolicy,
    PatchPolicyScopeType,
    PatchPolicyTargetOsFamily,
    PatchProgressUpdate,
    Site
  } from '$lib/types';

  const NEW_POLICY_ID = '__new_patch_policy__';
  const DEFAULT_POLICY_PRIORITY = 10000;
  const CUSTOM_POLICY_DEFAULT_PRIORITY = 100;
  const PATCH_PROGRESS_STALE_MS = 75_000;
  const PATCH_PROGRESS_TERMINAL_MS = 30_000;
  const PATCH_PROGRESS_POLL_MS = 5_000;

  type TabId = 'devices' | 'updates' | 'policies' | 'audit';
  type UpdateStateFilter =
    | 'all'
    | 'actionable'
    | 'detected'
    | 'downloaded'
    | 'installed'
    | 'failed'
    | 'blocked'
    | 'deferred'
    | 'superseded';
  type FeatureUpgradeReadinessFilter = 'all' | 'eligible' | 'needs_review' | 'blocked' | 'unknown';
  type FeatureUpgradeMediaFilter = 'all' | 'assigned' | 'missing' | 'staged';

  type FeatureUpgradeRow = {
    agentId: string;
    hostname: string;
    os: string;
    currentVersion: string;
    targetVersion: string;
    readiness: FeatureUpgradeReadinessFilter;
    readinessLabel: string;
    blockers: string[];
    warnings: string[];
    mediaStatus: 'assigned' | 'missing' | 'staged';
    mediaLabel: string;
    phase: string;
    lastPreflightAt: string | null;
    customerName: string | null;
    siteName: string | null;
    deviceType: PatchOverviewDevice['deviceType'];
    patchRing: PatchOverviewDevice['patchRing'];
    rebootRequired: boolean;
  };

  const tabs: Array<{ value: TabId; label: string }> = [
    { value: 'devices', label: 'Devices' },
    { value: 'updates', label: 'Updates' },
    { value: 'policies', label: 'Policies' },
    { value: 'audit', label: 'Audit' }
  ];

  let overview: PatchOverviewResponse | null = null;
  let customers: Customer[] = [];
  let sites: Site[] = [];
  let activeTab: TabId = 'devices';
  let featureUpgradeWorkspaceOpen = false;
  let loading = true;
  let actionLoading = false;
  let policySaving = false;
  let error: string | null = null;
  let refreshTimer: ReturnType<typeof setInterval> | null = null;
  let progressTimer: ReturnType<typeof setInterval> | null = null;
  let patchProgressItems: PatchProgressUpdate[] = [];
  let progressPollInFlight = false;
  let progressOverviewRefreshInFlight = false;

  let selectedAgentIds = new Set<string>();
  let customerFilterInput = 'All Customers';
  let customerFilterOpen = false;
  let customerFilterInputElement: HTMLInputElement | null = null;
  let updateFilter = '';
  let updateDeviceFilter = '';
  let updateCategoryFilter = 'all';
  let updateOsFilter = 'all';
  let updateCustomerFilter = 'all';
  let updateSiteFilter = 'all';
  let updateStateFilter: UpdateStateFilter = 'all';
  let updateSourceFilter = 'all';
  let deferUntil = '';
  let selectedUpdateKey: string | null = null;
  let featureUpgradeSearch = '';
  let featureUpgradeTargetFilter = 'all';
  let featureUpgradeReadinessFilter: FeatureUpgradeReadinessFilter = 'all';
  let featureUpgradeMediaFilter: FeatureUpgradeMediaFilter = 'all';
  let selectedFeatureUpgradeAgentIds = new Set<string>();

  let selectedPolicyId = '';
  let hydratedPolicyId = '';
  let policyName = '';
  let policyEnabled = true;
  let policyPriority = CUSTOM_POLICY_DEFAULT_PRIORITY;
  let policyScopeType: PatchPolicyScopeType = 'organization';
  let policyTargetOsFamily: PatchPolicyTargetOsFamily = 'all';
  let policyCustomerId = '';
  let policySiteId = '';
  let policyAgentId = '';
  let policyDeferralDays = 14;
  let policyManagedMode = true;
  let policyScanStart = '';
  let policyScanEnd = '';
  let policyDownloadInstallStart = '';
  let policyDownloadInstallEnd = '';
  let policyRebootStart = '';
  let policyRebootEnd = '';
  let policyTimezone = 'UTC';

  let auditSearch = '';
  let auditActionFilter = 'all';
  let auditDecisionFilter = 'all';
  let auditActorFilter = 'all';
  let auditUserFilter = '';
  let auditFrom = '';
  let auditTo = '';
  let pendingAuditSnapshot: PatchOverviewResponse | null = null;
  let newAuditRows = 0;

  $: devices = overview?.devices ?? [];
  $: updates = overview?.updates ?? [];
  $: policies = overview?.policies ?? [];
  $: overrides = overview?.overrides ?? [];
  $: decisions = overview?.decisions ?? [];
  $: activeOverrides = overrides.filter((override) => {
    if (!override.enabled) return false;
    if (!override.expiresAt) return true;
    const expiresAt = Date.parse(override.expiresAt);
    return Number.isNaN(expiresAt) || expiresAt > Date.now();
  });
  $: deviceHostnamesByAgentId = new Map(devices.map((device) => [device.agentId, device.hostname]));
  $: unassignedCustomerId = customers.find((customer) => customer.isUnassigned)?.id ?? '';
  $: effectiveCustomerFilter = resolveCustomerFilter(customerFilterInput);
  $: visibleDevices = devices.filter((device) => {
    if (effectiveCustomerFilter === 'all') return true;
    if (effectiveCustomerFilter === 'unassigned') return !device.customerId || device.customerId === unassignedCustomerId;
    return device.customerId === effectiveCustomerFilter;
  });
  $: selectedCount = selectedAgentIds.size;
  $: allVisibleSelected = visibleDevices.length > 0 && visibleDevices.every((device) => selectedAgentIds.has(device.agentId));
  $: customerFilterOptions = [
    'All Customers',
    'Unassigned',
    ...customers.filter((customer) => !customer.isUnassigned).map((customer) => customer.name)
  ];
  $: filteredCustomerOptions = customerFilterOptions.filter((option) =>
    !customerFilterInput.trim() || option.toLowerCase().includes(customerFilterInput.trim().toLowerCase())
  );
  $: updateCategories = uniqueSorted(updates.map((update) => update.category).filter(Boolean));
  $: updateOsFamilies = uniqueSorted(updates.flatMap((update) => update.osFamilies ?? []).filter(Boolean));
  $: updateCustomers = uniqueSorted(updates.flatMap((update) => update.customerNames ?? []).filter(Boolean));
  $: updateSites = uniqueSorted(updates.flatMap((update) => update.siteNames ?? []).filter(Boolean));
  $: updateSources = uniqueSorted(updates.map((update) => update.source).filter(Boolean));
  $: visibleUpdates = updates.filter(updateMatchesFilters);
  $: windowsDevices = devices.filter(isWindowsDevice);
  $: featureUpgradeRows = windowsDevices.map(buildFeatureUpgradeRow);
  $: featureUpgradeTargets = uniqueSorted(featureUpgradeRows.map((row) => row.targetVersion));
  $: visibleFeatureUpgradeRows = featureUpgradeRows.filter(featureUpgradeRowMatchesFilters);
  $: selectedFeatureUpgradeCount = selectedFeatureUpgradeAgentIds.size;
  $: allVisibleFeatureUpgradeSelected =
    visibleFeatureUpgradeRows.length > 0 && visibleFeatureUpgradeRows.every((row) => selectedFeatureUpgradeAgentIds.has(row.agentId));
  $: featureUpgradeTotals = {
    windows: featureUpgradeRows.length,
    eligible: featureUpgradeRows.filter((row) => row.readiness === 'eligible').length,
    review: featureUpgradeRows.filter((row) => row.readiness === 'needs_review').length,
    blocked: featureUpgradeRows.filter((row) => row.readiness === 'blocked').length,
    mediaMissing: featureUpgradeRows.filter((row) => row.mediaStatus === 'missing').length
  };
  $: auditActions = uniqueSorted(decisions.map((decision) => decision.action).filter(Boolean));
  $: auditDecisions = uniqueSorted(decisions.map((decision) => decision.decision).filter(Boolean));
  $: auditUsers = uniqueSorted(decisions.filter((decision) => decision.actorType === 'user' && decision.actorEmail).map((decision) => decision.actorEmail as string));
  $: visibleAuditRows = decisions.filter(auditRowMatchesFilters);
  $: totals = {
    devices: overview?.summary?.devices ?? devices.length,
    managed: overview?.summary?.managed ?? devices.filter((device) => device.patchManaged).length,
    pending: overview?.summary?.pending ?? devices.reduce((sum, device) => sum + device.pendingUpdates, 0),
    downloaded: overview?.summary?.downloaded ?? devices.reduce((sum, device) => sum + device.downloadedUpdates, 0),
    failed: overview?.summary?.failed ?? devices.reduce((sum, device) => sum + device.failedUpdates, 0),
    reboot:
      overview?.summary?.reboot ??
      devices.filter((device) => device.rebootRequired || device.rebootPendingUpdates > 0).length
  };
  $: activeScanProgressByAgentId = latestProgressByAgentId(
    patchProgressItems.filter((item) => item.eventType === 'patch.scan.progress' && item.status === 'running' && isProgressFresh(item))
  );
  $: activeInstallProgressByAgentId = latestProgressByAgentId(
    patchProgressItems.filter(
      (item) =>
        item.eventType === 'patch.install.progress' &&
        item.status === 'running' &&
        isProgressFresh(item)
    )
  );
  $: completedInstallProgressByAgentId = latestProgressByAgentId(
    patchProgressItems.filter(
      (item) =>
        item.eventType === 'patch.install.progress' &&
        item.status === 'completed' &&
        (item.summary?.installed ?? 0) > 0 &&
        isProgressFresh(item)
    )
  );
  $: isCreatingPolicy = selectedPolicyId === NEW_POLICY_ID;
  $: selectedPolicy = isCreatingPolicy ? null : policies.find((policy) => policy.id === selectedPolicyId) ?? policies[0] ?? null;
  $: selectedPolicyIsDefault = selectedPolicy?.isDefault === true;
  $: policyManagedModeUnsupported = policyTargetOsFamily === 'macos';
  $: policyManagedModeTooltip = policyManagedModeUnsupported
    ? 'Managed mode is unsupported for macOS-only policies because macOS updates cannot be effectively blocked this way.'
    : targetUsesWindowsNativeControl(policyTargetOsFamily)
      ? 'When enabled, Talos controls patching and applies native Windows Update policy on Windows devices.'
      : 'When enabled, Talos controls patching with the platform-native update tool.';
  $: filteredPolicySites = policyCustomerId ? sites.filter((site) => site.customerId === policyCustomerId) : sites;
  $: sortedDevices = [...devices].sort((a, b) => a.hostname.localeCompare(b.hostname));
  $: if (!selectedPolicyId && policies[0]) selectedPolicyId = policies[0].id;
  $: if (selectedPolicyId && hydratedPolicyId !== selectedPolicyId) hydratePolicyForm();
  $: if (policyManagedModeUnsupported && policyManagedMode) policyManagedMode = false;
  $: if (overview) selectedAgentIds = new Set([...selectedAgentIds].filter((agentId) => devices.some((device) => device.agentId === agentId)));
  $: if (overview && (!selectedUpdateKey || !updates.some((update) => update.updateKey === selectedUpdateKey))) {
    selectedUpdateKey = updates[0]?.updateKey ?? null;
  }
  $: if (overview) {
    selectedFeatureUpgradeAgentIds = new Set(
      [...selectedFeatureUpgradeAgentIds].filter((agentId) => featureUpgradeRows.some((row) => row.agentId === agentId))
    );
  }
  $: topbarConfig.set({
    title: 'Patch Management',
    action: {
      label: 'Refresh',
      disabled: loading,
      run: () => fetchData()
    }
  });

  function uniqueSorted(values: string[]) {
    return [...new Set(values)].sort((a, b) => a.localeCompare(b));
  }

  function resolveCustomerFilter(input: string) {
    const normalized = input.trim().toLowerCase();
    if (!normalized || normalized === 'all customers') return 'all';
    if (normalized === 'unassigned') return 'unassigned';
    return customers.find((customer) => customer.name.toLowerCase() === normalized)?.id ?? 'all';
  }

  function progressTimestamp(item: PatchProgressUpdate) {
    const parsed = Date.parse(item.receivedAt ?? item.reportedAt);
    return Number.isNaN(parsed) ? null : parsed;
  }

  function isProgressFresh(item: PatchProgressUpdate, now = Date.now()) {
    const timestamp = progressTimestamp(item);
    if (timestamp === null) return false;
    const ttl = item.status === 'running' || item.status === 'queued' ? PATCH_PROGRESS_STALE_MS : PATCH_PROGRESS_TERMINAL_MS;
    return now - timestamp <= ttl;
  }

  function latestProgressByAgentId(items: PatchProgressUpdate[]) {
    const latest = new Map<string, PatchProgressUpdate>();
    for (const item of items) {
      const current = latest.get(item.agentId);
      if (!current || (progressTimestamp(item) ?? 0) >= (progressTimestamp(current) ?? 0)) {
        latest.set(item.agentId, item);
      }
    }
    return latest;
  }

  function formatDate(value: string | null | undefined, fallback = 'Not scanned') {
    if (!value) return fallback;
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? fallback : new Date(parsed).toLocaleString();
  }

  function formatCategory(category: string | null | undefined) {
    if (!category) return 'Other';
    const labels: Record<string, string> = {
      microsoft_product: 'Microsoft product',
      uwp_app: 'UWP app'
    };
    return labels[category] ?? category.replace(/_/g, ' ').replace(/\b\w/g, (char) => char.toUpperCase());
  }

  function isWindowsDevice(device: PatchOverviewDevice) {
    return /\bwindows\b/i.test(device.os);
  }

  function isMacosDevice(device: PatchOverviewDevice) {
    return /\bmac\s?os\b|\bdarwin\b/i.test(device.os);
  }

  function macosUpdateAccountNeedsAttention(device: PatchOverviewDevice) {
    const status = device.macosUpdateAccount?.status;
    return isMacosDevice(device) && (!status || !['ready', 'notRequired'].includes(status));
  }

  function macosUpdateAccountTitle(device: PatchOverviewDevice) {
    return device.macosUpdateAccount?.failureMessage ?? 'Mac Software Updates readiness has not been reported as ready.';
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

  function buildFeatureUpgradeRow(device: PatchOverviewDevice): FeatureUpgradeRow {
    const os = device.os || 'Windows';
    const lower = os.toLowerCase();
    const currentVersion = inferWindowsVersion(os);
    const isServer = lower.includes('server');
    const targetVersion = isServer ? 'Windows Server 2025' : 'Windows 11 25H2';
    const blockers: string[] = [];
    const warnings: string[] = [];
    let readiness: FeatureUpgradeReadinessFilter = 'unknown';
    let readinessLabel = 'Preflight required';

    if (device.rebootRequired || device.rebootPendingUpdates > 0) {
      blockers.push('Pending reboot');
    }

    if (isServer) {
      const roles = serverRoleSummary(device);
      if (roles) warnings.push(roles);
      warnings.push('Maintenance window approval required');
      readiness = blockers.length > 0 ? 'blocked' : 'needs_review';
      readinessLabel = blockers.length > 0 ? 'Blocked' : 'Needs review';
    } else if (lower.includes('windows 11')) {
      readiness = blockers.length > 0 ? 'blocked' : 'eligible';
      readinessLabel = blockers.length > 0 ? 'Blocked' : 'Ready for 25H2 preflight';
    } else if (lower.includes('windows 10')) {
      warnings.push('TPM and Secure Boot checks pending');
      warnings.push('Setup compatibility scan pending');
      readiness = blockers.length > 0 ? 'blocked' : 'needs_review';
      readinessLabel = blockers.length > 0 ? 'Blocked' : 'Needs Win11 eligibility check';
    }

    const mediaStatus: FeatureUpgradeRow['mediaStatus'] = 'missing';

    return {
      agentId: device.agentId,
      hostname: device.hostname,
      os,
      currentVersion,
      targetVersion,
      readiness,
      readinessLabel,
      blockers,
      warnings,
      mediaStatus,
      mediaLabel: targetVersion.includes('Server') ? 'Server 2025 ISO not assigned' : 'Windows 11 25H2 ISO not assigned',
      phase: 'Not started',
      lastPreflightAt: null,
      customerName: device.customerName,
      siteName: device.siteName,
      deviceType: device.deviceType,
      patchRing: device.patchRing,
      rebootRequired: device.rebootRequired || device.rebootPendingUpdates > 0
    };
  }

  function inferWindowsVersion(os: string) {
    const buildMatch = os.match(/\b(2[0-9]H[12])\b/i);
    if (buildMatch) return buildMatch[1].toUpperCase();
    const serverMatch = os.match(/Windows Server\s+(\d{4})/i);
    if (serverMatch) return `Server ${serverMatch[1]}`;
    if (/Windows 11/i.test(os)) return 'Windows 11';
    if (/Windows 10/i.test(os)) return 'Windows 10';
    return 'Windows';
  }

  function featureUpgradeRowMatchesFilters(row: FeatureUpgradeRow) {
    const query = featureUpgradeSearch.trim().toLowerCase();
    if (featureUpgradeTargetFilter !== 'all' && row.targetVersion !== featureUpgradeTargetFilter) return false;
    if (featureUpgradeReadinessFilter !== 'all' && row.readiness !== featureUpgradeReadinessFilter) return false;
    if (featureUpgradeMediaFilter !== 'all' && row.mediaStatus !== featureUpgradeMediaFilter) return false;
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
    ]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(query));
  }

  function updateState(update: PatchOverviewUpdate): UpdateStateFilter {
    if (update.failedDevices > 0) return 'failed';
    if (update.blockedDevices > 0) return 'blocked';
    if (update.deferredDevices > 0) return 'deferred';
    if (update.installedDevices > 0) return 'installed';
    if (update.downloadedDevices > 0) return 'downloaded';
    if (update.supersededDevices > 0) return 'superseded';
    if (update.detectedDevices > 0) return 'detected';
    return 'actionable';
  }

  function updateMatchesFilters(update: PatchOverviewUpdate) {
    const query = updateFilter.trim().toLowerCase();
    const deviceQuery = updateDeviceFilter.trim().toLowerCase();
    const associatedHostnames = update.associatedHostnames ?? update.affectedHostnames ?? [];
    const associatedAgentIds = update.associatedAgentIds ?? update.affectedAgentIds ?? [];

    if (updateCategoryFilter !== 'all' && update.category !== updateCategoryFilter) return false;
    if (updateOsFilter !== 'all' && !(update.osFamilies ?? []).includes(updateOsFilter)) return false;
    if (updateCustomerFilter !== 'all' && !(update.customerNames ?? []).includes(updateCustomerFilter)) return false;
    if (updateSiteFilter !== 'all' && !(update.siteNames ?? []).includes(updateSiteFilter)) return false;
    if (updateSourceFilter !== 'all' && update.source !== updateSourceFilter) return false;
    if (updateStateFilter !== 'all' && !updateMatchesState(update, updateStateFilter)) return false;
    if (
      deviceQuery &&
      ![...associatedHostnames, ...associatedAgentIds, ...(update.affectedHostnames ?? []), ...(update.affectedAgentIds ?? [])].some((value) =>
        String(value).toLowerCase().includes(deviceQuery)
      )
    ) {
      return false;
    }
    if (!query) return true;
    return [
      update.title,
      update.kbArticle,
      update.category,
      formatCategory(update.category),
      update.updateKey,
      update.source,
      update.releaseDateSource,
      ...(update.osFamilies ?? []),
      ...(update.customerNames ?? []),
      ...(update.siteNames ?? []),
      ...(update.affectedHostnames ?? []),
      ...associatedHostnames,
      ...(update.deviceTypes ?? []),
      ...(update.patchRings ?? [])
    ]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(query));
  }

  function updateMatchesState(update: PatchOverviewUpdate, state: UpdateStateFilter) {
    if (state === 'all') return true;
    if (state === 'actionable') return update.affectedDevices > 0 && update.blockedDevices === 0 && update.deferredDevices === 0;
    if (state === 'detected') return update.detectedDevices > 0;
    if (state === 'downloaded') return update.downloadedDevices > 0;
    if (state === 'installed') return update.installedDevices > 0;
    if (state === 'failed') return update.failedDevices > 0;
    if (state === 'blocked') return update.blockedDevices > 0;
    if (state === 'deferred') return update.deferredDevices > 0;
    if (state === 'superseded') return update.supersededDevices > 0;
    return true;
  }

  function auditDeviceLabel(agentId: string) {
    return deviceHostnamesByAgentId.get(agentId) ?? agentId;
  }

  function actorLabel(actorType: string, actorEmail: string | null) {
    if (actorType === 'user') return 'Manual override via policy engine';
    if (actorType === 'agent') return 'Agent result';
    return 'Policy engine';
  }

  function formatTargetOsFamily(value: PatchPolicyTargetOsFamily | string | null | undefined) {
    if (value === 'windows') return 'Windows';
    if (value === 'linux') return 'Linux';
    if (value === 'macos') return 'macOS';
    return 'All devices';
  }

  function targetUsesWindowsNativeControl(value: PatchPolicyTargetOsFamily | string | null | undefined) {
    return value === 'all' || value === 'windows';
  }

  function overrideDeviceLabel(override: PatchOverride) {
    if (override.targetHostname) return override.targetHostname;
    if (override.targetAgentId) return override.targetAgentId;
    if (override.scopeType === 'device') return deviceHostnamesByAgentId.get(override.scopeKey) ?? override.scopeKey;
    return `${override.scopeType}:${override.scopeKey}`;
  }

  function overrideRequesterLabel(override: PatchOverride) {
    return override.createdByEmail ?? override.createdBy ?? 'Unknown requester';
  }

  function overrideUpdateLabel(override: PatchOverride) {
    return override.updateKey ?? override.kbArticle ?? override.category ?? 'All applicable updates';
  }

  function overrideStatusLabel(override: PatchOverride) {
    const status = override.latestActionStatus ?? (override.enabled ? 'active' : 'disabled');
    return override.latestActionPhase ? `${status} / ${override.latestActionPhase}` : status;
  }

  function auditRowMatchesFilters(row: PatchDecisionLog) {
    const query = auditSearch.trim().toLowerCase();
    const decidedAt = Date.parse(row.decidedAt);
    const from = auditFrom ? new Date(auditFrom).getTime() : null;
    const to = auditTo ? new Date(auditTo).getTime() : null;
    if (auditActionFilter !== 'all' && row.action !== auditActionFilter) return false;
    if (auditDecisionFilter !== 'all' && row.decision !== auditDecisionFilter) return false;
    if (auditActorFilter !== 'all' && row.actorType !== auditActorFilter) return false;
    if (auditActorFilter === 'user' && auditUserFilter.trim() && !String(row.actorEmail ?? '').toLowerCase().includes(auditUserFilter.trim().toLowerCase())) return false;
    if (from !== null && (Number.isNaN(decidedAt) || decidedAt < from)) return false;
    if (to !== null && (Number.isNaN(decidedAt) || decidedAt > to)) return false;
    if (!query) return true;
    return [
      auditDeviceLabel(row.agentId),
      row.agentId,
      row.operationId,
      row.action,
      row.decision,
      row.reason,
      row.actorType,
      row.actorEmail,
      actorLabel(row.actorType, row.actorEmail),
      row.actionStatus,
      row.actionPhase,
      ...(row.updateKeys ?? [])
    ]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(query));
  }

  function selectCustomerFilter(option: string) {
    customerFilterInput = option;
    customerFilterOpen = false;
    customerFilterInputElement?.blur();
  }

  function toggleDevice(agentId: string) {
    const next = new Set(selectedAgentIds);
    next.has(agentId) ? next.delete(agentId) : next.add(agentId);
    selectedAgentIds = next;
  }

  function toggleAllVisible() {
    const next = new Set(selectedAgentIds);
    if (allVisibleSelected) {
      visibleDevices.forEach((device) => next.delete(device.agentId));
    } else {
      visibleDevices.forEach((device) => next.add(device.agentId));
    }
    selectedAgentIds = next;
  }

  function toggleFeatureUpgradeDevice(agentId: string) {
    const next = new Set(selectedFeatureUpgradeAgentIds);
    next.has(agentId) ? next.delete(agentId) : next.add(agentId);
    selectedFeatureUpgradeAgentIds = next;
  }

  function toggleAllVisibleFeatureUpgrades() {
    const next = new Set(selectedFeatureUpgradeAgentIds);
    if (allVisibleFeatureUpgradeSelected) {
      visibleFeatureUpgradeRows.forEach((row) => next.delete(row.agentId));
    } else {
      visibleFeatureUpgradeRows.forEach((row) => next.add(row.agentId));
    }
    selectedFeatureUpgradeAgentIds = next;
  }

  function targetAgentIds() {
    return [...selectedAgentIds];
  }

  function selectedUpdate() {
    return updates.find((update) => update.updateKey === selectedUpdateKey) ?? null;
  }

  async function fetchData(background = false, pollProgressAfterLoad = true) {
    try {
      if (!background) loading = true;
      error = null;
      const [overviewData, customerData, siteData] = await Promise.all([
        patchApi.getOverview(),
        customerApi.getCustomers(),
        siteApi.getSites()
      ]);
      overview = overviewData;
      customers = customerData;
      sites = siteData;
      pendingAuditSnapshot = null;
      newAuditRows = 0;
      if (pollProgressAfterLoad) {
        await pollPatchProgress();
      }
    } catch (err) {
      console.error('Failed to load patch overview:', err);
      error = err instanceof Error ? err.message : 'Failed to load patch overview';
    } finally {
      if (!background) loading = false;
    }
  }

  async function pollPatchProgress() {
    if (activeTab !== 'devices' || visibleDevices.length === 0 || (typeof document !== 'undefined' && document.hidden)) return;
    if (progressPollInFlight) return;
    progressPollInFlight = true;
    try {
      const response = await patchApi.queryProgress(visibleDevices.map((device) => device.agentId));
      const now = Date.now();
      patchProgressItems = (response.items ?? []).filter((item) => isProgressFresh(item, now));
      const hasTerminalProgress = patchProgressItems.some((item) => item.status === 'completed' || item.status === 'failed' || item.status === 'cancelled');
      if (hasTerminalProgress && !progressOverviewRefreshInFlight) {
        progressOverviewRefreshInFlight = true;
        try {
          await fetchData(true, false);
        } finally {
          progressOverviewRefreshInFlight = false;
        }
      }
    } catch (err) {
      console.warn('Failed to check patch progress:', err);
    } finally {
      progressPollInFlight = false;
    }
  }

  function newestDecisionTimestamp(rows: PatchDecisionLog[]) {
    const values = rows.map((row) => Date.parse(row.decidedAt)).filter((value) => !Number.isNaN(value));
    return values.length > 0 ? Math.max(...values) : 0;
  }

  async function checkAuditAlerts() {
    if (!overview || activeTab !== 'audit' || (typeof document !== 'undefined' && document.hidden)) return;
    try {
      const next = await patchApi.getOverview();
      const knownIds = new Set(decisions.map((row) => row.id));
      const latestKnown = newestDecisionTimestamp(decisions);
      const newRows = (next.decisions ?? []).filter((row) => {
        const decidedAt = Date.parse(row.decidedAt);
        return !knownIds.has(row.id) && (latestKnown === 0 || (!Number.isNaN(decidedAt) && decidedAt >= latestKnown));
      }).length;
      if (newRows > 0) {
        pendingAuditSnapshot = next;
        newAuditRows = newRows;
      }
    } catch (err) {
      console.warn('Failed to check audit alerts:', err);
    }
  }

  function refreshAuditAlerts() {
    if (pendingAuditSnapshot) {
      overview = pendingAuditSnapshot;
      pendingAuditSnapshot = null;
      newAuditRows = 0;
      return;
    }
    void fetchData();
  }

  async function runDeviceAction(action: string) {
    const agentIds = targetAgentIds();
    if (agentIds.length === 0) {
      toast({ title: 'Select at least one device', variant: 'destructive' });
      return;
    }
    try {
      actionLoading = true;
      const response = await patchApi.runAction({ action, agentIds });
      toast({
        title: 'Patch action queued',
        description: `${response.targetedDevices} device(s) targeted.`
      });
      await fetchData(true, false);
    } catch (err) {
      toast({
        title: 'Patch action failed',
        description: err instanceof Error ? err.message : 'Request failed',
        variant: 'destructive'
      });
    } finally {
      actionLoading = false;
    }
  }

  async function runUpdateAction(action: string, update: PatchOverviewUpdate) {
    try {
      actionLoading = true;
      const affectedAgentIds = update.affectedAgentIds ?? [];
      await patchApi.runAction({
        action,
        agentIds: affectedAgentIds.length > 0 ? affectedAgentIds : undefined,
        updateKeys: [update.updateKey],
        kbArticle: update.kbArticle,
        category: update.category,
        deferUntil: action === 'defer_update' && deferUntil ? new Date(deferUntil).toISOString() : undefined
      });
      toast({ title: 'Update override saved' });
      await fetchData(true);
    } catch (err) {
      toast({
        title: 'Update action failed',
        description: err instanceof Error ? err.message : 'Request failed',
        variant: 'destructive'
      });
    } finally {
      actionLoading = false;
    }
  }

  function runFeatureUpgradeAction(action: string, agentId?: string) {
    const actionLabels: Record<string, string> = {
      preflight: 'Preflight checks',
      stage_iso: 'ISO staging',
      start_upgrade: 'Feature upgrade',
      schedule: 'Upgrade schedule',
      assign_media: 'ISO media assignment',
      upload_media: 'ISO upload'
    };
    const needsSelection = !['assign_media', 'upload_media'].includes(action);
    const targetedCount = agentId ? 1 : selectedFeatureUpgradeAgentIds.size;
    if (needsSelection && targetedCount === 0) {
      toast({ title: 'Select at least one Windows device', variant: 'destructive' });
      return;
    }
    toast({
      title: actionLabels[action] ?? 'Feature upgrade action',
      description: needsSelection
        ? `${targetedCount} device(s) selected. Open the Feature Upgrade Center to run this workflow.`
        : 'ISO media management is handled in the Feature Upgrade Center.'
    });
  }

  function openFeatureUpgradeWorkspace() {
    featureUpgradeWorkspaceOpen = true;
  }

  function closeFeatureUpgradeWorkspace() {
    featureUpgradeWorkspaceOpen = false;
  }

  function newPolicy() {
    selectedPolicyId = NEW_POLICY_ID;
    hydratedPolicyId = '';
  }

  function hydratePolicyForm() {
    if (isCreatingPolicy) {
      policyName = 'New patch policy';
      policyEnabled = true;
      policyPriority = CUSTOM_POLICY_DEFAULT_PRIORITY;
      policyScopeType = 'organization';
      policyTargetOsFamily = 'all';
      policyCustomerId = '';
      policySiteId = '';
      policyAgentId = '';
      policyDeferralDays = 14;
      policyManagedMode = true;
      policyScanStart = '';
      policyScanEnd = '';
      policyDownloadInstallStart = '';
      policyDownloadInstallEnd = '';
      policyRebootStart = '';
      policyRebootEnd = '';
      policyTimezone = 'UTC';
      hydratedPolicyId = selectedPolicyId;
      return;
    }

    const policy = selectedPolicy;
    if (!policy) return;
    const config = policy.policyConfig as {
      windows?: {
        scan?: { start?: string | null; end?: string | null; timezone?: string | null };
        download?: { start?: string | null; end?: string | null; timezone?: string | null };
        install?: { start?: string | null; end?: string | null; timezone?: string | null };
        reboot?: { start?: string | null; end?: string | null; timezone?: string | null };
      };
    } | null;
    const downloadInstallWindow = config?.windows?.download ?? config?.windows?.install;
    const siteCustomerId = policy.siteId ? sites.find((site) => site.id === policy.siteId)?.customerId : null;
    policyName = policy.name;
    policyEnabled = policy.enabled;
    policyPriority = policy.priority ?? (policy.isDefault ? DEFAULT_POLICY_PRIORITY : CUSTOM_POLICY_DEFAULT_PRIORITY);
    policyScopeType = policy.scopeType;
    policyTargetOsFamily = policy.targetOsFamily ?? 'all';
    policyCustomerId = policy.customerId ?? (policy.scopeType === 'customer' ? policy.scopeKey : siteCustomerId ?? '');
    policySiteId = policy.siteId ?? (policy.scopeType === 'site' ? policy.scopeKey : '');
    policyAgentId = policy.agentId ?? (policy.scopeType === 'device' ? policy.scopeKey : '');
    policyDeferralDays = policy.deferralDays ?? 0;
    policyManagedMode = policy.managedMode ?? policy.nativeWindowsUpdateControl ?? true;
    policyScanStart = config?.windows?.scan?.start ?? '';
    policyScanEnd = config?.windows?.scan?.end ?? '';
    policyDownloadInstallStart = downloadInstallWindow?.start ?? policy.maintenanceWindowStart ?? '';
    policyDownloadInstallEnd = downloadInstallWindow?.end ?? policy.maintenanceWindowEnd ?? '';
    policyRebootStart = config?.windows?.reboot?.start ?? '';
    policyRebootEnd = config?.windows?.reboot?.end ?? '';
    policyTimezone =
      downloadInstallWindow?.timezone ??
      config?.windows?.scan?.timezone ??
      config?.windows?.reboot?.timezone ??
      policy.maintenanceWindowTimezone ??
      'UTC';
    hydratedPolicyId = selectedPolicyId;
  }

  function policyScopePayload() {
    if (policyScopeType === 'organization') return { scopeType: 'organization' as const };
    if (policyScopeType === 'customer' && policyCustomerId) {
      return { scopeType: 'customer' as const, customerId: policyCustomerId, scopeId: policyCustomerId };
    }
    if (policyScopeType === 'site' && policySiteId) {
      return { scopeType: 'site' as const, siteId: policySiteId, scopeId: policySiteId };
    }
    if (policyScopeType === 'device' && policyAgentId) {
      return { scopeType: 'device' as const, agentId: policyAgentId, scopeId: policyAgentId };
    }
    return null;
  }

  async function savePolicy() {
    const priority = Number(policyPriority);
    const deferralDays = Number(policyDeferralDays);
    if (!policyName.trim()) {
      toast({ title: 'Policy name is required', variant: 'destructive' });
      return;
    }
    if (!Number.isInteger(deferralDays) || deferralDays < 0 || deferralDays > 365) {
      toast({ title: 'Deferral days must be between 0 and 365', variant: 'destructive' });
      return;
    }
    if (!selectedPolicyIsDefault && (!Number.isInteger(priority) || priority < 0 || priority >= DEFAULT_POLICY_PRIORITY)) {
      toast({ title: 'Priority must be between 0 and 9999', variant: 'destructive' });
      return;
    }
    const scope = policyScopePayload();
    if (!selectedPolicyIsDefault && !scope) {
      toast({ title: 'Select a policy target', variant: 'destructive' });
      return;
    }

    const existingConfig = (selectedPolicy?.policyConfig ?? {}) as Record<string, unknown>;
    const effectiveManagedMode = policyManagedMode && !policyManagedModeUnsupported;
    const nativeWindowsUpdateControl =
      effectiveManagedMode && targetUsesWindowsNativeControl(policyTargetOsFamily);
    const policyConfig = {
      ...existingConfig,
      managedMode: effectiveManagedMode,
      nativeWindowsUpdateControl,
      windows: {
        ...((existingConfig.windows as Record<string, unknown> | undefined) ?? {}),
        scan: { enabled: true, start: policyScanStart || null, end: policyScanEnd || null, timezone: policyTimezone },
        download: { enabled: true, start: policyDownloadInstallStart || null, end: policyDownloadInstallEnd || null, timezone: policyTimezone },
        install: { enabled: true, start: policyDownloadInstallStart || null, end: policyDownloadInstallEnd || null, timezone: policyTimezone },
        reboot: { enabled: true, start: policyRebootStart || null, end: policyRebootEnd || null, timezone: policyTimezone }
      }
    };

    try {
      policySaving = true;
      const common = {
        name: policyName.trim(),
        targetOsFamily: policyTargetOsFamily,
        deferralDays,
        managedMode: effectiveManagedMode,
        nativeWindowsUpdateControl,
        maintenanceWindowStart: policyDownloadInstallStart || null,
        maintenanceWindowEnd: policyDownloadInstallEnd || null,
        maintenanceWindowTimezone: policyTimezone,
        policyConfig,
        enabled: selectedPolicyIsDefault ? true : policyEnabled
      };

      if (isCreatingPolicy) {
        if (!scope) return;
        const saved = await patchApi.savePolicy({
          ...scope,
          ...common,
          priority,
          approvalMode: 'auto_approve_all',
          rebootBehavior: 'allow'
        });
        selectedPolicyId = saved.id;
      } else if (selectedPolicy) {
        await patchApi.updatePolicy(selectedPolicy.id, {
          ...(selectedPolicy.isDefault || !scope ? {} : scope),
          ...common,
          priority: selectedPolicy.isDefault ? undefined : priority
        });
      }
      toast({ title: 'Patch policy saved' });
      await fetchData(true);
    } catch (err) {
      toast({
        title: 'Policy save failed',
        description: err instanceof Error ? err.message : 'Request failed',
        variant: 'destructive'
      });
    } finally {
      policySaving = false;
    }
  }

  async function deletePolicy() {
    if (!selectedPolicy || selectedPolicy.isDefault || !confirm(`Delete ${selectedPolicy.name}?`)) return;
    try {
      policySaving = true;
      await patchApi.deletePolicy(selectedPolicy.id);
      toast({ title: 'Patch policy deleted' });
      selectedPolicyId = '';
      hydratedPolicyId = '';
      await fetchData(true);
    } catch (err) {
      toast({
        title: 'Policy delete failed',
        description: err instanceof Error ? err.message : 'Request failed',
        variant: 'destructive'
      });
    } finally {
      policySaving = false;
    }
  }

  function exportCsv<T extends object>(filename: string, rows: T[]) {
    const keys = Object.keys(rows[0] ?? {});
    if (keys.length === 0) return;
    const escape = (value: unknown) => `"${String(value ?? '').replace(/"/g, '""')}"`;
    const csv = [
      keys.join(','),
      ...rows.map((row) => {
        const record = row as Record<string, unknown>;
        return keys.map((key) => escape(record[key])).join(',');
      })
    ].join('\n');
    const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    link.click();
    URL.revokeObjectURL(url);
  }

  function progressLabel(item: PatchProgressUpdate | undefined, fallback: string) {
    if (!item) return fallback;
    const percent = Math.round(Math.max(0, Math.min(100, item.overallPercent ?? item.phasePercent ?? 0)));
    const current = item.currentUpdate?.title;
    return current && item.phase === 'installing' ? `${fallback} ${percent}% - ${current}` : `${fallback} ${percent}%`;
  }

  onMount(() => {
    void fetchData();
    progressTimer = setInterval(() => void pollPatchProgress(), PATCH_PROGRESS_POLL_MS);
    refreshTimer = setInterval(() => void checkAuditAlerts(), 20_000);
    return () => {
      if (progressTimer) clearInterval(progressTimer);
      if (refreshTimer) clearInterval(refreshTimer);
    };
  });

  onDestroy(() => topbarConfig.set(null));
</script>

<svelte:head>
  <title>Patch Management | Talos</title>
</svelte:head>

<div class="patch-page">
  {#if error}
    <div class="error-band">{error}</div>
  {/if}

  <section class="metrics" aria-label="Patch totals">
    <div><span>{totals.devices}</span><small>Devices</small></div>
    <div><span>{totals.managed}</span><small>Managed</small></div>
    <div><span>{totals.pending}</span><small>Pending updates</small></div>
    <div><span>{totals.downloaded}</span><small>Downloaded/staged updates</small></div>
    <div><span>{totals.failed}</span><small>Failed updates</small></div>
    <div><span>{totals.reboot}</span><small>Need reboot</small></div>
  </section>

  <nav class="tabs" aria-label="Patch views">
    {#each tabs as tab}
      <button class:active={activeTab === tab.value} on:click={() => (activeTab = tab.value)}>{tab.label}</button>
    {/each}
  </nav>

  {#if loading}
    <div class="empty-state">Loading patch state...</div>
  {:else if featureUpgradeWorkspaceOpen}
    <section class="upgrade-hero">
      <div>
        <button class="back-link" type="button" on:click={closeFeatureUpgradeWorkspace}>Back to Patch Management</button>
        <span class="section-kicker">Feature upgrade center</span>
        <h2>ISO-based Windows upgrade planning</h2>
        <p>Preflight devices, assign trusted media, stage payloads, and schedule disruptive OS upgrades from a standalone workbench.</p>
      </div>
      <div class="upgrade-hero-actions">
        <Button variant="secondary" size="sm" on:click={() => runFeatureUpgradeAction('upload_media')}>
          <Plus size={15} /> Add ISO
        </Button>
        <Button size="sm" on:click={() => runFeatureUpgradeAction('preflight')}>
          <ClipboardCheck size={15} /> Start preflight
        </Button>
      </div>
    </section>
    <section class="upgrade-summary" aria-label="Feature upgrade totals">
      <div>
        <Server size={18} />
        <strong>{featureUpgradeTotals.windows}</strong>
        <span>Windows devices</span>
      </div>
      <div>
        <CheckCircle2 size={18} />
        <strong>{featureUpgradeTotals.eligible}</strong>
        <span>Eligible devices</span>
      </div>
      <div>
        <ClipboardCheck size={18} />
        <strong>{featureUpgradeTotals.review}</strong>
        <span>Need preflight review</span>
      </div>
      <div>
        <ShieldAlert size={18} />
        <strong>{featureUpgradeTotals.blocked}</strong>
        <span>Blocked</span>
      </div>
      <div>
        <Disc3 size={18} />
        <strong>{featureUpgradeTotals.mediaMissing}</strong>
        <span>Missing ISO assignment</span>
      </div>
    </section>

    <section class="feature-upgrade-layout">
      <div class="feature-upgrade-main">
        <section class="toolbar">
          <label class="search"><Search size={16} /> <input bind:value={featureUpgradeSearch} placeholder="Search device, OS, target, customer, blocker" /></label>
          <label>Target
            <select bind:value={featureUpgradeTargetFilter}>
              <option value="all">All targets</option>
              {#each featureUpgradeTargets as target}<option value={target}>{target}</option>{/each}
            </select>
          </label>
          <label>Readiness
            <select bind:value={featureUpgradeReadinessFilter}>
              <option value="all">All readiness</option>
              <option value="eligible">Eligible</option>
              <option value="needs_review">Needs review</option>
              <option value="blocked">Blocked</option>
              <option value="unknown">Unknown</option>
            </select>
          </label>
          <label>Media
            <select bind:value={featureUpgradeMediaFilter}>
              <option value="all">All media</option>
              <option value="assigned">Assigned</option>
              <option value="staged">Staged</option>
              <option value="missing">Missing</option>
            </select>
          </label>
        </section>
        <section class="secondary-controls">
          <span>{selectedFeatureUpgradeCount} selected</span>
          <Button variant="secondary" size="sm" on:click={() => runFeatureUpgradeAction('preflight')}>
            <ClipboardCheck size={15} /> Run preflight
          </Button>
          <Button variant="secondary" size="sm" on:click={() => runFeatureUpgradeAction('stage_iso')}>
            <HardDriveDownload size={15} /> Stage ISO
          </Button>
          <Button size="sm" on:click={() => runFeatureUpgradeAction('start_upgrade')}>
            <PlayCircle size={15} /> Start upgrade
          </Button>
          <Button variant="secondary" size="sm" on:click={() => runFeatureUpgradeAction('schedule')}>
            <CalendarClock size={15} /> Schedule
          </Button>
          <Button variant="secondary" size="sm" on:click={() => exportCsv('feature-upgrades.csv', visibleFeatureUpgradeRows)}>
            <FileDown size={15} /> Export CSV
          </Button>
          <Button variant="secondary" size="sm" on:click={() => {
            featureUpgradeSearch = '';
            featureUpgradeTargetFilter = 'all';
            featureUpgradeReadinessFilter = 'all';
            featureUpgradeMediaFilter = 'all';
          }}>Clear filters</Button>
          <span>{visibleFeatureUpgradeRows.length} of {featureUpgradeRows.length} Windows devices</span>
        </section>

        <div class="table-wrap upgrade-table">
          <table>
            <thead>
              <tr>
                <th class="check"><input type="checkbox" checked={allVisibleFeatureUpgradeSelected} on:change={toggleAllVisibleFeatureUpgrades} aria-label="Select visible feature upgrade devices" /></th>
                <th>Device</th>
                <th>Upgrade path</th>
                <th>Readiness</th>
                <th>ISO media</th>
                <th>Phase</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each visibleFeatureUpgradeRows as row (row.agentId)}
                <tr>
                  <td class="check"><input type="checkbox" checked={selectedFeatureUpgradeAgentIds.has(row.agentId)} on:change={() => toggleFeatureUpgradeDevice(row.agentId)} aria-label={`Select ${row.hostname}`} /></td>
                  <td>
                    <button class="device-link" type="button" on:click={() => goto(`/dashboard/rmm/${row.agentId}`)}>
                      <strong>{row.hostname}</strong>
                    </button>
                    <small>{row.customerName ?? 'Unassigned'}{row.siteName ? ` / ${row.siteName}` : ''}</small>
                    <small>{row.deviceType} / {row.patchRing}</small>
                  </td>
                  <td>
                    <strong>{row.currentVersion} -> {row.targetVersion}</strong>
                    <small>{row.os}</small>
                  </td>
                  <td>
                    <span class:status-eligible={row.readiness === 'eligible'} class:status-review={row.readiness === 'needs_review'} class:status-blocked={row.readiness === 'blocked'} class="status-pill">{row.readinessLabel}</span>
                    {#if row.blockers.length > 0}
                      <small class="blocked-text">{row.blockers.join(', ')}</small>
                    {:else if row.warnings.length > 0}
                      <small>{row.warnings.join(', ')}</small>
                    {:else}
                      <small>No blockers</small>
                    {/if}
                  </td>
                  <td>
                    <strong>{row.mediaStatus === 'missing' ? 'Missing' : row.mediaStatus}</strong>
                    <small>{row.mediaLabel}</small>
                  </td>
                  <td>
                    <strong>{row.phase}</strong>
                    <small>{formatDate(row.lastPreflightAt, 'No preflight run')}</small>
                  </td>
                  <td class="actions">
                    <button title="Run preflight" on:click|stopPropagation={() => runFeatureUpgradeAction('preflight', row.agentId)}><ClipboardCheck size={15} /></button>
                    <button title="Stage ISO" on:click|stopPropagation={() => runFeatureUpgradeAction('stage_iso', row.agentId)}><HardDriveDownload size={15} /></button>
                    <button title="Start upgrade" on:click|stopPropagation={() => runFeatureUpgradeAction('start_upgrade', row.agentId)}><PlayCircle size={15} /></button>
                  </td>
                </tr>
              {:else}
                <tr><td colspan="7">No Windows devices match the current feature upgrade filters.</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
      </div>

      <aside class="feature-upgrade-side">
        <section>
          <div class="policy-list-header">
            <h2>ISO media library</h2>
            <Button size="sm" variant="secondary" on:click={() => runFeatureUpgradeAction('upload_media')}>
              <Plus size={15} /> Add ISO
            </Button>
          </div>
          <div class="media-card">
            <Disc3 size={18} />
            <div>
              <strong>Windows 11 25H2 Enterprise x64</strong>
              <small>Media record / checksum pending</small>
            </div>
            <button title="Assign ISO" on:click={() => runFeatureUpgradeAction('assign_media')}><Plus size={15} /></button>
          </div>
          <div class="media-card">
            <Disc3 size={18} />
            <div>
              <strong>Windows Server 2025 x64</strong>
              <small>Media record / checksum pending</small>
            </div>
            <button title="Assign ISO" on:click={() => runFeatureUpgradeAction('assign_media')}><Plus size={15} /></button>
          </div>
        </section>

        <section>
          <h2>Preflight checklist</h2>
          <ul class="checklist">
            <li><CheckCircle2 size={15} /> Supported source and target upgrade path</li>
            <li><CheckCircle2 size={15} /> Edition and language compatibility</li>
            <li><CheckCircle2 size={15} /> No pending reboot from current patch/snapshot state</li>
            <li><CheckCircle2 size={15} /> System drive free space from fresh snapshot</li>
            <li><CheckCircle2 size={15} /> BitLocker state from fresh snapshot</li>
          </ul>
        </section>
      </aside>
    </section>
  {:else if activeTab === 'devices'}
    <section class="toolbar">
      <label class="sr-only" for="patch-customer-filter">Filter by customer</label>
      <div class="relative" data-customer-filter-root>
        <input
          bind:this={customerFilterInputElement}
          id="patch-customer-filter"
          class="glass-input h-9 min-w-[12rem] w-72 max-w-full rounded-lg px-3 py-2 text-sm"
          bind:value={customerFilterInput}
          placeholder="Filter by customer"
          type="text"
          autocomplete="off"
          role="combobox"
          aria-haspopup="listbox"
          aria-expanded={customerFilterOpen}
          aria-controls="patch-customer-filter-listbox"
          on:focus={() => {
            customerFilterOpen = true;
            if (customerFilterInput === 'All Customers') customerFilterInput = '';
          }}
          on:blur={() => {
            setTimeout(() => {
              customerFilterOpen = false;
              if (!customerFilterInput.trim()) customerFilterInput = 'All Customers';
            }, 150);
          }}
          on:keydown={(event) => {
            if (event.key === 'Escape') {
              customerFilterOpen = false;
              customerFilterInputElement?.blur();
            }
          }}
        />
        {#if customerFilterOpen}
          <ul id="patch-customer-filter-listbox" role="listbox" class="filter-dropdown">
            {#each filteredCustomerOptions as option}
              <li
                role="option"
                tabindex="-1"
                aria-selected={option === customerFilterInput}
                class:filter-option-active={option === customerFilterInput}
                on:mousedown={(event) => {
                  event.preventDefault();
                  selectCustomerFilter(option);
                }}
              >
                {option}
              </li>
            {/each}
            {#if filteredCustomerOptions.length === 0}
              <li class="px-3 py-2 text-sm text-muted-foreground" role="presentation">No matches</li>
            {/if}
          </ul>
        {/if}
      </div>
      <span>{selectedCount} selected</span>
      <Button variant="secondary" size="sm" on:click={() => runDeviceAction('scan_now')} disabled={actionLoading}>
        <Search size={15} /> Scan
      </Button>
      <Button size="sm" on:click={() => runDeviceAction('download_now')} disabled={actionLoading}>
        <Zap size={15} /> Download & Install
      </Button>
      <Button variant="secondary" size="sm" on:click={() => runDeviceAction('reboot_now')} disabled={actionLoading}>
        <RefreshCw size={15} /> Reboot
      </Button>
    </section>

    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th class="check"><input type="checkbox" checked={allVisibleSelected} on:change={toggleAllVisible} aria-label="Select visible devices" /></th>
            <th>Device</th>
            <th>Pending</th>
            <th>Downloaded/staged</th>
            <th>Failed</th>
            <th>Deferred</th>
            <th>Blocked</th>
            <th>Last scan</th>
          </tr>
        </thead>
        <tbody>
          {#each visibleDevices as device (device.agentId)}
            <tr>
              <td class="check"><input type="checkbox" checked={selectedAgentIds.has(device.agentId)} on:change={() => toggleDevice(device.agentId)} aria-label={`Select ${device.hostname}`} /></td>
              <td>
                <div class="device-name-row">
                  <button class="device-link" type="button" on:click={() => goto(`/dashboard/rmm/${device.agentId}`)}>
                    <strong>{device.hostname}</strong>
                  </button>
                  {#if activeInstallProgressByAgentId.has(device.agentId)}
                    <span class="patch-sync-icon" title="Patch download and install in progress"><RefreshCw size={14} /></span>
                  {:else if activeScanProgressByAgentId.has(device.agentId)}
                    <span class="patch-sync-icon" title="Patch scan in progress"><RefreshCw size={14} /></span>
                  {:else if completedInstallProgressByAgentId.has(device.agentId)}
                    <span class="patch-installed-icon" title="Patches installed"><CheckCircle2 size={14} /></span>
                  {:else if device.rebootRequired || device.rebootPendingUpdates > 0}
                    <span class="patch-reboot-icon" title="Reboot required"><RefreshCw size={14} /></span>
                  {/if}
                  {#if macosUpdateAccountNeedsAttention(device)}
                    <span class="patch-warning-icon" title={macosUpdateAccountTitle(device)}><ShieldAlert size={14} /></span>
                  {/if}
                </div>
                <small>{device.os}</small>
                {#if macosUpdateAccountNeedsAttention(device)}
                  <small class="progress-line">{!device.macosUpdateAccount?.status ? 'Software Updates readiness unknown' : device.macosUpdateAccount.status === 'needsEnrollment' ? 'Software Updates enrollment needed' : 'Software Updates account not ready'}</small>
                {/if}
              </td>
              <td>
                <strong>{device.pendingUpdates}</strong>
                {#if activeInstallProgressByAgentId.has(device.agentId)}
                  <small class="progress-line">{progressLabel(activeInstallProgressByAgentId.get(device.agentId), 'Downloading and installing patches')}</small>
                {:else if activeScanProgressByAgentId.has(device.agentId)}
                  <small class="progress-line">{progressLabel(activeScanProgressByAgentId.get(device.agentId), 'Scanning patches')}</small>
                {:else if completedInstallProgressByAgentId.has(device.agentId)}
                  <small class="installed-line">Installed {completedInstallProgressByAgentId.get(device.agentId)?.summary?.installed ?? 0} updates</small>
                {/if}
              </td>
              <td>{device.downloadedUpdates}</td>
              <td>{device.failedUpdates}</td>
              <td>{device.deferredUpdates}</td>
              <td>{device.blockedUpdates}</td>
              <td>{formatDate(device.lastScanAt)}</td>
            </tr>
          {:else}
            <tr><td colspan="8">No devices match the current filters.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if activeTab === 'updates'}
    <section class="toolbar">
      <label class="search"><Search size={16} /> <input bind:value={updateFilter} placeholder="Search KB, title, category, customer, site, OS" /></label>
      <label>Device <input bind:value={updateDeviceFilter} placeholder="Hostname or agent ID" /></label>
      <label>Category <select bind:value={updateCategoryFilter}><option value="all">All categories</option>{#each updateCategories as category}<option value={category}>{formatCategory(category)}</option>{/each}</select></label>
      <label>OS <select bind:value={updateOsFilter}><option value="all">All OS</option>{#each updateOsFamilies as os}<option value={os}>{os}</option>{/each}</select></label>
      <Button variant="secondary" size="sm" on:click={() => exportCsv('patch-updates.csv', visibleUpdates)}>
        <FileDown size={15} /> Export CSV
      </Button>
    </section>
    <section class="secondary-controls">
      <label>Customer <select bind:value={updateCustomerFilter}><option value="all">All customers</option>{#each updateCustomers as customer}<option value={customer}>{customer}</option>{/each}</select></label>
      <label>Site <select bind:value={updateSiteFilter}><option value="all">All sites</option>{#each updateSites as site}<option value={site}>{site}</option>{/each}</select></label>
      <label>State <select bind:value={updateStateFilter}><option value="all">All states</option><option value="actionable">Actionable</option><option value="detected">Detected</option><option value="downloaded">Downloaded/staged</option><option value="installed">Installed</option><option value="failed">Failed</option><option value="blocked">Blocked</option><option value="deferred">Deferred</option><option value="superseded">Superseded</option></select></label>
      <label>Source <select bind:value={updateSourceFilter}><option value="all">All sources</option>{#each updateSources as source}<option value={source}>{source}</option>{/each}</select></label>
      <label>Defer until <input bind:value={deferUntil} type="datetime-local" /></label>
      <Button variant="secondary" size="sm" on:click={() => {
        updateFilter = '';
        updateDeviceFilter = '';
        updateCategoryFilter = 'all';
        updateOsFilter = 'all';
        updateCustomerFilter = 'all';
        updateSiteFilter = 'all';
        updateStateFilter = 'all';
        updateSourceFilter = 'all';
      }}>Clear filters</Button>
      <span>{visibleUpdates.length} of {updates.length} updates</span>
    </section>
    <div class="table-wrap">
      <table>
        <thead><tr><th>Update</th><th>Category</th><th>Target</th><th>Release</th><th>Affected</th><th>State</th><th>Actions</th></tr></thead>
        <tbody>
          {#each visibleUpdates as update (update.updateKey)}
            <tr class:selected={selectedUpdateKey === update.updateKey} on:click={() => (selectedUpdateKey = update.updateKey)}>
              <td><strong>{update.kbArticle ?? 'No KB'}</strong><small>{update.title}</small></td>
              <td><span class="pill">{formatCategory(update.category)}</span></td>
              <td>
                {#each (update.osFamilies ?? []).slice(0, 3) as os}<span class="pill">{os}</span>{/each}
                <small>{(update.customerNames ?? []).slice(0, 2).join(', ') || 'All customers'}{(update.siteNames ?? []).length > 0 ? ` / ${(update.siteNames ?? []).slice(0, 2).join(', ')}` : ''}</small>
                <small>{(update.associatedHostnames ?? update.affectedHostnames ?? []).slice(0, 2).join(', ') || 'No associated devices'}</small>
              </td>
              <td>{formatDate(update.releaseDate, 'First detected')}</td>
              <td><strong>{update.affectedDevices}</strong>{#if update.associatedDevices > update.affectedDevices}<small>{update.associatedDevices} associated</small>{/if}</td>
              <td><small>{update.installedDevices} installed, {update.downloadedDevices} downloaded/staged, {update.failedDevices} failed, {update.blockedDevices} blocked, {update.deferredDevices} deferred</small></td>
              <td class="actions">
                <button title="Approve" on:click|stopPropagation={() => runUpdateAction('approve_update', update)}><CheckCircle2 size={15} /></button>
                <button title="Emergency approve" on:click|stopPropagation={() => runUpdateAction('emergency_approve', update)}><ShieldAlert size={15} /></button>
                <button title="Block" on:click|stopPropagation={() => runUpdateAction('block_update', update)}><Ban size={15} /></button>
                <button title="Defer" on:click|stopPropagation={() => runUpdateAction('defer_update', update)}><CalendarClock size={15} /></button>
              </td>
            </tr>
          {:else}
            <tr><td colspan="7">No updates match the current filters.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if activeTab === 'policies'}
    <section class="policy-grid">
      <div class="policy-list">
        <div class="policy-list-header">
          <h2>Policies</h2>
          <Button size="sm" on:click={newPolicy}><Plus size={15} /> New</Button>
        </div>
        {#each policies as policy (policy.id)}
          <button class="policy-item" class:active={!isCreatingPolicy && selectedPolicy?.id === policy.id} on:click={() => (selectedPolicyId = policy.id)}>
            <strong>{policy.name}</strong>
            <small>{policy.isDefault ? 'default' : policy.scopeType} / {formatTargetOsFamily(policy.targetOsFamily)} / priority {policy.priority}</small>
          </button>
        {/each}
        {#if isCreatingPolicy}
          <button class="policy-item active" on:click={() => (selectedPolicyId = NEW_POLICY_ID)}>
            <strong>New patch policy</strong>
            <small>{policyScopeType} / {formatTargetOsFamily(policyTargetOsFamily)} / priority {policyPriority}</small>
          </button>
        {/if}
      </div>

      {#if selectedPolicy || isCreatingPolicy}
        <form class="policy-editor" on:submit|preventDefault={savePolicy}>
          <div class="form-row wide">
            <label>Name <input bind:value={policyName} /></label>
            <label>Enabled <input type="checkbox" bind:checked={policyEnabled} disabled={selectedPolicyIsDefault} /></label>
            <label>
              <span
                class="label-heading"
                title="Lower numbers take precedence. If priorities tie, the more specific policy wins, then the most recently updated policy."
              >Priority <Info size={15} aria-hidden="true" /></span>
              <input bind:value={policyPriority} type="number" min="0" max="9999" disabled={selectedPolicyIsDefault} />
            </label>
            <label>Target OS
              <select bind:value={policyTargetOsFamily} disabled={selectedPolicyIsDefault}>
                <option value="all">All devices</option>
                <option value="windows">Windows</option>
                <option value="linux">Linux</option>
                <option value="macos">macOS</option>
              </select>
            </label>
          </div>
          <div class="form-row wide">
            <label>Scope
              <select bind:value={policyScopeType} disabled={selectedPolicyIsDefault}>
                <option value="organization">Organization</option>
                <option value="customer">Customer</option>
                <option value="site">Site</option>
                <option value="device">Device</option>
              </select>
            </label>
            {#if policyScopeType === 'customer'}
              <label>Customer <select bind:value={policyCustomerId} disabled={selectedPolicyIsDefault}><option value="">Select customer</option>{#each customers.filter((customer) => !customer.isUnassigned) as customer}<option value={customer.id}>{customer.name}</option>{/each}</select></label>
            {:else if policyScopeType === 'site'}
              <label>Customer <select bind:value={policyCustomerId} disabled={selectedPolicyIsDefault} on:change={() => (policySiteId = '')}><option value="">All customers</option>{#each customers.filter((customer) => !customer.isUnassigned) as customer}<option value={customer.id}>{customer.name}</option>{/each}</select></label>
              <label>Site <select bind:value={policySiteId} disabled={selectedPolicyIsDefault}><option value="">Select site</option>{#each filteredPolicySites as site}<option value={site.id}>{site.name}</option>{/each}</select></label>
            {:else if policyScopeType === 'device'}
              <label>Device <select bind:value={policyAgentId} disabled={selectedPolicyIsDefault}><option value="">Select device</option>{#each sortedDevices as device}<option value={device.agentId}>{device.hostname}</option>{/each}</select></label>
            {/if}
          </div>
          <div class="form-row">
            <label class:disabled-field={policyManagedModeUnsupported}>
              <span class="label-heading" title={policyManagedModeTooltip}>Managed mode <Info size={15} aria-hidden="true" /></span>
              <input
                type="checkbox"
                bind:checked={policyManagedMode}
                disabled={policyManagedModeUnsupported}
                title={policyManagedModeTooltip}
              />
            </label>
            <label>Deferral days <input bind:value={policyDeferralDays} type="number" min="0" max="365" /></label>
          </div>
          <div class="form-row">
            <label>Scan start <input bind:value={policyScanStart} type="time" /></label>
            <label>Scan end <input bind:value={policyScanEnd} type="time" /></label>
            <label>Download and install start <input bind:value={policyDownloadInstallStart} type="time" /></label>
            <label>Download and install end <input bind:value={policyDownloadInstallEnd} type="time" /></label>
            <label>Reboot start <input bind:value={policyRebootStart} type="time" /></label>
            <label>Reboot end <input bind:value={policyRebootEnd} type="time" /></label>
            <label>Timezone <input bind:value={policyTimezone} /></label>
          </div>
          <div class="policy-actions">
            <Button type="submit" disabled={policySaving}><Save size={16} /> Save policy</Button>
            {#if selectedPolicy && !selectedPolicy.isDefault}
              <Button type="button" variant="destructive" disabled={policySaving} on:click={deletePolicy}><Trash2 size={16} /> Delete</Button>
            {/if}
          </div>
        </form>
      {/if}
    </section>
  {:else if activeTab === 'audit'}
    <section class="toolbar">
      <label class="search"><Search size={16} /> <input bind:value={auditSearch} placeholder="Search audit by device, actor, action, reason" /></label>
      <label>Action <select bind:value={auditActionFilter}><option value="all">All actions</option>{#each auditActions as action}<option value={action}>{action}</option>{/each}</select></label>
      <label>Decision <select bind:value={auditDecisionFilter}><option value="all">All decisions</option>{#each auditDecisions as decision}<option value={decision}>{decision}</option>{/each}</select></label>
      <label>Actor <select bind:value={auditActorFilter}><option value="all">All actors</option><option value="user">All users</option><option value="system">System</option><option value="agent">Talos agent</option></select></label>
      {#if auditActorFilter === 'user'}
        <label>User <input bind:value={auditUserFilter} list="audit-user-options" placeholder="Search/select user" /><datalist id="audit-user-options">{#each auditUsers as user}<option value={user}></option>{/each}</datalist></label>
      {/if}
      <Button variant="secondary" size="sm" on:click={() => exportCsv('patch-audit.csv', visibleAuditRows)}>
        <FileDown size={15} /> Export CSV
      </Button>
    </section>
    <section class="secondary-controls">
      <label>From <input bind:value={auditFrom} type="datetime-local" /></label>
      <label>To <input bind:value={auditTo} type="datetime-local" /></label>
      <Button variant="secondary" size="sm" on:click={() => {
        auditSearch = '';
        auditActionFilter = 'all';
        auditDecisionFilter = 'all';
        auditActorFilter = 'all';
        auditUserFilter = '';
        auditFrom = '';
        auditTo = '';
      }}>Clear filters</Button>
      <span>{visibleAuditRows.length} of {decisions.length} audit rows</span>
    </section>
    <section class="audit-overrides">
      <div class="policy-list-header">
        <h2>Active overrides</h2>
        <span>{activeOverrides.length} active</span>
      </div>
      <div class="table-wrap compact">
        <table>
          <thead><tr><th>Action</th><th>Target</th><th>Status</th><th>Operation</th><th>Update</th><th>Requested by</th><th>Expires</th></tr></thead>
          <tbody>
            {#each activeOverrides as override (override.id)}
              <tr>
                <td><strong>{override.action}</strong><small>{formatDate(override.createdAt, 'Unknown time')}</small></td>
                <td><strong>{overrideDeviceLabel(override)}</strong><small>{override.scopeType}:{override.scopeKey}</small></td>
                <td><strong>{overrideStatusLabel(override)}</strong><small>{formatDate(override.latestActionUpdatedAt, 'No worker status')}</small></td>
                <td>{override.operationId ?? 'Not started'}</td>
                <td>{overrideUpdateLabel(override)}</td>
                <td>{overrideRequesterLabel(override)}</td>
                <td>{formatDate(override.expiresAt, 'No expiry')}</td>
              </tr>
            {:else}
              <tr><td colspan="7">No active overrides.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
    <div class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Device</th><th>Source</th><th>Action</th><th>Status</th><th>Decision</th><th>Reason</th></tr></thead>
        <tbody>
          {#each visibleAuditRows as row (row.id)}
            <tr>
              <td>{formatDate(row.decidedAt)}</td>
              <td>{auditDeviceLabel(row.agentId)}</td>
              <td><strong>{actorLabel(row.actorType, row.actorEmail)}</strong><small>{row.actorEmail ?? row.actorType}</small></td>
              <td><strong>{row.action}</strong><small>{row.operationId}</small></td>
              <td><strong>{row.actionStatus ?? 'recorded'}</strong><small>{row.actionPhase ?? formatDate(row.actionUpdatedAt, '')}</small></td>
              <td>{row.decision}</td>
              <td>{row.reason}</td>
            </tr>
          {:else}
            <tr><td colspan="7">No audit rows match the current filters.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if newAuditRows > 0}
      <aside class="new-alerts-banner" aria-live="polite">
        <div><strong>{newAuditRows} new {newAuditRows === 1 ? 'alert' : 'alerts'} detected</strong><small>The audit view has newer patch decisions. Refresh when ready.</small></div>
        <Button size="sm" on:click={refreshAuditAlerts}><RefreshCw size={15} /> Refresh alerts</Button>
      </aside>
    {/if}
  {/if}
</div>

<style>
  .patch-page {
    display: flex;
    min-height: 100%;
    flex-direction: column;
    gap: 1.25rem;
    padding: 1.75rem;
    color: rgb(221 235 255);
  }

  .error-band,
  .empty-state {
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 1rem;
    background: rgba(255, 255, 255, 0.05);
  }

  .error-band {
    border-color: rgba(255, 99, 99, 0.35);
    color: rgb(255 190 190);
  }

  .metrics {
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    border: 1px solid rgba(105, 135, 180, 0.25);
  }

  .metrics div {
    min-height: 5.25rem;
    padding: 1.05rem;
    border-right: 1px solid rgba(105, 135, 180, 0.22);
    background: rgba(255, 255, 255, 0.025);
  }

  .metrics div:last-child {
    border-right: 0;
  }

  .metrics span {
    display: block;
    font-size: 1.45rem;
    font-weight: 800;
  }

  .metrics small,
  td small {
    display: block;
    margin-top: 0.25rem;
    color: rgb(145 164 198);
    font-size: 0.78rem;
  }

  .tabs {
    display: flex;
    gap: 1.5rem;
    border-bottom: 1px solid rgba(105, 135, 180, 0.25);
  }

  .tabs button {
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: rgb(145 164 198);
    padding: 0.75rem 0;
    font-weight: 700;
  }

  .tabs button.active {
    border-color: rgb(70 200 255);
    color: white;
  }

  .toolbar,
  .secondary-controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
  }

  .toolbar label,
  .secondary-controls label,
  .policy-editor label {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: rgb(145 164 198);
    font-size: 0.82rem;
  }

  input,
  select {
    min-height: 2.25rem;
    border: 1px solid rgba(118, 142, 190, 0.35);
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.055);
    color: rgb(221 235 255);
    padding: 0.45rem 0.65rem;
  }

  input:disabled,
  select:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .search {
    min-width: min(50vw, 50rem);
    flex: 1;
  }

  .search input {
    width: 100%;
  }

  .filter-dropdown {
    position: absolute;
    z-index: 50;
    margin-top: 0.25rem;
    max-height: 16rem;
    width: 100%;
    overflow: auto;
    border: 1px solid rgba(118, 142, 190, 0.35);
    border-radius: 8px;
    background: rgb(14 24 48);
    padding: 0.25rem;
  }

  .filter-dropdown li {
    cursor: pointer;
    border-radius: 6px;
    padding: 0.5rem 0.65rem;
    font-size: 0.88rem;
  }

  .filter-dropdown li:hover,
  .filter-option-active {
    background: rgba(63, 132, 255, 0.25);
  }

  .table-wrap {
    overflow: auto;
    border: 1px solid rgba(105, 135, 180, 0.24);
    background: rgba(255, 255, 255, 0.025);
  }

  table {
    width: 100%;
    min-width: 980px;
    border-collapse: collapse;
  }

  th,
  td {
    border-bottom: 1px solid rgba(105, 135, 180, 0.18);
    padding: 0.9rem 1rem;
    text-align: left;
    vertical-align: top;
    font-size: 0.86rem;
  }

  th {
    color: rgb(113 143 190);
    font-weight: 800;
  }

  tbody tr:hover,
  tbody tr.selected {
    background: rgba(63, 132, 255, 0.08);
  }

  .check {
    width: 2.75rem;
  }

  .device-link {
    border: 0;
    background: transparent;
    color: rgb(148 198 255);
    padding: 0;
    text-align: left;
  }

  .device-name-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .patch-sync-icon :global(svg) {
    animation: spin 1s linear infinite;
  }

  .patch-sync-icon,
  .patch-installed-icon,
  .patch-reboot-icon,
  .patch-warning-icon {
    display: inline-flex;
    color: rgb(125 200 255);
  }

  .patch-installed-icon {
    color: rgb(98 230 170);
  }

  .patch-reboot-icon {
    color: rgb(255 205 92);
  }

  .patch-warning-icon {
    color: rgb(255 170 90);
  }

  .pill {
    display: inline-flex;
    width: fit-content;
    margin: 0 0.25rem 0.25rem 0;
    border: 1px solid rgba(120, 170, 255, 0.35);
    border-radius: 999px;
    padding: 0.18rem 0.5rem;
    background: rgba(80, 135, 255, 0.12);
    color: rgb(180 210 255);
    font-size: 0.75rem;
    font-weight: 700;
  }

  .progress-line,
  .installed-line {
    color: rgb(140 210 255);
  }

  .actions {
    white-space: nowrap;
  }

  .actions button {
    margin-right: 0.35rem;
    border: 1px solid rgba(115, 160, 240, 0.35);
    border-radius: 7px;
    background: rgba(50, 95, 175, 0.22);
    color: rgb(210 228 255);
    padding: 0.45rem;
  }

  .upgrade-hero {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: flex-end;
    border: 1px solid rgba(84, 188, 255, 0.28);
    border-radius: 8px;
    background:
      linear-gradient(135deg, rgba(10, 48, 96, 0.7), rgba(32, 26, 72, 0.55)),
      rgba(255, 255, 255, 0.03);
    padding: 1.2rem;
  }

  .section-kicker {
    display: block;
    margin-bottom: 0.35rem;
    color: rgb(110 200 255);
    font-size: 0.75rem;
    font-weight: 800;
    text-transform: uppercase;
  }

  .upgrade-hero h2 {
    margin: 0;
    font-size: 1.35rem;
  }

  .upgrade-hero p {
    max-width: 48rem;
    margin: 0.35rem 0 0;
    color: rgb(165 185 220);
    font-size: 0.9rem;
  }

  .back-link {
    display: inline-flex;
    margin-bottom: 0.75rem;
    border: 0;
    background: transparent;
    color: rgb(135 205 255);
    padding: 0;
    font-size: 0.82rem;
    font-weight: 800;
  }

  .upgrade-hero-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.65rem;
    justify-content: flex-end;
  }

  .upgrade-summary {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    border: 1px solid rgba(105, 135, 180, 0.24);
    background: rgba(255, 255, 255, 0.025);
  }

  .upgrade-summary div {
    display: grid;
    min-height: 5.4rem;
    grid-template-columns: auto 1fr;
    gap: 0.3rem 0.65rem;
    align-content: center;
    border-right: 1px solid rgba(105, 135, 180, 0.18);
    padding: 1rem;
  }

  .upgrade-summary div:last-child {
    border-right: 0;
  }

  .upgrade-summary :global(svg) {
    color: rgb(118 190 255);
    grid-row: span 2;
  }

  .upgrade-summary strong {
    font-size: 1.25rem;
    line-height: 1;
  }

  .upgrade-summary span {
    color: rgb(145 164 198);
    font-size: 0.78rem;
  }

  .feature-upgrade-layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(18rem, 24rem);
    gap: 1rem;
    align-items: start;
  }

  .feature-upgrade-main,
  .feature-upgrade-side {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 1rem;
  }

  .feature-upgrade-side section {
    border: 1px solid rgba(105, 135, 180, 0.24);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.03);
    padding: 1rem;
  }

  .feature-upgrade-side h2 {
    margin: 0 0 0.75rem;
    font-size: 1.05rem;
  }

  .upgrade-table table {
    min-width: 1180px;
  }

  .status-pill {
    display: inline-flex;
    width: fit-content;
    border: 1px solid rgba(145, 164, 198, 0.35);
    border-radius: 999px;
    padding: 0.18rem 0.55rem;
    background: rgba(145, 164, 198, 0.1);
    color: rgb(215 226 245);
    font-size: 0.75rem;
    font-weight: 800;
  }

  .status-eligible {
    border-color: rgba(98, 230, 170, 0.38);
    background: rgba(31, 128, 92, 0.22);
    color: rgb(160 245 205);
  }

  .status-review {
    border-color: rgba(255, 205, 92, 0.42);
    background: rgba(145, 105, 25, 0.24);
    color: rgb(255 222 145);
  }

  .status-blocked {
    border-color: rgba(255, 99, 99, 0.42);
    background: rgba(150, 45, 58, 0.22);
    color: rgb(255 184 184);
  }

  .blocked-text {
    color: rgb(255 184 184);
  }

  .media-card {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 0.7rem;
    align-items: center;
    border: 1px solid rgba(105, 135, 180, 0.2);
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.035);
    padding: 0.75rem;
  }

  .media-card + .media-card {
    margin-top: 0.65rem;
  }

  .media-card :global(svg) {
    color: rgb(118 190 255);
  }

  .media-card small {
    display: block;
    margin-top: 0.2rem;
    color: rgb(145 164 198);
  }

  .media-card button {
    display: inline-flex;
    border: 1px solid rgba(115, 160, 240, 0.35);
    border-radius: 7px;
    background: rgba(50, 95, 175, 0.22);
    color: rgb(210 228 255);
    padding: 0.45rem;
  }

  .checklist {
    display: grid;
    gap: 0.65rem;
    margin: 0;
    padding: 0;
    list-style: none;
    color: rgb(180 202 235);
    font-size: 0.84rem;
  }

  .checklist li {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .checklist :global(svg) {
    flex: 0 0 auto;
    color: rgb(98 230 170);
  }

  .policy-grid {
    display: grid;
    grid-template-columns: minmax(15rem, 22rem) minmax(0, 1fr);
    gap: 1rem;
  }

  .policy-list,
  .policy-editor,
  .audit-overrides {
    border: 1px solid rgba(105, 135, 180, 0.24);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.03);
    padding: 1rem;
  }

  .policy-list-header,
  .policy-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .policy-list h2,
  .audit-overrides h2 {
    margin: 0;
    font-size: 1.05rem;
  }

  .policy-item {
    display: block;
    width: 100%;
    margin-top: 0.65rem;
    border: 1px solid rgba(105, 135, 180, 0.2);
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.035);
    color: inherit;
    padding: 0.75rem;
    text-align: left;
  }

  .policy-item.active {
    border-color: rgba(75, 170, 255, 0.65);
    background: rgba(50, 110, 210, 0.18);
  }

  .policy-item small,
  .audit-overrides small {
    display: block;
    margin-top: 0.25rem;
    color: rgb(145 164 198);
  }

  .form-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(11rem, 1fr));
    gap: 0.8rem;
    margin-bottom: 0.9rem;
  }

  .form-row.wide {
    grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
  }

  .policy-editor label {
    align-items: stretch;
    flex-direction: column;
  }

  .label-heading {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

  .disabled-field {
    opacity: 0.62;
  }

  .audit-overrides {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .audit-overrides .policy-list-header span {
    color: rgb(145 164 198);
    font-size: 0.85rem;
    font-weight: 700;
  }

  .table-wrap.compact table {
    min-width: 1120px;
  }

  .new-alerts-banner {
    position: sticky;
    bottom: 1rem;
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    border: 1px solid rgba(75, 170, 255, 0.45);
    border-radius: 8px;
    background: rgba(17, 38, 82, 0.96);
    padding: 0.9rem;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  @media (max-width: 1100px) {
    .metrics {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }

    .upgrade-summary {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .upgrade-summary div {
      border-bottom: 1px solid rgba(105, 135, 180, 0.18);
    }

    .feature-upgrade-layout {
      grid-template-columns: 1fr;
    }

    .policy-grid {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 700px) {
    .patch-page {
      padding: 1rem;
    }

    .metrics {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .upgrade-summary {
      grid-template-columns: 1fr;
    }

    .upgrade-summary div {
      border-right: 0;
    }

    .search {
      min-width: 100%;
    }
  }
</style>
