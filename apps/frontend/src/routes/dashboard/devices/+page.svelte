<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import {
    ChevronDown,
    Database,
    Download,
    Folder,
    ListFilter,
    Monitor,
    PlayCircle,
    RefreshCw,
    Save,
    Search,
    Server,
    Terminal,
    Trash2,
    TriangleAlert
  } from 'lucide-svelte';
  import { browser } from '$app/environment';
  import { customerApi, installerApi, rmmApi, siteApi } from '$lib/api';
  import {
    DEFAULT_DEVICE_LIST_FILTERS,
    DEFAULT_DEVICE_LIST_STATE,
    loadDeviceListState,
    normalizeDeviceListState,
    saveDeviceListState
  } from '$lib/deviceListState';
  import { toast } from '$lib/toast';
  import type {
    Customer,
    RmmConnectResponse,
    RmmDevice,
    RmmDeviceListFilters,
    RmmDeviceListQuery,
    RmmDeviceListSortBy,
    RmmDeviceListSortDirection,
    RmmDeviceSavedView,
    RmmViewerConnectionSummary,
    Site
  } from '$lib/types';
  import { detectViewerInstallerPlatform, isDesktopViewerLaunchSupported, launchViewerDeepLink } from '$lib/viewer-launcher';
  import {
    formatViewerSessionKind,
    groupViewerConnectionsByAgent,
    VIEWER_CONNECTION_POLL_MS,
    waitForViewerSessionConnected
  } from '$lib/viewer-session-status';

  const STALE_THRESHOLD_MS = 5 * 60 * 1000; // 5 minutes
  const PATCH_SCAN_COMMAND = 'UsoClient StartScan';
  const emptyFilters = (): RmmDeviceListFilters => ({
    ...DEFAULT_DEVICE_LIST_FILTERS,
    customerId: 'all',
    siteId: 'all'
  });

  let devices: RmmDevice[] = [];
  let totalDevices = 0;
  let currentPage = DEFAULT_DEVICE_LIST_STATE.page;
  let pageSize = DEFAULT_DEVICE_LIST_STATE.pageSize;
  let sortBy: RmmDeviceListSortBy = DEFAULT_DEVICE_LIST_STATE.sortBy;
  let sortDirection: RmmDeviceListSortDirection = DEFAULT_DEVICE_LIST_STATE.sortDirection;
  let filters: RmmDeviceListFilters = emptyFilters();
  let filterPanelOpen = true;
  let deviceFetchSequence = 0;
  let savedViews: RmmDeviceSavedView[] = [];
  let savedViewsLoading = false;
  let selectedSavedViewId = '';
  let saveViewName = '';
  let customers: Customer[] = [];
  let sites: (Site & { deviceCount?: number; customerName?: string })[] = [];
  let loading = true;
  let customersLoading = true;
  let sitesLoading = true;
  let error: string | null = null;
  let lastUpdated: string | null = null;
  let customerFilterInput = 'All Customers';
  let selectedDeviceIds = new Set<string>();
  let selectedDeviceCache = new Map<string, RmmDevice>();
  let bulkCustomerInput = '';
  let bulkSiteInput = '';
  let bulkUpdating = false;
  let bulkSiteUpdating = false;
  let bulkDeleting = false;
  let bulkSnapshotRequesting = false;
  let bulkCommandRunning = false;
  let bulkPatchScanning = false;
  let bulkCommandInput = '';
  let selectAllRef: HTMLInputElement | null = null;
  let unassignedCustomerId = '';
  let filteredDevices: RmmDevice[] = [];
  let visibleDeviceIds: string[] = [];
  let allVisibleSelected = false;
  let someVisibleSelected = false;
  let effectiveCustomerFilter = 'all';
  let bulkCustomerId = '';
  let bulkSiteId: string | null = '';
  let openActionsAgentId: string | null = null;
  let openActionsDevice: RmmDevice | null = null;
  let menuPosition: { top: number; left: number } | null = null;
  let customerFilterOpen = false;
  let customerFilterInputEl: HTMLInputElement | null = null;
  let viewerInstallerDownloading = false;
  let viewerLaunchOverlayOpen = false;
  let viewerLaunchOverlayLabel = 'Viewer';
  let viewerLaunchTimedOut = false;
  let cancelViewerLaunchWait: (() => void) | null = null;
  let viewerConnections = new Map<string, RmmViewerConnectionSummary[]>();
  let viewerConnectionsPollTimer: ReturnType<typeof setInterval> | null = null;

  const MENU_WIDTH_PX = 176; // w-44
  const MENU_HEIGHT_PX = 132;
  const GAP = 6;

  const openActionsMenu = (device: RmmDevice, event: MouseEvent) => {
    // Use the wrapper div's rect for reliable positioning (avoids currentTarget loss across component boundary)
    const wrapper = (document.querySelector(`[data-rmm-actions-root="${device.agentId}"]`) as HTMLElement)
      ?? (event.currentTarget ?? event.target) as HTMLElement;
    const rect = wrapper.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Prefer opening to the left of the trigger so we don't cover the Viewer button
    let left = rect.left - MENU_WIDTH_PX - GAP;
    if (left < GAP) left = rect.right + GAP;
    if (left + MENU_WIDTH_PX > vw - GAP) left = vw - MENU_WIDTH_PX - GAP;
    if (left < GAP) left = GAP;

    const spaceBelow = vh - rect.bottom;
    let top: number;
    if (spaceBelow >= MENU_HEIGHT_PX + GAP) {
      top = rect.bottom + GAP;
    } else {
      const spaceAbove = rect.top;
      top = spaceAbove >= MENU_HEIGHT_PX + GAP
        ? rect.top - MENU_HEIGHT_PX - GAP
        : rect.bottom + GAP;
    }
    if (top < GAP) top = GAP;
    if (top + MENU_HEIGHT_PX > vh - GAP) top = vh - MENU_HEIGHT_PX - GAP;

    menuPosition = { top, left };
    openActionsAgentId = device.agentId;
    openActionsDevice = device;
  };

  const closeActionsMenu = () => {
    openActionsAgentId = null;
    openActionsDevice = null;
    menuPosition = null;
  };

  const formatLastSeen = (value: string) => {
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return 'Unknown';
    return new Date(parsed).toLocaleString();
  };

  const isOnline = (value: string) => {
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return false;
    return Date.now() - parsed < STALE_THRESHOLD_MS;
  };

  const getHealthStatus = (device: RmmDevice) =>
    device.health?.status ?? (isOnline(device.lastSeen) ? 'healthy' : 'offline');

  const formatHealthStatus = (device: RmmDevice) => {
    const status = getHealthStatus(device);
    if (status === 'healthy') return 'Healthy';
    if (status === 'warning') return 'Warning';
    if (status === 'critical') return 'Critical';
    if (status === 'offline') return 'Offline';
    return status ? status.charAt(0).toUpperCase() + status.slice(1) : 'Unknown';
  };

  const healthBadgeClass = (device: RmmDevice) => {
    const status = getHealthStatus(device);
    if (status === 'healthy') return 'aero-badge-online';
    if (status === 'warning') return 'aero-severity-medium health-pill';
    if (status === 'critical') return 'aero-severity-critical health-pill';
    if (status === 'offline') return 'aero-badge-offline';
    return 'aero-badge-neutral';
  };

  const healthReasonPreview = (device: RmmDevice) =>
    device.health?.reasons?.slice(0, 2).map((reason) => reason.summary).join(' · ') || '';

  const currentDeviceListState = (): RmmDeviceListQuery => normalizeDeviceListState({
    page: currentPage,
    pageSize,
    sortBy,
    sortDirection,
    filters
  });

  const persistDeviceListState = () => {
    if (!browser) return;
    saveDeviceListState(localStorage, currentDeviceListState());
  };

  const applyDeviceListState = (state: RmmDeviceListQuery, options: { fetch?: boolean } = {}) => {
    const normalized = normalizeDeviceListState(state);
    currentPage = normalized.page;
    pageSize = normalized.pageSize;
    sortBy = normalized.sortBy;
    sortDirection = normalized.sortDirection;
    filters = { ...emptyFilters(), ...normalized.filters };
    clearSelection();
    persistDeviceListState();
    if (options.fetch ?? true) {
      void fetchDevices();
    }
  };

  const updateFilter = <K extends keyof RmmDeviceListFilters>(key: K, value: RmmDeviceListFilters[K]) => {
    filters = { ...filters, [key]: value };
    currentPage = 1;
    clearSelection();
    persistDeviceListState();
    void fetchDevices();
  };

  const selectedIds = () => Array.from(selectedDeviceIds);

  const rememberSelectedDevices = (deviceIds: string[]) => {
    const pageDevices = new Map(devices.map((device) => [device.agentId, device]));
    const next = new Map(selectedDeviceCache);
    for (const id of deviceIds) {
      const device = pageDevices.get(id);
      if (device) next.set(id, device);
    }
    selectedDeviceCache = next;
  };

  const isLinuxOsText = (value: unknown): boolean => {
    const normalized = typeof value === 'string' ? value.trim().toLowerCase() : '';
    return Boolean(normalized && /\b(linux|debian|ubuntu|fedora|centos|rhel|rocky|alma|suse|arch)\b/.test(normalized));
  };

  const isMacosOsText = (value: unknown): boolean => {
    const normalized = typeof value === 'string' ? value.trim().toLowerCase() : '';
    return Boolean(normalized && /\b(macos|mac os|mac os x|os x|darwin)\b/.test(normalized));
  };

  const isLinuxDevice = (device: RmmDevice): boolean => isLinuxOsText(device.osName) || isLinuxOsText(device.os);
  const isMacosDevice = (device: RmmDevice): boolean => isMacosOsText(device.osName) || isMacosOsText(device.os);

  const fetchDevices = async () => {
    const sequence = ++deviceFetchSequence;
    try {
      loading = true;
      error = null;
      const response = await rmmApi.getDeviceList(currentDeviceListState());
      if (sequence !== deviceFetchSequence) return;
      devices = response.items;
      totalDevices = response.total;
      currentPage = response.page;
      pageSize = response.pageSize;
      sortBy = response.sortBy;
      sortDirection = response.sortDirection;
      filters = { ...emptyFilters(), ...response.filters };
      rememberSelectedDevices(devices.filter((device) => selectedDeviceIds.has(device.agentId)).map((device) => device.agentId));
      lastUpdated = new Date().toLocaleTimeString();
      persistDeviceListState();
    } catch (err) {
      if (sequence !== deviceFetchSequence) return;
      console.error('Failed to fetch devices:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch devices';
    } finally {
      if (sequence === deviceFetchSequence) {
        loading = false;
      }
    }
  };

  const fetchSavedViews = async () => {
    try {
      savedViewsLoading = true;
      savedViews = await rmmApi.getDeviceSavedViews();
    } catch (err) {
      console.error('Failed to fetch saved views:', err);
    } finally {
      savedViewsLoading = false;
    }
  };

  const fetchCustomers = async () => {
    try {
      customersLoading = true;
      customers = await customerApi.getCustomers();
    } catch (err) {
      console.error('Failed to fetch customers:', err);
    } finally {
      customersLoading = false;
    }
  };

  const fetchSites = async () => {
    try {
      sitesLoading = true;
      sites = await siteApi.getSites();
    } catch (err) {
      console.error('Failed to fetch sites:', err);
    } finally {
      sitesLoading = false;
    }
  };

  const fetchViewerConnections = async () => {
    try {
      const connections = await rmmApi.getViewerConnections();
      viewerConnections = groupViewerConnectionsByAgent(connections);
    } catch (err) {
      console.error('Failed to fetch viewer connections:', err);
    }
  };

  const startViewerConnectionsPolling = () => {
    if (viewerConnectionsPollTimer) {
      clearInterval(viewerConnectionsPollTimer);
    }
    viewerConnectionsPollTimer = setInterval(() => {
      if (document.visibilityState !== 'visible') {
        return;
      }
      void fetchViewerConnections();
    }, VIEWER_CONNECTION_POLL_MS);
  };

  const getViewerConnectionsForAgent = (agentId: string) => viewerConnections.get(agentId) ?? [];

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
      await fetchViewerConnections();
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

  const handleConnect = async (device: RmmDevice) => {
    await attemptViewerLaunch(
      'Viewer',
      device.agentId,
      async () => await rmmApi.connectDevice(device.agentId),
      'Failed to open viewer.'
    );
  };

  const handleOpenShell = async (device: RmmDevice) => {
    await attemptViewerLaunch(
      'Shell',
      device.agentId,
      async () => await rmmApi.connectShell(device.agentId),
      'Failed to open shell.'
    );
  };

  const handleOpenFileTransfer = async (device: RmmDevice) => {
    await attemptViewerLaunch(
      'File Transfer',
      device.agentId,
      async () => await rmmApi.connectFileTransfer(device.agentId),
      'Failed to open file transfer.'
    );
  };

  const handleOpenRegistry = async (device: RmmDevice) => {
    await attemptViewerLaunch(
      'Remote Registry',
      device.agentId,
      async () => await rmmApi.connectRegistry(device.agentId),
      'Failed to open remote registry.'
    );
  };

  const toggleDeviceSelection = (deviceId: string) => {
    const next = new Set(selectedDeviceIds);
    const cache = new Map(selectedDeviceCache);
    if (next.has(deviceId)) {
      next.delete(deviceId);
      cache.delete(deviceId);
    } else {
      next.add(deviceId);
      const device = devices.find((item) => item.agentId === deviceId);
      if (device) cache.set(deviceId, device);
    }
    selectedDeviceIds = next;
    selectedDeviceCache = cache;
  };

  const toggleSelectAll = (deviceIds: string[]) => {
    const next = new Set(selectedDeviceIds);
    const cache = new Map(selectedDeviceCache);
    const allSelected = deviceIds.every((id) => next.has(id));
    if (allSelected) {
      deviceIds.forEach((id) => {
        next.delete(id);
        cache.delete(id);
      });
    } else {
      const pageDevices = new Map(devices.map((device) => [device.agentId, device]));
      deviceIds.forEach((id) => {
        next.add(id);
        const device = pageDevices.get(id);
        if (device) cache.set(id, device);
      });
    }
    selectedDeviceIds = next;
    selectedDeviceCache = cache;
  };

  const clearSelection = () => {
    selectedDeviceIds = new Set();
    selectedDeviceCache = new Map();
  };

  const resolveCustomerName = (device: RmmDevice) => {
    if (device.customerName) return device.customerName;
    if (device.customerId) {
      const match = customers.find((customer) => customer.id === device.customerId);
      if (match) return match.name;
      if (device.customerId === unassignedCustomerId) return 'Unassigned';
      return 'Unknown';
    }
    return 'Unassigned';
  };

  const resolveSiteName = (device: RmmDevice) => {
    if (device.siteName) return device.siteName;
    if (device.siteId) {
      const match = sites.find((s) => s.id === device.siteId);
      return match ? match.name : '—';
    }
    return '—';
  };

  const handleBulkMove = async () => {
    if (!bulkCustomerId) {
      alert('Select a customer to move devices.');
      return;
    }

    try {
      bulkUpdating = true;
      await rmmApi.bulkUpdateCustomer(Array.from(selectedDeviceIds), bulkCustomerId);
      await fetchDevices();
      clearSelection();
      bulkCustomerInput = '';
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to move devices');
    } finally {
      bulkUpdating = false;
    }
  };

  const handleBulkMoveToSite = async () => {
    const siteIdToUse = bulkSiteId === '' ? null : bulkSiteId;
    try {
      bulkSiteUpdating = true;
      await rmmApi.bulkUpdateSite(Array.from(selectedDeviceIds), siteIdToUse);
      await fetchDevices();
      clearSelection();
      bulkSiteInput = '';
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to move devices to site');
    } finally {
      bulkSiteUpdating = false;
    }
  };

  const handleBulkDelete = async () => {
    const totalSelected = selectedDeviceIds.size;
    if (totalSelected === 0) return;
    const confirmed = window.confirm(`Delete ${totalSelected} device(s)? This cannot be undone.`);
    if (!confirmed) return;

    try {
      bulkDeleting = true;
      await rmmApi.bulkDeleteDevices(Array.from(selectedDeviceIds));
      await fetchDevices();
      clearSelection();
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to delete devices');
    } finally {
      bulkDeleting = false;
    }
  };

  const handleBulkSnapshot = async () => {
    const ids = selectedIds();
    if (ids.length === 0) return;
    try {
      bulkSnapshotRequesting = true;
      const results = await Promise.allSettled(ids.map((agentId) => rmmApi.requestSnapshot(agentId)));
      const failed = results.filter((result) => result.status === 'rejected').length;
      toast({
        title: 'Snapshot requests sent',
        description: failed ? `${ids.length - failed} queued, ${failed} failed.` : `${ids.length} request(s) queued.`
      });
    } catch (err) {
      toast({
        title: 'Snapshot request failed',
        description: err instanceof Error ? err.message : 'Failed to request telemetry snapshots.'
      });
    } finally {
      bulkSnapshotRequesting = false;
    }
  };

  const runCommandForSelected = async (command: string, options: { patchScan?: boolean } = {}) => {
    const ids = selectedIds();
    if (ids.length === 0 || !command.trim()) return;
    const setLoading = (value: boolean) => {
      if (options.patchScan) {
        bulkPatchScanning = value;
      } else {
        bulkCommandRunning = value;
      }
    };

    try {
      setLoading(true);
      const results = await Promise.allSettled(ids.map((agentId) => rmmApi.executeScript(agentId, command.trim())));
      const failed = results.filter((result) => result.status === 'rejected').length;
      toast({
        title: options.patchScan ? 'Patch scan requested' : 'Command dispatched',
        description: failed ? `${ids.length - failed} succeeded, ${failed} failed.` : `${ids.length} device(s) accepted the request.`
      });
      if (!options.patchScan) {
        bulkCommandInput = '';
      }
    } finally {
      setLoading(false);
    }
  };

  const handleBulkPatchScan = async () => {
    await runCommandForSelected(PATCH_SCAN_COMMAND, { patchScan: true });
  };

  const csvEscape = (value: unknown) => {
    const text = value === null || value === undefined ? '' : String(value);
    return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };

  const loadRowsForExport = async () => {
    const selected = selectedIds();
    if (selected.length === 0) return devices;

    const missing = selected.filter((id) => !selectedDeviceCache.has(id));
    if (missing.length > 0) {
      const fetched = await Promise.allSettled(missing.map((agentId) => rmmApi.getDevice(agentId)));
      const cache = new Map(selectedDeviceCache);
      for (const result of fetched) {
        if (result.status === 'fulfilled') {
          cache.set(result.value.agentId, result.value);
        }
      }
      selectedDeviceCache = cache;
    }

    return selected
      .map((id) => selectedDeviceCache.get(id))
      .filter((device): device is RmmDevice => Boolean(device));
  };

  const handleExportCsv = async () => {
    const rows = await loadRowsForExport();
    if (selectedDeviceIds.size > 0 && rows.length < selectedDeviceIds.size) {
      toast({
        title: 'Some selected devices were not exported',
        description: `${selectedDeviceIds.size - rows.length} selected device(s) could not be loaded.`
      });
    }
    const header = [
      'agentId',
      'hostname',
      'customer',
      'site',
      'os',
      'ip',
      'version',
      'status',
      'lastSeen',
      'pendingUpdates',
      'rebootRequired',
      'alertSeverity',
      'tags'
    ];
    const lines = [
      header.join(','),
      ...rows.map((device) => [
        device.agentId,
        device.hostname,
        resolveCustomerName(device),
        resolveSiteName(device),
        device.osName ?? device.os,
        device.ip,
        device.agentVersion ?? device.version ?? '',
        isOnline(device.lastSeen) ? 'online' : 'offline',
        device.lastSeen,
        device.pendingUpdatesCount ?? '',
        device.rebootRequired === null || device.rebootRequired === undefined ? '' : String(device.rebootRequired),
        device.alertSeverity ?? '',
        (device.tags ?? []).join('; ')
      ].map(csvEscape).join(','))
    ];
    saveBlobFile(
      `talos-devices-${new Date().toISOString().replace(/[:.]/g, '-')}.csv`,
      new Blob([lines.join('\r\n')], { type: 'text/csv;charset=utf-8' })
    );
  };

  const saveCurrentView = async () => {
    const name = saveViewName.trim();
    if (!name) {
      toast({ title: 'Saved view name required' });
      return;
    }
    try {
      const view = await rmmApi.createDeviceSavedView({
        name,
        state: currentDeviceListState()
      });
      savedViews = [view, ...savedViews.filter((item) => item.id !== view.id)];
      selectedSavedViewId = view.id;
      saveViewName = '';
      toast({ title: 'Device view saved' });
    } catch (err) {
      toast({
        title: 'Unable to save view',
        description: err instanceof Error ? err.message : 'Saved view could not be created.'
      });
    }
  };

  const updateCurrentSavedView = async () => {
    if (!selectedSavedViewId) return;
    try {
      const view = await rmmApi.updateDeviceSavedView(selectedSavedViewId, {
        state: currentDeviceListState()
      });
      savedViews = savedViews.map((item) => item.id === view.id ? view : item);
      toast({ title: 'Device view updated' });
    } catch (err) {
      toast({
        title: 'Unable to update view',
        description: err instanceof Error ? err.message : 'Saved view could not be updated.'
      });
    }
  };

  const applySavedView = () => {
    const view = savedViews.find((item) => item.id === selectedSavedViewId);
    if (!view) return;
    applyDeviceListState({
      page: 1,
      pageSize: view.pageSize,
      sortBy: view.sortBy,
      sortDirection: view.sortDirection,
      filters: view.filters
    });
  };

  const deleteCurrentSavedView = async () => {
    if (!selectedSavedViewId) return;
    const id = selectedSavedViewId;
    try {
      await rmmApi.deleteDeviceSavedView(id);
      savedViews = savedViews.filter((item) => item.id !== id);
      selectedSavedViewId = '';
      toast({ title: 'Device view deleted' });
    } catch (err) {
      toast({
        title: 'Unable to delete view',
        description: err instanceof Error ? err.message : 'Saved view could not be deleted.'
      });
    }
  };

  const setSort = (field: RmmDeviceListSortBy) => {
    if (sortBy === field) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      sortBy = field;
      sortDirection = field === 'lastSeen' || field === 'status' || field === 'alertSeverity' ? 'desc' : 'asc';
    }
    currentPage = 1;
    persistDeviceListState();
    void fetchDevices();
  };

  const sortLabel = (field: RmmDeviceListSortBy) => sortBy === field
    ? (sortDirection === 'asc' ? ' ▲' : ' ▼')
    : '';

  const totalPages = () => Math.max(1, Math.ceil(totalDevices / pageSize));

  const goToPage = (page: number) => {
    currentPage = Math.min(Math.max(1, page), totalPages());
    persistDeviceListState();
    void fetchDevices();
  };

  onMount(async () => {
    if (browser) {
      const persisted = loadDeviceListState(localStorage);
      if (persisted) {
        currentPage = persisted.page;
        pageSize = persisted.pageSize;
        sortBy = persisted.sortBy;
        sortDirection = persisted.sortDirection;
        filters = { ...emptyFilters(), ...persisted.filters };
      }
    }
    await Promise.all([fetchDevices(), fetchCustomers(), fetchSites(), fetchSavedViews(), fetchViewerConnections()]);
    startViewerConnectionsPolling();
  });

  onMount(() => {
    const onClickOutside = (event: MouseEvent) => {
      if (!openActionsAgentId) return;
      const target = event.target as HTMLElement | null;
      const root = target?.closest?.('[data-rmm-actions-root]') as HTMLElement | null;
      if (!root || root.getAttribute('data-rmm-actions-root') !== openActionsAgentId) {
        closeActionsMenu();
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        closeActionsMenu();
      }
    };

    document.addEventListener('mousedown', onClickOutside);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('mousedown', onClickOutside);
      document.removeEventListener('keydown', onKeyDown);
    };
  });

  onDestroy(() => {
    if (viewerConnectionsPollTimer) {
      clearInterval(viewerConnectionsPollTimer);
      viewerConnectionsPollTimer = null;
    }
  });

  $: {
    const normalizedFilter = customerFilterInput.trim().toLowerCase();
    const normalizedBulk = bulkCustomerInput.trim().toLowerCase();
    const normalizedBulkSite = bulkSiteInput.trim().toLowerCase();
    const customerIdByName = new Map(
      customers.map((customer) => [customer.name.toLowerCase(), customer.id])
    );

    if (!normalizedFilter || normalizedFilter === 'all customers') {
      effectiveCustomerFilter = 'all';
    } else if (normalizedFilter === 'unassigned') {
      effectiveCustomerFilter = 'unassigned';
    } else {
      effectiveCustomerFilter = customerIdByName.get(normalizedFilter) ?? 'all';
    }

    if (!normalizedBulk) {
      bulkCustomerId = '';
    } else if (normalizedBulk === 'unassigned') {
      bulkCustomerId = unassignedCustomerId;
    } else {
      bulkCustomerId = customerIdByName.get(normalizedBulk) ?? '';
    }

    if (!normalizedBulkSite) {
      bulkSiteId = '';
    } else if (normalizedBulkSite === 'no site' || normalizedBulkSite === '—') {
      bulkSiteId = null;
    } else {
      const found = siteOptions.find(
        (o) => o.label.toLowerCase() === normalizedBulkSite
      );
      bulkSiteId = found ? found.siteId : '';
    }
  }

  $: siteOptions = [
    { label: 'No site', siteId: null as string | null },
    ...sites.map((s) => ({
      label: `${s.customerName ?? '—'} → ${s.name}`,
      siteId: s.id as string
    }))
  ];

  $: filteredDevices = devices;

  $: {
    const unassigned = customers.find((customer) => customer.isUnassigned);
    unassignedCustomerId = unassigned?.id ?? '';
  }

  $: visibleDeviceIds = filteredDevices.map((device) => device.agentId);
  $: allVisibleSelected =
    visibleDeviceIds.length > 0 && visibleDeviceIds.every((id) => selectedDeviceIds.has(id));
  $: someVisibleSelected = visibleDeviceIds.some((id) => selectedDeviceIds.has(id));

  $: if (selectAllRef) {
    selectAllRef.indeterminate = someVisibleSelected && !allVisibleSelected;
  }

  $: customerFilterOptions = [
    'All Customers',
    'Unassigned',
    ...customers.filter((c) => !c.isUnassigned).map((c) => c.name)
  ];

  $: filteredCustomerOptions = (() => {
    const q = customerFilterInput.trim().toLowerCase();
    if (!q) return customerFilterOptions;
    return customerFilterOptions.filter((opt) => opt.toLowerCase().includes(q));
  })();

  const setCustomerFilter = (value: string) => {
    customerFilterInput = value;
    customerFilterOpen = false;
  };
</script>

<div class="space-y-3">
  <div class="toolbar-panel rounded-md p-2">
    <div class="flex flex-wrap items-center gap-2">
      <span class="stat-chip">
        <Server class="h-3.5 w-3.5 opacity-60" />
        <span class="font-semibold">{totalDevices}</span>
        <span class="opacity-55">devices</span>
      </span>
      <span class="stat-chip">
        <span class="h-1.5 w-1.5 rounded-full bg-emerald-400 opacity-80"></span>
        <span class="font-semibold">{devices.filter((device) => getHealthStatus(device) === 'healthy').length}</span>
        <span class="opacity-55">healthy</span>
      </span>
      <span class="stat-chip">
        <TriangleAlert class="h-3.5 w-3.5 opacity-65" />
        <span class="font-semibold">{devices.filter((device) => getHealthStatus(device) !== 'healthy').length}</span>
        <span class="opacity-55">needs attention</span>
      </span>
      <span class="stat-chip opacity-50">
        Updated {lastUpdated ?? '—'}
      </span>
      <Button onclick={() => filterPanelOpen = !filterPanelOpen} size="sm" className="h-8 gap-1.5">
        <ListFilter class="h-3.5 w-3.5" />
        Filters
      </Button>
      <Button onclick={fetchDevices} size="sm" className="h-8 gap-1.5" disabled={loading}>
        <RefreshCw class={`h-3.5 w-3.5 ${loading ? 'animate-spin' : ''}`} />
        Refresh
      </Button>
    </div>

    {#if filterPanelOpen}
      <div class="filter-grid mt-2">
        <label>
          <span>Search</span>
          <div class="relative">
            <Search class="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <input
              class="glass-input h-9 w-full rounded-md py-2 pl-8 pr-2 text-sm"
              value={filters.q ?? ''}
              placeholder="Hostname, IP, OS..."
              oninput={(event) => updateFilter('q', (event.currentTarget as HTMLInputElement).value)}
            />
          </div>
        </label>
        <label>
          <span>Customer</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.customerId ?? 'all'}
            disabled={customersLoading}
            onchange={(event) => updateFilter('customerId', (event.currentTarget as HTMLSelectElement).value)}
          >
            <option value="all">All customers</option>
            <option value="unassigned">Unassigned</option>
            {#each customers.filter((customer) => !customer.isUnassigned) as customer}
              <option value={customer.id}>{customer.name}</option>
            {/each}
          </select>
        </label>
        <label>
          <span>Site</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.siteId ?? 'all'}
            disabled={sitesLoading}
            onchange={(event) => updateFilter('siteId', (event.currentTarget as HTMLSelectElement).value)}
          >
            <option value="all">All sites</option>
            <option value="none">No site</option>
            {#each siteOptions.filter((option) => option.siteId !== null) as option}
              <option value={option.siteId}>{option.label}</option>
            {/each}
          </select>
        </label>
        <label>
          <span>Status</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.status}
            onchange={(event) => updateFilter('status', (event.currentTarget as HTMLSelectElement).value as RmmDeviceListFilters['status'])}
          >
            <option value="all">Any status</option>
            <option value="online">Online</option>
            <option value="offline">Offline</option>
          </select>
        </label>
        <label>
          <span>OS</span>
          <input
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.os ?? ''}
            placeholder="Windows, macOS..."
            oninput={(event) => updateFilter('os', (event.currentTarget as HTMLInputElement).value)}
          />
        </label>
        <label>
          <span>Version</span>
          <input
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.version ?? ''}
            placeholder="Agent version"
            oninput={(event) => updateFilter('version', (event.currentTarget as HTMLInputElement).value)}
          />
        </label>
        <label>
          <span>Tag / group</span>
          <input
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.tag ?? ''}
            placeholder="VIP, lab, ring..."
            oninput={(event) => updateFilter('tag', (event.currentTarget as HTMLInputElement).value)}
          />
        </label>
        <label>
          <span>Pending updates</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.pendingUpdates === true ? 'true' : filters.pendingUpdates === false ? 'false' : 'all'}
            onchange={(event) => {
              const value = (event.currentTarget as HTMLSelectElement).value;
              updateFilter('pendingUpdates', value === 'all' ? null : value === 'true');
            }}
          >
            <option value="all">Any</option>
            <option value="true">Has updates</option>
            <option value="false">No updates</option>
          </select>
        </label>
        <label>
          <span>Reboot</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.rebootRequired === true ? 'true' : filters.rebootRequired === false ? 'false' : 'all'}
            onchange={(event) => {
              const value = (event.currentTarget as HTMLSelectElement).value;
              updateFilter('rebootRequired', value === 'all' ? null : value === 'true');
            }}
          >
            <option value="all">Any</option>
            <option value="true">Required</option>
            <option value="false">Not required</option>
          </select>
        </label>
        <label>
          <span>Alert severity</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.alertSeverity ?? 'all'}
            onchange={(event) => {
              const value = (event.currentTarget as HTMLSelectElement).value;
              updateFilter('alertSeverity', value === 'all' ? null : value as RmmDeviceListFilters['alertSeverity']);
            }}
          >
            <option value="all">Any alert</option>
            <option value="warning">Warning+</option>
            <option value="error">Error+</option>
            <option value="critical">Critical</option>
          </select>
        </label>
        <label>
          <span>Last seen age</span>
          <select
            class="glass-input h-9 w-full rounded-md px-2 text-sm"
            value={filters.lastSeenAgeMinutes ?? 'all'}
            onchange={(event) => {
              const value = (event.currentTarget as HTMLSelectElement).value;
              updateFilter('lastSeenAgeMinutes', value === 'all' ? null : Number(value));
            }}
          >
            <option value="all">Any age</option>
            <option value="15">Older than 15m</option>
            <option value="60">Older than 1h</option>
            <option value="1440">Older than 24h</option>
            <option value="10080">Older than 7d</option>
            <option value="43200">Older than 30d</option>
          </select>
        </label>
        <div class="filter-actions">
          <Button
            variant="outline"
            size="sm"
            className="h-9"
            onclick={() => applyDeviceListState({ ...currentDeviceListState(), page: 1, filters: emptyFilters() })}
          >
            Clear filters
          </Button>
        </div>
      </div>
    {/if}

    <div class="saved-view-row mt-2">
      <select
        class="glass-input h-9 min-w-[12rem] rounded-md px-2 text-sm"
        bind:value={selectedSavedViewId}
        disabled={savedViewsLoading}
      >
        <option value="">Saved views</option>
        {#each savedViews as view}
          <option value={view.id}>{view.name}</option>
        {/each}
      </select>
      <Button variant="outline" size="sm" className="h-9" onclick={applySavedView} disabled={!selectedSavedViewId}>Apply</Button>
      <Button variant="outline" size="sm" className="h-9" onclick={updateCurrentSavedView} disabled={!selectedSavedViewId}>Update</Button>
      <Button variant="outline" size="sm" className="h-9" onclick={deleteCurrentSavedView} disabled={!selectedSavedViewId}>Delete</Button>
      <input
        class="glass-input h-9 min-w-[12rem] rounded-md px-2 text-sm"
        bind:value={saveViewName}
        placeholder="New view name"
      />
      <Button size="sm" className="h-9 gap-1.5" onclick={saveCurrentView}>
        <Save class="h-3.5 w-3.5" />
        Save view
      </Button>
    </div>
  </div>

  <Card>
    <CardContent className="p-3">
      {#if loading}
        <div class="flex items-center justify-center py-8">
          <div class="animate-spin rounded-full h-6 w-6 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if error}
        <div class="text-sm text-destructive">{error}</div>
      {:else}
        {#if selectedDeviceIds.size > 0}
          <div class="bulk-bar mb-2 flex flex-col gap-2 rounded-md px-2.5 py-1.5 text-xs sm:flex-row sm:items-center sm:justify-between">
            <div>{selectedDeviceIds.size} device(s) selected</div>
            <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:flex-wrap">
              <input
                list="bulk-customer-options"
                class="glass-input flex h-9 rounded-md px-2 text-sm"
                bind:value={bulkCustomerInput}
                placeholder="Move to customer..."
              />
              <datalist id="bulk-customer-options">
                <option value="Unassigned"></option>
                {#each customers.filter((customer) => !customer.isUnassigned) as customer}
                  <option value={customer.name}></option>
                {/each}
              </datalist>
              <input
                list="bulk-site-options"
                class="glass-input flex h-9 min-w-[12rem] rounded-md px-2 text-sm"
                bind:value={bulkSiteInput}
                placeholder="Move to site..."
                disabled={sitesLoading}
              />
              <datalist id="bulk-site-options">
                <option value="No site"></option>
                {#each siteOptions.filter((o) => o.siteId !== null) as opt}
                  <option value={opt.label}></option>
                {/each}
              </datalist>
              <input
                class="glass-input flex h-9 min-w-[14rem] rounded-md px-2 text-sm"
                bind:value={bulkCommandInput}
                placeholder="Approved command..."
              />
              <div class="flex gap-2">
                <Button onclick={handleBulkMove} disabled={bulkUpdating || !bulkCustomerId}>
                  {bulkUpdating ? 'Moving...' : 'Move to customer'}
                </Button>
                <Button
                  onclick={handleBulkMoveToSite}
                  disabled={bulkSiteUpdating || bulkSiteId === ''}
                >
                  {bulkSiteUpdating ? 'Moving...' : 'Move to site'}
                </Button>
                <Button
                  variant="outline"
                  onclick={handleBulkDelete}
                  disabled={bulkDeleting || selectedDeviceIds.size === 0}
                >
                  <Trash2 class="h-4 w-4" />
                  {bulkDeleting ? 'Deleting...' : 'Delete'}
                </Button>
                <Button
                  variant="outline"
                  onclick={handleBulkSnapshot}
                  disabled={bulkSnapshotRequesting || selectedDeviceIds.size === 0}
                >
                  <RefreshCw class={`h-4 w-4 ${bulkSnapshotRequesting ? 'animate-spin' : ''}`} />
                  {bulkSnapshotRequesting ? 'Requesting...' : 'Snapshot'}
                </Button>
                <Button
                  variant="outline"
                  onclick={() => runCommandForSelected(bulkCommandInput)}
                  disabled={bulkCommandRunning || selectedDeviceIds.size === 0 || !bulkCommandInput.trim()}
                >
                  <PlayCircle class="h-4 w-4" />
                  {bulkCommandRunning ? 'Running...' : 'Run command'}
                </Button>
                <Button
                  variant="outline"
                  onclick={handleBulkPatchScan}
                  disabled={bulkPatchScanning || selectedDeviceIds.size === 0}
                >
                  <RefreshCw class={`h-4 w-4 ${bulkPatchScanning ? 'animate-spin' : ''}`} />
                  {bulkPatchScanning ? 'Scanning...' : 'Patch scan'}
                </Button>
                <Button
                  variant="outline"
                  onclick={handleExportCsv}
                  disabled={devices.length === 0}
                >
                  <Download class="h-4 w-4" />
                  Export CSV
                </Button>
                <Button variant="outline" onclick={clearSelection}>Clear</Button>
              </div>
            </div>
          </div>
        {/if}
        <Table className="text-sm">
          <TableHeader>
            <TableRow>
              <TableHead className="h-8 w-8 px-2 text-sm">
                <input
                  type="checkbox"
                  bind:this={selectAllRef}
                  checked={allVisibleSelected}
                  onchange={() => toggleSelectAll(visibleDeviceIds)}
                  aria-label="Select all devices"
                />
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('hostname')}>Hostname{sortLabel('hostname')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('customer')}>Customer{sortLabel('customer')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('site')}>Site{sortLabel('site')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('os')}>OS{sortLabel('os')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">IP</TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('version')}>Version{sortLabel('version')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('status')}>Status{sortLabel('status')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('lastSeen')}>Last Seen{sortLabel('lastSeen')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('pendingUpdates')}>Updates{sortLabel('pendingUpdates')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">
                <button class="sort-button" type="button" onclick={() => setSort('alertSeverity')}>Alerts{sortLabel('alertSeverity')}</button>
              </TableHead>
              <TableHead className="h-8 px-2 text-sm">Viewers</TableHead>
              <TableHead className="h-8 px-2 text-right text-sm">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each filteredDevices as device}
              <TableRow>
                <TableCell className="p-2 py-1.5">
                  <input
                    type="checkbox"
                    checked={selectedDeviceIds.has(device.agentId)}
                    onchange={() => toggleDeviceSelection(device.agentId)}
                    aria-label={`Select ${device.hostname}`}
                  />
                </TableCell>
                <TableCell className="p-2 py-1.5 font-medium">
                  <a
                    class="device-link"
                    href={`/dashboard/rmm/${device.agentId}`}
                    aria-label={`Open details for ${device.hostname}`}
                  >
                    {device.hostname}
                  </a>
                </TableCell>
                <TableCell className="p-2 py-1.5">{resolveCustomerName(device)}</TableCell>
                <TableCell className="p-2 py-1.5">{resolveSiteName(device)}</TableCell>
                <TableCell className="p-2 py-1.5">
                  <div>{device.osName ?? device.os}</div>
                  {#if device.osVersion}
                    <div class="text-xs aero-muted">{device.osVersion}</div>
                  {/if}
                </TableCell>
                <TableCell className="p-2 py-1.5">{device.ip}</TableCell>
                <TableCell className="p-2 py-1.5">{device.agentVersion ?? device.version ?? '—'}</TableCell>
                <TableCell className="p-2 py-1.5">
                  <div class="health-cell">
                    <span class={healthBadgeClass(device)}>{formatHealthStatus(device)}</span>
                    {#if healthReasonPreview(device)}
                      <span class="health-reason" title={healthReasonPreview(device)}>{healthReasonPreview(device)}</span>
                    {/if}
                  </div>
                </TableCell>
                <TableCell className="p-2 py-1.5">{formatLastSeen(device.lastSeen)}</TableCell>
                <TableCell className="p-2 py-1.5">
                  <div>{device.pendingUpdatesCount ?? '—'}</div>
                  {#if device.rebootRequired}
                    <div class="text-xs text-amber-300">Reboot required</div>
                  {/if}
                  {#if (device.tags ?? []).length > 0}
                    <div class="tag-list">
                      {#each (device.tags ?? []).slice(0, 2) as tag}
                        <span>{tag}</span>
                      {/each}
                    </div>
                  {/if}
                </TableCell>
                <TableCell className="p-2 py-1.5">
                  {#if device.alertSeverity}
                    <span class={`alert-pill alert-${device.alertSeverity}`}>{device.alertSeverity}</span>
                  {:else}
                    <span class="aero-muted">—</span>
                  {/if}
                </TableCell>
                <TableCell className="p-2 py-1.5">
                  {@const connections = getViewerConnectionsForAgent(device.agentId)}
                  {#if connections.length === 0}
                    <span class="aero-muted">—</span>
                  {:else}
                    <div class="viewer-presence-list">
                      <div class="viewer-presence-count">{connections.length} active</div>
                      {#each connections.slice(0, 2) as connection (connection.sessionId)}
                        <div class="viewer-presence-item">
                          {(connection.userEmail ?? connection.userId ?? 'Unknown user')} · {formatViewerSessionKind(connection.kind)}
                        </div>
                      {/each}
                      {#if connections.length > 2}
                        <div class="viewer-presence-item">+{connections.length - 2} more</div>
                      {/if}
                    </div>
                  {/if}
                </TableCell>
                <TableCell className="p-2 py-1.5 text-right">
                  {@const isLinux = isLinuxDevice(device)}
                  <div class="inline-flex items-center justify-end gap-1">
                    {#if isLinux}
                      <Button
                        variant="outline"
                        size="sm"
                        className="gap-1 text-sm"
                        title="Open interactive shell"
                        aria-label={`Open shell for ${device.hostname}`}
                        onclick={() => handleOpenShell(device)}
                      >
                        <Terminal class="h-4 w-4" />
                        Shell
                      </Button>
                    {:else}
                      <Button
                        variant="outline"
                        size="sm"
                        className="gap-1 text-sm"
                        title="Open remote desktop viewer"
                        aria-label={`Open viewer for ${device.hostname}`}
                        onclick={() => handleConnect(device)}
                      >
                        <Monitor class="h-4 w-4" />
                        Viewer
                      </Button>
                    {/if}

                    <div
                      class={`relative ${openActionsAgentId === device.agentId ? 'z-50' : ''}`}
                      data-rmm-actions-root={device.agentId}
                    >
                      <Button
                        variant="outline"
                        size="sm"
                        className="px-2"
                        title="More tools"
                        aria-label={`More actions for ${device.hostname}`}
                        aria-haspopup="menu"
                        aria-expanded={openActionsAgentId === device.agentId}
                        onclick={(e: MouseEvent & { detail?: unknown }) => {
                          const detail = e.detail as unknown;
                          const native = detail && typeof detail === 'object' && detail instanceof MouseEvent
                            ? detail
                            : e as unknown as MouseEvent;
                          if (openActionsAgentId === device.agentId) {
                            closeActionsMenu();
                          } else {
                            openActionsMenu(device, native);
                          }
                        }}
                      >
                        <ChevronDown class="h-4 w-4" />
                      </Button>

                      <!-- dropdown rendered at page root to avoid backdrop-filter containment -->
                    </div>
                  </div>
                </TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
        {#if filteredDevices.length === 0}
          <div class="text-center py-4 text-sm aero-muted">No devices have checked in yet.</div>
        {/if}
        <div class="pagination-row mt-3">
          <div class="text-xs aero-muted">
            Page {currentPage} of {totalPages()} · {totalDevices} matching device(s)
          </div>
          <div class="flex items-center gap-2">
            <select
              class="glass-input h-8 rounded-md px-2 text-xs"
              value={pageSize}
              onchange={(event) => {
                pageSize = Number((event.currentTarget as HTMLSelectElement).value);
                currentPage = 1;
                persistDeviceListState();
                void fetchDevices();
              }}
            >
              <option value="25">25 / page</option>
              <option value="50">50 / page</option>
              <option value="100">100 / page</option>
              <option value="250">250 / page</option>
              <option value="500">500 / page</option>
            </select>
            <Button variant="outline" size="sm" className="h-8" onclick={() => goToPage(currentPage - 1)} disabled={currentPage <= 1}>Previous</Button>
            <Button variant="outline" size="sm" className="h-8" onclick={() => goToPage(currentPage + 1)} disabled={currentPage >= totalPages()}>Next</Button>
          </div>
        </div>
      {/if}
    </CardContent>
  </Card>
</div>

<!-- Actions dropdown — rendered outside all glass-card elements so position:fixed uses the viewport,
     not a backdrop-filter containing block. -->
{#if openActionsDevice && menuPosition}
  {@const openActionsDeviceIsLinux = isLinuxDevice(openActionsDevice)}
  {@const openActionsDeviceIsMacos = isMacosDevice(openActionsDevice)}
  <div
    role="menu"
    class="fixed z-[200] w-44 rounded-lg py-1 actions-dropdown"
    style="top: {menuPosition.top}px; left: {menuPosition.left}px;"
    data-rmm-actions-root={openActionsDevice.agentId}
  >
    {#if !openActionsDeviceIsLinux}
      <button
        type="button"
        role="menuitem"
        class="dropdown-action-item"
        onclick={() => { const d = openActionsDevice; closeActionsMenu(); if (d) void handleOpenShell(d); }}
      >
        <Terminal class="h-4 w-4" />Shell
      </button>
    {/if}
    <button
      type="button" role="menuitem"
      class="dropdown-action-item"
      onclick={() => { const d = openActionsDevice; closeActionsMenu(); if (d) void handleOpenFileTransfer(d); }}
    >
      <Folder class="h-4 w-4" />File transfer
    </button>
    {#if !openActionsDeviceIsLinux && !openActionsDeviceIsMacos}
      <button
        type="button" role="menuitem"
        class="dropdown-action-item"
        onclick={() => { const d = openActionsDevice; closeActionsMenu(); if (d) void handleOpenRegistry(d); }}
      >
        <Database class="h-4 w-4" />Registry
      </button>
    {/if}
  </div>
{/if}

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
            <Button variant="ghost" onclick={() => cancelViewerLaunchWait?.()}>Cancel</Button>
            <Button onclick={downloadViewerInstaller} disabled={viewerInstallerDownloading}>
              {viewerInstallerDownloading ? 'Downloading...' : 'Download Viewer'}
            </Button>
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .toolbar-panel {
    background: rgba(255,255,255,0.04);
    border: 1px solid rgba(255,255,255,0.08);
  }
  :global(html.light) .toolbar-panel {
    background: rgba(255,255,255,0.55);
    border-color: rgba(100,158,220,0.22);
  }
  .filter-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
    gap: 0.5rem;
    align-items: end;
  }
  .filter-grid label {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.72rem;
    color: rgba(200,230,255,0.68);
  }
  :global(html.light) .filter-grid label {
    color: rgba(10,30,90,0.68);
  }
  .filter-actions {
    display: flex;
    align-items: end;
  }
  .saved-view-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: center;
  }
  .sort-button {
    display: inline-flex;
    max-width: 100%;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
  .sort-button:hover {
    color: white;
  }
  :global(html.light) .sort-button:hover {
    color: #0a1628;
  }
  .tag-list {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    margin-top: 0.25rem;
  }
  .tag-list span {
    max-width: 8rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: 999px;
    padding: 0.05rem 0.35rem;
    font-size: 0.68rem;
    background: rgba(255,255,255,0.07);
    color: rgba(200,230,255,0.72);
  }
  :global(html.light) .tag-list span {
    background: rgba(50,100,200,0.08);
    color: rgba(10,30,90,0.72);
  }
  .alert-pill {
    display: inline-flex;
    border-radius: 999px;
    padding: 0.12rem 0.45rem;
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: capitalize;
  }
  .alert-info { background: rgba(80,160,255,0.14); color: #93c5fd; }
  .alert-warning, .alert-warn { background: rgba(250,204,21,0.14); color: #facc15; }
  .alert-error { background: rgba(248,113,113,0.15); color: #fca5a5; }
  .alert-critical { background: rgba(244,63,94,0.2); color: #fda4af; }
  .pagination-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
  }
  .stat-chip {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 4px 10px; border-radius: 999px; font-size: 0.75rem;
    background: rgba(255,255,255,0.055); border: 1px solid rgba(255,255,255,0.09);
    color: rgba(200,230,255,0.78);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06);
  }
  :global(html.light) .stat-chip {
    background: rgba(255,255,255,0.62); border-color: rgba(100,158,220,0.22);
    color: rgba(10,40,120,0.7);
  }

  .health-cell {
    display: flex;
    min-width: 10rem;
    max-width: 18rem;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.25rem;
  }
  .health-pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 0.72rem;
    font-weight: 600;
  }
  .health-reason {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.72rem;
    color: rgba(200,225,255,0.68);
  }
  :global(html.light) .health-reason {
    color: rgba(10,30,90,0.68);
  }

  .bulk-bar {
    background: rgba(255,255,255,0.05); border: 1px solid rgba(255,255,255,0.09);
    color: rgba(200,230,255,0.8);
  }
  :global(html.light) .bulk-bar {
    background: rgba(255,255,255,0.55); border-color: rgba(100,158,220,0.22); color: #0a1628;
  }

  .filter-dropdown {
    background: rgba(10,20,55,0.92);
    backdrop-filter: blur(28px) saturate(180%);
    -webkit-backdrop-filter: blur(28px) saturate(180%);
    border: 1px solid rgba(70,140,255,0.18);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 20px 50px rgba(0,0,0,0.55);
  }
  .filter-option {
    color: rgba(200,225,255,0.8);
  }
  .filter-option:hover, .filter-option:focus { background: rgba(255,255,255,0.08); color: white; outline: none; }
  .filter-option-active { background: rgba(255,255,255,0.05); }
  :global(html.light) .filter-dropdown {
    background: rgba(245,251,255,0.96); border-color: rgba(100,158,218,0.28);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.9), 0 20px 50px rgba(0,40,120,0.14);
  }
  :global(html.light) .filter-option { color: rgba(10,30,90,0.8); }
  :global(html.light) .filter-option:hover { background: rgba(50,100,200,0.07); color: #0a1628; }
  :global(html.light) .filter-option-active { background: rgba(50,100,200,0.05); }

  .actions-dropdown {
    background: rgba(10,20,55,0.92);
    backdrop-filter: blur(28px) saturate(180%);
    -webkit-backdrop-filter: blur(28px) saturate(180%);
    border: 1px solid rgba(70,140,255,0.18);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.06), 0 20px 50px rgba(0,0,0,0.55);
  }
  .dropdown-action-item {
    display: flex; align-items: center; gap: 8px;
    width: 100%; padding: 8px 14px;
    font-size: 0.875rem; text-align: left;
    color: rgba(200,225,255,0.8); background: transparent; border: none;
    transition: background 0.14s, color 0.14s;
  }
  .dropdown-action-item:hover { background: rgba(255,255,255,0.08); color: white; }
  :global(html.light) .actions-dropdown {
    background: rgba(245,251,255,0.96); border-color: rgba(100,158,218,0.28);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.9), 0 18px 44px rgba(0,40,120,0.14);
  }
  :global(html.light) .dropdown-action-item { color: rgba(10,30,90,0.8); }
  :global(html.light) .dropdown-action-item:hover { background: rgba(50,100,200,0.07); color: #0a1628; }

  .viewer-presence-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .viewer-presence-count {
    font-size: 0.75rem;
    font-weight: 600;
  }
  .viewer-presence-item {
    font-size: 0.75rem;
    color: rgba(200,225,255,0.72);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 16rem;
  }
  :global(html.light) .viewer-presence-item {
    color: rgba(10,30,90,0.72);
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
