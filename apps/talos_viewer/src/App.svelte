<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { getCurrent, onOpenUrl, register } from '@tauri-apps/plugin-deep-link';
  import { onMount, tick } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import type {
    AgentFeatureCapabilities,
    AgentPlatform,
    ChatSessionCapabilitiesHttpResponse,
    FileTransferSessionCapabilitiesHttpResponse,
    RegistrySessionCapabilitiesHttpResponse,
    RemoteDesktopDisplayProfile,
    SessionCapabilitiesHttpResponse,
    ShellSessionCapabilitiesHttpResponse
  } from '@talos/protocol-types';
  import '@xterm/xterm/css/xterm.css';
  import RemoteRegistry from './RemoteRegistry.svelte';

  type ViewerTab = 'Remote Desktop' | 'System Shell' | 'File Transfer' | 'System Info' | 'Remote Registry';
  type SessionCapabilities = SessionCapabilitiesHttpResponse;
  type RegistryTransportCapabilities = RegistrySessionCapabilitiesHttpResponse;
  type FileTransferCapabilities = FileTransferSessionCapabilitiesHttpResponse;
  type ChatSessionCapabilities = ChatSessionCapabilitiesHttpResponse;
  type ShellCapabilities = ShellSessionCapabilitiesHttpResponse;

  const TAB_ORDER: ViewerTab[] = [
    'Remote Desktop',
    'System Shell',
    'File Transfer',
    'Remote Registry',
    'System Info'
  ];
  const WINDOWS_FEATURES: AgentFeatureCapabilities = {
    remoteDesktop: true,
    systemShell: true,
    fileTransfer: true,
    remoteRegistry: true,
    chat: true,
    systemInfo: true
  };
  const LIMITED_UNIX_FEATURES: AgentFeatureCapabilities = {
    remoteDesktop: false,
    systemShell: true,
    fileTransfer: true,
    remoteRegistry: false,
    chat: false,
    systemInfo: true
  };
  const MACOS_FEATURES: AgentFeatureCapabilities = {
    remoteDesktop: true,
    systemShell: true,
    fileTransfer: true,
    remoteRegistry: false,
    chat: true,
    systemInfo: true
  };
  const UNKNOWN_FEATURES: AgentFeatureCapabilities = {
    remoteDesktop: false,
    systemShell: false,
    fileTransfer: false,
    remoteRegistry: false,
    chat: false,
    systemInfo: true
  };

  let activeTab: ViewerTab = 'Remote Desktop';
  let settingsOpen = false;
  let aiAssistPanelOpen = false;
  let remoteDesktopStatus = 'Waiting for a connection from the web dashboard.';
  let remoteDesktopError: string | null = null;
  let remoteDesktopOutput: string | null = null;
  let remoteSessionInfo: SessionParams | null = null;
  let shellSessionInfo: SessionParams | null = null;
  let capabilities: SessionCapabilities | null = null;
  let connecting = false;
  let launchArgs: string[] = [];
  let launchUrl: string | null = null;
  let viewerTransport = 'auto';
  let activeTransport: 'quic' | 'relay' | null = null;
  let remoteDesktopConnected = false;
  let remoteConnectInFlight = false;
  let shellConnectInFlight = false;
  let remoteDesktopFrameImage: string | null = null;
  let remoteDesktopFrameWidth = 0;
  let remoteDesktopFrameHeight = 0;

  // Remote Registry state (separate transport from Remote Desktop)
  let registryConnected = false;
  let registryConnectInFlight = false;
  let registryStatus = 'Waiting to connect...';
  let registryError: string | null = null;
  let registryTransport: 'quic' | 'relay' | null = null;
  let registrySessionInfo: SessionParams | null = null;
  let registryCapabilities: RegistryTransportCapabilities | null = null;
  let registryQuicInProgress = false;
  let pendingRegistryRelayHello: string | null = null;
  let pendingRegistryRelayError: string | null = null;

  let autoConnect = true;
  let showNotifications = true;
  type VideoQuality = 'low' | 'medium' | 'high';
  type VideoQualityOption = {
    id: VideoQuality;
    label: string;
    bitrateKbps: number;
    hint: string;
  };

  let videoQuality: VideoQuality = 'high';
  let audioEnabled = true;
  let clipboardSync = true;
  let isLightMode = false;
  let viewerUpdateCheckInFlight = false;
  let viewerUpdateStatusMessage: string | null = null;
  let viewerVersion = 'Unknown';
  let viewerUpdateExitInProgress = false;
  let quicInProgress = false;
  let pendingRelayHello: string | null = null;
  let pendingRelayError: string | null = null;
  let sessionEnding = false;
  let remoteDesktopFrame: HTMLDivElement | null = null;
  let viewportObserver: ResizeObserver | null = null;
  let viewportObserving = false;
  let viewportSuppressed = false;
  let remoteHasFocus = false;
  let lastMoveSent = 0;
  let lastStartMenuBlocked: boolean | null = null;
  let lastClipRefreshKey = '';
  let remoteDesktopDropdownOpen = false;
  let navClipReady = false;
  let navClipReadyTimer: number | null = null;
  let sessionSwitchInFlight = false;
  let sessionSwitchError: string | null = null;
  /** 'console' = console session; number = native WTS session id when an RDP session is selected */
  let remoteDesktopContext: 'console' | number = 'console';
  /** null = not yet enumerated by agent; [] = no RDP sessions; non-empty = list from agent */
  type RdpSessionInfo = {
    logicalSessionId: number;
    nativeSessionId: number;
    kind: string;
    winStation: string;
    userName: string;
    state: string;
  };
  type RawRdpSessionInfo = Partial<RdpSessionInfo> & {
    sessionId?: number;
    session_id?: number;
    win_station?: string;
  };
  let rdpSessions: Array<RdpSessionInfo> | null = null;
  type CaptureOutputInfo = {
    index: number;
    name: string;
    displayId?: number;
    width?: number;
    height?: number;
    originX?: number;
    originY?: number;
    pointWidth?: number;
    pointHeight?: number;
    primary?: boolean;
  };
  const CAPTURE_OUTPUT_SWITCH_TIMEOUT_MS = 5000;
  /** null = not received from stream metadata yet */
  let captureOutputs: Array<CaptureOutputInfo> | null = null;
  let activeCaptureOutputIndex = 0;
  let pendingCaptureOutputIndex: number | null = null;
  let lastRequestedCaptureOutputIndex: number | null = null;
  let captureOutputSwitchTimeout: number | null = null;
  let captureOutputSwitchError: string | null = null;
  let remoteDesktopCaptureType: string | null = null;
  /** Reactive: avoid relying on template reads inside helper fns for disabled state */
  $: monitorPickerInteractive =
    captureOutputs !== null && captureOutputs.length > 1 && pendingCaptureOutputIndex === null;
  $: monitorPickerPanelVisible =
    monitorPickerOpen && (monitorPickerInteractive || captureOutputSwitchError !== null);
  let visibleRdpSessions: Array<RdpSessionInfo> = [];
  let consoleSession: RdpSessionInfo | null = null;
  let shellUserContexts: Array<RdpSessionInfo> = [];

  // Shell state
  type ShellRunAs = 'user' | 'system';
  let shellConnected = false;
  let shellStatus = '';
  let shellError: string | null = null;
  let linuxShellCredential: LinuxShellCredential | null = null;
  let linuxShellCredentialLoading = false;
  let linuxShellCredentialError: string | null = null;
  let shellCredentialPanelOpen = false;
  let shellTransport: 'quic' | 'relay' | 'tcp' | null = null;
  let shellCapabilities: ShellCapabilities | null = null;
  let shellQuicInProgress = false;
  let pendingShellRelayHello: string | null = null;
  let shellRunAs: ShellRunAs = 'system';
  let shellTargetSessionId: number | null = null;
  let shellRunAsDropdownOpen = false;
  let shellTerminalEl: HTMLDivElement | null = null;
  let shellTerminal: Terminal | null = null;
  let shellFitAddon: FitAddon | null = null;
  let shellResizeObserver: ResizeObserver | null = null;
  type ShellContextMenuState = {
    open: boolean;
    x: number;
    y: number;
    hasSelection: boolean;
  };
  let shellContextMenu: ShellContextMenuState = {
    open: false,
    x: 0,
    y: 0,
    hasSelection: false
  };
  type ShellAssistAction = 'command' | 'done' | 'needs_input';
  type ShellAssistProposal = {
    action: ShellAssistAction;
    command: string;
    explanation: string;
    risk: string;
    notes: string[];
    message: string;
    responseId?: string | null;
  };
  type ShellAssistTurn = {
    id: string;
    command: string;
    explanation: string;
    risk: string;
    responseId?: string | null;
    approved: boolean;
    output?: string | null;
  };
  let shellAssistPanelOpen = false;
  let shellAssistPrompt = '';
  let shellAssistGoal = '';
  let shellAssistStatus = '';
  let shellAssistError: string | null = null;
  let shellAssistInFlight = false;
  let shellAssistProposal: ShellAssistProposal | null = null;
  let shellAssistTurns: ShellAssistTurn[] = [];
  let shellAssistRunId = 0;
  let shellTranscriptBuffer = '';
  let shellTranscriptRevision = 0;
  const SHELL_TRANSCRIPT_LIMIT = 12_000;
  const SHELL_ASSIST_OUTPUT_IDLE_MS = 1_500;
  const SHELL_ASSIST_OUTPUT_MAX_WAIT_MS = 15_000;

  // File Transfer state
  type FileTransferConflictMode = 'prompt' | 'skip' | 'overwrite' | 'rename';
  type FileTransferPhase = 'preparing' | 'transferring' | 'finalizing';
  type FileTransferJob = {
    id: string;
    direction: 'upload' | 'download';
    fileName: string;
    bytesDone: number;
    bytesTotal: number;
    status: 'running' | 'done' | 'error' | 'cancelled';
    phase?: FileTransferPhase;
    message?: string;
    createdAt: number;
    updatedAt: number;
  };
  type PendingConflictOperation = {
    kind: 'upload' | 'download';
    jobId: string;
    localPaths?: string[];
    remotePaths?: string[];
    destination: string;
    conflictPath: string;
    conflictMessage: string;
  };
  type FileTransferProgressEvent = {
    jobId: string;
    direction: 'upload' | 'download';
    fileName: string;
    bytesDone: number;
    bytesTotal: number;
    phase?: FileTransferPhase;
    message?: string;
  };
  let fileTransferSessionInfo: SessionParams | null = null;
  let fileTransferCapabilities: FileTransferCapabilities | null = null;
  let fileTransferStatus = '';
  let fileTransferError: string | null = null;
  let fileTransferConnected = false;
  let fileTransferConnectInFlight = false;
  let fileTransferTransport: 'quic' | 'relay' | null = null;
  let aiAssistStatus = '';
  let aiAssistError: string | null = null;
  let aiAssistDraft = '';
  let aiAssistLines: string[] = [];
  let aiAssistActionLines: string[] = [];
  let aiAssistCurrentTaskId: string | null = null;
  let aiAssistPlanLines: string[] = [];
  let aiAssistStepIndex = 0;
  let aiAssistMaxSteps = 0;
  let aiAssistTaskStatus: AiAssistTaskStatus | null = null;
  let aiAssistStopRequested = false;
  let aiAssistTranscriptEl: HTMLDivElement | null = null;
  let aiAssistInFlight = false;
  let aiAssistReadyForPrompt = false;
  let aiAssistPromptDisabled = true;
  let aiAssistCanSend = false;
  let viewerHeartbeatTimer: ReturnType<typeof setInterval> | null = null;
  const viewerConnectedSessionIds = new Set<string>();
  const VIEWER_HEARTBEAT_INTERVAL_MS = 2000;
  const QUICK_CONNECT_AUTO_TIMEOUT_MS = 2000;
  const QUICK_CONNECT_FORCED_TIMEOUT_MS = 3000;
  const SHELL_QUICK_CONNECT_AUTO_TIMEOUT_MS = 1000;
  let chatSessionInfo: SessionParams | null = null;
  let chatCapabilities: ChatSessionCapabilities | null = null;
  let chatConnected = false;
  let chatStatus = '';
  let chatError: string | null = null;
  type ChatDeliveryState = 'sending' | 'sent' | 'failed';
  let chatMessages: Array<{
    id: string;
    fromViewer: boolean;
    text: string;
    state?: ChatDeliveryState;
  }> = [];
  let chatDraft = '';
  let chatPanelOpen = false;
  let fileTransferLocalPath = '/';
  let fileTransferRemotePath = '/';
  // Tree browsing roots. Keep "/" to show drive roots on Windows.
  let fileTransferLocalBrowseRoot = '/';
  let fileTransferRemoteBrowseRoot = '/';
  let fileTransferLocalEntries: FileTransferEntry[] = [];
  let fileTransferRemoteEntries: FileTransferEntry[] = [];
  // Selection model:
  // - Selecting a directory selects it as a "root" (everything under it implicitly selected).
  // - Selecting a file selects only that file.
  let fileTransferLocalSelectedFiles = new Set<string>();
  let fileTransferRemoteSelectedFiles = new Set<string>();
  let fileTransferLocalSelectedDirs = new Set<string>();
  let fileTransferRemoteSelectedDirs = new Set<string>();
  let fileTransferJobs: FileTransferJob[] = [];
  let fileTransferPendingConflict: PendingConflictOperation | null = null;
  let fileTransferRunningJobs = 0;
  let fileTransferDoneJobs = 0;
  let fileTransferFailedJobs = 0;

  type FileTransferTreeRow = {
    entry: FileTransferEntry;
    depth: number;
  };

  let fileTransferLocalDirCache: Record<string, FileTransferEntry[]> = {};
  let fileTransferRemoteDirCache: Record<string, FileTransferEntry[]> = {};
  let fileTransferLocalExpandedDirs = new Set<string>();
  let fileTransferRemoteExpandedDirs = new Set<string>();
  let fileTransferLocalLoadingDirs = new Set<string>();
  let fileTransferRemoteLoadingDirs = new Set<string>();

  type FileTransferContextMenuSide = 'local' | 'remote';
  type FileTransferContextMenuState = {
    open: boolean;
    x: number;
    y: number;
    side: FileTransferContextMenuSide;
    entry: FileTransferEntry | null;
  };

  let fileTransferContextMenu: FileTransferContextMenuState = {
    open: false,
    x: 0,
    y: 0,
    side: 'local',
    entry: null
  };

  type FileTransferDialogMode = 'rename' | 'delete';
  let fileTransferDialogOpen = false;
  let fileTransferDialogBusy = false;
  let fileTransferDialogError: string | null = null;
  let fileTransferDialogMode: FileTransferDialogMode = 'rename';
  let fileTransferDialogSide: FileTransferContextMenuSide = 'local';
  let fileTransferDialogEntry: FileTransferEntry | null = null;
  let fileTransferDialogName = '';

  // App-level confirm dialog (avoid WebView prompt/confirm).
  let appConfirmOpen = false;
  let appConfirmTitle = '';
  let appConfirmBody = '';
  let appConfirmOkLabel = 'OK';
  let appConfirmCancelLabel = 'Cancel';
  let appConfirmHideCancel = false;
  let appConfirmResolve: ((value: boolean) => void) | null = null;

  const openAppConfirm = (options: {
    title: string;
    body: string;
    okLabel?: string;
    cancelLabel?: string;
    hideCancel?: boolean;
  }): Promise<boolean> => {
    appConfirmTitle = options.title;
    appConfirmBody = options.body;
    appConfirmOkLabel = options.okLabel ?? 'OK';
    appConfirmCancelLabel = options.cancelLabel ?? 'Cancel';
    appConfirmHideCancel = options.hideCancel ?? false;
    appConfirmOpen = true;
    return new Promise<boolean>((resolve) => {
      appConfirmResolve = resolve;
    });
  };

  const closeAppConfirm = (value: boolean) => {
    appConfirmOpen = false;
    appConfirmHideCancel = false;
    const resolve = appConfirmResolve;
    appConfirmResolve = null;
    resolve?.(value);
  };

  // System Info state
  type UnknownRecord = Record<string, unknown>;
  type SystemInfoDevice = {
    agentId: string;
    hostname: string;
    os: string;
    ip: string;
    version?: string | null;
    lastInventory?: unknown;
    deviceDetails?: unknown;
  };
  type SessionDeviceInfoResponse = {
    device?: SystemInfoDevice | null;
    refreshed?: boolean;
    refreshError?: string | null;
  };
  type SystemInfoAgentContext = {
    apiBase: string;
    agentId: string;
  };
  let systemInfoData: SessionDeviceInfoResponse | null = null;
  let systemInfoLoading = false;
  let systemInfoRefreshing = false;
  let systemInfoError: string | null = null;
  let systemInfoRefreshError: string | null = null;
  let systemInfoLastUpdated: Date | null = null;
  let systemInfoPollTimer: number | null = null;
  let systemInfoContext: SystemInfoAgentContext | null = null;
  const SYSTEM_INFO_CONTEXT_STORAGE_KEY = 'talos_viewer_system_info_context';
  const SYSTEM_INFO_REFRESH_INTERVAL_MS = 30_000;

  let videoQualityOpen = false;
  let monitorPickerOpen = false;
  const videoQualityOptions: VideoQualityOption[] = [
    { id: 'low', label: 'Low', bitrateKbps: 2500, hint: '2.5 Mbps' },
    { id: 'medium', label: 'Medium', bitrateKbps: 8000, hint: '8 Mbps' },
    { id: 'high', label: 'High', bitrateKbps: 20000, hint: '20 Mbps' }
  ];
  const CONNECTION_RTT_HISTORY_LIMIT = 48;
  const CONNECTION_SPARKLINE_WIDTH = 240;
  const CONNECTION_SPARKLINE_HEIGHT = 72;

  type ConnectionSessionKind =
    | 'remote_desktop'
    | 'system_shell'
    | 'file_transfer'
    | 'remote_registry';
  type ConnectionTransport = 'quic' | 'relay' | 'tcp';
  type ConnectionType = 'lan_direct' | 'hole_punch' | 'relay' | 'direct_tcp' | 'quic';
  type ConnectionEndpoint = {
    ip: string;
    port: number;
  };
  type ConnectionStatePayload = {
    sessionKind: ConnectionSessionKind;
    transport: ConnectionTransport;
    connectionType: ConnectionType;
    captureType?: string | null;
    encryptionLabel: string;
    encryptionDetails?: string | null;
    remoteAddr?: string | null;
    viewerReflex?: ConnectionEndpoint | null;
    agentReflex?: ConnectionEndpoint | null;
    agentLocalAddrs?: { ip: string; prefix: number }[];
    connectMs?: number | null;
    relayTcpMs?: number | null;
    relayTlsMs?: number | null;
    relayHandshakeMs?: number | null;
  };
  type ConnectionStatsPayload = ConnectionStatePayload & {
    sampleAtMs: number;
    rttMs?: number | null;
    avgRttMs?: number | null;
    minRttMs?: number | null;
    maxRttMs?: number | null;
    sampleCount?: number;
  };
  type ConnectionLatencyPoint = {
    sampleAtMs: number;
    rttMs: number;
  };
  let connectionInfoOpen = false;
  type ConnectionStateMap = Record<ConnectionSessionKind, ConnectionStatePayload | null>;
  type ConnectionStatsMap = Record<ConnectionSessionKind, ConnectionStatsPayload | null>;
  type ConnectionLatencyHistoryMap = Record<ConnectionSessionKind, ConnectionLatencyPoint[]>;
  const buildEmptyConnectionStateMap = (): ConnectionStateMap => ({
    remote_desktop: null,
    system_shell: null,
    file_transfer: null,
    remote_registry: null
  });
  const buildEmptyConnectionStatsMap = (): ConnectionStatsMap => ({
    remote_desktop: null,
    system_shell: null,
    file_transfer: null,
    remote_registry: null
  });
  const buildEmptyConnectionLatencyHistoryMap = (): ConnectionLatencyHistoryMap => ({
    remote_desktop: [],
    system_shell: [],
    file_transfer: [],
    remote_registry: []
  });
  let connectionStateByKind = buildEmptyConnectionStateMap();
  let connectionStatsByKind = buildEmptyConnectionStatsMap();
  let connectionLatencyHistoryByKind = buildEmptyConnectionLatencyHistoryMap();
  let activeConnectionKind: ConnectionSessionKind | null = null;
  let connectionState: ConnectionStatePayload | null = null;
  let connectionStats: ConnectionStatsPayload | null = null;
  let connectionLatencyHistory: ConnectionLatencyPoint[] = [];
  let connectionSummary: ConnectionStatePayload | ConnectionStatsPayload | null = null;

  let visibleTabs: ViewerTab[] = [];
  let activeAgentFeatures: AgentFeatureCapabilities = UNKNOWN_FEATURES;
  let activeAgentPlatform: AgentPlatform = 'unknown';

  const normalizeAgentPlatform = (value: unknown): AgentPlatform => {
    if (typeof value !== 'string') {
      return 'unknown';
    }
    const normalized = value.trim().toLowerCase().replace(/[_-]+/g, ' ');
    if (normalized === 'windows' || normalized.includes('windows')) {
      return 'windows';
    }
    if (normalized === 'linux' || normalized.includes('linux')) {
      return 'linux';
    }
    if (
      normalized === 'macos' ||
      normalized === 'mac os' ||
      normalized === 'mac' ||
      normalized.includes('macos') ||
      normalized.includes('mac os') ||
      normalized.includes('darwin') ||
      normalized.includes('os x')
    ) {
      return 'macos';
    }
    if (normalized === 'unknown') {
      return 'unknown';
    }
    return 'unknown';
  };

  type CapabilitySource = {
    platform?: AgentPlatform | string | null;
    features?: Partial<AgentFeatureCapabilities> | null;
    agentOs?: string | null;
    agentOS?: string | null;
    os?: string | null;
  };

  const inferAgentPlatform = (source: CapabilitySource | null): AgentPlatform => {
    if (!source) {
      return 'unknown';
    }
    const explicitPlatform = normalizeAgentPlatform(source.platform);
    if (explicitPlatform !== 'unknown') {
      return explicitPlatform;
    }
    return normalizeAgentPlatform(source.agentOs ?? source.agentOS ?? source.os);
  };

  const normalizeAgentFeatures = (
    features: Partial<AgentFeatureCapabilities> | null | undefined,
    platform: AgentPlatform
  ): AgentFeatureCapabilities => {
    const fallback =
      platform === 'macos'
        ? MACOS_FEATURES
        : platform === 'linux'
          ? LIMITED_UNIX_FEATURES
        : platform === 'unknown'
          ? UNKNOWN_FEATURES
          : WINDOWS_FEATURES;
    return {
      remoteDesktop: features?.remoteDesktop ?? fallback.remoteDesktop,
      systemShell: features?.systemShell ?? fallback.systemShell,
      fileTransfer: features?.fileTransfer ?? fallback.fileTransfer,
      remoteRegistry: features?.remoteRegistry ?? fallback.remoteRegistry,
      chat: features?.chat ?? fallback.chat,
      systemInfo: features?.systemInfo ?? fallback.systemInfo
    };
  };

  const getCapabilitySource = (): CapabilitySource | null => {
    if (activeTab === 'System Shell') {
      return shellCapabilities ?? capabilities ?? fileTransferCapabilities ?? registryCapabilities ?? chatCapabilities ?? null;
    }
    if (activeTab === 'File Transfer') {
      return fileTransferCapabilities ?? capabilities ?? shellCapabilities ?? registryCapabilities ?? chatCapabilities ?? null;
    }
    if (activeTab === 'Remote Registry') {
      return registryCapabilities ?? capabilities ?? shellCapabilities ?? fileTransferCapabilities ?? chatCapabilities ?? null;
    }
    if (activeTab === 'Remote Desktop') {
      return capabilities ?? shellCapabilities ?? fileTransferCapabilities ?? registryCapabilities ?? chatCapabilities ?? null;
    }
    return capabilities ?? shellCapabilities ?? fileTransferCapabilities ?? registryCapabilities ?? chatCapabilities ?? null;
  };

  const hasSessionContext = (): boolean =>
    !!(remoteSessionInfo || shellSessionInfo || fileTransferSessionInfo || registrySessionInfo || chatSessionInfo);

  const featureForTab = (tab: ViewerTab, features: AgentFeatureCapabilities): boolean => {
    if (tab === 'Remote Desktop') return features.remoteDesktop;
    if (tab === 'System Shell') return features.systemShell;
    if (tab === 'File Transfer') return features.fileTransfer;
    if (tab === 'Remote Registry') return features.remoteRegistry;
    return features.systemInfo;
  };

  const isTabSupported = (tab: ViewerTab): boolean => featureForTab(tab, activeAgentFeatures);
  const isWindowsAgentPlatform = (): boolean => activeAgentPlatform === 'windows';
  const isMacAgentPlatform = (): boolean => activeAgentPlatform === 'macos';
  const isWindowsShellPlatform = (): boolean => isWindowsAgentPlatform();
  const isChatSupported = (): boolean => activeAgentFeatures.chat;

  type WindowWithTauri = Window & { __TAURI_INTERNALS__?: unknown; __TAURI__?: unknown };
  type ViewerUpdateCheckResult = {
    status: 'no_update' | 'update_ready';
    version?: string | null;
  };
  type RemoteDesktopFramePayload = {
    imageBase64: string;
    width: number;
    height: number;
    mimeType?: string | null;
  };

  const isTauriRuntime = (): boolean => {
    if (typeof window === 'undefined') return false;
    const w = window as WindowWithTauri;
    // Support both Tauri v2 and older runtime shims.
    return '__TAURI_INTERNALS__' in w || '__TAURI__' in w;
  };

  const invokeTauri = <T = unknown>(
    command: string,
    args?: Record<string, unknown>
  ): Promise<T> => {
    // Avoid calling into @tauri-apps/api when running in a plain browser.
    if (!isTauriRuntime()) {
      return Promise.reject(
        new Error('Tauri backend unavailable in browser-only mode (run `cargo tauri dev`).')
      );
    }
    return invoke<T>(command, args);
  };

  type FetchWithTimeoutInit = RequestInit & { timeoutMs?: number };

  const DEFAULT_HTTP_TIMEOUT_MS = 10_000;

  const fetchWithTimeout = async (
    input: RequestInfo | URL,
    init: FetchWithTimeoutInit = {}
  ): Promise<Response> => {
    const { timeoutMs = DEFAULT_HTTP_TIMEOUT_MS, signal, ...rest } = init;
    const controller = new AbortController();
    const onAbort = () => controller.abort();
    if (signal) {
      if (signal.aborted) {
        controller.abort();
      } else {
        signal.addEventListener('abort', onAbort, { once: true });
      }
    }
    const timeoutId = window.setTimeout(() => controller.abort(), timeoutMs);
    try {
      return await fetch(input, { ...rest, signal: controller.signal });
    } catch (error) {
      // Normalize timeouts to a regular Error with a readable message.
      if (error instanceof DOMException && error.name === 'AbortError') {
        throw new Error(`Request timed out after ${timeoutMs}ms`);
      }
      throw error;
    } finally {
      window.clearTimeout(timeoutId);
      if (signal) {
        signal.removeEventListener('abort', onAbort);
      }
    }
  };

  const getSystemInfoSessionContext = (): SessionParams | null => remoteSessionInfo ?? shellSessionInfo;

  const getSystemInfoAgentContext = (): SystemInfoAgentContext | null => {
    const info = getSystemInfoSessionContext();
    if (info?.apiBase && info.agentId) {
      return {
        apiBase: info.apiBase,
        agentId: info.agentId
      };
    }
    return systemInfoContext;
  };

  const rememberSystemInfoContext = (info: SessionParams | null | undefined) => {
    if (!info?.apiBase || !info.agentId) {
      return;
    }
    systemInfoContext = {
      apiBase: info.apiBase,
      agentId: info.agentId
    };
    if (typeof window !== 'undefined') {
      try {
        window.localStorage.setItem(
          SYSTEM_INFO_CONTEXT_STORAGE_KEY,
          JSON.stringify(systemInfoContext)
        );
      } catch {
        // Best-effort persistence only.
      }
    }
  };

  const restoreSystemInfoContext = () => {
    if (typeof window === 'undefined') {
      return;
    }
    try {
      const raw = window.localStorage.getItem(SYSTEM_INFO_CONTEXT_STORAGE_KEY);
      if (!raw) {
        return;
      }
      const parsed = JSON.parse(raw) as Partial<SystemInfoAgentContext>;
      if (
        typeof parsed?.apiBase === 'string' &&
        parsed.apiBase.trim() &&
        typeof parsed?.agentId === 'string' &&
        parsed.agentId.trim()
      ) {
        systemInfoContext = {
          apiBase: parsed.apiBase,
          agentId: parsed.agentId
        };
      }
    } catch {
      // Ignore invalid local storage payloads.
    }
  };

  const isExpectedSessionExpiryError = (message: string | null): boolean => {
    if (!message) return false;
    const normalized = message.trim().toLowerCase();
    return (
      normalized.includes('session not found') ||
      normalized.includes('invalid session token') ||
      normalized.includes('session expired')
    );
  };

  const clearSystemInfoPolling = () => {
    if (systemInfoPollTimer !== null) {
      window.clearInterval(systemInfoPollTimer);
      systemInfoPollTimer = null;
    }
  };

  const startSystemInfoPolling = () => {
    if (systemInfoPollTimer !== null) {
      return;
    }
    systemInfoPollTimer = window.setInterval(() => {
      if (activeTab !== 'System Info') {
        return;
      }
      void fetchSystemInfo({ refresh: true, background: true });
    }, SYSTEM_INFO_REFRESH_INTERVAL_MS);
  };

  const fetchSystemInfo = async (
    options: { refresh?: boolean; background?: boolean } = {}
  ): Promise<void> => {
    const { refresh = true, background = false } = options;
    const sessionContext = getSystemInfoSessionContext();
    const agentContext = getSystemInfoAgentContext();
    if (!agentContext) {
      if (!background) {
        const message = 'System info context unavailable. Open a dashboard viewer link once.';
        if (systemInfoData?.device) {
          systemInfoRefreshError = message;
        } else {
          systemInfoRefreshError = null;
          systemInfoError = message;
        }
      }
      return;
    }

    if (systemInfoLoading || systemInfoRefreshing) {
      return;
    }

    if (background) {
      systemInfoRefreshing = true;
    } else {
      systemInfoLoading = true;
      systemInfoError = null;
    }

    try {
      let payload: SessionDeviceInfoResponse | null = null;
      let sessionError: string | null = null;
      const refreshFlag = refresh ? '1' : '0';
      if (sessionContext?.apiBase) {
        const sessionUrl = `${sessionContext.apiBase}/api/rmm/session/${sessionContext.sessionId}/device-info?token=${encodeURIComponent(sessionContext.token)}&refresh=${refreshFlag}`;
        try {
          const response = await fetchWithTimeout(sessionUrl, { timeoutMs: 12_000 });
          if (!response.ok) {
            const detail = await response.text().catch(() => '');
            throw new Error(detail || `System info request failed (${response.status})`);
          }
          payload = (await response.json()) as SessionDeviceInfoResponse;
        } catch (error) {
          sessionError = error instanceof Error ? error.message : String(error);
        }
      }
      if (isExpectedSessionExpiryError(sessionError)) {
        sessionError = null;
      }

      if (!payload?.device) {
        const agentUrl = `${agentContext.apiBase}/api/rmm/devices/${encodeURIComponent(agentContext.agentId)}/fetch-details`;
        const response = await fetchWithTimeout(agentUrl, { method: 'POST', timeoutMs: 25_000 });
        if (!response.ok) {
          const detail = await response.text().catch(() => '');
          throw new Error(detail || `System info request failed (${response.status})`);
        }
        const device = (await response.json()) as SystemInfoDevice;
        payload = {
          device,
          refreshed: true,
          refreshError: sessionError
        };
      } else if (sessionError && !payload.refreshError) {
        payload.refreshError = sessionError;
      }

      if (!payload.device) {
        throw new Error('System info payload is empty');
      }
      systemInfoData = payload;
      systemInfoRefreshError = payload.refreshError ?? null;
      systemInfoError = null;
      systemInfoLastUpdated = new Date();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (systemInfoData?.device) {
        systemInfoRefreshError = message;
      } else {
        systemInfoRefreshError = null;
        systemInfoError = message;
      }
    } finally {
      if (background) {
        systemInfoRefreshing = false;
      } else {
        systemInfoLoading = false;
      }
    }
  };

  const fileTransferResponseType = (value: unknown): string => {
    const response = value as { type?: string };
    return response?.type ?? '';
  };

  $: fileTransferRunningJobs = fileTransferJobs.filter((job) => job.status === 'running').length;
  $: fileTransferDoneJobs = fileTransferJobs.filter(
    (job) => job.status === 'done' || job.status === 'cancelled'
  ).length;
  $: fileTransferFailedJobs = fileTransferJobs.filter((job) => job.status === 'error').length;

  const isListDirResponse = (
    value: FileTransferResponse
  ): value is Extract<FileTransferResponse, { type: 'list_dir_result' }> =>
    fileTransferResponseType(value) === 'list_dir_result';

  const createFileTransferJob = (
    direction: 'upload' | 'download',
    fileName: string
  ): FileTransferJob => ({
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    direction,
    fileName,
    bytesDone: 0,
    bytesTotal: 0,
    status: 'running',
    phase: 'preparing',
    createdAt: Date.now(),
    updatedAt: Date.now()
  });

  const fileTransferJobSortKey = (status: FileTransferJob['status']): number => {
    // Active transfers always stick to the top.
    if (status === 'running') return 0;
    // Everything else below, keep creation order stable.
    return 1;
  };

  $: sortedFileTransferJobs = [...fileTransferJobs].sort((a, b) => {
    const ak = fileTransferJobSortKey(a.status);
    const bk = fileTransferJobSortKey(b.status);
    if (ak !== bk) return ak - bk;
    // New jobs should go to the bottom (within each group).
    return (a.createdAt ?? a.updatedAt) - (b.createdAt ?? b.updatedAt);
  });

  const upsertFileTransferJob = (nextJob: FileTransferJob) => {
    const candidate: FileTransferJob = {
      ...nextJob,
      createdAt: nextJob.createdAt ?? Date.now(),
      updatedAt: nextJob.updatedAt ?? Date.now()
    };
    const existingIndex = fileTransferJobs.findIndex((job) => job.id === nextJob.id);
    if (existingIndex >= 0) {
      const updated = [...fileTransferJobs];
      updated[existingIndex] = candidate;
      fileTransferJobs = updated;
      return;
    }
    // Append new jobs to the bottom of the queue.
    fileTransferJobs = [...fileTransferJobs, candidate].slice(-50);
  };

  const updateFileTransferJob = (jobId: string, updates: Partial<FileTransferJob>) => {
    const existing = fileTransferJobs.find((job) => job.id === jobId);
    if (!existing) return;
    upsertFileTransferJob({ ...existing, ...updates, updatedAt: Date.now() });
  };

  const clearFinishedFileTransferJobs = () => {
    fileTransferJobs = fileTransferJobs.filter((job) => job.status === 'running');
  };

  const formatFileTransferPhase = (phase?: FileTransferPhase): string => {
    if (!phase) return '';
    if (phase === 'preparing') return 'Preparing';
    if (phase === 'finalizing') return 'Finalizing';
    return 'Transferring';
  };

  const stripWindowsVerbatimPrefix = (input: string): string => {
    if (input.startsWith('\\\\?\\UNC\\')) {
      return `\\\\${input.slice('\\\\?\\UNC\\'.length)}`;
    }
    if (input.startsWith('\\\\?\\')) {
      return input.slice('\\\\?\\'.length);
    }
    return input;
  };

  const parentPath = (input: string): string => {
    const rawPath = input.trim();
    if (!rawPath) {
      return '/';
    }

    const path = stripWindowsVerbatimPrefix(rawPath);
    if (path === '/') {
      return '/';
    }
    if (/^[a-zA-Z]:[\\/]?$/.test(path)) {
      return '/';
    }

    const normalized = path.replace(/[\\/]+$/, '');
    if (!normalized) {
      return '/';
    }
    if (/^\\\\[^\\/]+[\\/][^\\/]+$/.test(normalized)) {
      return '/';
    }

    const slashIndex = Math.max(normalized.lastIndexOf('\\'), normalized.lastIndexOf('/'));
    if (slashIndex < 0) {
      return '/';
    }

    const candidate = normalized.slice(0, slashIndex);
    if (/^[a-zA-Z]:$/.test(candidate)) {
      return `${candidate}\\`;
    }
    if (!candidate) {
      return '/';
    }
    return candidate;
  };

  const canNavigateUp = (path: string): boolean => {
    const trimmed = stripWindowsVerbatimPrefix(path.trim());
    if (!trimmed) {
      return false;
    }
    if (trimmed === '/') {
      return false;
    }
    // Windows drive roots (e.g. "C:\") don't have a meaningful parent folder.
    if (/^[a-zA-Z]:[\\/]?$/.test(trimmed)) {
      return false;
    }
    // UNC share root (e.g. "\\server\share\") should also be treated as root.
    if (/^\\\\[^\\/]+[\\/][^\\/]+[\\/]?$/.test(trimmed)) {
      return false;
    }
    return true;
  };

  const formatFileTransferModified = (value?: number | null): string => {
    if (!value || value <= 0) {
      return '';
    }
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) {
      return '';
    }
    return parsed.toLocaleString();
  };

  const clearFileTransferSelections = () => {
    fileTransferLocalSelectedFiles = new Set<string>();
    fileTransferRemoteSelectedFiles = new Set<string>();
    fileTransferLocalSelectedDirs = new Set<string>();
    fileTransferRemoteSelectedDirs = new Set<string>();
  };

  const clearFileTransferTrees = () => {
    fileTransferLocalDirCache = {};
    fileTransferRemoteDirCache = {};
    fileTransferLocalExpandedDirs = new Set<string>();
    fileTransferRemoteExpandedDirs = new Set<string>();
    fileTransferLocalLoadingDirs = new Set<string>();
    fileTransferRemoteLoadingDirs = new Set<string>();
  };

  const fileTransferSortEntries = (entries: FileTransferEntry[]): FileTransferEntry[] => {
    return [...entries].sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
  };

  const buildFileTransferTreeRows = (
    rootPath: string,
    cache: Record<string, FileTransferEntry[]>,
    expanded: Set<string>
  ): FileTransferTreeRow[] => {
    const rows: FileTransferTreeRow[] = [];
    const visiting = new Set<string>();
    const visit = (dirPath: string, depth: number) => {
      if (visiting.has(dirPath)) {
        return;
      }
      visiting.add(dirPath);
      const entries = fileTransferSortEntries(cache[dirPath] ?? []);
      for (const entry of entries) {
        rows.push({ entry, depth });
        if (entry.isDir && entry.path !== dirPath && expanded.has(entry.path)) {
          visit(entry.path, depth + 1);
        }
      }
      visiting.delete(dirPath);
    };
    visit(rootPath, 0);
    return rows;
  };

  const setLocalDirCache = (path: string, entries: FileTransferEntry[]) => {
    fileTransferLocalDirCache = { ...fileTransferLocalDirCache, [path]: entries };
  };

  const setRemoteDirCache = (path: string, entries: FileTransferEntry[]) => {
    fileTransferRemoteDirCache = { ...fileTransferRemoteDirCache, [path]: entries };
  };

  const looksLikeWindowsDriveRoot = (path: string): boolean => {
    const trimmed = stripWindowsVerbatimPrefix(path.trim());
    return /^[a-zA-Z]:\\$/.test(trimmed);
  };

  const pickDefaultDestinationFromRootEntries = (entries: FileTransferEntry[]): string => {
    const drive = entries.find((entry) => entry.isDir && looksLikeWindowsDriveRoot(entry.path));
    if (drive) {
      return drive.path;
    }
    // On non-Windows, "/" is a valid absolute destination.
    return '/';
  };

  const joinChildPath = (dirPath: string, childName: string): string => {
    if (dirPath === '/') {
      return `/${childName}`;
    }
    if (/[\\/]$/.test(dirPath)) {
      return `${dirPath}${childName}`;
    }
    const sep = dirPath.includes('\\') ? '\\' : '/';
    return `${dirPath}${sep}${childName}`;
  };

  const openFileTransferContextMenu = (
    event: MouseEvent,
    side: FileTransferContextMenuSide,
    entry: FileTransferEntry
  ) => {
    if (!entry.isDir) return;
    event.preventDefault();
    event.stopPropagation();

    // Basic clamping to keep the menu visible.
    const menuWidth = 190;
    const menuHeight = 140;
    const pad = 8;
    const x = Math.max(pad, Math.min((event.clientX ?? 0) + 2, window.innerWidth - menuWidth - pad));
    const y = Math.max(pad, Math.min((event.clientY ?? 0) + 2, window.innerHeight - menuHeight - pad));

    fileTransferContextMenu = { open: true, x, y, side, entry };
  };

  const closeFileTransferContextMenu = () => {
    if (!fileTransferContextMenu.open) return;
    fileTransferContextMenu = { ...fileTransferContextMenu, open: false, entry: null };
  };

  const closeFileTransferDialog = () => {
    if (fileTransferDialogBusy) return;
    fileTransferDialogOpen = false;
    fileTransferDialogError = null;
    fileTransferDialogEntry = null;
    fileTransferDialogName = '';
  };

  const isNonEditableRootFolder = (entry: FileTransferEntry): boolean => {
    const path = entry.path.trim();
    return path === '/' || looksLikeWindowsDriveRoot(path);
  };

  const openFileTransferRenameDialog = (side: FileTransferContextMenuSide, entry: FileTransferEntry) => {
    if (!entry.isDir || isNonEditableRootFolder(entry)) return;
    closeFileTransferContextMenu();
    fileTransferDialogMode = 'rename';
    fileTransferDialogSide = side;
    fileTransferDialogEntry = entry;
    fileTransferDialogName = entry.name;
    fileTransferDialogError = null;
    fileTransferDialogBusy = false;
    fileTransferDialogOpen = true;
  };

  const openFileTransferDeleteDialog = (side: FileTransferContextMenuSide, entry: FileTransferEntry) => {
    if (!entry.isDir || isNonEditableRootFolder(entry)) return;
    closeFileTransferContextMenu();
    fileTransferDialogMode = 'delete';
    fileTransferDialogSide = side;
    fileTransferDialogEntry = entry;
    fileTransferDialogName = '';
    fileTransferDialogError = null;
    fileTransferDialogBusy = false;
    fileTransferDialogOpen = true;
  };

  const submitFileTransferDialog = async () => {
    const entry = fileTransferDialogEntry;
    if (!fileTransferDialogOpen || fileTransferDialogBusy || !entry || !entry.isDir) return;
    if (isNonEditableRootFolder(entry)) return;

    fileTransferDialogError = null;
    fileTransferDialogBusy = true;

    const parent = parentPath(entry.path);
    try {
      if (fileTransferDialogMode === 'rename') {
        const nextName = fileTransferDialogName.trim();
        if (!nextName) {
          fileTransferDialogError = 'Folder name cannot be empty.';
          return;
        }
        if (/[\\/]/.test(nextName)) {
          fileTransferDialogError = 'Folder name cannot contain slashes.';
          return;
        }
        const toPath = joinChildPath(parent, nextName);
        if (fileTransferDialogSide === 'local') {
          await invokeTauri('file_transfer_local_rename', { fromPath: entry.path, toPath });
          if (fileTransferLocalPath === entry.path) {
            await setFileTransferLocalDestination(toPath);
          }
          void refreshLocalDirCached(parent);
        } else {
          await invokeTauri('file_transfer_remote_rename', { fromPath: entry.path, toPath });
          if (fileTransferRemotePath === entry.path) {
            await setFileTransferRemoteDestination(toPath);
          }
          void refreshRemoteDirCached(parent);
        }
        fileTransferStatus = 'Rename completed';
        closeFileTransferDialog();
        return;
      }

      // delete
      if (fileTransferDialogSide === 'local') {
        await invokeTauri('file_transfer_local_delete', { path: entry.path, recursive: true });
        if (fileTransferLocalPath === entry.path) {
          await setFileTransferLocalDestination(parent);
        }
        void refreshLocalDirCached(parent);
      } else {
        await invokeTauri('file_transfer_remote_delete', { path: entry.path, recursive: true });
        if (fileTransferRemotePath === entry.path) {
          await setFileTransferRemoteDestination(parent);
        }
        void refreshRemoteDirCached(parent);
      }
      fileTransferStatus = 'Delete completed';
      closeFileTransferDialog();
    } catch (error) {
      fileTransferDialogError = error instanceof Error ? error.message : String(error);
      fileTransferStatus = fileTransferDialogMode === 'rename' ? 'Rename failed' : 'Delete failed';
    } finally {
      fileTransferDialogBusy = false;
    }
  };

  const refreshFileTransferFolder = async () => {
    const menu = fileTransferContextMenu;
    const entry = menu.entry;
    if (!menu.open || !entry || !entry.isDir) return;
    closeFileTransferContextMenu();
    try {
      if (menu.side === 'local') {
        await refreshLocalDirCached(entry.path);
      } else {
        await refreshRemoteDirCached(entry.path);
      }
      fileTransferStatus = 'Folder refreshed';
    } catch (error) {
      fileTransferError = error instanceof Error ? error.message : String(error);
      fileTransferStatus = 'Refresh failed';
    }
  };

  const hasSelectedAncestorDir = (path: string, selectedDirs: Set<string>): boolean => {
    let current = path;
    while (true) {
      if (selectedDirs.has(current)) return true;
      const parent = parentPath(current);
      if (parent === current || parent === '/') return selectedDirs.has(parent);
      current = parent;
    }
  };

  const isLocalEntrySelected = (entry: FileTransferEntry): boolean => {
    if (entry.isDir) {
      return hasSelectedAncestorDir(entry.path, fileTransferLocalSelectedDirs);
    }
    return (
      fileTransferLocalSelectedFiles.has(entry.path) ||
      hasSelectedAncestorDir(parentPath(entry.path), fileTransferLocalSelectedDirs)
    );
  };

  const isRemoteEntrySelected = (entry: FileTransferEntry): boolean => {
    if (entry.isDir) {
      return hasSelectedAncestorDir(entry.path, fileTransferRemoteSelectedDirs);
    }
    return (
      fileTransferRemoteSelectedFiles.has(entry.path) ||
      hasSelectedAncestorDir(parentPath(entry.path), fileTransferRemoteSelectedDirs)
    );
  };

  const isLocalEntryCheckboxDisabled = (entry: FileTransferEntry): boolean => {
    // Avoid confusing UX: when a parent dir is selected, children are implicitly selected.
    if (!entry.isDir && hasSelectedAncestorDir(parentPath(entry.path), fileTransferLocalSelectedDirs)) {
      return true;
    }
    return false;
  };

  const isRemoteEntryCheckboxDisabled = (entry: FileTransferEntry): boolean => {
    if (!entry.isDir && hasSelectedAncestorDir(parentPath(entry.path), fileTransferRemoteSelectedDirs)) {
      return true;
    }
    return false;
  };

  const ensureLocalDirCached = async (path: string) => {
    if (fileTransferLocalDirCache[path]) return;
    if (fileTransferLocalLoadingDirs.has(path)) return;
    fileTransferLocalLoadingDirs = new Set([...fileTransferLocalLoadingDirs, path]);
    try {
      const response = await invokeTauri<FileTransferResponse>('file_transfer_list_local', { path });
      if (!isListDirResponse(response)) {
        throw new Error('Unexpected local directory response');
      }
      // Cache under the requested key and also under any canonicalized response path.
      const entries = response.entries ?? [];
      setLocalDirCache(path, entries);
      if (response.path && response.path !== path) {
        setLocalDirCache(response.path, entries);
      }
      fileTransferError = null;
    } catch (error) {
      fileTransferError = error instanceof Error ? error.message : String(error);
      fileTransferStatus = 'Local folder unavailable';
      throw error;
    } finally {
      const next = new Set(fileTransferLocalLoadingDirs);
      next.delete(path);
      fileTransferLocalLoadingDirs = next;
    }
  };

  const ensureRemoteDirCached = async (path: string) => {
    if (fileTransferRemoteDirCache[path]) return;
    if (fileTransferRemoteLoadingDirs.has(path)) return;
    fileTransferRemoteLoadingDirs = new Set([...fileTransferRemoteLoadingDirs, path]);
    try {
      const response = await invokeTauri<FileTransferResponse>('file_transfer_list_remote', { path });
      if (!isListDirResponse(response)) {
        throw new Error('Unexpected remote directory response');
      }
      // Cache under the requested key and also under any canonicalized response path.
      const entries = response.entries ?? [];
      setRemoteDirCache(path, entries);
      if (response.path && response.path !== path) {
        setRemoteDirCache(response.path, entries);
      }
    } finally {
      const next = new Set(fileTransferRemoteLoadingDirs);
      next.delete(path);
      fileTransferRemoteLoadingDirs = next;
    }
  };

  const toggleLocalDirExpanded = async (path: string) => {
    const next = new Set(fileTransferLocalExpandedDirs);
    if (next.has(path)) {
      next.delete(path);
      fileTransferLocalExpandedDirs = next;
      return;
    }
    next.add(path);
    fileTransferLocalExpandedDirs = next;
    await ensureLocalDirCached(path);
  };

  const toggleRemoteDirExpanded = async (path: string) => {
    const next = new Set(fileTransferRemoteExpandedDirs);
    if (next.has(path)) {
      next.delete(path);
      fileTransferRemoteExpandedDirs = next;
      return;
    }
    next.add(path);
    fileTransferRemoteExpandedDirs = next;
    await ensureRemoteDirCached(path);
  };

  const refreshLocalDirCached = async (path: string) => {
    const next = { ...fileTransferLocalDirCache };
    delete next[path];
    fileTransferLocalDirCache = next;
    await ensureLocalDirCached(path);
  };

  const refreshRemoteDirCached = async (path: string) => {
    const next = { ...fileTransferRemoteDirCache };
    delete next[path];
    fileTransferRemoteDirCache = next;
    await ensureRemoteDirCached(path);
  };

  const setFileTransferLocalDestination = async (path: string) => {
    fileTransferLocalPath = path;
    if (path.trim() && path.trim() !== '/') {
      await ensureLocalDirCached(path);
    }
  };

  const setFileTransferRemoteDestination = async (path: string) => {
    fileTransferRemotePath = path;
    if (path.trim() && path.trim() !== '/') {
      await ensureRemoteDirCached(path);
    }
  };

  const selectLocalDir = async (path: string) => {
    await setFileTransferLocalDestination(path);
    fileTransferLocalExpandedDirs = new Set([...fileTransferLocalExpandedDirs, path]);
  };

  const selectRemoteDir = async (path: string) => {
    await setFileTransferRemoteDestination(path);
    fileTransferRemoteExpandedDirs = new Set([...fileTransferRemoteExpandedDirs, path]);
  };

  const refreshLocalFileList = async (path = fileTransferLocalPath) => {
    if (!isTauriRuntime()) return;
    const response = await invokeTauri<FileTransferResponse>('file_transfer_list_local', { path });
    if (!isListDirResponse(response)) {
      throw new Error('Unexpected local file list response');
    }
    fileTransferLocalPath = response.path;
    fileTransferLocalEntries = response.entries ?? [];
    fileTransferLocalSelectedFiles = new Set<string>();
    fileTransferLocalSelectedDirs = new Set<string>();
    fileTransferLocalExpandedDirs = new Set<string>();
    fileTransferLocalLoadingDirs = new Set<string>();
    fileTransferLocalDirCache = {};
    const entries = response.entries ?? [];
    setLocalDirCache(path, entries);
    if (response.path && response.path !== path) {
      setLocalDirCache(response.path, entries);
    }
  };

  const refreshRemoteFileList = async (path = fileTransferRemotePath) => {
    if (!isTauriRuntime()) return;
    const response = await invokeTauri<FileTransferResponse>('file_transfer_list_remote', { path });
    if (!isListDirResponse(response)) {
      throw new Error('Unexpected remote file list response');
    }
    fileTransferRemotePath = response.path;
    fileTransferRemoteEntries = response.entries ?? [];
    fileTransferRemoteSelectedFiles = new Set<string>();
    fileTransferRemoteSelectedDirs = new Set<string>();
    fileTransferRemoteExpandedDirs = new Set<string>();
    fileTransferRemoteLoadingDirs = new Set<string>();
    fileTransferRemoteDirCache = {};
    const entries = response.entries ?? [];
    setRemoteDirCache(path, entries);
    if (response.path && response.path !== path) {
      setRemoteDirCache(response.path, entries);
    }
  };

  const fetchFileTransferCapabilities = async (
    info: SessionParams
  ): Promise<FileTransferCapabilities> => {
    if (!info.apiBase) {
      throw new Error('Missing api base for file transfer session');
    }
    const url = `${info.apiBase}/api/rmm/file-transfer/session/${info.sessionId}/capabilities?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
    if (!response.ok) {
      throw new Error(`File transfer capability lookup failed (${response.status})`);
    }
    return response.json();
  };

  const fetchChatCapabilities = async (info: SessionParams): Promise<ChatSessionCapabilities> => {
    if (!info.apiBase) {
      throw new Error('Missing api base for chat session');
    }
    const url = `${info.apiBase}/api/rmm/chat/session/${info.sessionId}/capabilities?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
    if (!response.ok) {
      throw new Error(`Chat capability lookup failed (${response.status})`);
    }
    return response.json();
  };

  const upsertChatMessage = (message: {
    id: string;
    fromViewer: boolean;
    text: string;
    state?: ChatDeliveryState;
  }) => {
    if (chatMessages.some((existing) => existing.id === message.id)) return;
    chatMessages = [...chatMessages, message];
  };

  const markChatMessage = (id: string, state: ChatDeliveryState) => {
    chatMessages = chatMessages.map((message) =>
      message.id === id ? { ...message, state } : message
    );
  };

  const failPendingChatMessages = () => {
    chatMessages = chatMessages.map((message) =>
      message.state === 'sending' ? { ...message, state: 'failed' } : message
    );
  };

  const disconnectChatSession = async () => {
    const info = chatSessionInfo;
    chatConnected = false;
    chatStatus = '';
    chatError = null;
    chatMessages = [];
    chatDraft = '';
    chatCapabilities = null;
    chatSessionInfo = null;
    if (!isTauriRuntime() || !info?.apiBase || !info.sessionId || !info.token) return;
    try {
      await invokeTauri('viewer_chat_disconnect', {
        apiBase: info.apiBase,
        sessionId: info.sessionId,
        token: info.token
      });
    } catch {}
  };

  const connectChatSession = async (info: SessionParams) => {
    chatSessionInfo = info;
    chatError = null;
    chatConnected = false;
    chatStatus = 'Connecting chat…';

    if (!isTauriRuntime()) {
      chatStatus = 'Tauri backend unavailable in browser-only mode.';
      return;
    }

    try {
      viewerTransport = await invokeTauri<string>('get_viewer_transport');
      const caps = await fetchChatCapabilities(info);
      chatCapabilities = caps;
      await invokeTauri('viewer_chat_connect', {
        sessionId: info.sessionId,
        token: info.token,
        apiBase: info.apiBase,
        viewerTransport,
        transports: caps.transports,
        agentReflex: caps.agentReflex ?? undefined,
        agentHost: caps.agentHost ?? undefined,
        agentLocalAddrs: caps.agentLocalAddrs ?? undefined,
        pskCertPem: caps.pskCertPem ?? undefined,
        relayUrl: caps.relayUrl ?? undefined,
        e2eKey: caps.e2eKey ?? undefined,
        quicTimeoutMs: quickConnectTimeoutMs()
      });
      chatConnected = true;
      chatStatus = 'Chat connected';
    } catch (err) {
      chatConnected = false;
      chatError = err instanceof Error ? err.message : String(err);
      chatStatus = 'Chat failed';
    }
  };

  const requestChatConnectFromRemote = async (): Promise<SessionParams> => {
    const sessionContext = remoteSessionInfo;
    if (!sessionContext?.apiBase || !sessionContext.sessionId || !sessionContext.token) {
      throw new Error('Remote session context is missing');
    }
    const url = `${sessionContext.apiBase}/api/rmm/session/${sessionContext.sessionId}/open-chat?token=${encodeURIComponent(sessionContext.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      const detail = await response.text().catch(() => '');
      throw new Error(detail || `Chat connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), sessionContext);
    if (!parsed) {
      throw new Error('Invalid chat connect URL received');
    }
    return parsed;
  };

  const closeChatPanel = () => {
    chatPanelOpen = false;
    void scheduleViewportRectRefresh();
  };

  const openChatPanel = () => {
    if (!isChatSupported()) {
      return;
    }
    settingsOpen = false;
    aiAssistPanelOpen = false;
    shellAssistPanelOpen = false;
    shellCredentialPanelOpen = false;
    connectionInfoOpen = false;
    remoteDesktopDropdownOpen = false;
    shellRunAsDropdownOpen = false;
    chatPanelOpen = true;
    if (!chatSessionInfo && remoteDesktopConnected && remoteSessionInfo?.sessionId) {
      chatStatus = 'Connecting chat…';
      void (async () => {
        try {
          const nextSession = await requestChatConnectFromRemote();
          chatSessionInfo = nextSession;
          await connectChatSession(nextSession);
        } catch (err) {
          chatConnected = false;
          chatStatus = 'Chat failed';
          chatError = err instanceof Error ? err.message : String(err);
        }
      })();
    }
    void scheduleViewportRectRefresh();
  };

  const toggleChatPanel = () => {
    if (chatPanelOpen) {
      closeChatPanel();
      return;
    }
    openChatPanel();
  };

  const sendViewerChatMessage = async () => {
    const t = chatDraft.trim();
    if (!t || !chatConnected) return;
    chatDraft = '';
    try {
      const sent = await invokeTauri<Record<string, unknown>>('viewer_chat_send', { text: t });
      if (typeof sent.id === 'string' && typeof sent.text === 'string') {
        upsertChatMessage({
          id: sent.id,
          fromViewer: true,
          text: sent.text,
          state: 'sending'
        });
      }
    } catch (err) {
      chatDraft = t;
      chatError = err instanceof Error ? err.message : String(err);
    }
  };

  const requestFileTransferConnectFromRemote = async (): Promise<SessionParams> => {
    const sessionContext = remoteSessionInfo ?? shellSessionInfo;
    if (!sessionContext?.apiBase || !sessionContext.sessionId || !sessionContext.token) {
      throw new Error('Remote session context is missing');
    }

    const isShellContext = sessionContext.mode === 'shell';
    const url = isShellContext
      ? `${sessionContext.apiBase}/api/rmm/shell/session/${sessionContext.sessionId}/open-file-transfer?token=${encodeURIComponent(sessionContext.token)}`
      : `${sessionContext.apiBase}/api/rmm/session/${sessionContext.sessionId}/open-file-transfer?token=${encodeURIComponent(sessionContext.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      const detail = await response.text().catch(() => '');
      throw new Error(detail || `File transfer connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), sessionContext);
    if (!parsed) {
      throw new Error('Invalid file transfer connect URL received');
    }
    return parsed;
  };

  const disconnectFileTransfer = async () => {
    const sessionInfo = fileTransferSessionInfo;
    resetConnectionInfo('file_transfer');
    fileTransferConnected = false;
    fileTransferTransport = null;
    fileTransferError = null;
    fileTransferStatus = '';
    fileTransferCapabilities = null;
    fileTransferLocalEntries = [];
    fileTransferRemoteEntries = [];
    fileTransferLocalPath = '/';
    fileTransferRemotePath = '/';
    fileTransferLocalBrowseRoot = '/';
    fileTransferRemoteBrowseRoot = '/';
    fileTransferPendingConflict = null;
    clearFileTransferSelections();
    clearFileTransferTrees();
    fileTransferSessionInfo = null;
    if (!isTauriRuntime()) return;
    try {
      await invokeTauri('file_transfer_disconnect');
    } catch {}
    if (sessionInfo?.apiBase && sessionInfo.sessionId && sessionInfo.token) {
      try {
        await fetchWithTimeout(
          `${sessionInfo.apiBase}/api/rmm/file-transfer/session/${sessionInfo.sessionId}/end?token=${encodeURIComponent(sessionInfo.token)}`,
          { method: 'POST', timeoutMs: 8_000 }
        );
      } catch {}
    }
  };

  const connectFileTransferSession = async (info: SessionParams) => {
    fileTransferSessionInfo = info;
    resetConnectionInfo('file_transfer');
    fileTransferStatus = 'Fetching file transfer capabilities...';
    fileTransferError = null;
    fileTransferConnected = false;
    fileTransferTransport = null;

    if (!isTauriRuntime()) {
      fileTransferStatus = 'Tauri backend unavailable in browser-only mode.';
      return;
    }

    try {
      viewerTransport = await invokeTauri<string>('get_viewer_transport');
      const caps = await fetchFileTransferCapabilities(info);
      fileTransferCapabilities = caps;
      const transport = await invokeTauri<string>('file_transfer_connect', {
        sessionId: info.sessionId,
        token: info.token,
        apiBase: info.apiBase ?? '',
        viewerTransport,
        transports: caps.transports ?? [],
        agentReflex: caps.agentReflex ?? undefined,
        agentHost: caps.agentHost ?? undefined,
        agentLocalAddrs: caps.agentLocalAddrs ?? undefined,
        pskCertPem: caps.pskCertPem ?? undefined,
        relayUrl: caps.relayUrl ?? undefined,
        e2eKey: caps.e2eKey ?? undefined,
        quicTimeoutMs: quickConnectTimeoutMs()
      });
      fileTransferTransport = transport === 'relay' ? 'relay' : 'quic';

      // Seed the tree roots BEFORE setting fileTransferConnected so the UI
      // never sees a connected-but-empty state that would trigger a reactive
      // auto-fetch racing with our own initialisation.
      clearFileTransferSelections();
      clearFileTransferTrees();
      fileTransferLocalBrowseRoot = '/';
      fileTransferRemoteBrowseRoot = '/';

      await Promise.all([ensureLocalDirCached('/'), ensureRemoteDirCached('/')]);

      const localRootEntries = fileTransferLocalDirCache['/'] ?? [];
      const remoteRootEntries = fileTransferRemoteDirCache['/'] ?? [];

      const defaultLocal = pickDefaultDestinationFromRootEntries(localRootEntries);
      const defaultRemote = pickDefaultDestinationFromRootEntries(remoteRootEntries);

      await Promise.all([setFileTransferLocalDestination(defaultLocal), setFileTransferRemoteDestination(defaultRemote)]);

      // Expand the default destination so the user immediately sees a folder list.
      fileTransferLocalExpandedDirs = new Set([...fileTransferLocalExpandedDirs, defaultLocal]);
      fileTransferRemoteExpandedDirs = new Set([...fileTransferRemoteExpandedDirs, defaultRemote]);

      await Promise.all([ensureLocalDirCached(defaultLocal), ensureRemoteDirCached(defaultRemote)]);

      // Mark connected only now — the cache is fully populated so the UI
      // renders correct data on the very first frame.
      fileTransferConnected = true;
      fileTransferStatus = `Connected via ${fileTransferTransport.toUpperCase()}`;
      void notifyViewerSessionConnected(info);
    } catch (error) {
      resetConnectionInfo('file_transfer');
      fileTransferError = error instanceof Error ? error.message : String(error);
      fileTransferStatus = 'Connection failed';
      fileTransferConnected = false;
      fileTransferTransport = null;
    }
  };

  const CONTROL_MOD_CTRL = 1;
  const CONTROL_MOD_SHIFT = 2;
  const CONTROL_MOD_ALT = 4;
  const CONTROL_MOD_WIN = 8;

  const aiAssistReady = () =>
    activeTab === 'Remote Desktop' &&
    remoteDesktopConnected &&
    !!remoteSessionInfo?.sessionId &&
    !!remoteSessionInfo?.token &&
    !!remoteSessionInfo?.apiBase &&
    !!remoteSessionInfo?.backendApi;

  $: aiAssistReadyForPrompt =
    activeTab === 'Remote Desktop' &&
    remoteDesktopConnected &&
    !!remoteSessionInfo?.sessionId &&
    !!remoteSessionInfo?.token &&
    !!remoteSessionInfo?.apiBase &&
    !!remoteSessionInfo?.backendApi;
  $: aiAssistPromptDisabled = aiAssistInFlight;
  $: aiAssistCanSend = !aiAssistInFlight && aiAssistDraft.trim().length > 0;

  const AI_ASSIST_READY_STATUS = 'Ready — describe a desktop goal for AI Assist.';

  const refreshAiAssistStatus = (force = false) => {
    if (aiAssistInFlight) {
      return;
    }
    let nextStatus: string;
    if (!isTauriRuntime()) {
      nextStatus = 'Tauri backend unavailable in browser-only mode.';
    } else if (activeTab !== 'Remote Desktop' || !remoteDesktopConnected || !remoteSessionInfo?.sessionId) {
      nextStatus = 'Open a live remote desktop session first.';
    } else if (!remoteSessionInfo.backendApi) {
      nextStatus = 'Reopen the viewer from the web dashboard so AI Assist receives the API backend URL.';
    } else {
      nextStatus = AI_ASSIST_READY_STATUS;
    }

    if (
      !force &&
      nextStatus === AI_ASSIST_READY_STATUS &&
      /^(Task complete|Stopped|AI desktop task needs approval|Executed \d+ AI action)/.test(aiAssistStatus)
    ) {
      return;
    }
    aiAssistStatus = nextStatus;
  };

  const formatAiAssistTaskStatus = (status: AiAssistTaskStatus | null): string =>
    status ? status.replace('_', ' ') : 'idle';

  const formatAiAssistAction = (action: AiAssistAction): string => {
    switch (action.type) {
      case 'move':
        return `Move to (${action.x}, ${action.y})`;
      case 'click':
        return `Click ${action.button} at (${action.x}, ${action.y})`;
      case 'double_click':
        return `Double-click ${action.button} at (${action.x}, ${action.y})`;
      case 'drag': {
        const first = action.path[0];
        const last = action.path[action.path.length - 1];
        if (!first || !last) {
          return `Drag ${action.button} with 0 points`;
        }
        return `Drag ${action.button} from (${first.x}, ${first.y}) to (${last.x}, ${last.y}) with ${action.path.length} points`;
      }
      case 'scroll':
        return `Scroll at (${action.x}, ${action.y}) by ${action.scrollY !== 0 ? action.scrollY : action.scrollX}`;
      case 'type':
        return `Type "${action.text}"`;
      case 'keypress':
        return `Keypress ${action.keys.join(' + ')}`;
      case 'wait':
        return `Wait ${action.ms} ms`;
    }
  };

  const normalizeAiKeyName = (raw: string): string => raw.trim().toUpperCase().replace(/\s+/g, '');

  const splitAiKeys = (keys: string[]): string[] =>
    keys.flatMap((key) =>
      key
        .split('+')
        .map((part) => normalizeAiKeyName(part))
        .filter((part) => part.length > 0)
    );

  const aiModifierBit = (key: string): number => {
    switch (normalizeAiKeyName(key)) {
      case 'CTRL':
      case 'CONTROL':
        return CONTROL_MOD_CTRL;
      case 'SHIFT':
        return CONTROL_MOD_SHIFT;
      case 'ALT':
      case 'OPTION':
        return CONTROL_MOD_ALT;
      case 'WIN':
      case 'META':
      case 'CMD':
      case 'COMMAND':
        return CONTROL_MOD_WIN;
      default:
        return 0;
    }
  };

  const aiKeyToVkey = (raw: string): number | null => {
    const key = normalizeAiKeyName(raw);
    switch (key) {
      case 'ENTER':
      case 'RETURN':
        return 0x0d;
      case 'ESC':
      case 'ESCAPE':
        return 0x1b;
      case 'TAB':
        return 0x09;
      case 'SPACE':
        return 0x20;
      case 'BACKSPACE':
        return 0x08;
      case 'DELETE':
      case 'DEL':
        return 0x2e;
      case 'HOME':
        return 0x24;
      case 'END':
        return 0x23;
      case 'PAGEUP':
        return 0x21;
      case 'PAGEDOWN':
        return 0x22;
      case 'UP':
      case 'ARROWUP':
        return 0x26;
      case 'DOWN':
      case 'ARROWDOWN':
        return 0x28;
      case 'LEFT':
      case 'ARROWLEFT':
        return 0x25;
      case 'RIGHT':
      case 'ARROWRIGHT':
        return 0x27;
      case 'INSERT':
        return 0x2d;
      case 'CTRL':
      case 'CONTROL':
        return 0x11;
      case 'SHIFT':
        return 0x10;
      case 'ALT':
      case 'OPTION':
        return 0x12;
      case 'WIN':
      case 'META':
      case 'CMD':
      case 'COMMAND':
        return 0x5b;
      default:
        if (/^[A-Z0-9]$/.test(key)) {
          return key.charCodeAt(0);
        }
        if (/^F([1-9]|1[0-2])$/.test(key)) {
          return 0x70 + Number(key.slice(1)) - 1;
        }
        return null;
    }
  };

  const AI_ASSIST_DEFAULT_SETTLE_MS = 500;
  const AI_ASSIST_MAX_SETTLE_MS = 30_000;

  const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

  const clampAiAssistWaitMs = (ms: number): number =>
    Math.max(0, Math.min(Number.isFinite(ms) ? ms : 0, AI_ASSIST_MAX_SETTLE_MS));

  const aiAssistPostActionSettleMs = (actions: AiAssistAction[]): number => {
    const finalAction = actions[actions.length - 1];
    return finalAction?.type === 'wait'
      ? clampAiAssistWaitMs(finalAction.ms)
      : AI_ASSIST_DEFAULT_SETTLE_MS;
  };

  const aiAssistImmediateActions = (actions: AiAssistAction[]): AiAssistAction[] => {
    const finalAction = actions[actions.length - 1];
    return finalAction?.type === 'wait' ? actions.slice(0, -1) : actions;
  };

  const sendAiControlEvent = async (event: Record<string, unknown>) => {
    if (!isTauriRuntime()) {
      throw new Error('Tauri backend unavailable in browser-only mode.');
    }
    if (!remoteDesktopConnected) {
      throw new Error('Remote desktop control is not connected.');
    }
    await invokeTauri('send_control', { event });
  };

  const sendAiModifierKey = async (key: string, down: boolean) => {
    const vkey = aiKeyToVkey(key);
    if (vkey === null) {
      throw new Error(`Unsupported modifier key: ${key}`);
    }
    await sendAiControlEvent({
      type: down ? 'keyDown' : 'keyUp',
      vkey,
      scan: 0,
      modifiers: 0
    });
    await sleep(20);
  };

  const withAiMouseModifiers = async (keys: string[], run: () => Promise<void>) => {
    const modifierKeys = splitAiKeys(keys).filter((key) => aiModifierBit(key) !== 0);
    for (const key of modifierKeys) {
      await sendAiModifierKey(key, true);
    }
    try {
      await run();
    } finally {
      for (const key of [...modifierKeys].reverse()) {
        await sendAiModifierKey(key, false);
      }
    }
  };

  const sendAiMouseMove = async (x: number, y: number, width: number, height: number) => {
    await sendAiControlEvent({
      type: 'mouseMove',
      x,
      y,
      elementWidth: width,
      elementHeight: height
    });
  };

  const sendAiMouseButton = async (
    button: 'left' | 'right' | 'middle',
    down: boolean,
    x: number,
    y: number,
    width: number,
    height: number
  ) => {
    const mappedButton = button === 'right' ? 1 : button === 'middle' ? 2 : 0;
    await sendAiControlEvent({
      type: 'mouseButton',
      button: mappedButton,
      down,
      x,
      y,
      elementWidth: width,
      elementHeight: height
    });
  };

  const sendAiMouseWheel = async (
    x: number,
    y: number,
    delta: number,
    width: number,
    height: number
  ) => {
    await sendAiControlEvent({
      type: 'mouseWheel',
      delta,
      x,
      y,
      elementWidth: width,
      elementHeight: height
    });
  };

  const sendAiKeypress = async (keys: string[]) => {
    const expandedKeys = splitAiKeys(keys);
    const modifierMask = expandedKeys.reduce((mask, key) => mask | aiModifierBit(key), 0);
    const nonModifierKeys = expandedKeys.filter((key) => aiModifierBit(key) === 0);

    if (nonModifierKeys.length === 0) {
      for (const key of expandedKeys) {
        await sendAiModifierKey(key, true);
      }
      for (const key of [...expandedKeys].reverse()) {
        await sendAiModifierKey(key, false);
      }
      return;
    }

    for (const key of nonModifierKeys) {
      const vkey = aiKeyToVkey(key);
      if (vkey === null) {
        throw new Error(`Unsupported keypress key: ${key}`);
      }
      await sendAiControlEvent({
        type: 'keyDown',
        vkey,
        scan: 0,
        modifiers: modifierMask
      });
      await sleep(20);
      await sendAiControlEvent({
        type: 'keyUp',
        vkey,
        scan: 0,
        modifiers: modifierMask
      });
      await sleep(35);
    }
  };

  const executeAiAssistActions = async (
    actions: AiAssistAction[],
    snapshot: RemoteDesktopSnapshot
  ) => {
    for (const action of actions) {
      switch (action.type) {
        case 'move':
          await withAiMouseModifiers(action.keys, async () => {
            await sendAiMouseMove(action.x, action.y, snapshot.width, snapshot.height);
          });
          await sleep(40);
          break;
        case 'click':
        case 'double_click':
          await withAiMouseModifiers(action.keys, async () => {
            const clicks = action.type === 'double_click' ? 2 : 1;
            await sendAiMouseMove(action.x, action.y, snapshot.width, snapshot.height);
            await sleep(40);
            for (let index = 0; index < clicks; index += 1) {
              await sendAiMouseButton(action.button, true, action.x, action.y, snapshot.width, snapshot.height);
              await sleep(25);
              await sendAiMouseButton(action.button, false, action.x, action.y, snapshot.width, snapshot.height);
              await sleep(70);
            }
          });
          break;
        case 'drag': {
          if (action.path.length < 2) {
            throw new Error('AI drag action requires at least 2 path points.');
          }
          await withAiMouseModifiers(action.keys, async () => {
            const [first, ...remaining] = action.path;
            let current = first;
            let buttonDown = false;
            await sendAiMouseMove(first.x, first.y, snapshot.width, snapshot.height);
            await sleep(40);
            try {
              await sendAiMouseButton(action.button, true, first.x, first.y, snapshot.width, snapshot.height);
              buttonDown = true;
              await sleep(25);
              for (const point of remaining) {
                current = point;
                await sendAiMouseMove(point.x, point.y, snapshot.width, snapshot.height);
                await sleep(30);
              }
            } finally {
              if (buttonDown) {
                await sendAiMouseButton(
                  action.button,
                  false,
                  current.x,
                  current.y,
                  snapshot.width,
                  snapshot.height
                );
                await sleep(25);
              }
            }
          });
          break;
        }
        case 'scroll': {
          const verticalDelta = action.scrollY !== 0 ? action.scrollY : action.scrollX;
          if (verticalDelta === 0) {
            break;
          }
          const wheelNotches = Math.max(1, Math.min(20, Math.ceil(Math.abs(verticalDelta) / 120)));
          const delta = Math.sign(verticalDelta) * 120;
          await withAiMouseModifiers(action.keys, async () => {
            await sendAiMouseMove(action.x, action.y, snapshot.width, snapshot.height);
            await sleep(40);
            for (let index = 0; index < wheelNotches; index += 1) {
              await sendAiMouseWheel(action.x, action.y, delta, snapshot.width, snapshot.height);
              await sleep(45);
            }
          });
          break;
        }
        case 'type':
          await sendAiControlEvent({ type: 'typedInput', text: action.text });
          await sleep(50);
          break;
        case 'keypress':
          await sendAiKeypress(action.keys);
          break;
        case 'wait':
          await sleep(Math.max(0, Math.min(action.ms, 30_000)));
          break;
      }
    }
  };

  const requestAiAssistActions = async (
    info: SessionParams,
    prompt: string,
    snapshot: RemoteDesktopSnapshot
  ): Promise<AiAssistResponse> => {
    if (!info.backendApi) {
      throw new Error('The viewer does not know the API backend URL for this session.');
    }
    const response = await fetchWithTimeout(`${info.backendApi}/rmm/ai/desktop-action`, {
      method: 'POST',
      timeoutMs: 90_000,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        prompt,
        screenshotBase64: snapshot.imageBase64,
        width: snapshot.width,
        height: snapshot.height,
        sessionId: info.sessionId,
        sessionToken: info.token,
        rmmApiBase: info.apiBase,
        platform: activeAgentPlatform
      })
    });
    if (!response.ok) {
      const detail = await response
        .json()
        .then((payload) =>
          typeof payload?.error === 'string' && payload.error.trim() ? payload.error.trim() : ''
        )
        .catch(async () => (await response.text().catch(() => '')).trim());
      throw new Error(detail || `AI desktop action request failed (${response.status})`);
    }
    return response.json();
  };

  const parseAiAssistErrorDetail = async (response: Response, fallback: string): Promise<string> => {
    const detail = await response
      .clone()
      .json()
      .then((payload) =>
        typeof payload?.error === 'string' && payload.error.trim() ? payload.error.trim() : ''
      )
      .catch(async () => (await response.text().catch(() => '')).trim());
    return detail || fallback;
  };

  const formatAiAssistRequestError = (error: unknown): string => {
    const message = error instanceof Error ? error.message : String(error);
    if (/load failed|failed to fetch|networkerror|cors|origin/i.test(message)) {
      return `AI backend request failed before the server returned details (${message}). Check backend connectivity and CORS for the packaged Tauri viewer.`;
    }
    return message;
  };

  const appendShellTranscript = (text: string) => {
    if (!text) return;
    shellTranscriptBuffer = `${shellTranscriptBuffer}${text}`.slice(-SHELL_TRANSCRIPT_LIMIT);
    shellTranscriptRevision += 1;
  };

  const stripShellControlSequences = (value: string): string =>
    value
      .replace(/\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g, '')
      .replace(/\r\n/g, '\n')
      .replace(/\r/g, '\n');

  const shellTranscriptForAi = (): string =>
    stripShellControlSequences(shellTranscriptBuffer).slice(-SHELL_TRANSCRIPT_LIMIT);

  const shellAssistHistoryForAi = () =>
    shellAssistTurns.slice(-8).map((turn) => ({
      command: turn.command,
      approved: turn.approved,
      output: turn.output ? stripShellControlSequences(turn.output).slice(-4_000) : null,
      responseId: turn.responseId ?? null
    }));

  const shellAssistActionLabel = (action: ShellAssistAction): string => {
    if (action === 'done') return 'Complete';
    if (action === 'needs_input') return 'Needs input';
    return 'Command';
  };

  const shellAssistTranscriptDelta = (before: string): string => {
    const current = shellTranscriptBuffer;
    if (!before) return current;
    return current.startsWith(before) ? current.slice(before.length) : current;
  };

  const shellAssistReady = () =>
    activeTab === 'System Shell' &&
    shellConnected &&
    !!shellSessionInfo?.sessionId &&
    !!shellSessionInfo?.token &&
    !!shellSessionInfo?.backendApi;

  const requestShellAssistProposal = async (
    info: SessionParams,
    prompt: string
  ): Promise<ShellAssistProposal> => {
    if (!info.backendApi) {
      throw new Error('The viewer does not know the API backend URL for this session.');
    }
    const response = await fetchWithTimeout(`${info.backendApi}/rmm/ai/shell-assist`, {
      method: 'POST',
      timeoutMs: 90_000,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        prompt,
        transcript: shellTranscriptForAi(),
        history: shellAssistHistoryForAi(),
        sessionId: info.sessionId,
        sessionToken: info.token,
        rmmApiBase: info.apiBase,
        platform: activeAgentPlatform
      })
    });
    if (!response.ok) {
      throw new Error(await parseAiAssistErrorDetail(response, `AI shell assist failed (${response.status})`));
    }
    return response.json();
  };

  const waitForShellTranscriptIdle = async (runId: number): Promise<'idle' | 'timeout' | 'cancelled'> =>
    new Promise((resolve) => {
      const startedAt = Date.now();
      let lastRevision = shellTranscriptRevision;
      let lastChangedAt = Date.now();

      const check = () => {
        if (runId !== shellAssistRunId || !shellConnected) {
          resolve('cancelled');
          return;
        }
        if (shellTranscriptRevision !== lastRevision) {
          lastRevision = shellTranscriptRevision;
          lastChangedAt = Date.now();
        }
        if (Date.now() - lastChangedAt >= SHELL_ASSIST_OUTPUT_IDLE_MS) {
          resolve('idle');
          return;
        }
        if (Date.now() - startedAt >= SHELL_ASSIST_OUTPUT_MAX_WAIT_MS) {
          resolve('timeout');
          return;
        }
        window.setTimeout(check, 250);
      };

      window.setTimeout(check, 250);
    });

  const logShellAssistApproval = async (
    info: SessionParams,
    proposal: ShellAssistProposal
  ): Promise<void> => {
    if (!info.backendApi) return;
    const response = await fetchWithTimeout(`${info.backendApi}/rmm/ai/shell-assist/approved`, {
      method: 'POST',
      timeoutMs: 12_000,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        command: proposal.command,
        responseId: proposal.responseId ?? null,
        sessionId: info.sessionId,
        sessionToken: info.token,
        rmmApiBase: info.apiBase,
        platform: activeAgentPlatform
      })
    });
    if (!response.ok) {
      throw new Error(await parseAiAssistErrorDetail(response, `AI shell approval log failed (${response.status})`));
    }
  };

  const requestNextShellAssistTurn = async (goal: string, runId: number) => {
    if (!shellAssistReady() || !shellSessionInfo) {
      shellAssistError = 'No active system shell session is ready for AI Assist.';
      shellAssistStatus = 'Shell AI Assist unavailable';
      return;
    }
    shellAssistInFlight = true;
    shellAssistError = null;
    shellAssistProposal = null;
    shellAssistStatus = shellAssistTurns.length > 0 ? 'Observing output and planning the next turn...' : 'Planning the first shell turn...';
    try {
      const proposal = await requestShellAssistProposal(shellSessionInfo, goal);
      if (runId !== shellAssistRunId) return;
      shellAssistProposal = {
        action: proposal.action ?? 'command',
        command: proposal.command ?? '',
        explanation: proposal.explanation ?? '',
        risk: proposal.risk ?? '',
        notes: Array.isArray(proposal.notes) ? proposal.notes : [],
        message: proposal.message ?? proposal.explanation ?? '',
        responseId: proposal.responseId ?? null
      };
      shellAssistStatus =
        shellAssistProposal.action === 'command'
          ? `Review turn ${shellAssistTurns.length + 1} before it is sent to the terminal.`
          : shellAssistProposal.message || shellAssistActionLabel(shellAssistProposal.action);
    } catch (error) {
      shellAssistError = formatAiAssistRequestError(error);
      shellAssistStatus = 'AI request failed';
    } finally {
      shellAssistInFlight = false;
    }
  };

  const startShellAssistGoal = async () => {
    const goal = shellAssistPrompt.trim();
    if (!goal || shellAssistInFlight) return;
    const runId = shellAssistRunId + 1;
    shellAssistRunId = runId;
    shellAssistGoal = goal;
    shellAssistPrompt = '';
    shellAssistTurns = [];
    shellAssistProposal = null;
    await requestNextShellAssistTurn(goal, runId);
  };

  const continueShellAssistGoal = async () => {
    if (!shellAssistGoal || shellAssistInFlight) return;
    await requestNextShellAssistTurn(shellAssistGoal, shellAssistRunId);
  };

  const sendShellAssistPrompt = async () => {
    const prompt = shellAssistPrompt.trim();
    if (shellAssistProposal?.action === 'needs_input' && prompt && shellAssistGoal) {
      shellAssistGoal = `${shellAssistGoal}\n\nOperator clarification: ${prompt}`;
      shellAssistPrompt = '';
      shellAssistProposal = null;
      await requestNextShellAssistTurn(shellAssistGoal, shellAssistRunId);
      return;
    }
    if (!shellAssistGoal) {
      await startShellAssistGoal();
    }
  };

  const approveShellAssistCommand = async () => {
    if (
      !shellAssistProposal ||
      shellAssistProposal.action !== 'command' ||
      !shellSessionInfo ||
      shellAssistInFlight
    ) {
      return;
    }
    const proposal = shellAssistProposal;
    const runId = shellAssistRunId;
    const transcriptBefore = shellTranscriptBuffer;
    shellAssistInFlight = true;
    shellAssistError = null;
    shellAssistStatus = 'Sending approved command...';
    try {
      await logShellAssistApproval(shellSessionInfo, proposal);
      const command = proposal.command.endsWith('\n') || proposal.command.endsWith('\r')
        ? proposal.command
        : `${proposal.command}\r`;
      const turn: ShellAssistTurn = {
        id: `${Date.now()}-${shellAssistTurns.length}`,
        command: proposal.command,
        explanation: proposal.explanation,
        risk: proposal.risk,
        responseId: proposal.responseId ?? null,
        approved: true,
        output: null
      };
      shellAssistTurns = [...shellAssistTurns, turn];
      shellAssistProposal = null;
      await invokeTauri('shell_write', { data: command });
      shellAssistStatus = 'Approved command sent. Observing terminal output...';
      const waitResult = await waitForShellTranscriptIdle(runId);
      const output = stripShellControlSequences(shellAssistTranscriptDelta(transcriptBefore)).trim().slice(-4_000);
      shellAssistTurns = shellAssistTurns.map((item) =>
        item.id === turn.id ? { ...item, output: output || '(no captured output)' } : item
      );
      if (waitResult === 'cancelled' || runId !== shellAssistRunId) {
        return;
      }
      if (waitResult === 'timeout') {
        shellAssistStatus = 'Still waiting on terminal output. Continue when the prompt is ready.';
        return;
      }
      shellAssistInFlight = false;
      await requestNextShellAssistTurn(shellAssistGoal, runId);
      return;
    } catch (error) {
      shellAssistError = formatAiAssistRequestError(error);
      shellAssistStatus = 'Failed to send approved command';
    } finally {
      shellAssistInFlight = false;
    }
  };

  const rejectShellAssistCommand = () => {
    if (shellAssistProposal?.action === 'command') {
      shellAssistTurns = [
        ...shellAssistTurns,
        {
          id: `${Date.now()}-${shellAssistTurns.length}`,
          command: shellAssistProposal.command,
          explanation: shellAssistProposal.explanation,
          risk: shellAssistProposal.risk,
          responseId: shellAssistProposal.responseId ?? null,
          approved: false,
          output: 'Operator rejected this proposed command.'
        }
      ];
    }
    shellAssistProposal = null;
    shellAssistStatus = shellAssistGoal
      ? 'Proposal rejected. Continue to ask for another turn, or stop this goal.'
      : 'Proposal dismissed.';
  };

  const stopShellAssistGoal = () => {
    shellAssistRunId += 1;
    shellAssistGoal = '';
    shellAssistPrompt = '';
    shellAssistProposal = null;
    shellAssistTurns = [];
    shellAssistInFlight = false;
    shellAssistError = null;
    shellAssistStatus = shellAssistReady()
      ? 'Describe the goal you want the shell agent to achieve.'
      : 'Open a live system shell session first.';
  };

  const requestAiAssistTaskStart = async (
    info: SessionParams,
    goal: string,
    snapshot: RemoteDesktopSnapshot
  ): Promise<AiAssistTaskStepResponse> => {
    if (!info.backendApi) {
      throw new Error('The viewer does not know the API backend URL for this session.');
    }
    const response = await fetchWithTimeout(`${info.backendApi}/rmm/ai/desktop-task/start`, {
      method: 'POST',
      timeoutMs: 90_000,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        goal,
        screenshotBase64: snapshot.imageBase64,
        width: snapshot.width,
        height: snapshot.height,
        sessionId: info.sessionId,
        sessionToken: info.token,
        rmmApiBase: info.apiBase,
        platform: activeAgentPlatform
      })
    });
    if (!response.ok) {
      throw new Error(await parseAiAssistErrorDetail(response, `AI desktop task start failed (${response.status})`));
    }
    return response.json();
  };

  const requestAiAssistTaskContinue = async (
    info: SessionParams,
    taskId: string,
    snapshot: RemoteDesktopSnapshot,
    lastStepResult: string
  ): Promise<AiAssistTaskStepResponse> => {
    if (!info.backendApi) {
      throw new Error('The viewer does not know the API backend URL for this session.');
    }
    const response = await fetchWithTimeout(`${info.backendApi}/rmm/ai/desktop-task/${encodeURIComponent(taskId)}/continue`, {
      method: 'POST',
      timeoutMs: 90_000,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        screenshotBase64: snapshot.imageBase64,
        width: snapshot.width,
        height: snapshot.height,
        sessionId: info.sessionId,
        sessionToken: info.token,
        rmmApiBase: info.apiBase,
        platform: activeAgentPlatform,
        lastStepResult
      })
    });
    if (!response.ok) {
      throw new Error(await parseAiAssistErrorDetail(response, `AI desktop task continue failed (${response.status})`));
    }
    return response.json();
  };

  const updateAiAssistTaskState = (response: AiAssistTaskStepResponse) => {
    aiAssistCurrentTaskId = response.taskId;
    aiAssistPlanLines = response.plan;
    aiAssistStepIndex = response.stepIndex;
    aiAssistMaxSteps = response.maxSteps;
    aiAssistTaskStatus = response.status;
    aiAssistActionLines = response.actions.map(formatAiAssistAction);
  };

  const appendAiAssistAssistantMessage = (response: AiAssistTaskStepResponse) => {
    const message = response.assistantMessage.trim();
    if (!message) {
      return;
    }
    const progress =
      response.maxSteps > 0
        ? `step ${response.stepIndex}/${response.maxSteps}`
        : `step ${response.stepIndex}`;
    aiAssistLines = [...aiAssistLines, `AI (${progress}): ${message}`];
    void scrollTranscriptToBottom();
  };

  const requestAiAssistStop = () => {
    if (!aiAssistInFlight) {
      return;
    }
    aiAssistStopRequested = true;
    aiAssistStatus = 'Stop requested — finishing the current action batch.';
  };

  const sendAiAssistMessage = async () => {
    const prompt = aiAssistDraft.trim();
    if (aiAssistInFlight || !prompt) {
      return;
    }
    if (!aiAssistReady() || !remoteSessionInfo) {
      refreshAiAssistStatus();
      aiAssistError = 'No active remote desktop session is ready for AI Assist.';
      return;
    }

    const sessionInfo = remoteSessionInfo;
    aiAssistDraft = '';
    aiAssistError = null;
    aiAssistInFlight = true;
    aiAssistStopRequested = false;
    aiAssistCurrentTaskId = null;
    aiAssistPlanLines = [];
    aiAssistStepIndex = 0;
    aiAssistMaxSteps = 0;
    aiAssistTaskStatus = null;
    aiAssistActionLines = [];
    aiAssistLines = [...aiAssistLines, `You: ${prompt}`];
    void scrollTranscriptToBottom();

    try {
      aiAssistStatus = 'Capturing remote desktop…';
      let snapshot = await invokeTauri<RemoteDesktopSnapshot>('capture_remote_desktop_snapshot');
      aiAssistStatus = 'Starting desktop task…';
      let response = await requestAiAssistTaskStart(sessionInfo, prompt, snapshot);
      let viewerStepGuard = 0;

      while (true) {
        viewerStepGuard += 1;
        updateAiAssistTaskState(response);
        appendAiAssistAssistantMessage(response);

        if (response.status === 'complete') {
          aiAssistStatus = `Task complete at step ${response.stepIndex}/${response.maxSteps}.`;
          break;
        }
        if (response.status === 'failed') {
          aiAssistStatus = 'AI desktop task failed.';
          aiAssistError = response.assistantMessage || 'The AI desktop task failed.';
          break;
        }
        if (response.status === 'needs_approval') {
          aiAssistStatus = 'AI desktop task needs approval before continuing.';
          break;
        }
        if (response.status !== 'running') {
          aiAssistStatus = `AI desktop task stopped with status: ${formatAiAssistTaskStatus(response.status)}.`;
          break;
        }
        if (response.actions.length === 0) {
          aiAssistStatus = 'AI desktop task stopped because no actions were returned.';
          aiAssistError = 'The task is still running, but the backend returned no actions to execute.';
          break;
        }
        if (viewerStepGuard > Math.max(1, response.maxSteps)) {
          aiAssistStatus = 'AI desktop task stopped at the viewer step limit.';
          aiAssistError = 'The viewer stopped the task to avoid an infinite loop.';
          break;
        }

        aiAssistStatus = `Executing step ${response.stepIndex}/${response.maxSteps}…`;
        const postActionSettleMs = aiAssistPostActionSettleMs(response.actions);
        await executeAiAssistActions(aiAssistImmediateActions(response.actions), snapshot);

        if (aiAssistStopRequested) {
          aiAssistStatus = `Stopped after executing ${response.actions.length} action${response.actions.length === 1 ? '' : 's'} at step ${response.stepIndex}.`;
          break;
        }

        if (postActionSettleMs > 0) {
          aiAssistStatus = `Waiting ${postActionSettleMs} ms for the remote desktop to settle…`;
          await sleep(postActionSettleMs);
        }

        aiAssistStatus = 'Capturing updated remote desktop…';
        snapshot = await invokeTauri<RemoteDesktopSnapshot>('capture_remote_desktop_snapshot');
        aiAssistStatus = `Continuing task after step ${response.stepIndex}/${response.maxSteps}…`;
        response = await requestAiAssistTaskContinue(
          sessionInfo,
          response.taskId,
          snapshot,
          `Executed ${response.actions.length} action(s) at step ${response.stepIndex}.`
        );
      }
    } catch (error) {
      aiAssistError = formatAiAssistRequestError(error);
      aiAssistStatus = 'AI request failed';
    } finally {
      aiAssistInFlight = false;
    }
  };

  const handleAiAssistPromptKeydown = (event: KeyboardEvent) => {
    if (event.key !== 'Enter' || event.shiftKey) {
      return;
    }
    event.preventDefault();
    if (!aiAssistCanSend) {
      refreshAiAssistStatus();
      return;
    }
    void sendAiAssistMessage();
  };

  const ensureFileTransferSessionLive = async () => {
    if (fileTransferConnected || fileTransferConnectInFlight) {
      return;
    }
    fileTransferError = null;
    fileTransferConnectInFlight = true;
    try {
      if (fileTransferSessionInfo?.apiBase && fileTransferSessionInfo.agentId) {
        await connectFileTransferSession(fileTransferSessionInfo);
        return;
      }

      const sessionContext = remoteSessionInfo ?? shellSessionInfo;
      if (!sessionContext?.apiBase || !sessionContext.agentId) {
        fileTransferStatus = 'Open a remote desktop or shell session first.';
        fileTransferError = 'No active session context available for file transfer.';
        return;
      }

      fileTransferStatus = 'Preparing file transfer session...';
      const nextSession = await requestFileTransferConnectFromRemote();
      fileTransferSessionInfo = nextSession;
      await connectFileTransferSession(nextSession);
    } catch (error) {
      fileTransferError = error instanceof Error ? error.message : String(error);
      fileTransferStatus = 'Connection failed';
    } finally {
      fileTransferConnectInFlight = false;
    }
  };

  const handleFileTransferResult = (
    operation: 'upload' | 'download',
    jobId: string,
    response: FileTransferResponse,
    context: { localPaths?: string[]; remotePaths?: string[]; destination: string }
  ) => {
    const type = fileTransferResponseType(response);
    if (type === 'transfer_complete') {
      const complete = response as Extract<FileTransferResponse, { type: 'transfer_complete' }>;
      const bytesTransferred = Math.max(
        (complete.bytesTransferred ??
          (complete as unknown as { bytes_transferred?: number }).bytes_transferred ??
          0) as number,
        0
      );
      const extractedEntries = Math.max(
        (complete.extractedEntries ??
          (complete as unknown as { extracted_entries?: number }).extracted_entries ??
          0) as number,
        0
      );
      updateFileTransferJob(jobId, {
        status: 'done',
        // Don't keep showing "Finalizing" once we're done.
        phase: undefined,
        message: extractedEntries > 0 ? `Completed (${extractedEntries} item(s))` : 'Completed',
        // Preserve the final transfer size.
        bytesDone: bytesTransferred,
        bytesTotal: bytesTransferred
      });
      fileTransferStatus = `${operation === 'upload' ? 'Upload' : 'Download'} completed`;
      fileTransferPendingConflict = null;
      void refreshLocalDirCached(fileTransferLocalPath);
      void refreshRemoteDirCached(fileTransferRemotePath);
      return;
    }
    if (type === 'conflict') {
      const conflict = response as Extract<FileTransferResponse, { type: 'conflict' }>;
      updateFileTransferJob(jobId, {
        status: 'error',
        phase: 'finalizing',
        message: `Conflict: ${conflict.path}`
      });
      fileTransferPendingConflict = {
        kind: operation,
        jobId,
        localPaths: context.localPaths,
        remotePaths: context.remotePaths,
        destination: context.destination,
        conflictPath: conflict.path,
        conflictMessage: conflict.message
      };
      fileTransferStatus = 'Conflict detected. Choose an action.';
      return;
    }
    if (type === 'error') {
      const error = response as Extract<FileTransferResponse, { type: 'error' }>;
      if ((error.message ?? '').trim().toLowerCase() === 'cancelled') {
        updateFileTransferJob(jobId, {
          status: 'cancelled',
          phase: undefined,
          message: undefined
        });
        fileTransferStatus = 'Transfer cancelled';
        fileTransferPendingConflict = null;
        return;
      }
      updateFileTransferJob(jobId, {
        status: 'error',
        phase: 'finalizing',
        message: error.message
      });
      fileTransferError = error.message;
      fileTransferStatus = 'Transfer failed';
      return;
    }
    updateFileTransferJob(jobId, {
      status: 'error',
      phase: 'finalizing',
      message: 'Unexpected transfer response'
    });
    fileTransferStatus = 'Transfer failed';
  };

  const startFileTransferUpload = async (conflictMode: FileTransferConflictMode = 'prompt') => {
    if (!isTauriRuntime()) return;
    const localPaths = Array.from(
      new Set([
        ...Array.from(fileTransferLocalSelectedDirs),
        ...Array.from(fileTransferLocalSelectedFiles)
      ])
    );
    if (localPaths.length === 0) {
      fileTransferStatus = 'Select one or more local files/folders to upload.';
      return;
    }
    const job = createFileTransferJob('upload', `${localPaths.length} item(s)`);
    upsertFileTransferJob(job);
    fileTransferStatus = 'Uploading...';
    try {
      const response = await invokeTauri<FileTransferResponse>('file_transfer_upload', {
        jobId: job.id,
        localPaths,
        remoteDestination: fileTransferRemotePath,
        conflictMode
      });
      handleFileTransferResult('upload', job.id, response, {
        localPaths,
        destination: fileTransferRemotePath
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.trim().toLowerCase() === 'cancelled') {
        updateFileTransferJob(job.id, { status: 'cancelled', phase: undefined, message: undefined });
        fileTransferStatus = 'Transfer cancelled';
        return;
      }
      updateFileTransferJob(job.id, { status: 'error', phase: 'finalizing', message });
      fileTransferError = message;
      fileTransferStatus = 'Upload failed';
    }
  };

  const startFileTransferDownload = async (conflictMode: FileTransferConflictMode = 'prompt') => {
    if (!isTauriRuntime()) return;
    const remotePaths = Array.from(
      new Set([
        ...Array.from(fileTransferRemoteSelectedDirs),
        ...Array.from(fileTransferRemoteSelectedFiles)
      ])
    );
    if (remotePaths.length === 0) {
      fileTransferStatus = 'Select one or more remote files/folders to download.';
      return;
    }
    const job = createFileTransferJob('download', `${remotePaths.length} item(s)`);
    upsertFileTransferJob(job);
    fileTransferStatus = 'Downloading...';
    try {
      const response = await invokeTauri<FileTransferResponse>('file_transfer_download', {
        jobId: job.id,
        remotePaths,
        localDestination: fileTransferLocalPath,
        conflictMode
      });
      handleFileTransferResult('download', job.id, response, {
        remotePaths,
        destination: fileTransferLocalPath
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.trim().toLowerCase() === 'cancelled') {
        updateFileTransferJob(job.id, { status: 'cancelled', phase: undefined, message: undefined });
        fileTransferStatus = 'Transfer cancelled';
        return;
      }
      updateFileTransferJob(job.id, { status: 'error', phase: 'finalizing', message });
      fileTransferError = message;
      fileTransferStatus = 'Download failed';
    }
  };

  const cancelFileTransferJob = async (jobId: string) => {
    if (!isTauriRuntime()) return;
    updateFileTransferJob(jobId, { message: 'Cancelling...' });
    try {
      await invokeTauri('file_transfer_cancel', { jobId });
    } catch {}
  };

  const resolveFileTransferConflict = async (mode: Exclude<FileTransferConflictMode, 'prompt'>) => {
    const pending = fileTransferPendingConflict;
    if (!pending) return;
    fileTransferPendingConflict = null;
    if (pending.kind === 'upload') {
      if (pending.localPaths && pending.localPaths.length > 0) {
        try {
          const response = await invokeTauri<FileTransferResponse>('file_transfer_upload', {
            jobId: pending.jobId,
            localPaths: pending.localPaths,
            remoteDestination: pending.destination,
            conflictMode: mode
          });
          handleFileTransferResult('upload', pending.jobId, response, {
            localPaths: pending.localPaths,
            destination: pending.destination
          });
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          updateFileTransferJob(pending.jobId, { status: 'error', phase: 'finalizing', message });
          fileTransferStatus = 'Upload failed';
          fileTransferError = message;
        }
      }
      return;
    }
    if (pending.remotePaths && pending.remotePaths.length > 0) {
      try {
        const response = await invokeTauri<FileTransferResponse>('file_transfer_download', {
          jobId: pending.jobId,
          remotePaths: pending.remotePaths,
          localDestination: pending.destination,
          conflictMode: mode
        });
        handleFileTransferResult('download', pending.jobId, response, {
          remotePaths: pending.remotePaths,
          destination: pending.destination
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        updateFileTransferJob(pending.jobId, { status: 'error', phase: 'finalizing', message });
        fileTransferStatus = 'Download failed';
        fileTransferError = message;
      }
    }
  };

  const toggleLocalSelection = (entry: FileTransferEntry) => {
    if (entry.isDir) {
      const next = new Set(fileTransferLocalSelectedDirs);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      fileTransferLocalSelectedDirs = next;
      return;
    }
    if (isLocalEntryCheckboxDisabled(entry)) return;
    const next = new Set(fileTransferLocalSelectedFiles);
    if (next.has(entry.path)) next.delete(entry.path);
    else next.add(entry.path);
    fileTransferLocalSelectedFiles = next;
  };

  const toggleRemoteSelection = (entry: FileTransferEntry) => {
    if (entry.isDir) {
      const next = new Set(fileTransferRemoteSelectedDirs);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      fileTransferRemoteSelectedDirs = next;
      return;
    }
    if (isRemoteEntryCheckboxDisabled(entry)) return;
    const next = new Set(fileTransferRemoteSelectedFiles);
    if (next.has(entry.path)) next.delete(entry.path);
    else next.add(entry.path);
    fileTransferRemoteSelectedFiles = next;
  };

  const openLocalEntry = async (entry: FileTransferEntry) => {
    if (!entry.isDir) {
      toggleLocalSelection(entry);
      return;
    }
    await selectLocalDir(entry.path);
  };

  const openRemoteEntry = async (entry: FileTransferEntry) => {
    if (!entry.isDir) {
      toggleRemoteSelection(entry);
      return;
    }
    await selectRemoteDir(entry.path);
  };

  const selectTab = async (tab: ViewerTab) => {
    if (!isTabSupported(tab)) {
      return;
    }
    if (tab !== 'System Info') {
      clearSystemInfoPolling();
    }
    activeTab = tab;
    remoteDesktopDropdownOpen = false;
    shellRunAsDropdownOpen = false;
    videoQualityOpen = false;
    if (tab === 'System Shell') {
      await ensureShellSessionLive();
      await refreshRdpSessionsForShell();
    } else if (tab === 'Remote Desktop') {
      await ensureRemoteSessionLive();
    } else if (tab === 'Remote Registry') {
      await ensureRegistrySessionLive();
    } else if (tab === 'File Transfer') {
      await ensureFileTransferSessionLive();
    } else if (tab === 'System Info') {
      await fetchSystemInfo({ refresh: true });
      if (getSystemInfoAgentContext()) {
        startSystemInfoPolling();
      }
    }
  };

  const handleSystemShellNavClick = () => {
    if (activeTab !== 'System Shell') {
      void selectTab('System Shell');
      shellRunAsDropdownOpen = false;
      return;
    }
    if (!isWindowsShellPlatform()) {
      shellRunAsDropdownOpen = false;
      return;
    }
    const opening = !shellRunAsDropdownOpen;
    shellRunAsDropdownOpen = opening;
    if (opening) {
      void refreshRdpSessionsForShell();
    }
  };

  const handleRemoteDesktopNavClick = () => {
    if (activeTab !== 'Remote Desktop') {
      void selectTab('Remote Desktop');
      remoteDesktopDropdownOpen = false;
      navClipReady = false;
      void scheduleViewportRectRefresh();
      return;
    }
    const opening = !remoteDesktopDropdownOpen;
    remoteDesktopDropdownOpen = !remoteDesktopDropdownOpen;
    if (opening) {
      videoQualityOpen = false;
      monitorPickerOpen = false;
      navClipReady = false;
      scheduleNavClipReadyFallback();
      void scheduleViewportRectRefresh();
      return;
    }
    videoQualityOpen = false;
    clearNavClipReadyFallback();
    navClipReady = false;
    void scheduleViewportRectRefresh();
  };

  const sessionSwitchStateLabel = (choice: 'console' | number): string => {
    if (choice === 'console') {
      return formatConsoleContextLabel();
    }
    const session = findSessionByNativeId(choice);
    return session ? formatRdpContextLabel(session) : `Session ${choice}`;
  };

  const closeRemoteDesktopDropdown = () => {
    remoteDesktopDropdownOpen = false;
    videoQualityOpen = false;
    clearNavClipReadyFallback();
    navClipReady = false;
    void scheduleViewportRectRefresh();
  };

  const findSessionByNativeId = (sessionId: number) =>
    (rdpSessions ?? []).find(
      (session) => normalizeSessionId(session.nativeSessionId) === normalizeSessionId(sessionId)
    );

  const isDisconnectedSession = (session: { state: string } | undefined): boolean =>
    (session?.state ?? '').trim().toLowerCase() === 'disconnected';

  const handleRemoteDesktopContextSelect = async (choice: 'console' | number) => {
    if (sessionSwitchInFlight) {
      return;
    }
    if (activeTransport === null) {
      sessionSwitchError = 'No active remote desktop session';
      return;
    }
    sessionSwitchError = null;
    const targetSession =
      choice === 'console' ? consoleSession : findSessionByNativeId(normalizeSessionId(choice));
    const targetSessionId =
      choice === 'console'
        ? normalizeSessionId(consoleSession?.nativeSessionId ?? -1)
        : normalizeSessionId(choice);
    if (targetSessionId <= 0) {
      sessionSwitchError = 'Invalid session selection';
      return;
    }
    if (!targetSession) {
      sessionSwitchError = 'Selected session is no longer available';
      return;
    }

    closeRemoteDesktopDropdown();

    if (targetSession.kind === 'rdp' && isDisconnectedSession(targetSession)) {
      const shouldLogoff = await openAppConfirm({
        title: 'Log Off Disconnected Session?',
        body: `Session ${targetSession.logicalSessionId} is disconnected.\n\nLog out this disconnected session?`,
        okLabel: 'Log off',
        cancelLabel: 'Keep'
      });
      if (!shouldLogoff) {
        return;
      }
      sessionSwitchInFlight = true;
      remoteDesktopStatus = `Logging off ${formatRdpContextLabel(targetSession)}...`;
      try {
        await invokeTauri('send_control', {
          event: {
            type: 'sessionLogoff',
            sessionId: targetSessionId
          }
        });
        remoteDesktopStatus = `Logoff requested for ${formatRdpContextLabel(targetSession)}`;
      } catch (error) {
        sessionSwitchError = error instanceof Error ? error.message : String(error);
        remoteDesktopStatus = 'Session logoff request failed';
      } finally {
        sessionSwitchInFlight = false;
      }
      return;
    }

    sessionSwitchInFlight = true;
    remoteDesktopStatus = `Switching to ${sessionSwitchStateLabel(choice)}...`;
    try {
      await invokeTauri('send_control', {
        event: {
          type: 'sessionSwitch',
          sessionId: targetSessionId
        }
      });
      remoteDesktopContext = choice;
      remoteDesktopStatus = `Switch requested: ${sessionSwitchStateLabel(choice)}`;
    } catch (error) {
      sessionSwitchError = error instanceof Error ? error.message : String(error);
      remoteDesktopStatus = 'Session switch request failed';
    } finally {
      sessionSwitchInFlight = false;
    }
  };

  const clearCaptureOutputSwitchTimeout = () => {
    if (captureOutputSwitchTimeout !== null) {
      window.clearTimeout(captureOutputSwitchTimeout);
      captureOutputSwitchTimeout = null;
    }
  };

  const captureOutputByIndex = (index: number): CaptureOutputInfo | undefined =>
    (captureOutputs ?? []).find((out) => out.index === index);

  const captureOutputName = (out: CaptureOutputInfo | undefined): string => {
    const name = out?.name?.trim();
    if (name) {
      return name;
    }
    return out ? `Display ${out.index + 1}` : 'selected display';
  };

  const captureOutputDetails = (out: CaptureOutputInfo): string => {
    const details: string[] = [];
    if (typeof out.width === 'number' && typeof out.height === 'number') {
      details.push(`${out.width}x${out.height}`);
    }
    if (out.primary) {
      details.push('Primary');
    }
    return details.join(' | ');
  };

  const monitorPickerTitle = (): string => {
    if (pendingCaptureOutputIndex !== null) {
      return `Switching to ${captureOutputName(captureOutputByIndex(pendingCaptureOutputIndex))}...`;
    }
    if (monitorPickerInteractive) {
      return remoteDesktopCaptureType
        ? `Switch display (${formatCaptureType(remoteDesktopCaptureType)})`
        : 'Switch display';
    }
    if (captureOutputs === null) {
      return 'Waiting for display list from session...';
    }
    return captureOutputs.length > 1 ? 'Switch already in progress' : 'Only one display - nothing to switch';
  };

  const failPendingCaptureOutputSwitch = (message: string, keepLastRequest = true) => {
    const failedIndex = pendingCaptureOutputIndex;
    clearCaptureOutputSwitchTimeout();
    pendingCaptureOutputIndex = null;
    if (!keepLastRequest || failedIndex === null) {
      lastRequestedCaptureOutputIndex = null;
    }
    captureOutputSwitchError = message;
    monitorPickerOpen = true;
    void scheduleViewportRectRefresh();
  };

  const handleCaptureOutputSelect = async (index: number) => {
    if (!remoteDesktopConnected || pendingCaptureOutputIndex !== null) {
      return;
    }
    if (activeTransport === null) {
      lastRequestedCaptureOutputIndex = null;
      captureOutputSwitchError = 'No active remote desktop session';
      monitorPickerOpen = true;
      void scheduleViewportRectRefresh();
      return;
    }
    const output = captureOutputByIndex(index);
    if (!output) {
      lastRequestedCaptureOutputIndex = null;
      captureOutputSwitchError = 'Selected display is no longer available';
      monitorPickerOpen = true;
      void scheduleViewportRectRefresh();
      return;
    }
    if (index === activeCaptureOutputIndex) {
      lastRequestedCaptureOutputIndex = null;
      captureOutputSwitchError = null;
      monitorPickerOpen = false;
      void scheduleViewportRectRefresh();
      return;
    }
    captureOutputSwitchError = null;
    pendingCaptureOutputIndex = index;
    lastRequestedCaptureOutputIndex = index;
    monitorPickerOpen = false;
    clearCaptureOutputSwitchTimeout();
    captureOutputSwitchTimeout = window.setTimeout(() => {
      if (pendingCaptureOutputIndex === index) {
        failPendingCaptureOutputSwitch(
          `Timed out waiting for ${captureOutputName(output)} to become active`
        );
      }
    }, CAPTURE_OUTPUT_SWITCH_TIMEOUT_MS);
    void scheduleViewportRectRefresh();
    try {
      await invokeTauri('send_control', {
        event: { type: 'captureOutputSwitch', index }
      });
    } catch (error) {
      if (pendingCaptureOutputIndex === index) {
        clearCaptureOutputSwitchTimeout();
        pendingCaptureOutputIndex = null;
      }
      if (lastRequestedCaptureOutputIndex === index) {
        lastRequestedCaptureOutputIndex = null;
      }
      captureOutputSwitchError = error instanceof Error ? error.message : String(error);
      monitorPickerOpen = true;
      void scheduleViewportRectRefresh();
    }
  };

  const normalizeSessionId = (sessionId: number): number => {
    const parsed = Number(sessionId);
    return Number.isFinite(parsed) ? parsed : -1;
  };

  const normalizeRdpSessions = (
    sessions: Array<RawRdpSessionInfo> | null | undefined
  ): Array<RdpSessionInfo> => {
    if (!Array.isArray(sessions)) {
      return [];
    }

    const candidates = sessions
      .map((session) => {
        const nativeSessionId = normalizeSessionId(
          session.nativeSessionId ??
            session.sessionId ??
            session.session_id ??
            session.logicalSessionId ??
            -1
        );
        return {
          logicalSessionId: normalizeSessionId(session.logicalSessionId ?? -1),
          nativeSessionId,
          kind: session.kind?.trim().toLowerCase() ?? '',
          winStation: session.winStation ?? session.win_station ?? '',
          userName: session.userName ?? '',
          state: session.state ?? ''
        };
      })
      .filter((session) => session.nativeSessionId > 0 && session.nativeSessionId < 65536)
      .sort((a, b) => a.nativeSessionId - b.nativeSessionId);

    const explicitConsoleIndex = candidates.findIndex(
      (session) =>
        session.kind === 'console' ||
        session.winStation.trim().toLowerCase() === 'console' ||
        session.logicalSessionId === 1
    );
    const fallbackConsoleIndex =
      explicitConsoleIndex >= 0
        ? explicitConsoleIndex
        : candidates.findIndex((session) => !session.userName.trim());
    const consoleIndex = fallbackConsoleIndex >= 0 ? fallbackConsoleIndex : 0;

    let nextRdpLogicalId = 2;
    return candidates.flatMap((session, index) => {
      const isConsole = index === consoleIndex || session.kind === 'console';
      if (!isConsole && !session.userName.trim()) {
        return [];
      }
      const logicalSessionId = isConsole
        ? 1
        : session.logicalSessionId > 1
          ? session.logicalSessionId
          : nextRdpLogicalId++;
      return [
        {
          ...session,
          logicalSessionId,
          kind: isConsole ? 'console' : 'rdp'
        }
      ];
    });
  };

  const isConsoleSession = (session: RdpSessionInfo | null | undefined): boolean =>
    !!session &&
    ((session.kind ?? '').trim().toLowerCase() === 'console' ||
      normalizeSessionId(session.logicalSessionId) === 1);

  const formatRdpSessionLabel = (session: RdpSessionInfo): string => {
    const normalizedId = normalizeSessionId(session.logicalSessionId);
    if (normalizedId <= 1 || isConsoleSession(session)) {
      return 'Console';
    }
    return `RDP ${normalizedId - 1}`;
  };

  const formatSessionStateLabel = (state?: string): string => {
    if (!state) {
      return 'Unknown';
    }
    const normalized = state.trim().toLowerCase();
    if (normalized === 'active') {
      return 'Active';
    }
    if (normalized === 'disconnected') {
      return 'Disconnected';
    }
    return normalized.charAt(0).toUpperCase() + normalized.slice(1);
  };

  const formatConsoleContextLabel = (): string => {
    const user = consoleSession?.userName?.trim() || 'Unknown\\Unknown';
    const normalizedUser = user.replace('/', '\\').toLowerCase();
    const state = formatSessionStateLabel(consoleSession?.state);
    if (normalizedUser === 'unknown\\unknown' || normalizedUser === 'unknown') {
      return 'Console';
    }
    return `Console - ${user} - ${state}`;
  };

  const formatRdpContextLabel = (session: RdpSessionInfo): string => {
    const label = formatRdpSessionLabel(session);
    const user = session.userName?.trim() || 'Unknown\\Unknown';
    const state = formatSessionStateLabel(session.state);
    return `${label} - ${user} - ${state}`;
  };

  const formatShellUserContextLabel = (session: RdpSessionInfo): string => {
    if (isConsoleSession(session)) {
      return formatConsoleContextLabel();
    }
    return formatRdpContextLabel(session);
  };

  const syncStartMenuBlocked = () => {
    const blocked = activeTab === 'Remote Desktop';
    if (lastStartMenuBlocked === blocked) return;
    lastStartMenuBlocked = blocked;
    void invokeTauri('set_start_menu_blocked', { blocked }).catch(() => {});
  };

  const openSettings = () => {
    connectionInfoOpen = false;
    aiAssistPanelOpen = false;
    shellAssistPanelOpen = false;
    shellCredentialPanelOpen = false;
    chatPanelOpen = false;
    settingsOpen = true;
    void scheduleViewportRectRefresh();
  };

  const toggleSettings = () => {
    if (settingsOpen) {
      closeSettings();
      return;
    }
    openSettings();
  };

  const closeSettings = () => {
    settingsOpen = false;
    void scheduleViewportRectRefresh();
  };

  const closeConnectionInfo = () => {
    connectionInfoOpen = false;
    void scheduleViewportRectRefresh();
  };

  const getConnectionSessionKindForTab = (tab: string): ConnectionSessionKind | null => {
    if (tab === 'Remote Desktop') return 'remote_desktop';
    if (tab === 'System Shell') return 'system_shell';
    if (tab === 'File Transfer') return 'file_transfer';
    if (tab === 'Remote Registry') return 'remote_registry';
    return null;
  };

  const getConnectionStatusForKind = (kind: ConnectionSessionKind | null): boolean => {
    if (kind === 'remote_desktop') return remoteDesktopConnected;
    if (kind === 'system_shell') return shellConnected;
    if (kind === 'file_transfer') return fileTransferConnected;
    if (kind === 'remote_registry') return registryConnected;
    return false;
  };

  const normalizeRemoteAddress = (value: string | null | undefined): string | null => {
    if (!value) return null;
    return value.replace(/^[a-z]+:\/\//i, '');
  };

  const formatEndpointValue = (endpoint: ConnectionEndpoint | null | undefined): string | null => {
    if (!endpoint?.ip || typeof endpoint.port !== 'number') {
      return null;
    }
    return `${endpoint.ip}:${endpoint.port}`;
  };

  const getTransportRemoteAddress = (options: {
    transport: ConnectionTransport;
    relayUrl?: string | null;
    host?: string | null;
    port?: number | null;
    reflex?: ConnectionEndpoint | null;
  }): string | null => {
    if (options.transport === 'relay') {
      return normalizeRemoteAddress(options.relayUrl);
    }
    return (
      formatEndpointValue(options.reflex) ??
      (options.host && typeof options.port === 'number' ? `${options.host}:${options.port}` : null) ??
      options.host ??
      null
    );
  };

  const shouldShowConnectionLatencyChart = (
    kind: ConnectionSessionKind | null,
    summary: ConnectionStatePayload | ConnectionStatsPayload | null
  ): boolean => {
    if (!kind || !summary) return false;
    return kind === 'remote_desktop' || kind === 'system_shell';
  };

  const quickConnectTimeoutMs = () =>
    viewerTransport === 'auto' ? QUICK_CONNECT_AUTO_TIMEOUT_MS : QUICK_CONNECT_FORCED_TIMEOUT_MS;
  const shellQuickConnectTimeoutMs = () =>
    viewerTransport === 'auto' ? SHELL_QUICK_CONNECT_AUTO_TIMEOUT_MS : QUICK_CONNECT_FORCED_TIMEOUT_MS;

  const buildSyntheticConnectionState = (
    kind: ConnectionSessionKind
  ): ConnectionStatePayload | null => {
    if (kind === 'remote_desktop') {
      if (!remoteDesktopConnected || !activeTransport) return null;
      return {
        sessionKind: kind,
        transport: activeTransport,
        connectionType: activeTransport === 'relay' ? 'relay' : 'quic',
        encryptionLabel:
          activeTransport === 'relay' ? 'TLS + E2E ChaCha20-Poly1305' : 'Pinned QUIC TLS',
        encryptionDetails:
          activeTransport === 'relay'
            ? 'Relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305.'
            : 'QUIC session authenticated with the per-session pinned certificate.',
        remoteAddr: getTransportRemoteAddress({
          transport: activeTransport,
          relayUrl: remoteSessionInfo?.relayUrl,
          host: capabilities?.agentHost ?? remoteSessionInfo?.host ?? null,
          reflex: capabilities?.agentReflex ?? null
        }),
        agentReflex: capabilities?.agentReflex ?? null,
        agentLocalAddrs: capabilities?.agentLocalAddrs ?? []
      };
    }
    if (kind === 'system_shell') {
      if (!shellConnected || !shellTransport) return null;
      if (shellTransport === 'tcp') {
        return {
          sessionKind: kind,
          transport: 'tcp',
          connectionType: 'direct_tcp',
          encryptionLabel: 'Session Token Auth',
          encryptionDetails:
            'Direct shell session authenticated with the per-session token. This path does not include QUIC or relay transport encryption.',
          remoteAddr:
            shellSessionInfo?.host && shellSessionInfo?.port
              ? `${shellSessionInfo.host}:${shellSessionInfo.port}`
              : shellSessionInfo?.host ?? null,
          agentReflex: shellCapabilities?.agentReflex ?? null,
          agentLocalAddrs: shellCapabilities?.agentLocalAddrs ?? []
        };
      }
      return {
        sessionKind: kind,
        transport: shellTransport,
        connectionType: shellTransport === 'relay' ? 'relay' : 'quic',
        encryptionLabel:
          shellTransport === 'relay' ? 'TLS + E2E ChaCha20-Poly1305' : 'Pinned QUIC TLS',
        encryptionDetails:
          shellTransport === 'relay'
            ? 'Shell relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305.'
            : 'Shell QUIC session authenticated with the per-session pinned certificate.',
        remoteAddr: getTransportRemoteAddress({
          transport: shellTransport,
          relayUrl: shellSessionInfo?.relayUrl,
          host: shellCapabilities?.agentHost ?? shellSessionInfo?.host ?? null,
          port: shellSessionInfo?.port ?? null,
          reflex: shellCapabilities?.agentReflex ?? null
        }),
        agentReflex: shellCapabilities?.agentReflex ?? null,
        agentLocalAddrs: shellCapabilities?.agentLocalAddrs ?? []
      };
    }
    if (kind === 'file_transfer') {
      if (!fileTransferConnected || !fileTransferTransport) return null;
      return {
        sessionKind: kind,
        transport: fileTransferTransport,
        connectionType: fileTransferTransport === 'relay' ? 'relay' : 'quic',
        encryptionLabel:
          fileTransferTransport === 'relay' ? 'TLS + E2E ChaCha20-Poly1305' : 'Pinned QUIC TLS',
        encryptionDetails:
          fileTransferTransport === 'relay'
            ? 'File transfer relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305.'
            : 'File transfer QUIC session authenticated with the per-session pinned certificate.',
        remoteAddr: getTransportRemoteAddress({
          transport: fileTransferTransport,
          relayUrl: fileTransferSessionInfo?.relayUrl,
          reflex: fileTransferCapabilities?.agentReflex ?? null
        }),
        agentReflex: fileTransferCapabilities?.agentReflex ?? null,
        agentLocalAddrs: fileTransferCapabilities?.agentLocalAddrs ?? []
      };
    }
    if (kind !== 'remote_registry') return null;
    if (!registryConnected || !registryTransport) return null;
    return {
      sessionKind: kind,
      transport: registryTransport,
      connectionType: registryTransport === 'relay' ? 'relay' : 'quic',
      encryptionLabel:
        registryTransport === 'relay' ? 'TLS + E2E ChaCha20-Poly1305' : 'Pinned QUIC TLS',
      encryptionDetails:
        registryTransport === 'relay'
          ? 'Registry relay traffic is protected by TLS to the relay and encrypted end-to-end with ChaCha20-Poly1305.'
          : 'Registry QUIC session authenticated with the per-session pinned certificate.',
      remoteAddr: getTransportRemoteAddress({
        transport: registryTransport,
        relayUrl: registrySessionInfo?.relayUrl,
        reflex: registryCapabilities?.agentReflex ?? null
      }),
      agentReflex: registryCapabilities?.agentReflex ?? null,
      agentLocalAddrs: registryCapabilities?.agentLocalAddrs ?? []
    };
  };

  const resetConnectionInfo = (kind: ConnectionSessionKind | 'all' = 'all') => {
    connectionInfoOpen = false;
    if (kind === 'all') {
      connectionStateByKind = buildEmptyConnectionStateMap();
      connectionStatsByKind = buildEmptyConnectionStatsMap();
      connectionLatencyHistoryByKind = buildEmptyConnectionLatencyHistoryMap();
    } else {
      connectionStateByKind = {
        ...connectionStateByKind,
        [kind]: null
      };
      connectionStatsByKind = {
        ...connectionStatsByKind,
        [kind]: null
      };
      connectionLatencyHistoryByKind = {
        ...connectionLatencyHistoryByKind,
        [kind]: []
      };
    }
    void scheduleViewportRectRefresh();
  };

  const scrollTranscriptToBottom = async () => {
    await tick();
    if (aiAssistPanelOpen && aiAssistTranscriptEl) {
      aiAssistTranscriptEl.scrollTop = aiAssistTranscriptEl.scrollHeight;
    }
  };

  const refitShellTerminalAfterLayout = async () => {
    await tick();
    if (!shellTerminal || !shellFitAddon) return;
    shellFitAddon.fit();
    await invokeTauri('shell_resize', { cols: shellTerminal.cols, rows: shellTerminal.rows }).catch(() => {});
  };

  const closeAiAssistPanel = () => {
    aiAssistPanelOpen = false;
    void scheduleViewportRectRefresh();
  };

  const closeShellAssistPanel = () => {
    shellAssistPanelOpen = false;
    void refitShellTerminalAfterLayout();
    void scheduleViewportRectRefresh();
  };

  const closeShellCredentialPanel = () => {
    shellCredentialPanelOpen = false;
    void refitShellTerminalAfterLayout();
    void scheduleViewportRectRefresh();
  };

  const openShellCredentialPanel = () => {
    settingsOpen = false;
    chatPanelOpen = false;
    connectionInfoOpen = false;
    aiAssistPanelOpen = false;
    shellAssistPanelOpen = false;
    remoteDesktopDropdownOpen = false;
    shellRunAsDropdownOpen = false;
    shellCredentialPanelOpen = true;
    linuxShellCredentialError = null;
    closeShellContextMenu();
    void refitShellTerminalAfterLayout();
    void scheduleViewportRectRefresh();
  };

  const toggleShellCredentialPanel = () => {
    if (shellCredentialPanelOpen) {
      closeShellCredentialPanel();
      return;
    }
    openShellCredentialPanel();
  };

  const openShellAssistPanel = () => {
    settingsOpen = false;
    chatPanelOpen = false;
    connectionInfoOpen = false;
    aiAssistPanelOpen = false;
    shellCredentialPanelOpen = false;
    remoteDesktopDropdownOpen = false;
    shellRunAsDropdownOpen = false;
    shellAssistPanelOpen = true;
    closeShellContextMenu();
    if (!shellAssistStatus) {
      shellAssistStatus = shellAssistReady()
        ? 'Describe the goal you want the shell agent to achieve.'
        : 'Open a live system shell session first.';
    }
    void refitShellTerminalAfterLayout();
    void scheduleViewportRectRefresh();
  };

  const toggleShellAssistPanel = () => {
    if (shellAssistPanelOpen) {
      closeShellAssistPanel();
      return;
    }
    openShellAssistPanel();
  };

  const openAiAssistPanel = () => {
    settingsOpen = false;
    chatPanelOpen = false;
    connectionInfoOpen = false;
    shellAssistPanelOpen = false;
    shellCredentialPanelOpen = false;
    remoteDesktopDropdownOpen = false;
    shellRunAsDropdownOpen = false;
    aiAssistPanelOpen = true;
    refreshAiAssistStatus(true);
    void scrollTranscriptToBottom();
    void scheduleViewportRectRefresh();
  };

  const toggleAiAssistPanel = () => {
    if (aiAssistPanelOpen) {
      closeAiAssistPanel();
      return;
    }
    openAiAssistPanel();
  };

  const toggleConnectionInfo = () => {
    const opening = !connectionInfoOpen;
    connectionInfoOpen = opening;
    if (opening) {
      aiAssistPanelOpen = false;
      shellAssistPanelOpen = false;
      shellCredentialPanelOpen = false;
      chatPanelOpen = false;
      settingsOpen = false;
      remoteDesktopDropdownOpen = false;
      shellRunAsDropdownOpen = false;
      clearNavClipReadyFallback();
      navClipReady = false;
    }
    void scheduleViewportRectRefresh();
  };

  const applyConnectionState = (payload: ConnectionStatePayload) => {
    const kind = payload.sessionKind;
    connectionStateByKind = {
      ...connectionStateByKind,
      [kind]: payload
    };
    if (connectionStatsByKind[kind]) {
      connectionStatsByKind = {
        ...connectionStatsByKind,
        [kind]: {
          ...connectionStatsByKind[kind],
          ...payload
        } as ConnectionStatsPayload
      };
    }
  };

  const applyConnectionStats = (payload: ConnectionStatsPayload) => {
    const kind = payload.sessionKind;
    connectionStateByKind = {
      ...connectionStateByKind,
      [kind]: payload
    };
    connectionStatsByKind = {
      ...connectionStatsByKind,
      [kind]: payload
    };
    if (typeof payload.rttMs !== 'number' || !Number.isFinite(payload.rttMs)) {
      return;
    }
    connectionLatencyHistoryByKind = {
      ...connectionLatencyHistoryByKind,
      [kind]: [
        ...connectionLatencyHistoryByKind[kind],
        {
          sampleAtMs: payload.sampleAtMs,
          rttMs: payload.rttMs
        }
      ].slice(-CONNECTION_RTT_HISTORY_LIMIT)
    };
  };

  const applyTheme = (light: boolean) => {
    if (light) {
      document.documentElement.classList.add('light');
    } else {
      document.documentElement.classList.remove('light');
    }
    isLightMode = light;
  };

  const toggleTheme = async () => {
    const next = !isLightMode;
    applyTheme(next);
    try {
      await invoke('set_theme_preference', { theme: next ? 'light' : 'dark' });
    } catch (_) { /* best effort */ }
  };

  const checkForViewerUpdates = async () => {
    if (viewerUpdateCheckInFlight) return;
    viewerUpdateCheckInFlight = true;
    viewerUpdateStatusMessage = null;
    try {
      const result = await invokeTauri<ViewerUpdateCheckResult>('viewer_check_for_updates');
      if (result.status === 'update_ready' && result.version) {
        settingsOpen = false;
        await scheduleViewportRectRefresh();
        const confirmed = await openAppConfirm({
          title: 'Update Ready',
          body: `Version ${result.version} is ready to install.\n\nThe viewer will close to complete the update. Continue?`,
          okLabel: 'Update Now',
          cancelLabel: 'Later'
        });
        if (confirmed) {
          const launched = await invokeTauri<boolean>('viewer_apply_staged_update');
          if (!launched) {
            viewerUpdateStatusMessage = 'Update was prepared, but no staged package was available to launch.';
          }
          return;
        }
        viewerUpdateStatusMessage = `Update ${result.version} is downloaded and ready to install later.`;
        return;
      }
      viewerUpdateStatusMessage = 'You are up to date.';
    } catch (error) {
      viewerUpdateStatusMessage = error instanceof Error ? error.message : String(error);
    } finally {
      viewerUpdateCheckInFlight = false;
    }
  };

  const loadThemePreference = async () => {
    try {
      const pref = await invoke<string>('get_theme_preference');
      applyTheme(pref === 'light');
    } catch (_) {
      applyTheme(false);
    }
  };

  const viewerSessionBasePath = (info: SessionParams | null): string | null => {
    if (!info?.apiBase || !info.sessionId || !info.token) return null;
    if (info.mode === 'shell') return `${info.apiBase}/api/rmm/shell/session/${info.sessionId}`;
    if (info.mode === 'file_transfer') return `${info.apiBase}/api/rmm/file-transfer/session/${info.sessionId}`;
    if (info.mode === 'registry') return `${info.apiBase}/api/rmm/registry/session/${info.sessionId}`;
    if (info.mode === 'chat') return `${info.apiBase}/api/rmm/chat/session/${info.sessionId}`;
    return `${info.apiBase}/api/rmm/session/${info.sessionId}`;
  };

  const notifyViewerSessionConnected = async (info: SessionParams | null) => {
    const basePath = viewerSessionBasePath(info);
    if (!basePath || !info || viewerConnectedSessionIds.has(info.sessionId)) return;
    try {
      await fetchWithTimeout(
        `${basePath}/viewer-connected?token=${encodeURIComponent(info.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
      viewerConnectedSessionIds.add(info.sessionId);
    } catch {}
  };

  const notifyViewerSessionHeartbeat = async (info: SessionParams | null) => {
    const basePath = viewerSessionBasePath(info);
    if (!basePath || !info) return;
    try {
      await fetchWithTimeout(
        `${basePath}/viewer-heartbeat?token=${encodeURIComponent(info.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
    } catch {}
  };

  const notifySessionEnd = async () => {
    if (!remoteSessionInfo?.apiBase || !remoteSessionInfo.sessionId || !remoteSessionInfo.token) return;
    try {
      await fetchWithTimeout(
        `${remoteSessionInfo.apiBase}/api/rmm/session/${remoteSessionInfo.sessionId}/end?token=${encodeURIComponent(remoteSessionInfo.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
    } catch {}
  };

  const notifyShellEnd = async () => {
    if (!shellSessionInfo?.apiBase || !shellSessionInfo.sessionId || !shellSessionInfo.token) return;
    try {
      await fetchWithTimeout(
        `${shellSessionInfo.apiBase}/api/rmm/shell/session/${shellSessionInfo.sessionId}/end?token=${encodeURIComponent(shellSessionInfo.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
    } catch {}
  };

  const notifyFileTransferEnd = async () => {
    if (!fileTransferSessionInfo?.apiBase || !fileTransferSessionInfo.sessionId || !fileTransferSessionInfo.token) return;
    try {
      await fetchWithTimeout(
        `${fileTransferSessionInfo.apiBase}/api/rmm/file-transfer/session/${fileTransferSessionInfo.sessionId}/end?token=${encodeURIComponent(fileTransferSessionInfo.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
    } catch {}
  };

  const notifyRegistryEnd = async () => {
    if (!registrySessionInfo?.apiBase || !registrySessionInfo.sessionId || !registrySessionInfo.token) return;
    try {
      await fetchWithTimeout(
        `${registrySessionInfo.apiBase}/api/rmm/registry/session/${registrySessionInfo.sessionId}/end?token=${encodeURIComponent(registrySessionInfo.token)}`,
        { method: 'POST', timeoutMs: 8_000 }
      );
    } catch {}
  };

  const endSession = async (notifyRemoteEnds = true) => {
    if (sessionEnding) return;
    sessionEnding = true;
    clearSystemInfoPolling();
    resetConnectionInfo();
    remoteDesktopStatus = 'Ending session...';
    try {
      await Promise.allSettled([
        invokeTauri('disconnect_quic'),
        invokeTauri('disconnect_relay'),
        invokeTauri('registry_disconnect_quic'),
        invokeTauri('registry_disconnect_relay'),
        invokeTauri('shell_disconnect'),
        invokeTauri('file_transfer_disconnect'),
        chatSessionInfo?.apiBase && chatSessionInfo.sessionId && chatSessionInfo.token
          ? invokeTauri('viewer_chat_disconnect', {
              apiBase: chatSessionInfo.apiBase,
              sessionId: chatSessionInfo.sessionId,
              token: chatSessionInfo.token
            })
          : Promise.resolve()
      ]);
      await invokeTauri('clear_control_state');
      if (notifyRemoteEnds) {
        await Promise.allSettled([
          notifySessionEnd(),
          notifyShellEnd(),
          notifyFileTransferEnd(),
          notifyRegistryEnd(),
          disconnectChatSession()
        ]);
      }
    } catch {}
    remoteDesktopOutput = null;
    remoteDesktopFrameImage = null;
    remoteDesktopFrameWidth = 0;
    remoteDesktopFrameHeight = 0;
    remoteDesktopError = null;
    remoteDesktopStatus = 'Session ended';
    fileTransferStatus = '';
    fileTransferError = null;
    fileTransferTransport = null;
    activeTransport = null;
    remoteDesktopConnected = false;
    shellConnected = false;
    shellTransport = null;
    shellCapabilities = null;
    shellTranscriptBuffer = '';
    shellQuicInProgress = false;
    pendingShellRelayHello = null;
    fileTransferConnected = false;
    registryConnected = false;
    viewerConnectedSessionIds.clear();
    remoteSessionInfo = null;
    shellSessionInfo = null;
    fileTransferSessionInfo = null;
    aiAssistPanelOpen = false;
    shellAssistPanelOpen = false;
    shellCredentialPanelOpen = false;
    shellContextMenu = { ...shellContextMenu, open: false };
    linuxShellCredential = null;
    linuxShellCredentialError = null;
    aiAssistStatus = '';
    aiAssistError = null;
    aiAssistDraft = '';
    aiAssistLines = [];
    aiAssistActionLines = [];
    aiAssistCurrentTaskId = null;
    aiAssistPlanLines = [];
    aiAssistStepIndex = 0;
    aiAssistMaxSteps = 0;
    aiAssistTaskStatus = null;
    aiAssistStopRequested = false;
    aiAssistInFlight = false;
    settingsOpen = false;
    connectionInfoOpen = false;
    registrySessionInfo = null;
    chatSessionInfo = null;
    chatCapabilities = null;
    chatConnected = false;
    chatStatus = '';
    chatError = null;
    chatMessages = [];
    chatDraft = '';
    chatPanelOpen = false;
    registryCapabilities = null;
    registryTransport = null;
    registryError = null;
    registryStatus = 'Session ended';
    registryQuicInProgress = false;
    pendingRegistryRelayHello = null;
    pendingRegistryRelayError = null;
    registryConnectInFlight = false;
    connecting = false;
    remoteConnectInFlight = false;
    shellConnectInFlight = false;
    quicInProgress = false;
    pendingRelayHello = null;
    pendingRelayError = null;
    rdpSessions = null;
    clearCaptureOutputSwitchTimeout();
    captureOutputs = null;
    activeCaptureOutputIndex = 0;
    pendingCaptureOutputIndex = null;
    lastRequestedCaptureOutputIndex = null;
    captureOutputSwitchError = null;
    remoteDesktopCaptureType = null;
    sessionEnding = false;
  };

  const toggleMonitorPicker = () => {
    if (!monitorPickerInteractive) {
      return;
    }
    const opening = !monitorPickerOpen;
    monitorPickerOpen = opening;
    if (opening) {
      connectionInfoOpen = false;
      settingsOpen = false;
      aiAssistPanelOpen = false;
      shellCredentialPanelOpen = false;
      remoteDesktopDropdownOpen = false;
      shellRunAsDropdownOpen = false;
      videoQualityOpen = false;
      clearNavClipReadyFallback();
      navClipReady = false;
    }
    void scheduleViewportRectRefresh();
  };

  const toggleVideoQualityDropdown = () => {
    const opening = !videoQualityOpen;
    videoQualityOpen = opening;
    if (opening) {
      connectionInfoOpen = false;
      settingsOpen = false;
      aiAssistPanelOpen = false;
      shellCredentialPanelOpen = false;
      monitorPickerOpen = false;
      shellRunAsDropdownOpen = false;
    }
    void scheduleViewportRectRefresh();
  };

  const selectedVideoQualityOption = () =>
    videoQualityOptions.find((option) => option.id === videoQuality) ?? videoQualityOptions[2];

  const applySelectedVideoQuality = async () => {
    if (!isTauriRuntime() || !remoteDesktopConnected || activeTransport === null) {
      return;
    }
    const option = selectedVideoQualityOption();
    try {
      await invokeTauri('send_control', {
        event: {
          type: 'streamBitrate',
          kbps: option.bitrateKbps
        }
      });
    } catch {
      // Best-effort tuning update; keep the active session running if control delivery fails.
    }
  };

  const selectVideoQuality = (quality: VideoQuality) => {
    videoQuality = quality;
    videoQualityOpen = false;
    void applySelectedVideoQuality();
    void scheduleViewportRectRefresh();
  };

  const formatConnectionType = (
    summary: ConnectionStatePayload | ConnectionStatsPayload | null
  ): string => {
    if (!summary) {
      return 'Waiting for a session';
    }
    if (summary.connectionType === 'lan_direct') {
      return 'QUIC over LAN';
    }
    if (summary.connectionType === 'hole_punch') {
      return 'QUIC Hole Punch';
    }
    if (summary.connectionType === 'direct_tcp') {
      return 'Direct TCP';
    }
    if (summary.connectionType === 'quic') {
      return 'QUIC';
    }
    return 'Relay';
  };

  const formatTransportLabel = (
    summary: ConnectionStatePayload | ConnectionStatsPayload | null
  ): string => {
    if (!summary) {
      return 'Unavailable';
    }
    if (summary.transport === 'quic') return 'QUIC';
    if (summary.transport === 'tcp') return 'TCP';
    return 'Relay';
  };

  const formatCaptureType = (value: string | null | undefined): string => {
    if (!value) {
      return 'Unavailable';
    }
    if (value === 'legacy') {
      return 'Legacy';
    }
    if (value === 'modern_cpu') {
      return 'Legacy';
    }
    if (value === 'modern_gpu') {
      return 'Modern GPU';
    }
    if (value === 'experimental') {
      return 'Experimental';
    }
    return value;
  };

  const formatConnectionEndpoint = (
    endpoint: ConnectionEndpoint | null | undefined
  ): string => {
    if (!endpoint?.ip || typeof endpoint.port !== 'number') {
      return 'Unavailable';
    }
    return `${endpoint.ip}:${endpoint.port}`;
  };

  const formatLatencyMs = (value: number | null | undefined): string => {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      return 'Waiting...';
    }
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ms`;
  };

  const formatDurationMs = (value: number | null | undefined): string => {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      return 'N/A';
    }
    return `${value.toFixed(0)} ms`;
  };

  const formatLocalAddr = (value: { ip: string; prefix: number }): string => {
    return `${value.ip}/${value.prefix}`;
  };

  const getConnectionHealthLabel = (
    connected: boolean,
    rttMs: number | null | undefined,
    expectsLatency: boolean
  ): string => {
    if (!connected) {
      return 'Disconnected';
    }
    if (!expectsLatency) {
      return 'Connected';
    }
    if (typeof rttMs !== 'number' || !Number.isFinite(rttMs)) {
      return 'Stabilizing';
    }
    if (rttMs < 35) {
      return 'Excellent';
    }
    if (rttMs < 80) {
      return 'Good';
    }
    if (rttMs < 140) {
      return 'Fair';
    }
    return 'Poor';
  };

  const buildLatencySparklinePath = (
    samples: ConnectionLatencyPoint[],
    width: number,
    height: number
  ): string => {
    if (samples.length === 0) {
      const mid = (height / 2).toFixed(2);
      return `M 0 ${mid} L ${width} ${mid}`;
    }
    const values = samples.map((sample) => sample.rttMs);
    const min = Math.min(...values);
    const max = Math.max(...values);
    const range = Math.max(1, max - min);
    return samples
      .map((sample, index) => {
        const x = samples.length === 1 ? width / 2 : (index / (samples.length - 1)) * width;
        const normalized = (sample.rttMs - min) / range;
        const y = height - normalized * height;
        return `${index === 0 ? 'M' : 'L'} ${x.toFixed(2)} ${y.toFixed(2)}`;
      })
      .join(' ');
  };

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
      if (typeof value === 'number' && Number.isFinite(value)) {
        return value;
      }
    }
    return null;
  };

  const pickUnixSeconds = (...values: Array<unknown>): number | null => {
    for (const value of values) {
      if (typeof value === 'number' && Number.isFinite(value)) {
        return value > 10_000_000_000 ? Math.floor(value / 1000) : value;
      }
      if (typeof value === 'string' && value.trim()) {
        const parsed = Date.parse(value);
        if (Number.isFinite(parsed)) {
          return Math.floor(parsed / 1000);
        }
      }
    }
    return null;
  };

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

  const formatUptime = (seconds?: number | null) => {
    if (!seconds || seconds <= 0) return 'Unknown';
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return days > 0 ? `${days}d ${hours}h ${minutes}m` : `${hours}h ${minutes}m`;
  };

  const formatPercent = (value?: number | null) => {
    if (value === null || value === undefined || Number.isNaN(value)) {
      return '—';
    }
    return `${Math.max(0, Math.min(100, value)).toFixed(1)}%`;
  };

  const formatTimestamp = (value: Date | null) => {
    if (!value) return 'Unknown';
    if (Number.isNaN(value.getTime())) return 'Unknown';
    return value.toLocaleString();
  };

  const formatBootTime = (unixSeconds?: number | null) => {
    if (!unixSeconds || unixSeconds <= 0) return 'Unknown';
    const parsed = new Date(unixSeconds * 1000);
    if (Number.isNaN(parsed.getTime())) return 'Unknown';
    return parsed.toLocaleString();
  };

  const unwrapSystemInfoPayload = (value: unknown): UnknownRecord | null => {
    const record = asRecord(value);
    if (!record) return null;
    const snapshot = asRecord(record.snapshot);
    return (
      asRecord(record.inventory) ??
      asRecord(record.collection) ??
      asRecord(snapshot?.collection) ??
      record
    );
  };

  const getSystemDetailsSource = (device: SystemInfoDevice | null): UnknownRecord | null =>
    unwrapSystemInfoPayload(device?.deviceDetails ?? null);

  const getSystemInventorySource = (device: SystemInfoDevice | null): UnknownRecord | null =>
    unwrapSystemInfoPayload(device?.lastInventory ?? null);

  const getCpuSummary = (source: UnknownRecord | null) => {
    const hardware = source ? asRecord(source.hardware) : null;
    const cpu = source ? (asRecord(source.cpu) ?? asRecord(hardware?.cpu)) : null;
    if (!cpu) return null;
    return {
      brand: pickString(cpu.brand, cpu.manufacturer) ?? 'Unknown',
      cores: pickNumber(cpu.cores, cpu.threads, cpu.logical_cores, cpu.logicalCores),
      frequencyMHz: pickNumber(cpu.frequency_mhz, cpu.frequencyMHz, cpu.frequencyMhz, cpu.frequency)
    };
  };

  const getMemorySummary = (source: UnknownRecord | null) => {
    const hardware = source ? asRecord(source.hardware) : null;
    const memory = source ? (asRecord(source.memory) ?? asRecord(hardware?.memory)) : null;
    if (!memory) return null;
    const total = pickNumber(memory.total_bytes, memory.totalBytes);
    const available = pickNumber(memory.available_bytes, memory.availableBytes);
    let usedPercent: number | null = null;
    if (typeof total === 'number' && total > 0 && typeof available === 'number' && available >= 0) {
      const used = Math.max(0, total - available);
      usedPercent = (used / total) * 100;
    }
    return { total, available, usedPercent };
  };

  const getDisks = (source: UnknownRecord | null) => {
    const hardware = source ? asRecord(source.hardware) : null;
    const disks = source
      ? Array.isArray(source.disks)
        ? source.disks
        : hardware?.disks
      : null;
    if (!Array.isArray(disks)) return [];
    const normalized = disks.flatMap((disk) => {
      const record = asRecord(disk);
      const volumes = Array.isArray(record?.volumes) ? record.volumes : null;
      const volumeRecords = volumes
        ?.map((volume) => asRecord(volume))
        .filter((volume): volume is UnknownRecord => volume !== null);
      const rows = volumeRecords && volumeRecords.length > 0 ? volumeRecords : [null];
      return rows.map((volume) => {
        const total = pickNumber(
          volume?.total_bytes,
          volume?.totalBytes,
          record?.total_bytes,
          record?.totalBytes,
          record?.size_bytes,
          record?.sizeBytes
        );
        const available = pickNumber(
          volume?.available_bytes,
          volume?.availableBytes,
          volume?.free_bytes,
          volume?.freeBytes,
          record?.available_bytes,
          record?.availableBytes,
          record?.free_bytes,
          record?.freeBytes
        );
        let usedPercent: number | null = null;
        if (
          typeof total === 'number' &&
          total > 0 &&
          typeof available === 'number' &&
          available >= 0
        ) {
          const used = Math.max(0, total - available);
          usedPercent = (used / total) * 100;
        }
        return {
          name: pickString(record?.name, record?.model, record?.device_id, record?.deviceId) ?? 'Disk',
          mount: pickString(
            volume?.drive_letter,
            volume?.driveLetter,
            record?.mount_point,
            record?.mountPoint
          ) ?? '—',
          total,
          available,
          fs: pickString(volume?.filesystem, record?.file_system, record?.fileSystem) ?? '—',
          usedPercent
        };
      });
    });
    const byDevice = new Map<string, (typeof normalized)[number]>();
    for (const disk of normalized) {
      const key = `${disk.name}|${disk.total ?? 'unknown'}|${disk.fs}`;
      const existing = byDevice.get(key);
      if (!existing || diskMountPriority(disk.mount) < diskMountPriority(existing.mount)) {
        byDevice.set(key, disk);
      }
    }
    return [...byDevice.values()].sort((a, b) => {
      const rankDelta = diskMountPriority(a.mount) - diskMountPriority(b.mount);
      return rankDelta !== 0 ? rankDelta : a.mount.localeCompare(b.mount);
    });
  };

  const diskMountPriority = (mount: string): number => {
    if (mount === '/') return 0;
    if (mount.startsWith('/boot')) return 1;
    return 10 + mount.split('/').filter(Boolean).length;
  };

  const getNetworks = (source: UnknownRecord | null) => {
    const network = source ? asRecord(source.network) : null;
    const networks = source
      ? Array.isArray(source.networks)
        ? source.networks
        : network?.adapters
      : null;
    if (!Array.isArray(networks)) return [];
    return networks.map((network) => {
      const record = asRecord(network);
      return {
        name: pickString(record?.name, record?.description) ?? 'Network',
        ips: getNetworkIps(record),
        gateways: pickStringArray(record?.gateways),
        dnsServers: pickStringArray(record?.dns_servers, record?.dnsServers),
        received: pickNumber(record?.received_bytes, record?.receivedBytes),
        transmitted: pickNumber(record?.transmitted_bytes, record?.transmittedBytes)
      };
    });
  };

  const pickStringArray = (...values: Array<unknown>): string[] => {
    for (const value of values) {
      if (!Array.isArray(value)) continue;
      return value
        .map((item) => (typeof item === 'string' ? item.trim() : String(item ?? '').trim()))
        .filter(Boolean);
    }
    return [];
  };

  const getNetworkIps = (record: UnknownRecord | null) => {
    const ips = record ? record.ips : null;
    if (Array.isArray(ips) && ips.length > 0) {
      return ips
        .map((ip) => {
          const ipRecord = asRecord(ip);
          if (!ipRecord) {
            const label = typeof ip === 'string' ? ip.trim() : String(ip ?? '').trim();
            return label ? { label } : null;
          }
          const address = pickString(ipRecord.address, ipRecord.ip) ?? '';
          const prefix = pickNumber(ipRecord.prefix);
          const netmask = pickString(ipRecord.netmask, ipRecord.subnetMask, ipRecord.subnet_mask);
          const suffix = prefix != null ? `/${prefix}` : '';
          const mask = netmask ? ` (${netmask})` : '';
          const label = address ? `${address}${suffix}${mask}` : '';
          return label ? { label } : null;
        })
        .filter((ip): ip is { label: string } => ip !== null);
    }
    return pickStringArray(record?.ip_addresses, record?.ipAddresses).map((label) => ({ label }));
  };

  const getSystemSummary = (source: UnknownRecord | null) => {
    const operatingSystem = source ? asRecord(source.operating_system) : null;
    const system = source
      ? (asRecord(source.system) ?? asRecord(operatingSystem?.system))
      : null;
    if (!system) return null;
    const os = asRecord(system.os);
    return {
      name: pickString(system.name, system.hostname, system.os_name, system.distro, os?.name) ?? 'Unknown',
      kernelVersion: pickString(system.kernelVersion, system.kernel_version, os?.build) ?? 'Unknown',
      osVersion: pickString(system.osVersion, system.os_version, os?.version, os?.edition) ?? 'Unknown',
      uptimeSeconds: pickNumber(system.uptimeSeconds, system.uptime_seconds),
      bootTime: pickUnixSeconds(system.bootTime, system.boot_time)
    };
  };

  const getProcesses = (source: UnknownRecord | null) => {
    const processes = source ? source.processes : null;
    if (!Array.isArray(processes)) return [];
    const records: UnknownRecord[] = [];
    for (const process of processes) {
      const record = asRecord(process);
      if (record) records.push(record);
    }
    return records.sort((a, b) => {
      const cpuDelta =
        (pickNumber(b.cpu, b.cpuUsage) ?? 0) - (pickNumber(a.cpu, a.cpuUsage) ?? 0);
      if (cpuDelta !== 0) return cpuDelta;
      return (
        (pickNumber(b.memory, b.memoryBytes) ?? 0) -
        (pickNumber(a.memory, a.memoryBytes) ?? 0)
      );
    });
  };

  const getProcessName = (process: UnknownRecord) => {
    const name = pickString(process.name, process.processName, process.command, process.path);
    if (name) return name;
    const nested = asRecord(process.name);
    return pickString(nested?.name, nested?.value, nested?.process, nested?.command) ?? 'Unknown';
  };

  const clearNavClipReadyFallback = () => {
    if (navClipReadyTimer !== null) {
      window.clearTimeout(navClipReadyTimer);
      navClipReadyTimer = null;
    }
  };

  const scheduleNavClipReadyFallback = () => {
    clearNavClipReadyFallback();
    navClipReadyTimer = window.setTimeout(() => {
      navClipReadyTimer = null;
      if (!remoteDesktopDropdownOpen || navClipReady) {
        return;
      }
      navClipReady = true;
      void scheduleViewportRectRefresh();
    }, 220);
  };

  const handleClickOutside = (event: MouseEvent) => {
    const target = event.target as HTMLElement;
    if (!target.closest('.ft-context-menu')) {
      closeFileTransferContextMenu();
    }
    if (!target.closest('.shell-context-menu')) {
      closeShellContextMenu();
    }
    let shouldRefreshViewport = false;
    if (!target.closest('.custom-dropdown')) {
      if (videoQualityOpen || monitorPickerOpen) {
        shouldRefreshViewport = true;
      }
      videoQualityOpen = false;
      monitorPickerOpen = false;
    }
    if (!target.closest('.nav-item-with-dropdown')) {
      if (remoteDesktopDropdownOpen || videoQualityOpen) {
        shouldRefreshViewport = true;
      }
      remoteDesktopDropdownOpen = false;
      shellRunAsDropdownOpen = false;
      videoQualityOpen = false;
      clearNavClipReadyFallback();
      navClipReady = false;
    }
    if (!target.closest('.connection-info-container') && connectionInfoOpen) {
      connectionInfoOpen = false;
      shouldRefreshViewport = true;
    }
    if (shouldRefreshViewport) {
      void scheduleViewportRectRefresh();
    }
  };

  const handleWindowKeydown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      closeFileTransferContextMenu();
      closeShellContextMenu();
      if (appConfirmOpen) {
        closeAppConfirm(false);
      }
      if (settingsOpen) {
        closeSettings();
      }
      if (chatPanelOpen) {
        closeChatPanel();
      }
      if (shellAssistPanelOpen) {
        closeShellAssistPanel();
      }
      if (shellCredentialPanelOpen) {
        closeShellCredentialPanel();
      }
      const wasOpen = remoteDesktopDropdownOpen;
      const wasVideoQualityOpen = videoQualityOpen;
      const wasMonitorPickerOpen = monitorPickerOpen;
      const wasConnectionInfoOpen = connectionInfoOpen;
      remoteDesktopDropdownOpen = false;
      videoQualityOpen = false;
      monitorPickerOpen = false;
      shellRunAsDropdownOpen = false;
      connectionInfoOpen = false;
      clearNavClipReadyFallback();
      navClipReady = false;
      if (wasOpen || wasVideoQualityOpen || wasMonitorPickerOpen || wasConnectionInfoOpen) {
        void scheduleViewportRectRefresh();
      }
    }
  };

  type ViewportOcclusionRect = {
    x: number;
    y: number;
    width: number;
    height: number;
  };

  type ViewportSetRectPayload = {
    x: number;
    y: number;
    width: number;
    height: number;
    occlusions?: ViewportOcclusionRect[];
  };

  let viewportUpdateInFlight = false;
  let viewportQueuedPayload: ViewportSetRectPayload | null = null;

  const queueViewportRectUpdate = (payload: ViewportSetRectPayload) => {
    viewportQueuedPayload = payload;
    void flushViewportRectQueue();
  };

  const flushViewportRectQueue = async () => {
    if (viewportUpdateInFlight) {
      return;
    }
    while (viewportQueuedPayload) {
      const payload = viewportQueuedPayload;
      viewportQueuedPayload = null;
      viewportUpdateInFlight = true;
      try {
        await invokeTauri('viewport_set_rect', payload);
      } catch {
        // Best-effort UI sync; keep queue draining even on transient invoke failures.
      } finally {
        viewportUpdateInFlight = false;
      }
    }
  };

  const collectViewportOcclusions = (): ViewportOcclusionRect[] => {
    // Clip only concrete overlay panels, not full-screen backdrop layers.
    // Backdrop occlusion would hide the entire native viewport.
    const selectors: string[] = [];
    if (remoteDesktopDropdownOpen) {
      selectors.push('.nav-dropdown-menu');
    }
    if (connectionInfoOpen) {
      selectors.push('.connection-info-panel');
    }
    if (videoQualityOpen) {
      selectors.push('.quality-dropdown-panel');
    }
    if (monitorPickerOpen) {
      selectors.push('.monitor-picker-panel');
    }
    if (settingsOpen) {
      selectors.push('.settings-sidebar');
    }
    if (aiAssistPanelOpen) {
      selectors.push('.ai-assist-sidebar');
    }
    if (chatPanelOpen) {
      selectors.push('.chat-sidebar');
    }
    if (appConfirmOpen) {
      selectors.push('.registry-modal');
    }
    const occlusions: ViewportOcclusionRect[] = [];
    for (const selector of selectors) {
      const nodes = Array.from(document.querySelectorAll<HTMLElement>(selector));
      for (const node of nodes) {
        const rect = node.getBoundingClientRect();
        const left = Math.floor(rect.left);
        const top = Math.floor(rect.top);
        const right = Math.ceil(rect.right);
        const bottom = Math.ceil(rect.bottom);
        const width = right - left;
        const height = bottom - top;
        if (width <= 0 || height <= 0) continue;
        occlusions.push({
          x: left,
          y: top,
          width,
          height
        });
      }
    }
    return occlusions;
  };

  const updateViewportRect = () => {
    // Only surface the native viewport when we actually have a live remote session.
    // Otherwise the Win32 popup viewport can end up "invisible but clickable" and
    // block interaction behind the window.
    if (
      activeTab !== 'Remote Desktop' ||
      !remoteDesktopFrame ||
      !remoteDesktopConnected ||
      activeTransport === null
    ) {
      return;
    }
    const rect = remoteDesktopFrame.getBoundingClientRect();
    const maxW = typeof window !== 'undefined' ? window.innerWidth : 0;
    const maxH = typeof window !== 'undefined' ? window.innerHeight : 0;
    // Clamp to the actual WebView viewport. On Windows/Tauri the DOM rect can
    // exceed the real client rect by a constant offset, which causes the Rust
    // side to clamp and the viewport to appear "stuck" on resize.
    const left = Math.floor(Math.max(0, Math.min(rect.left, maxW)));
    const top = Math.floor(Math.max(0, Math.min(rect.top, maxH)));
    const right = Math.ceil(Math.max(0, Math.min(rect.right, maxW)));
    const bottom = Math.ceil(Math.max(0, Math.min(rect.bottom, maxH)));
    const x = left;
    const y = top;
    const width = Math.max(0, right - left);
    const height = Math.max(0, bottom - top);
    // Avoid sending a transient 0x0 rect when the tab just switched and the DOM
    // has not finished laying out yet. A 0x0 rect hides the native viewport and
    // clears Rust-side `last_rect`, which can make the remote desktop appear blank.
    if (width === 0 || height === 0) {
      return;
    }
    const occlusions = collectViewportOcclusions();
    queueViewportRectUpdate({ x, y, width, height, occlusions });
  };

  const scheduleViewportRectRefresh = async () => {
    await tick();
    updateViewportRect();
  };

  const handleOverlayAnimationEnd = (event: AnimationEvent) => {
    const target = event.currentTarget as HTMLElement | null;
    if (target?.classList.contains('nav-dropdown-menu')) {
      clearNavClipReadyFallback();
      navClipReady = true;
    }
    void scheduleViewportRectRefresh();
  };

  $: {
    const clipRefreshKey = [
      activeTab,
      settingsOpen ? 'settings-open' : 'settings-closed',
      chatPanelOpen ? 'chat-open' : 'chat-closed',
      connectionInfoOpen ? 'connection-info-open' : 'connection-info-closed',
      appConfirmOpen ? 'app-confirm-open' : 'app-confirm-closed',
      navClipReady ? 'nav-ready' : 'nav-pending',
      rdpSessions === null ? 'rdp-null' : `rdp-${rdpSessions.length}`
    ].join('|');
    if (clipRefreshKey !== lastClipRefreshKey) {
      lastClipRefreshKey = clipRefreshKey;
      if (activeTab === 'Remote Desktop') {
        void scheduleViewportRectRefresh();
      }
    }
  }

  $: if (remoteDesktopDropdownOpen && !navClipReady && navClipReadyTimer === null) {
    scheduleNavClipReadyFallback();
  }

  $: if (aiAssistPanelOpen && !aiAssistInFlight && !aiAssistError) {
    refreshAiAssistStatus();
  }

  $: {
    activeTab;
    capabilities;
    shellCapabilities;
    fileTransferCapabilities;
    registryCapabilities;
    chatCapabilities;
    remoteSessionInfo;
    shellSessionInfo;
    fileTransferSessionInfo;
    registrySessionInfo;
    chatSessionInfo;
    const source = getCapabilitySource();
    const hasSession = hasSessionContext();
    const platform = inferAgentPlatform(source);
    const features = source
      ? normalizeAgentFeatures(source.features, platform)
      : hasSession
        ? WINDOWS_FEATURES
        : UNKNOWN_FEATURES;
    const nextVisibleTabs = TAB_ORDER.filter((tab) => featureForTab(tab, features));
    activeAgentPlatform = platform;
    activeAgentFeatures = features;
    visibleTabs = source || hasSession ? (nextVisibleTabs.length > 0 ? nextVisibleTabs : ['System Info']) : [];
    if ((source || hasSession) && !visibleTabs.includes(activeTab)) {
      activeTab = visibleTabs[0];
      remoteDesktopDropdownOpen = false;
      shellRunAsDropdownOpen = false;
      videoQualityOpen = false;
      monitorPickerOpen = false;
    }
    if (!features.chat && chatPanelOpen) {
      chatPanelOpen = false;
    }
  }

  $: activeConnectionKind = getConnectionSessionKindForTab(activeTab);
  $:
    connectionState =
      activeConnectionKind === null
        ? null
        : connectionStateByKind[activeConnectionKind] ??
          buildSyntheticConnectionState(activeConnectionKind);
  $:
    connectionStats =
      activeConnectionKind === null ? null : connectionStatsByKind[activeConnectionKind];
  $:
    connectionLatencyHistory =
      activeConnectionKind === null ? [] : connectionLatencyHistoryByKind[activeConnectionKind];
  $: connectionSummary = connectionStats ?? connectionState;

  $: {
    const sessions = rdpSessions ?? [];
    consoleSession = sessions.find((session) => isConsoleSession(session)) ?? null;
    visibleRdpSessions = isMacAgentPlatform()
      ? []
      : sessions
          .filter(
            (session) =>
              !isConsoleSession(session) && normalizeSessionId(session.nativeSessionId) < 65536
          )
          .sort(
            (a, b) =>
              normalizeSessionId(a.logicalSessionId) - normalizeSessionId(b.logicalSessionId)
          );
    shellUserContexts = sessions
      .filter((session) => {
        const sessionId = normalizeSessionId(session.nativeSessionId);
        return sessionId > 0 && sessionId < 65536 && !!session.userName?.trim();
      })
      .sort(
        (a, b) =>
          normalizeSessionId(a.logicalSessionId) - normalizeSessionId(b.logicalSessionId)
      );
  }

  const stopViewportObserver = () => {
    if (viewportObserver && viewportObserving) {
      viewportObserver.disconnect();
      viewportObserving = false;
    }
    queueViewportRectUpdate({ x: 0, y: 0, width: 0, height: 0 });
  };

  const canSendControl = () =>
    activeTab === 'Remote Desktop' && remoteHasFocus && activeTransport !== null;

  const mapMouseButton = (button: number) => {
    if (button === 0) return 0;
    if (button === 2) return 1;
    if (button === 1) return 2;
    return button;
  };

  const getRelativePosition = (event: MouseEvent) => {
    if (!remoteDesktopFrame) return null;
    const rect = remoteDesktopFrame.getBoundingClientRect();
    let contentLeft = rect.left;
    let contentTop = rect.top;
    let contentWidth = rect.width;
    let contentHeight = rect.height;

    if (remoteDesktopFrameWidth > 0 && remoteDesktopFrameHeight > 0 && rect.width > 0 && rect.height > 0) {
      const frameAspect = remoteDesktopFrameWidth / remoteDesktopFrameHeight;
      const rectAspect = rect.width / rect.height;
      if (rectAspect > frameAspect) {
        contentWidth = rect.height * frameAspect;
        contentLeft = rect.left + (rect.width - contentWidth) / 2;
      } else {
        contentHeight = rect.width / frameAspect;
        contentTop = rect.top + (rect.height - contentHeight) / 2;
      }
    }

    return {
      x: Math.max(0, Math.min(contentWidth, event.clientX - contentLeft)),
      y: Math.max(0, Math.min(contentHeight, event.clientY - contentTop)),
      width: Math.max(1, contentWidth),
      height: Math.max(1, contentHeight)
    };
  };

  const sendControl = async (event: Record<string, unknown>) => {
    if (!canSendControl()) return;
    try {
      await invokeTauri('send_control', { event });
    } catch {}
  };

  const handleMouseMove = (event: MouseEvent) => {
    if (!canSendControl()) return;
    const now = performance.now();
    if (now - lastMoveSent < 8) return;
    lastMoveSent = now;
    const pos = getRelativePosition(event);
    if (!pos) return;
    void sendControl({
      type: 'mouseMove',
      x: Math.round(pos.x),
      y: Math.round(pos.y),
      elementWidth: Math.round(pos.width),
      elementHeight: Math.round(pos.height)
    });
  };

  const handleMouseButton = (event: MouseEvent, down: boolean) => {
    if (!remoteDesktopFrame) return;
    remoteDesktopFrame.focus();
    remoteHasFocus = true;
    const pos = getRelativePosition(event);
    if (!pos) return;
    void sendControl({
      type: 'mouseButton',
      button: mapMouseButton(event.button),
      down,
      x: Math.round(pos.x),
      y: Math.round(pos.y),
      elementWidth: Math.round(pos.width),
      elementHeight: Math.round(pos.height)
    });
  };

  const handleWheel = (event: WheelEvent) => {
    if (!canSendControl()) return;
    const pos = getRelativePosition(event);
    if (!pos) return;
    const delta = event.deltaY === 0 ? 0 : Math.sign(event.deltaY) * 120;
    if (delta === 0) return;
    void sendControl({
      type: 'mouseWheel',
      delta,
      x: Math.round(pos.x),
      y: Math.round(pos.y),
      elementWidth: Math.round(pos.width),
      elementHeight: Math.round(pos.height)
    });
  };

  const handleKeyDown = async (event: KeyboardEvent) => {
    if (!canSendControl()) return;
    if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      try {
        const text = await navigator.clipboard.readText();
        if (text) {
          await sendControl({ type: 'typedInput', text });
        }
      } catch {}
      return;
    }
    const isSecureAttentionShortcut =
      event.ctrlKey &&
      event.shiftKey &&
      (event.key === 'Delete' || event.code === 'Delete' || event.keyCode === 46);
    if (isSecureAttentionShortcut) {
      event.preventDefault();
      if (!event.repeat) {
        await sendControl({ type: 'secureAttention' });
      }
      return;
    }
    event.preventDefault();
    const modifiers =
      (event.ctrlKey ? 1 : 0) |
      (event.shiftKey ? 2 : 0) |
      (event.altKey ? 4 : 0) |
      (event.metaKey ? 8 : 0);
    await sendControl({
      type: 'keyDown',
      vkey: event.keyCode ?? 0,
      scan: 0,
      modifiers
    });
  };

  const handleKeyUp = async (event: KeyboardEvent) => {
    if (!canSendControl()) return;
    event.preventDefault();
    const modifiers =
      (event.ctrlKey ? 1 : 0) |
      (event.shiftKey ? 2 : 0) |
      (event.altKey ? 4 : 0) |
      (event.metaKey ? 8 : 0);
    await sendControl({
      type: 'keyUp',
      vkey: event.keyCode ?? 0,
      scan: 0,
      modifiers
    });
  };

  const handlePaste = async (event: ClipboardEvent) => {
    if (!canSendControl() || !clipboardSync) return;
    const text = event.clipboardData?.getData('text') ?? '';
    if (!text) return;
    event.preventDefault();
    await sendControl({ type: 'clipboard', text });
  };

  type SessionParams = {
    sessionId: string;
    token: string;
    agentId: string;
    apiBase: string | null;
    backendApi: string | null;
    mode?: string | null;
    host?: string | null;
    port?: number | null;
    runAs?: string | null;
    targetSessionId?: number | null;
    systemSupported?: boolean;
    relayUrl?: string | null;
    e2eKey?: string | null;
  };

  type LinuxShellCredential = {
    agentId: string;
    username: string;
    password: string;
    credentialId?: string | null;
    version?: number | null;
    updatedAt?: string | null;
  };

  type RemoteDesktopSnapshot = {
    imageBase64: string;
    width: number;
    height: number;
  };

  type AiAssistPoint = {
    x: number;
    y: number;
  };

  type AiAssistAction =
    | {
        type: 'move';
        x: number;
        y: number;
        keys: string[];
      }
    | {
        type: 'click' | 'double_click';
        x: number;
        y: number;
        button: 'left' | 'right' | 'middle';
        keys: string[];
      }
    | {
        type: 'drag';
        path: AiAssistPoint[];
        button: 'left' | 'right' | 'middle';
        keys: string[];
      }
    | {
        type: 'scroll';
        x: number;
        y: number;
        scrollX: number;
        scrollY: number;
        keys: string[];
      }
    | {
        type: 'type';
        text: string;
      }
    | {
        type: 'keypress';
        keys: string[];
      }
    | {
        type: 'wait';
        ms: number;
      };

  type AiAssistResponse = {
    assistantMessage: string;
    actions: AiAssistAction[];
    responseId?: string | null;
  };

  type AiAssistTaskStatus = 'running' | 'complete' | 'failed' | 'needs_approval';

  type AiAssistTaskStepResponse = {
    taskId: string;
    status: AiAssistTaskStatus;
    plan: string[];
    assistantMessage: string;
    actions: AiAssistAction[];
    responseId?: string | null;
    stepIndex: number;
    maxSteps: number;
  };

  type FileTransferEntry = {
    name: string;
    path: string;
    isDir: boolean;
    sizeBytes: number;
    modifiedUnixMs?: number | null;
  };

  type FileTransferResponse =
    | { type: 'list_dir_result'; path: string; entries: FileTransferEntry[] }
    | { type: 'download_ready'; fileName: string; sizeBytes: number; isArchive: boolean }
    | { type: 'upload_ready' }
    | { type: 'ok' }
    | {
        type: 'transfer_complete';
        // Prefer camelCase in UI, but accept snake_case from Rust.
        bytesTransferred?: number;
        extractedEntries?: number;
        bytes_transferred?: number;
        extracted_entries?: number;
      }
    | { type: 'conflict'; path: string; message: string }
    | { type: 'error'; message: string };

  type ConnectResponsePayload = {
    url?: string;
    sessionId?: string;
    session_id?: string;
  };

  type RdpSessionsResponsePayload = {
    sessions?: Array<RdpSessionInfo>;
  };

  const normalizeShellRunAs = (value: string | null | undefined): ShellRunAs =>
    value?.toLowerCase() === 'user' ? 'user' : 'system';

  const shellRunAsStatusLabel = (runAs: ShellRunAs, targetSessionId: number | null): string => {
    if (!isWindowsShellPlatform()) {
      if (activeAgentPlatform === 'linux') return 'configured Linux shell user';
      if (activeAgentPlatform === 'macos') return 'macOS root shell';
      return 'configured shell user';
    }
    if (runAs === 'system') {
      return 'SYSTEM';
    }
    if (targetSessionId === null) {
      return 'logged-in user';
    }
    const session = (rdpSessions ?? []).find(
      (candidate) =>
        normalizeSessionId(candidate.nativeSessionId) === normalizeSessionId(targetSessionId)
    );
    if (!session) {
      return `session ${targetSessionId}`;
    }
    const user = session.userName?.trim();
    return user ? user : `session ${targetSessionId}`;
  };

  const parseRmmUrl = (url: string): SessionParams | null => {
    try {
      const parsed = new URL(url);
      if (parsed.protocol !== 'rmm:') return null;
      const sessionId = parsed.searchParams.get('session');
      const token = parsed.searchParams.get('token');
      const agentId = parsed.searchParams.get('agent');
      if (!sessionId || !token || !agentId) return null;

      const portStr = parsed.searchParams.get('port');
      const apiBase = parsed.searchParams.get('api');
      if (apiBase) {
        void invokeTauri('remember_update_api_base', { apiBase }).catch(() => {});
      }
      return {
        sessionId,
        token,
        agentId,
        apiBase,
        backendApi: parsed.searchParams.get('backendApi'),
        mode: parsed.searchParams.get('mode')?.replace(/-/g, '_'),
        host: parsed.searchParams.get('host'),
        port: portStr ? parseInt(portStr, 10) : null,
        runAs: parsed.searchParams.get('runAs'),
        targetSessionId: parsed.searchParams.get('targetSessionId')
          ? Number(parsed.searchParams.get('targetSessionId'))
          : null,
        systemSupported: parsed.searchParams.get('system') === '1',
        relayUrl: parsed.searchParams.get('relayUrl'),
        e2eKey: parsed.searchParams.get('e2eKey'),
      };
    } catch {
      return null;
    }
  };

  const inheritSessionContext = (
    next: SessionParams | null,
    source: SessionParams | null | undefined
  ): SessionParams | null => {
    if (!next) {
      return null;
    }
    return {
      ...next,
      backendApi: next.backendApi ?? source?.backendApi ?? null
    };
  };

  const runStubPipeline = (caps: SessionCapabilities, info: SessionParams) => {
    const codec = caps.codecs[0] ?? 'unknown';
    remoteDesktopOutput = `Hello World - agent ${info.agentId} (${codec}, ${caps.encoding})`;
    remoteDesktopConnected = true;
  };

  const normalizeDisplayProfileId = (id: string): string => {
    if (id === 'modern_cpu') return 'legacy';
    return id;
  };

  const selectRemoteDesktopDisplayProfile = (caps: SessionCapabilities): RemoteDesktopDisplayProfile => {
    const fallbackProfile: RemoteDesktopDisplayProfile = {
      id: 'legacy',
      protocol: 'legacy_ivf',
      codec: 'vp8',
      compression: 'ivf',
      priority: 3
    };
    const profiles = caps.displayProfiles ?? [];
    for (const profileId of ['modern_gpu', 'legacy', 'modern_cpu', 'experimental']) {
      const profile = profiles.find((candidate) => candidate.id === profileId);
      if (profile) {
        return profile;
      }
    }

    const selectedProfileId = caps.selectedDisplayProfile
      ? normalizeDisplayProfileId(caps.selectedDisplayProfile)
      : null;
    const selectedProfile = selectedProfileId
      ? profiles.find((profile) => normalizeDisplayProfileId(profile.id) === selectedProfileId)
      : undefined;
    if (selectedProfile) {
      return selectedProfile;
    }

    return fallbackProfile;
  };

  const isRemoteDesktopFirstFrameMessage = (message: string): boolean =>
    /first frame rendered|streaming started/i.test(message);

  const isRemoteDesktopWaitingForFrameMessage = (message: string): boolean =>
    /waiting for first frame|relay connected|stream connected|hello-world/i.test(message);

  const fetchCapabilities = async (info: SessionParams): Promise<SessionCapabilities> => {
    if (!info.apiBase) {
      throw new Error('Missing api base in rmm:// link');
    }
    const url = `${info.apiBase}/api/rmm/session/${info.sessionId}/capabilities?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
    if (!response.ok) {
      throw new Error(`Capability lookup failed (${response.status})`);
    }
    return response.json();
  };

  const requestRelay = async (info: SessionParams): Promise<void> => {
    if (!info.apiBase) return;
    const url = `${info.apiBase}/api/rmm/session/${info.sessionId}/request-relay?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Request relay failed (${response.status})`);
    }
  };

  const fetchRegistryCapabilities = async (
    info: SessionParams
  ): Promise<RegistryTransportCapabilities> => {
    if (!info.apiBase) {
      throw new Error('Missing api base in rmm:// link');
    }
    const url = `${info.apiBase}/api/rmm/registry/session/${info.sessionId}/capabilities?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
    if (!response.ok) {
      throw new Error(`Registry capability lookup failed (${response.status})`);
    }
    return response.json();
  };

  const requestRegistryRelay = async (info: SessionParams): Promise<void> => {
    if (!info.apiBase) return;
    const url = `${info.apiBase}/api/rmm/registry/session/${info.sessionId}/request-relay?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Registry relay request failed (${response.status})`);
    }
  };

  const requestShellRelay = async (info: SessionParams): Promise<void> => {
    if (!info.apiBase) return;
    const url = `${info.apiBase}/api/rmm/shell/session/${info.sessionId}/request-relay?token=${encodeURIComponent(info.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Shell relay request failed (${response.status})`);
    }
  };

  const parseConnectUrl = (payload: ConnectResponsePayload): string => {
    if (payload.url && payload.url.trim()) return payload.url;
    throw new Error('Connect response missing url');
  };

  const refreshRdpSessionsForShell = async () => {
    const sessionContext = remoteSessionInfo ?? shellSessionInfo;
    if (!sessionContext?.apiBase || !sessionContext.agentId) {
      return;
    }
    try {
      const url = `${sessionContext.apiBase}/api/rmm/devices/${sessionContext.agentId}/rdp-sessions`;
      const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
      if (!response.ok) {
        return;
      }
      const payload = (await response.json()) as RdpSessionsResponsePayload;
      if (Array.isArray(payload.sessions)) {
        rdpSessions = normalizeRdpSessions(payload.sessions);
      }
    } catch {
      // Best-effort refresh; keep existing session list if request fails.
    }
  };

  const requestShellConnectFromRemote = async (): Promise<SessionParams> => {
    const sessionContext = remoteSessionInfo;
    if (
      !sessionContext?.apiBase ||
      !sessionContext.agentId ||
      !sessionContext.sessionId ||
      !sessionContext.token
    ) {
      throw new Error('Remote session context is missing');
    }
    const runAs = shellRunAs;
    const targetSessionPart =
      runAs === 'user' && shellTargetSessionId !== null
        ? `&targetSessionId=${encodeURIComponent(String(shellTargetSessionId))}`
        : '';
    const url = `${sessionContext.apiBase}/api/rmm/devices/${encodeURIComponent(sessionContext.agentId)}/connect-shell?runAs=${encodeURIComponent(runAs)}${targetSessionPart}&session=${encodeURIComponent(sessionContext.sessionId)}&token=${encodeURIComponent(sessionContext.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 25_000 });
    if (!response.ok) {
      const detail = await response.text().catch(() => '');
      throw new Error(detail || `Shell connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), sessionContext);
    if (!parsed) {
      throw new Error('Invalid shell connect URL received');
    }
    rememberSystemInfoContext(parsed);
    return parsed;
  };

  const requestRemoteConnectFromShell = async (): Promise<SessionParams> => {
    if (!shellSessionInfo?.apiBase || !shellSessionInfo.sessionId || !shellSessionInfo.token) {
      throw new Error('Shell session context is missing');
    }
    const url = `${shellSessionInfo.apiBase}/api/rmm/shell/session/${shellSessionInfo.sessionId}/open-desktop?token=${encodeURIComponent(shellSessionInfo.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Remote desktop connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), shellSessionInfo);
    if (!parsed) {
      throw new Error('Invalid remote desktop connect URL received');
    }
    rememberSystemInfoContext(parsed);
    return parsed;
  };

  const requestRegistryConnectFromRemote = async (): Promise<SessionParams> => {
    if (!remoteSessionInfo?.apiBase || !remoteSessionInfo.sessionId || !remoteSessionInfo.token) {
      throw new Error('Remote session context is missing');
    }
    const url = `${remoteSessionInfo.apiBase}/api/rmm/session/${remoteSessionInfo.sessionId}/open-registry?token=${encodeURIComponent(remoteSessionInfo.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Remote registry connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), remoteSessionInfo);
    if (!parsed) {
      throw new Error('Invalid remote registry connect URL received');
    }
    rememberSystemInfoContext(parsed);
    return parsed;
  };

  const requestRegistryConnectFromShell = async (): Promise<SessionParams> => {
    if (!shellSessionInfo?.apiBase || !shellSessionInfo.sessionId || !shellSessionInfo.token) {
      throw new Error('Shell session context is missing');
    }
    const url = `${shellSessionInfo.apiBase}/api/rmm/shell/session/${shellSessionInfo.sessionId}/open-registry?token=${encodeURIComponent(shellSessionInfo.token)}`;
    const response = await fetchWithTimeout(url, { method: 'POST', timeoutMs: 15_000 });
    if (!response.ok) {
      throw new Error(`Remote registry connect failed (${response.status})`);
    }
    const payload = (await response.json()) as ConnectResponsePayload;
    const parsed = inheritSessionContext(parseRmmUrl(parseConnectUrl(payload)), shellSessionInfo);
    if (!parsed) {
      throw new Error('Invalid remote registry connect URL received');
    }
    rememberSystemInfoContext(parsed);
    return parsed;
  };

  // ── Shell helpers ──────────────────────────────────────────────────────
  const initShellTerminal = async () => {
    await tick();
    if (!shellTerminalEl) return;
    if (shellTerminal) {
      shellFitAddon?.fit();
      shellTerminal.focus();
      return;
    }

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'Cascadia Code', 'Consolas', 'Courier New', monospace",
      theme: {
        background: '#0c1e28',
        foreground: '#d4d4d4',
        cursor: '#d4d4d4',
        selectionBackground: '#264f78',
      },
      allowProposedApi: true,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(shellTerminalEl);
    fitAddon.fit();

    shellTerminal = term;
    shellFitAddon = fitAddon;

    // The viewport is hidden when switching tabs; keep the control channel intact so
    // returning to Remote Desktop does not lose interaction.

    // Send initial size to agent.
    await invokeTauri('shell_resize', { cols: term.cols, rows: term.rows });

    // Wire input: keystrokes → agent (string directly, no byte encoding needed).
    term.onData(async (data: string) => {
      try {
        await invokeTauri('shell_write', { data });
      } catch (err) {
        console.error('[shell] write failed:', err);
      }
    });

    // Focus the terminal so it receives keyboard input.
    term.focus();

    // Also focus on click in case focus was lost.
    shellTerminalEl.addEventListener('click', () => {
      shellTerminal?.focus();
    });

    // Fit on resize.
    shellResizeObserver = new ResizeObserver(() => {
      if (shellFitAddon) {
        shellFitAddon.fit();
      }
      if (shellTerminal) {
        invokeTauri('shell_resize', { cols: shellTerminal.cols, rows: shellTerminal.rows }).catch(() => {});
      }
    });
    shellResizeObserver.observe(shellTerminalEl);
  };

  const closeShellContextMenu = () => {
    if (!shellContextMenu.open) return;
    shellContextMenu = { ...shellContextMenu, open: false };
  };

  const openShellContextMenu = (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (activeTab !== 'System Shell') {
      return;
    }

    const menuWidth = 220;
    const menuHeight = 212;
    const pad = 8;
    const x = Math.max(pad, Math.min((event.clientX ?? 0) + 2, window.innerWidth - menuWidth - pad));
    const y = Math.max(pad, Math.min((event.clientY ?? 0) + 2, window.innerHeight - menuHeight - pad));
    shellContextMenu = {
      open: true,
      x,
      y,
      hasSelection: !!shellTerminal?.getSelection()
    };
    shellTerminal?.focus();
  };

  const copyShellSelection = async () => {
    const selection = shellTerminal?.getSelection() ?? '';
    if (!selection) return;
    try {
      await navigator.clipboard.writeText(selection);
    } catch (error) {
      console.error('[shell] copy selection failed:', error);
    } finally {
      closeShellContextMenu();
      shellTerminal?.focus();
    }
  };

  const pasteShellClipboard = async () => {
    if (!shellConnected) return;
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        await invokeTauri('shell_write', { data: text });
      }
    } catch (error) {
      console.error('[shell] paste clipboard failed:', error);
    } finally {
      closeShellContextMenu();
      shellTerminal?.focus();
    }
  };

  const selectAllShellText = () => {
    shellTerminal?.selectAll();
    closeShellContextMenu();
    shellTerminal?.focus();
  };

  const clearShellScreen = async () => {
    closeShellContextMenu();
    shellTerminal?.focus();
    if (!shellConnected) return;
    try {
      await invokeTauri('shell_write', { data: '\x0c' });
    } catch (error) {
      console.error('[shell] clear screen failed:', error);
    }
  };

  const fetchShellCapabilities = async (info: SessionParams): Promise<ShellCapabilities | null> => {
    if (!info.apiBase) return null;
    try {
      const url = `${info.apiBase}/api/rmm/shell/session/${info.sessionId}/capabilities?token=${encodeURIComponent(info.token)}`;
      const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
      if (!response.ok) return null;
      return response.json();
    } catch {
      return null;
    }
  };

  const revealLinuxShellCredential = async () => {
    const info = shellSessionInfo;
    if (!info?.apiBase || !info.sessionId || !info.token) {
      linuxShellCredentialError = 'Open a live shell session first.';
      return;
    }
    linuxShellCredentialLoading = true;
    linuxShellCredentialError = null;
    try {
      const url = `${info.apiBase}/api/rmm/shell/session/${info.sessionId}/linux-shell-credential?token=${encodeURIComponent(info.token)}`;
      const response = await fetchWithTimeout(url, { timeoutMs: 12_000 });
      if (!response.ok) {
        const detail = await response.text().catch(() => '');
        throw new Error(detail || `Credential reveal failed (${response.status})`);
      }
      linuxShellCredential = await response.json();
    } catch (error) {
      linuxShellCredentialError = error instanceof Error ? error.message : String(error);
    } finally {
      linuxShellCredentialLoading = false;
    }
  };

  const copyLinuxShellCredential = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      linuxShellCredentialError = `${label} copied.`;
    } catch {
      linuxShellCredentialError = `Failed to copy ${label.toLowerCase()}.`;
    }
  };

  const insertLinuxShellCredentialPassword = async () => {
    if (!linuxShellCredential?.password) {
      linuxShellCredentialError = 'Reveal the password first.';
      return;
    }
    if (!shellConnected) {
      linuxShellCredentialError = 'Open a live shell session first.';
      return;
    }
    try {
      await invokeTauri('shell_write', { data: linuxShellCredential.password });
      linuxShellCredentialError = 'Password inserted into terminal.';
      shellTerminal?.focus();
    } catch (error) {
      linuxShellCredentialError =
        error instanceof Error ? error.message : 'Failed to insert password into terminal.';
    }
  };

  const activateShellRelay = async (helloMessage?: string) => {
    if (!shellSessionInfo) {
      return;
    }
    if (shellTransport === 'relay' && shellConnected) {
      return;
    }
    await invokeTauri('shell_select_relay', { token: shellSessionInfo.token });
    shellQuicInProgress = false;
    pendingShellRelayHello = null;
    shellTransport = 'relay';
    shellConnected = true;
    shellError = null;
    shellStatus = 'Connected';
    if (helloMessage) {
      shellTerminal?.writeln(`\r\n\x1b[90m[${helloMessage}]\x1b[0m`);
    }
    await initShellTerminal();
    void notifyViewerSessionConnected(shellSessionInfo);
    void invokeTauri('shell_disconnect_quic').catch(() => {});
  };

  const activateShellQuic = async (helloMessage?: string) => {
    if (!shellSessionInfo) {
      return;
    }
    if (shellTransport === 'quic' && shellConnected) {
      return;
    }
    await invokeTauri('shell_select_quic', { token: shellSessionInfo.token });
    shellQuicInProgress = false;
    pendingShellRelayHello = null;
    shellTransport = 'quic';
    shellConnected = true;
    shellError = null;
    shellStatus = 'Connected';
    if (helloMessage) {
      shellTerminal?.writeln(`\r\n\x1b[90m[${helloMessage}]\x1b[0m`);
    }
    await initShellTerminal();
    void notifyViewerSessionConnected(shellSessionInfo);
    void invokeTauri('shell_disconnect_relay').catch(() => {});
  };

  const prepareShellRelayTransport = async (
    info: SessionParams,
    caps: ShellCapabilities | null,
    runAs: ShellRunAs,
    targetSessionId: number | null,
    updateStatus = true
  ): Promise<string> => {
    const relayUrl = caps?.relayUrl ?? info.relayUrl;
    const e2eKey = caps?.e2eKey ?? info.e2eKey;
    if (!relayUrl || !e2eKey) {
      throw new Error('Shell relay configuration missing');
    }
    if (updateStatus) {
      shellStatus = `Connecting via relay as ${shellRunAsStatusLabel(runAs, targetSessionId)}...`;
      await tick();
    }
    await requestShellRelay(info);
    return invokeTauri<string>('shell_connect_relay', {
      sessionId: info.sessionId,
      relayUrl,
      e2eKey,
      token: info.token,
    });
  };

  const connectShellRelaySession = async (
    info: SessionParams,
    caps: ShellCapabilities | null,
    runAs: ShellRunAs,
    targetSessionId: number | null
  ) => {
    const helloMessage = await prepareShellRelayTransport(info, caps, runAs, targetSessionId);
    await activateShellRelay(helloMessage);
  };

  const connectShellSession = async (info: SessionParams) => {
    resetConnectionInfo('system_shell');
    linuxShellCredential = null;
    linuxShellCredentialError = null;
    const runAs = normalizeShellRunAs(info.runAs ?? shellRunAs);
    const targetSessionId =
      typeof info.targetSessionId === 'number' && Number.isFinite(info.targetSessionId)
        ? normalizeSessionId(info.targetSessionId)
        : shellTargetSessionId;
    shellStatus = `Connecting as ${shellRunAsStatusLabel(runAs, targetSessionId)}...`;
    shellError = null;
    shellConnected = false;
    shellTransport = null;
    shellCapabilities = null;
    shellQuicInProgress = false;
    pendingShellRelayHello = null;
    await tick();

    try {
      viewerTransport = await invokeTauri<string>('get_viewer_transport');
      const caps = await fetchShellCapabilities(info);
      shellCapabilities = caps;
      const supportsQuic =
        viewerTransport !== 'tcprelay' &&
        caps?.transports?.includes('quic') &&
        !!caps?.agentReflex &&
        !!caps?.pskCertPem;
      const supportsRelay =
        caps?.transports?.includes('relay') &&
        !!(caps?.relayUrl ?? info.relayUrl) &&
        !!(caps?.e2eKey ?? info.e2eKey);

      if (viewerTransport === 'tcprelay' && supportsRelay) {
        await connectShellRelaySession(info, caps, runAs, targetSessionId);
        return;
      }

      if (supportsQuic) {
        const shouldRaceRelay = viewerTransport === 'auto' && supportsRelay;
        shellStatus = shouldRaceRelay
          ? `Connecting via QUIC and relay as ${shellRunAsStatusLabel(runAs, targetSessionId)}...`
          : `Connecting via QUIC as ${shellRunAsStatusLabel(runAs, targetSessionId)}...`;
        shellQuicInProgress = true;
        pendingShellRelayHello = null;
        await tick();

        if (shouldRaceRelay) {
          const relayPromise = prepareShellRelayTransport(info, caps, runAs, targetSessionId, false);
          const quicPromise = invokeTauri<string>('shell_connect_quic', {
            sessionId: info.sessionId,
            token: info.token,
            agentReflex: caps!.agentReflex,
            agentHost: caps!.agentHost ?? undefined,
            agentLocalAddrs: caps!.agentLocalAddrs ?? undefined,
            pskCertPem: caps!.pskCertPem,
            apiBase: info.apiBase ?? '',
            quicTimeoutMs: shellQuickConnectTimeoutMs(),
          });
          const quicOutcome = quicPromise.then(
            (helloMessage) => ({ kind: 'quic' as const, helloMessage }),
            (error) => ({ kind: 'quic_error' as const, error })
          );
          const quicPreferenceTimer = new Promise<{ kind: 'timeout' }>((resolve) => {
            window.setTimeout(() => resolve({ kind: 'timeout' }), SHELL_QUICK_CONNECT_AUTO_TIMEOUT_MS);
          });
          const firstOutcome = await Promise.race([quicOutcome, quicPreferenceTimer]);

          if (firstOutcome.kind === 'quic') {
            await activateShellQuic(firstOutcome.helloMessage);
            void relayPromise
              .catch(() => null)
              .finally(() => invokeTauri('shell_disconnect_relay').catch(() => {}));
            return;
          }

          shellQuicInProgress = false;
          shellStatus =
            firstOutcome.kind === 'timeout'
              ? 'QUIC timed out after 1s, using relay...'
              : 'QUIC failed, using relay...';
          void quicPromise
            .catch(() => null)
            .finally(() => {
              if (shellTransport !== 'quic') {
                void invokeTauri('shell_disconnect_quic').catch(() => {});
              }
            });
          const relayHelloMessage = await relayPromise;
          await activateShellRelay(relayHelloMessage);
          return;
        }

        try {
          const helloMessage = await invokeTauri<string>('shell_connect_quic', {
            sessionId: info.sessionId,
            token: info.token,
            agentReflex: caps!.agentReflex,
            agentHost: caps!.agentHost ?? undefined,
            agentLocalAddrs: caps!.agentLocalAddrs ?? undefined,
            pskCertPem: caps!.pskCertPem,
            apiBase: info.apiBase ?? '',
            quicTimeoutMs: shellQuickConnectTimeoutMs(),
          });
          await activateShellQuic(helloMessage);
        } catch (quicErr) {
          shellQuicInProgress = false;
          if (viewerTransport === 'quic' || !supportsRelay) {
            throw quicErr;
          }
          shellStatus = 'QUIC failed, trying relay...';
          await connectShellRelaySession(info, caps, runAs, targetSessionId);
        }
        return;
      }

      if (supportsRelay || (info.relayUrl && info.e2eKey)) {
        await connectShellRelaySession(info, caps, runAs, targetSessionId);
        return;
      }

      if (info.host && info.port) {
        await invokeTauri('shell_connect', {
          host: info.host,
          port: info.port,
          token: info.token,
        });
        shellTransport = 'tcp';
        shellConnected = true;
        shellStatus = 'Connected';
        await initShellTerminal();
        void notifyViewerSessionConnected(info);
        return;
      }

      throw new Error('No compatible shell transport available');
    } catch (err) {
      shellError = err instanceof Error ? err.message : String(err);
      shellStatus = 'Connection failed';
      shellQuicInProgress = false;
    }
  };

  const connectRemoteDesktopSession = async (info: SessionParams) => {
    resetConnectionInfo('remote_desktop');
    remoteDesktopError = null;
    remoteDesktopOutput = null;
    remoteDesktopFrameImage = null;
    remoteDesktopFrameWidth = 0;
    remoteDesktopFrameHeight = 0;
    capabilities = null;
    remoteDesktopConnected = false;
    connecting = true;
    remoteDesktopStatus = 'Fetching capabilities...';
    activeTransport = null;
    pendingRelayHello = null;
    pendingRelayError = null;

    try {
      viewerTransport = await invokeTauri<string>('get_viewer_transport');
      const caps = await fetchCapabilities(info);
      capabilities = caps;
      const selectedProfile = selectRemoteDesktopDisplayProfile(caps);

      if (viewerTransport === 'tcprelay' && caps.transports.includes('relay') && caps.relayUrl && caps.e2eKey) {
        remoteDesktopStatus = 'Connecting via relay...';
        await requestRelay(info);
        await invokeTauri('connect_relay', {
          sessionId: info.sessionId,
          relayUrl: caps.relayUrl,
          e2eKey: caps.e2eKey,
          token: info.token,
          apiBase: info.apiBase ?? undefined,
          selectedStreamProtocol: selectedProfile.protocol
        });
      } else if (caps.agentReflex && caps.pskCertPem && caps.transports.includes('quic') && viewerTransport !== 'tcprelay') {
        remoteDesktopStatus = 'Connecting via QUIC...';
        quicInProgress = true;
        pendingRelayHello = null;
        pendingRelayError = null;
        if (viewerTransport === 'auto' && caps.transports.includes('relay') && caps.relayUrl && caps.e2eKey) {
          try {
            await requestRelay(info);
            await invokeTauri('connect_relay', {
              sessionId: info.sessionId,
              relayUrl: caps.relayUrl,
              e2eKey: caps.e2eKey,
              token: info.token,
              apiBase: info.apiBase ?? undefined,
              selectedStreamProtocol: selectedProfile.protocol
            });
          } catch {}
        }
        const quicTimeoutMs = quickConnectTimeoutMs();
        try {
          await invokeTauri('connect_quic', {
            sessionId: info.sessionId,
            token: info.token,
            agentReflex: caps.agentReflex,
            agentHost: caps.agentHost ?? undefined,
            agentLocalAddrs: caps.agentLocalAddrs ?? undefined,
            pskCertPem: caps.pskCertPem,
            apiBase: info.apiBase ?? undefined,
            quicTimeoutMs,
            selectedStreamProtocol: selectedProfile.protocol
          });
        } catch (err) {
          quicInProgress = false;
          if (viewerTransport === 'quic') {
            remoteDesktopError = err instanceof Error ? err.message : String(err);
            remoteDesktopStatus = 'Connection failed';
          }
        }
      } else if (caps.transports.includes('relay') && caps.relayUrl && caps.e2eKey && viewerTransport === 'auto') {
        remoteDesktopStatus = 'Connecting via relay...';
        await requestRelay(info);
        await invokeTauri('connect_relay', {
          sessionId: info.sessionId,
          relayUrl: caps.relayUrl,
          e2eKey: caps.e2eKey,
          token: info.token,
          apiBase: info.apiBase ?? undefined,
          selectedStreamProtocol: selectedProfile.protocol
        });
      } else if (caps.transports.includes('relay')) {
        remoteDesktopOutput = 'Relay configuration missing or transport is quic-only.';
        remoteDesktopStatus = 'Relay unavailable';
      } else {
        runStubPipeline(caps, info);
        remoteDesktopStatus = 'Connected';
        void notifyViewerSessionConnected(info);
      }
    } catch (error) {
      remoteDesktopError = error instanceof Error ? error.message : String(error);
      remoteDesktopStatus = 'Connection failed';
    } finally {
      connecting = false;
    }
  };

  const ensureShellSessionLive = async () => {
    if (shellConnected || shellConnectInFlight) {
      if (activeTab === 'System Shell') {
        shellTerminal?.focus();
      }
      return;
    }
    shellConnectInFlight = true;
    try {
      shellSessionInfo = await requestShellConnectFromRemote();
      shellRunAs = normalizeShellRunAs(shellSessionInfo.runAs);
      shellTargetSessionId =
        typeof shellSessionInfo.targetSessionId === 'number' &&
        Number.isFinite(shellSessionInfo.targetSessionId)
          ? normalizeSessionId(shellSessionInfo.targetSessionId)
          : null;
      await connectShellSession(shellSessionInfo);
      await refreshRdpSessionsForShell();
    } catch (error) {
      shellError = error instanceof Error ? error.message : String(error);
      shellStatus = 'Connection failed';
    } finally {
      shellConnectInFlight = false;
    }
  };

  const handleShellRunAsSelect = async (
    selectedRunAs: ShellRunAs,
    selectedTargetSessionId: number | null = null
  ) => {
    shellRunAsDropdownOpen = false;
    if (
      selectedRunAs === shellRunAs &&
      normalizeSessionId(selectedTargetSessionId ?? -1) === normalizeSessionId(shellTargetSessionId ?? -1)
    ) {
      return;
    }
    shellRunAs = selectedRunAs;
    shellTargetSessionId =
      selectedRunAs === 'user' && selectedTargetSessionId !== null
        ? normalizeSessionId(selectedTargetSessionId)
        : null;
    if (activeTab !== 'System Shell' || shellConnectInFlight) {
      return;
    }
    shellConnectInFlight = true;
    try {
      await disconnectShell();
      shellSessionInfo = await requestShellConnectFromRemote();
      shellRunAs = normalizeShellRunAs(shellSessionInfo.runAs);
      shellTargetSessionId =
        typeof shellSessionInfo.targetSessionId === 'number' &&
        Number.isFinite(shellSessionInfo.targetSessionId)
          ? normalizeSessionId(shellSessionInfo.targetSessionId)
          : null;
      await connectShellSession(shellSessionInfo);
    } catch (error) {
      shellError = error instanceof Error ? error.message : String(error);
      shellStatus = 'Connection failed';
    } finally {
      shellConnectInFlight = false;
    }
  };

  const ensureRemoteSessionLive = async () => {
    if (remoteDesktopConnected || remoteConnectInFlight || activeTransport !== null || quicInProgress) {
      return;
    }
    remoteConnectInFlight = true;
    try {
      if (!remoteSessionInfo || remoteDesktopStatus === 'Connection failed') {
        remoteSessionInfo = await requestRemoteConnectFromShell();
      }
      await connectRemoteDesktopSession(remoteSessionInfo);
    } finally {
      remoteConnectInFlight = false;
    }
  };

  const disconnectRegistrySession = async () => {
    if (registryConnectInFlight) return;
    registryConnectInFlight = true;
    registryError = null;
    registryStatus = 'Disconnecting...';
    try {
      await Promise.allSettled([
        invokeTauri('registry_disconnect_quic'),
        invokeTauri('registry_disconnect_relay')
      ]);
    } catch {
      // Best-effort only.
    } finally {
      registryConnected = false;
      registryTransport = null;
      registryStatus = 'Not connected';
      registryQuicInProgress = false;
      pendingRegistryRelayHello = null;
      pendingRegistryRelayError = null;
      registryConnectInFlight = false;
    }
  };

  const connectRegistrySession = async (info: SessionParams) => {
    registryError = null;
    registryCapabilities = null;
    registryConnected = false;
    registryTransport = null;
    registryStatus = 'Fetching capabilities...';
    registryQuicInProgress = false;
    pendingRegistryRelayHello = null;
    pendingRegistryRelayError = null;

    try {
      // Ensure only one registry transport is active.
      await Promise.allSettled([
        invokeTauri('registry_disconnect_quic'),
        invokeTauri('registry_disconnect_relay')
      ]);

      viewerTransport = await invokeTauri<string>('get_viewer_transport');
      const caps = await fetchRegistryCapabilities(info);
      registryCapabilities = caps;

      const supportsRelay = caps.transports.includes('relay') && !!caps.relayUrl && !!caps.e2eKey;
      const supportsQuic =
        viewerTransport !== 'tcprelay' &&
        caps.transports.includes('quic') &&
        !!caps.agentReflex &&
        !!caps.pskCertPem;

      if (viewerTransport === 'tcprelay') {
        if (!supportsRelay) {
          throw new Error('Relay unavailable for registry session');
        }
        registryStatus = 'Connecting via relay...';
        await requestRegistryRelay(info);
        await invokeTauri('registry_connect_relay', {
          sessionId: info.sessionId,
          relayUrl: caps.relayUrl,
          e2eKey: caps.e2eKey
        });
        return;
      }

      if (supportsQuic) {
        registryStatus = 'Connecting via QUIC...';
        registryQuicInProgress = true;
        pendingRegistryRelayHello = null;
        pendingRegistryRelayError = null;

        if (viewerTransport === 'auto' && supportsRelay) {
          void (async () => {
            try {
              await requestRegistryRelay(info);
              await invokeTauri('registry_connect_relay', {
                sessionId: info.sessionId,
                relayUrl: caps.relayUrl,
                e2eKey: caps.e2eKey
              });
            } catch {}
          })();
        }

        try {
          await invokeTauri('registry_connect_quic', {
            sessionId: info.sessionId,
            token: info.token, // token not logged
            agentReflex: caps.agentReflex,
            agentHost: caps.agentHost ?? undefined,
            agentLocalAddrs: caps.agentLocalAddrs ?? undefined,
            pskCertPem: caps.pskCertPem,
            apiBase: info.apiBase ?? undefined,
            quicTimeoutMs: quickConnectTimeoutMs()
          });
        } catch (quicErr) {
          registryQuicInProgress = false;
          if (viewerTransport === 'quic' || !supportsRelay) {
            throw quicErr;
          }
        }
        return;
      }

      if (supportsRelay) {
        registryStatus = 'Connecting via relay...';
        await requestRegistryRelay(info);
        await invokeTauri('registry_connect_relay', {
          sessionId: info.sessionId,
          relayUrl: caps.relayUrl,
          e2eKey: caps.e2eKey
        });
        return;
      }

      throw new Error('No compatible transport for registry session');
    } catch (err) {
      registryError = err instanceof Error ? err.message : String(err);
      registryStatus = 'Connection failed';
      registryConnected = false;
      registryTransport = null;
    }
  };

  const ensureRegistrySessionLive = async () => {
    if (registryConnectInFlight || registryConnected) {
      return;
    }
    registryConnectInFlight = true;
    try {
      registryError = null;
      registryStatus = 'Preparing session...';
      await tick();
      if (!registrySessionInfo) {
        if (remoteSessionInfo) {
          registrySessionInfo = await requestRegistryConnectFromRemote();
        } else if (shellSessionInfo) {
          registrySessionInfo = await requestRegistryConnectFromShell();
        } else {
          throw new Error('Registry session context is missing');
        }
      }
      await connectRegistrySession(registrySessionInfo);
    } catch (err) {
      registryError = err instanceof Error ? err.message : String(err);
      registryStatus = 'Connection failed';
    } finally {
      registryConnectInFlight = false;
    }
  };

  const disconnectShell = async () => {
    shellResizeObserver?.disconnect();
    shellResizeObserver = null;
    shellTerminal?.dispose();
    shellTerminal = null;
    shellFitAddon = null;
    shellConnected = false;
    shellTransport = null;
    shellCapabilities = null;
    shellQuicInProgress = false;
    pendingShellRelayHello = null;
    shellStatus = '';
    shellError = null;
    linuxShellCredential = null;
    linuxShellCredentialError = null;
    shellCredentialPanelOpen = false;
    shellContextMenu = { ...shellContextMenu, open: false };
    shellTranscriptBuffer = '';
    shellTranscriptRevision = 0;
    shellAssistRunId += 1;
    shellAssistGoal = '';
    shellAssistProposal = null;
    shellAssistTurns = [];
    shellAssistInFlight = false;
    try {
      await invokeTauri('shell_disconnect');
    } catch {}
  };

  const handleDeepLink = async (url: string) => {
    const info = parseRmmUrl(url);
    if (!info) return;
    rememberSystemInfoContext(info);

    // Shell mode: route directly to shell connection flow.
    if (info.mode === 'shell') {
      shellRunAs = normalizeShellRunAs(info.runAs);
      shellTargetSessionId =
        typeof info.targetSessionId === 'number' && Number.isFinite(info.targetSessionId)
          ? normalizeSessionId(info.targetSessionId)
          : null;
      shellSessionInfo = info;
      activeTab = 'System Shell';
      shellConnectInFlight = true;
      try {
        await connectShellSession(info);
        await refreshRdpSessionsForShell();
      } finally {
        shellConnectInFlight = false;
      }
      return;
    }
    if (info.mode === 'desktop') {
      shellRunAs = 'system';
      shellTargetSessionId = null;
    }
    if (info.mode === 'file_transfer') {
      fileTransferSessionInfo = info;
      activeTab = 'File Transfer';
      fileTransferConnectInFlight = true;
      try {
        await connectFileTransferSession(info);
      } finally {
        fileTransferConnectInFlight = false;
      }
      return;
    }
    if (info.mode === 'registry') {
      registrySessionInfo = info;
      activeTab = 'Remote Registry';
      registryConnectInFlight = true;
      try {
        await connectRegistrySession(info);
      } finally {
        registryConnectInFlight = false;
      }
      return;
    }
    if (info.mode === 'chat') {
      chatSessionInfo = info;
      chatPanelOpen = true;
      remoteSessionInfo = info;
      activeTab = 'Remote Desktop';
      remoteConnectInFlight = true;
      try {
        await connectChatSession(info);
      } finally {
        remoteConnectInFlight = false;
      }
      return;
    }
    remoteSessionInfo = info;
    activeTab = 'Remote Desktop';
    remoteConnectInFlight = true;
    try {
      await connectRemoteDesktopSession(info);
    } finally {
      remoteConnectInFlight = false;
    }
  };

  const spawnSessionWindow = async (url: string) => {
    if (typeof url !== 'string' || !url.startsWith('rmm://')) {
      return;
    }
    try {
      await invokeTauri<string>('spawn_session_window', { url });
    } catch (err) {
      remoteDesktopError = err instanceof Error ? err.message : String(err);
      remoteDesktopStatus = 'Failed to open session window';
      remoteDesktopConnected = false;
    }
  };

  onMount(() => {
    let disposed = false;
    const handleBeforeUnload = () => {
      if (viewerUpdateExitInProgress) return;
      void endSession();
    };
    const initialize = async () => {
      restoreSystemInfoContext();
      void loadThemePreference();
      if (isTauriRuntime()) {
        void getVersion()
          .then((version) => {
            viewerVersion = version;
          })
          .catch(() => {});
      }
      if (isTauriRuntime()) {
      await listen('viewer:update-before-exit', async () => {
        viewerUpdateExitInProgress = true;
        try {
          await endSession(true);
        } finally {
          await invokeTauri('viewer_complete_update_exit_cleanup').catch(() => {});
        }
      });
      const windowLabel = await invokeTauri<string>('get_window_label').catch(() => 'main');
      const isDispatcherWindow = windowLabel === 'main';
      const isSessionWindow = windowLabel.startsWith('session-');

      if (isSessionWindow) {
        const sessionEvents = getCurrentWebviewWindow();
        // Session windows own all per-session event listeners. The dispatcher ("main") window
        // must not listen for these, otherwise it can accidentally toggle connection state and
        // suppress or mis-size the native viewport.
        await sessionEvents.listen<string>('quic:hello', (event) => {
          const message = event.payload.trim();
          if (
            isRemoteDesktopWaitingForFrameMessage(message) &&
            !isRemoteDesktopFirstFrameMessage(message)
          ) {
            remoteDesktopOutput = `Tunnel live: ${message}`;
            remoteDesktopError = null;
            remoteDesktopStatus = 'Waiting for first frame';
            pendingRelayError = null;
            return;
          }
          quicInProgress = false;
          activeTransport = 'quic';
          remoteDesktopConnected = true;
          remoteDesktopOutput = `Tunnel live: ${message}`;
          remoteDesktopError = null;
          remoteDesktopStatus = 'Connected';
          pendingRelayHello = null;
          pendingRelayError = null;
          void notifyViewerSessionConnected(remoteSessionInfo);
          void applySelectedVideoQuality();
          void invokeTauri('disconnect_relay').catch(() => {});
        });
        await sessionEvents.listen('quic:ended', () => {
          if (activeTransport === 'quic') {
            remoteDesktopOutput = '';
            remoteDesktopError = null;
            remoteDesktopStatus = 'Session ended';
            activeTransport = null;
            remoteDesktopConnected = false;
            resetConnectionInfo('remote_desktop');
          }
        });
        await sessionEvents.listen<string>('quic:error', async (event) => {
          quicInProgress = false;
          if (
            viewerTransport === 'auto' &&
            capabilities?.transports.includes('relay') &&
            capabilities?.relayUrl &&
            capabilities?.e2eKey &&
            remoteSessionInfo
          ) {
            remoteDesktopStatus = 'QUIC failed, trying relay...';
            if (pendingRelayHello) {
              remoteDesktopOutput = `Relay live: ${pendingRelayHello}`;
              remoteDesktopError = null;
              remoteDesktopStatus = 'Connected';
              pendingRelayHello = null;
              pendingRelayError = null;
              activeTransport = 'relay';
              remoteDesktopConnected = true;
              void applySelectedVideoQuality();
              return;
            }
            if (pendingRelayError) {
              remoteDesktopConnected = false;
              remoteDesktopError = `QUIC failed: ${event.payload}; relay failed: ${pendingRelayError}`;
              remoteDesktopStatus = 'Connection failed';
              return;
            }
            return;
          }
          remoteDesktopConnected = false;
          remoteDesktopError = event.payload;
          remoteDesktopStatus = 'Connection failed';
          resetConnectionInfo('remote_desktop');
        });
        await sessionEvents.listen<string>('relay:hello', (event) => {
          const message = event.payload.trim();
          if (activeTransport === 'quic') {
            void invokeTauri('disconnect_relay').catch(() => {});
            return;
          }
          if (
            isRemoteDesktopWaitingForFrameMessage(message) &&
            !isRemoteDesktopFirstFrameMessage(message)
          ) {
            pendingRelayError = null;
            if (viewerTransport === 'auto' && quicInProgress) {
              return;
            }
            remoteDesktopOutput = `Relay live: ${message}`;
            remoteDesktopError = null;
            remoteDesktopStatus = 'Waiting for first frame';
            return;
          }
          if (viewerTransport === 'auto' && quicInProgress) {
            pendingRelayError = null;
            pendingRelayHello = message;
            return;
          }
          activeTransport = 'relay';
          remoteDesktopConnected = true;
          remoteDesktopOutput = `Relay live: ${message}`;
          remoteDesktopError = null;
          remoteDesktopStatus = 'Connected';
          pendingRelayError = null;
          void notifyViewerSessionConnected(remoteSessionInfo);
          void applySelectedVideoQuality();
        });
        await sessionEvents.listen<RemoteDesktopFramePayload>('remote-desktop:frame', (event) => {
          const mimeType = event.payload.mimeType ?? 'image/png';
          remoteDesktopFrameImage = `data:${mimeType};base64,${event.payload.imageBase64}`;
          remoteDesktopFrameWidth = event.payload.width;
          remoteDesktopFrameHeight = event.payload.height;
        });
        await sessionEvents.listen<string>('relay:error', (event) => {
          if (activeTransport === 'quic') {
            return;
          }
          if (viewerTransport === 'auto' && quicInProgress && activeTransport !== 'relay') {
            pendingRelayError = event.payload;
            return;
          }
          if (viewerTransport === 'auto' && activeTransport !== 'relay' && !remoteDesktopConnected) {
            pendingRelayError = event.payload;
            remoteDesktopConnected = false;
            remoteDesktopError = event.payload;
            remoteDesktopStatus = 'Connection failed';
            resetConnectionInfo('remote_desktop');
            return;
          }
          remoteDesktopConnected = false;
          remoteDesktopError = event.payload;
          remoteDesktopStatus = 'Relay failed';
          resetConnectionInfo('remote_desktop');
        });
        await sessionEvents.listen('relay:ended', () => {
          if (activeTransport === 'relay') {
            remoteDesktopOutput = '';
            remoteDesktopError = null;
            remoteDesktopStatus = 'Session ended';
            activeTransport = null;
            remoteDesktopConnected = false;
            resetConnectionInfo('remote_desktop');
          }
        });
        await sessionEvents.listen<Record<string, unknown>>('chat/inbound', (event) => {
          const p = event.payload;
          if (!p || typeof p !== 'object') return;
          if (
            p.kind === 'message' &&
            typeof (p as { text?: unknown }).text === 'string' &&
            typeof (p as { id?: unknown }).id === 'string'
          ) {
            const fromViewer = !!(
              (p as { fromViewer?: boolean }).fromViewer ?? (p as { from_viewer?: boolean }).from_viewer
            );
            upsertChatMessage({
              id: (p as { id: string }).id,
              fromViewer,
              text: (p as { text: string }).text,
              state: 'sent'
            });
          }
        });
        await sessionEvents.listen<Record<string, unknown>>('chat/ack', (event) => {
          const p = event.payload;
          if (!p || typeof p !== 'object') return;
          const messageId =
            (p as { messageId?: unknown }).messageId ?? (p as { message_id?: unknown }).message_id;
          if (typeof messageId === 'string') {
            markChatMessage(messageId, 'sent');
          }
        });
        await sessionEvents.listen<Record<string, unknown>>('chat/status', (event) => {
          const p = event.payload;
          if (!p || typeof p !== 'object') return;
          if (typeof (p as { connected?: unknown }).connected === 'boolean') {
            chatConnected = (p as { connected: boolean }).connected;
            if (!chatConnected) {
              chatStatus = 'Chat disconnected';
              failPendingChatMessages();
              const error = (p as { error?: unknown }).error;
              if (typeof error === 'string' && error.trim()) {
                chatError = error;
              }
            }
          }
        });
        await sessionEvents.listen<ConnectionStatePayload>('connection:state', (event) => {
          applyConnectionState(event.payload);
        });
        await sessionEvents.listen<ConnectionStatsPayload>('connection:stats', (event) => {
          applyConnectionStats(event.payload);
        });
        await sessionEvents.listen<{ sessions: Array<RdpSessionInfo> }>(
          'rdp_sessions',
          (event) => {
            rdpSessions = normalizeRdpSessions(event.payload?.sessions);
            if (sessionSwitchInFlight) {
              sessionSwitchInFlight = false;
            }
          }
        );
        await sessionEvents.listen<{
          outputs: Array<CaptureOutputInfo>;
          activeIndex: number;
          captureType?: string | null;
        }>('capture_outputs', (event) => {
          const payload = event.payload;
          const list = payload?.outputs;
          captureOutputs = Array.isArray(list) ? [...list] : null;
          const nextActiveIndex =
            typeof payload?.activeIndex === 'number'
              ? payload.activeIndex
              : list?.[0]?.index ?? 0;
          activeCaptureOutputIndex = nextActiveIndex;
          if (typeof payload?.captureType === 'string' && payload.captureType.trim()) {
            remoteDesktopCaptureType = payload.captureType;
          }
          if (
            pendingCaptureOutputIndex !== null &&
            captureOutputs !== null &&
            !captureOutputs.some((out) => out.index === pendingCaptureOutputIndex)
          ) {
            failPendingCaptureOutputSwitch('Selected display is no longer available', false);
            return;
          }
          if (pendingCaptureOutputIndex !== null && nextActiveIndex === pendingCaptureOutputIndex) {
            clearCaptureOutputSwitchTimeout();
            pendingCaptureOutputIndex = null;
            lastRequestedCaptureOutputIndex = null;
            captureOutputSwitchError = null;
            monitorPickerOpen = false;
            void scheduleViewportRectRefresh();
          } else if (
            pendingCaptureOutputIndex === null &&
            lastRequestedCaptureOutputIndex !== null &&
            nextActiveIndex === lastRequestedCaptureOutputIndex
          ) {
            lastRequestedCaptureOutputIndex = null;
            captureOutputSwitchError = null;
            monitorPickerOpen = false;
            void scheduleViewportRectRefresh();
          }
        });

        // Remote Registry events (separate from Remote Desktop)
        await sessionEvents.listen<string>('registry:quic:hello', () => {
          registryQuicInProgress = false;
          pendingRegistryRelayHello = null;
          pendingRegistryRelayError = null;
          registryTransport = 'quic';
          registryConnected = true;
          registryError = null;
          registryStatus = 'Connected';
          void notifyViewerSessionConnected(registrySessionInfo);
          void invokeTauri('registry_disconnect_relay').catch(() => {});
        });
        await sessionEvents.listen<string>('registry:quic:error', (event) => {
          registryQuicInProgress = false;
          if (
            viewerTransport === 'auto' &&
            registryCapabilities?.transports.includes('relay') &&
            registryCapabilities?.relayUrl &&
            registryCapabilities?.e2eKey &&
            registrySessionInfo
          ) {
            registryStatus = 'QUIC failed, trying relay...';
            if (pendingRegistryRelayHello) {
              registryTransport = 'relay';
              registryConnected = true;
              registryError = null;
              registryStatus = 'Connected';
              pendingRegistryRelayHello = null;
              pendingRegistryRelayError = null;
              return;
            }
            if (pendingRegistryRelayError) {
              registryTransport = null;
              registryConnected = false;
              registryError = `QUIC failed: ${event.payload}; relay failed: ${pendingRegistryRelayError}`;
              registryStatus = 'Connection failed';
              resetConnectionInfo('remote_registry');
              return;
            }
            return;
          }
          registryTransport = null;
          registryConnected = false;
          registryError = event.payload;
          registryStatus = 'Connection failed';
          resetConnectionInfo('remote_registry');
        });
        await sessionEvents.listen('registry:quic:ended', () => {
          registryQuicInProgress = false;
          if (registryTransport === 'quic') {
            registryTransport = null;
            registryConnected = false;
            registryStatus = 'Disconnected';
            resetConnectionInfo('remote_registry');
          }
        });
        await sessionEvents.listen<string>('registry:relay:hello', (event) => {
          if (registryTransport === 'quic') {
            void invokeTauri('registry_disconnect_relay').catch(() => {});
            return;
          }
          if (viewerTransport === 'auto' && registryQuicInProgress) {
            pendingRegistryRelayError = null;
            pendingRegistryRelayHello = event.payload;
            return;
          }
          registryTransport = 'relay';
          registryConnected = true;
          registryError = null;
          registryStatus = 'Connected';
          void notifyViewerSessionConnected(registrySessionInfo);
          void invokeTauri('registry_disconnect_quic').catch(() => {});
        });
        await sessionEvents.listen<string>('registry:relay:error', (event) => {
          if (registryTransport === 'quic') {
            return;
          }
          if (viewerTransport === 'auto' && registryQuicInProgress && registryTransport !== 'relay') {
            pendingRegistryRelayError = event.payload;
            return;
          }
          registryTransport = null;
          registryConnected = false;
          registryError = event.payload;
          registryStatus = 'Relay failed';
          resetConnectionInfo('remote_registry');
        });
        await sessionEvents.listen('registry:relay:ended', () => {
          if (registryTransport === 'relay') {
            registryTransport = null;
            registryConnected = false;
            registryStatus = 'Disconnected';
            resetConnectionInfo('remote_registry');
          }
        });

        await sessionEvents.listen<string>('shell:quic:hello', async (event) => {
          await activateShellQuic(event.payload);
        });
        await sessionEvents.listen<string>('shell:quic:error', async (event) => {
          shellQuicInProgress = false;
          if (
            viewerTransport === 'auto' &&
            shellCapabilities?.transports.includes('relay') &&
            (shellCapabilities?.relayUrl || shellSessionInfo?.relayUrl) &&
            (shellCapabilities?.e2eKey || shellSessionInfo?.e2eKey) &&
            shellSessionInfo
          ) {
            shellStatus = 'QUIC failed, trying relay...';
            if (pendingShellRelayHello) {
              await activateShellRelay(pendingShellRelayHello);
            } else {
              try {
                await connectShellRelaySession(
                  shellSessionInfo,
                  shellCapabilities,
                  shellRunAs,
                  shellTargetSessionId
                );
              } catch (error) {
                shellTransport = null;
                shellConnected = false;
                shellError = error instanceof Error ? error.message : String(error);
                shellStatus = 'Connection failed';
                resetConnectionInfo('system_shell');
              }
            }
            return;
          }
          if (shellTransport === 'quic' || viewerTransport === 'quic') {
            shellTransport = null;
            shellConnected = false;
            shellError = event.payload;
            shellStatus = 'Connection failed';
            resetConnectionInfo('system_shell');
          }
        });
        await sessionEvents.listen('shell:quic:ended', () => {
          if (shellTransport === 'quic') {
            shellTransport = null;
            shellConnected = false;
            shellStatus = 'Disconnected';
            resetConnectionInfo('system_shell');
          }
        });
        await sessionEvents.listen<string>('shell:relay:hello', async (event) => {
          if (shellTransport === 'quic') {
            void invokeTauri('shell_disconnect_relay').catch(() => {});
            return;
          }
          if (viewerTransport === 'auto' && shellQuicInProgress) {
            pendingShellRelayHello = event.payload;
            return;
          }
          await activateShellRelay(event.payload);
        });
        await sessionEvents.listen<string>('shell:relay:error', (event) => {
          if (shellTransport === 'relay') {
            shellTransport = null;
            shellConnected = false;
            shellError = event.payload;
            shellStatus = 'Relay failed';
            resetConnectionInfo('system_shell');
          }
        });
        await sessionEvents.listen('shell:relay:ended', () => {
          if (shellTransport === 'relay') {
            shellTransport = null;
            shellConnected = false;
            shellStatus = 'Disconnected';
            resetConnectionInfo('system_shell');
          }
        });

        // Shell events
        await sessionEvents.listen<number[]>('shell:data', (event) => {
          if (shellTerminal && event.payload) {
            const bytes = new Uint8Array(event.payload);
            appendShellTranscript(new TextDecoder().decode(bytes));
            shellTerminal.write(bytes);
          }
        });
        await sessionEvents.listen<number>('shell:exit', (event) => {
          shellConnected = false;
          shellTransport = null;
          shellStatus = `Shell exited (code ${event.payload})`;
          resetConnectionInfo('system_shell');
          shellTerminal?.writeln(`\r\n\x1b[90m[Process exited with code ${event.payload}]\x1b[0m`);
        });
        await sessionEvents.listen<string>('shell:error', (event) => {
          shellConnected = false;
          shellTransport = null;
          shellError = event.payload;
          shellStatus = 'Disconnected';
          resetConnectionInfo('system_shell');
          shellTerminal?.writeln(`\r\n\x1b[31m[Error: ${event.payload}]\x1b[0m`);
        });

        await sessionEvents.listen<FileTransferProgressEvent>('file-transfer:progress', (event) => {
          if (!event.payload?.jobId) {
            return;
          }
          const existing = fileTransferJobs.find((job) => job.id === event.payload.jobId);
          const progressJob: FileTransferJob = {
            id: event.payload.jobId,
            direction: event.payload.direction ?? existing?.direction ?? 'upload',
            fileName: event.payload.fileName || existing?.fileName || 'Transfer',
            bytesDone: Math.max(event.payload.bytesDone ?? existing?.bytesDone ?? 0, 0),
            bytesTotal: Math.max(event.payload.bytesTotal ?? existing?.bytesTotal ?? 0, 0),
            status:
              existing?.status === 'done' || existing?.status === 'error' || existing?.status === 'cancelled'
                ? existing.status
                : 'running',
            phase: event.payload.phase ?? existing?.phase ?? 'transferring',
            message: event.payload.message ?? existing?.message,
            createdAt: existing?.createdAt ?? Date.now(),
            updatedAt: Date.now()
          };
          upsertFileTransferJob(progressJob);
          if (progressJob.status === 'running') {
            const operation = progressJob.direction === 'upload' ? 'Upload' : 'Download';
            if (progressJob.phase === 'preparing') {
              fileTransferStatus = `${operation} preparing...`;
            } else if (progressJob.phase === 'finalizing') {
              fileTransferStatus = `${operation} finalizing...`;
            } else {
              fileTransferStatus = `${operation} in progress...`;
            }
          }
        });

        viewerHeartbeatTimer = window.setInterval(() => {
          if (remoteDesktopConnected) {
            void notifyViewerSessionHeartbeat(remoteSessionInfo);
          }
          if (shellConnected) {
            void notifyViewerSessionHeartbeat(shellSessionInfo);
          }
          if (fileTransferConnected) {
            void notifyViewerSessionHeartbeat(fileTransferSessionInfo);
          }
          if (registryConnected) {
            void notifyViewerSessionHeartbeat(registrySessionInfo);
          }
        }, VIEWER_HEARTBEAT_INTERVAL_MS);

        const initialUrl = await invokeTauri<string | null>('take_initial_url').catch(() => null);
        launchUrl = initialUrl ?? null;
        if (launchUrl) {
          await handleDeepLink(launchUrl);
        }
      } else if (isDispatcherWindow) {
        await listen<string>('rmm:open-url', (event) => {
          const url = event.payload;
          if (typeof url !== 'string' || !url.startsWith('rmm://')) {
            return;
          }
          // Single-instance forwarding (Windows): a new protocol launch forwards argv
          // through Rust; we open a new session window for every deep link.
          void spawnSessionWindow(url);
        });

        try {
          await register('rmm');
        } catch {}

        const currentUrls = await getCurrent();
        const urlFromDeepLink = currentUrls?.[0];
        launchArgs = await invokeTauri<string[]>('get_arg_dump').catch(() => []);
        launchUrl = urlFromDeepLink ?? null;
        if (launchUrl) {
          await spawnSessionWindow(launchUrl);
        }

        await onOpenUrl((urls) => {
          for (const url of urls) {
            void spawnSessionWindow(url);
          }
        });
      }
      } else {
        remoteDesktopStatus = 'Tauri runtime not detected. Run `cargo tauri dev` for remote sessions.';
      }

      if (disposed) return;
      viewportObserver = new ResizeObserver(() => {
        updateViewportRect();
      });
    };

    void initialize();
    window.addEventListener('beforeunload', handleBeforeUnload);

    return () => {
      disposed = true;
      if (viewerHeartbeatTimer) {
        clearInterval(viewerHeartbeatTimer);
        viewerHeartbeatTimer = null;
      }
      clearNavClipReadyFallback();
      clearSystemInfoPolling();
      window.removeEventListener('beforeunload', handleBeforeUnload);
      stopViewportObserver();
      remoteHasFocus = false;
      void endSession();
    };
  });

  $: if (
    activeTab === 'Remote Desktop' &&
    remoteDesktopFrame &&
    remoteDesktopConnected &&
    activeTransport !== null
  ) {
    viewportSuppressed = false;
    if (viewportObserver && !viewportObserving) {
      viewportObserver.observe(remoteDesktopFrame);
      viewportObserving = true;
    }
    void scheduleViewportRectRefresh();
  } else {
    // Always hide native viewport and disable native input forwarding when
    // not on the Remote Desktop tab (including initial shell-mode launch).
    stopViewportObserver();
    if (!viewportSuppressed) {
      remoteHasFocus = false;
      viewportSuppressed = true;
    }
    if (activeTab === 'System Shell' && shellTerminal) {
      shellTerminal.focus();
    }
  }

  $: syncStartMenuBlocked();

  $: if (activeTab !== 'System Info') {
    clearSystemInfoPolling();
  }

  $: if (activeTab !== 'Remote Desktop' && monitorPickerOpen) {
    monitorPickerOpen = false;
    void scheduleViewportRectRefresh();
  }

  $: if (activeTab !== 'Remote Desktop' && aiAssistPanelOpen) {
    closeAiAssistPanel();
  }

  $: if (activeTab !== 'System Shell' && shellAssistPanelOpen) {
    closeShellAssistPanel();
  }

  $: if (activeTab !== 'System Shell' && shellCredentialPanelOpen) {
    closeShellCredentialPanel();
  }

  $: if (activeTab !== 'System Shell' && shellContextMenu.open) {
    closeShellContextMenu();
  }

  $: if (activeAgentPlatform !== 'linux' && shellCredentialPanelOpen) {
    closeShellCredentialPanel();
  }
</script>

<svelte:window on:click={handleClickOutside} on:keydown={handleWindowKeydown} />

<div class="app-container">
  <nav class="top-bar">
    <div class="nav-left">
      {#each visibleTabs as tab}
        {#if tab === 'Remote Desktop'}
          <div class="nav-item-with-dropdown">
            <button
              class="nav-item nav-item--remote"
              class:active={activeTab === tab}
              on:click={handleRemoteDesktopNavClick}
              aria-expanded={remoteDesktopDropdownOpen}
              aria-haspopup="menu"
            >
              <span>Remote Desktop</span>
              <svg
                class="nav-dropdown-icon"
                class:open={remoteDesktopDropdownOpen}
                xmlns="http://www.w3.org/2000/svg"
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <polyline points="4 8 12 16 20 8"></polyline>
              </svg>
            </button>
            {#if remoteDesktopDropdownOpen}
              <div
                class="nav-dropdown-menu"
                class:nav-dropdown-menu--quality-open={videoQualityOpen}
                role="menu"
                aria-label="Remote desktop contexts"
                on:animationend={handleOverlayAnimationEnd}
              >
                <div class="nav-dropdown-section-title">
                  {isMacAgentPlatform() ? 'Active Session' : 'Session Switch'}
                </div>
                {#if isMacAgentPlatform()}
                  <div class="nav-dropdown-item nav-dropdown-item--muted" role="presentation">
                    {formatConsoleContextLabel()}
                  </div>
                {:else}
                  <button
                    class="nav-dropdown-item"
                    class:selected={remoteDesktopContext === 'console'}
                    type="button"
                    role="menuitem"
                    disabled={sessionSwitchInFlight}
                    on:click={() => handleRemoteDesktopContextSelect('console')}
                  >
                    {formatConsoleContextLabel()}
                  </button>
                  {#if rdpSessions === null}
                    <div class="nav-dropdown-item nav-dropdown-item--muted" role="presentation">
                      RDP
                    </div>
                  {:else if visibleRdpSessions.length === 0}
                    <div class="nav-dropdown-item nav-dropdown-item--muted" role="presentation">
                      No active RDP sessions
                    </div>
                  {:else}
                    {#each visibleRdpSessions as session}
                      <button
                        class="nav-dropdown-item"
                        class:selected={remoteDesktopContext === session.nativeSessionId}
                        type="button"
                        role="menuitem"
                        disabled={sessionSwitchInFlight}
                        on:click={() => handleRemoteDesktopContextSelect(session.nativeSessionId)}
                      >
                        {formatRdpContextLabel(session)}
                      </button>
                    {/each}
                  {/if}
                {/if}
                {#if sessionSwitchError}
                  <div class="nav-dropdown-item nav-dropdown-item--muted" role="presentation">
                    {sessionSwitchError}
                  </div>
                {/if}
                <div class="nav-dropdown-section-title">Stream quality</div>
                <div class="custom-dropdown quality-dropdown-container quality-dropdown-container--in-remote-menu">
                  <button
                    class="quality-button quality-button--in-remote-menu"
                    class:active={videoQualityOpen}
                    type="button"
                    title="Stream Quality"
                    aria-expanded={videoQualityOpen}
                    aria-haspopup="menu"
                    on:click={toggleVideoQualityDropdown}
                  >
                    <span class="quality-button-label">Quality</span>
                    <span class="quality-button-value">{selectedVideoQualityOption().label}</span>
                  </button>
                  {#if videoQualityOpen}
                    <div
                      class="quality-dropdown-panel quality-dropdown-panel--in-remote-menu"
                      role="menu"
                      aria-label="Stream quality"
                      on:animationend={handleOverlayAnimationEnd}
                    >
                      {#each videoQualityOptions as option}
                        <button
                          class="quality-option"
                          class:selected={videoQuality === option.id}
                          type="button"
                          role="menuitemradio"
                          aria-checked={videoQuality === option.id}
                          on:click={() => selectVideoQuality(option.id)}
                        >
                          <span class="quality-option-main">
                            <strong>{option.label}</strong>
                            <span>{option.hint}</span>
                          </span>
                          {#if videoQuality === option.id}
                            <span class="quality-option-check">Active</span>
                          {/if}
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              </div>
            {/if}
          </div>
        {:else if tab === 'System Shell'}
          <div class="nav-item-with-dropdown">
            <button
              class="nav-item nav-item--remote"
              class:active={activeTab === tab}
              on:click={handleSystemShellNavClick}
              aria-expanded={isWindowsShellPlatform() ? shellRunAsDropdownOpen : undefined}
              aria-haspopup={isWindowsShellPlatform() ? 'menu' : undefined}
            >
              <span>System Shell</span>
              {#if isWindowsShellPlatform()}
                <svg
                  class="nav-dropdown-icon"
                  class:open={shellRunAsDropdownOpen}
                  xmlns="http://www.w3.org/2000/svg"
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <polyline points="4 8 12 16 20 8"></polyline>
                </svg>
              {/if}
            </button>
            {#if shellRunAsDropdownOpen && isWindowsShellPlatform()}
              <div class="nav-dropdown-menu" role="menu" aria-label="Shell run-as options">
                <div class="nav-dropdown-section-title">Run As</div>
                <button
                  class="nav-dropdown-item"
                  class:selected={shellRunAs === 'system' && shellTargetSessionId === null}
                  type="button"
                  role="menuitem"
                  disabled={shellConnectInFlight}
                  on:click={() => handleShellRunAsSelect('system', null)}
                >
                  SYSTEM
                </button>
                {#if shellUserContexts.length === 0}
                  <div class="nav-dropdown-item nav-dropdown-item--muted" role="presentation">
                    No logged-in sessions detected
                  </div>
                {:else}
                  {#each shellUserContexts as session}
                    <button
                      class="nav-dropdown-item"
                      class:selected={
                        shellRunAs === 'user' && shellTargetSessionId === session.nativeSessionId
                      }
                      type="button"
                      role="menuitem"
                      disabled={shellConnectInFlight}
                      on:click={() => handleShellRunAsSelect('user', session.nativeSessionId)}
                    >
                      {formatShellUserContextLabel(session)}
                    </button>
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
        {:else}
          <button
            class="nav-item"
            class:active={activeTab === tab}
            on:click={() => void selectTab(tab)}
          >
            {tab}
          </button>
        {/if}
      {/each}
    </div>
    <div class="nav-right">
      {#if visibleTabs.length > 0}
      {#if activeTab === 'Remote Desktop'}
        <button
          type="button"
          class="nav-aux-icon-button"
          class:active={aiAssistPanelOpen}
          title="AI Assist"
          aria-label="AI Assist"
          on:click={() => void toggleAiAssistPanel()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="m12 3-1.9 5.1L5 10l5.1 1.9L12 17l1.9-5.1L19 10l-5.1-1.9L12 3Z"></path>
            <path d="M5 3v4"></path>
            <path d="M3 5h4"></path>
            <path d="M19 17v4"></path>
            <path d="M17 19h4"></path>
          </svg>
        </button>
        <div class="custom-dropdown monitor-picker-container">
          <button
            class="monitor-picker-button"
            class:active={monitorPickerOpen}
            class:pending={pendingCaptureOutputIndex !== null}
            type="button"
            disabled={!monitorPickerInteractive}
            title={monitorPickerTitle()}
            aria-expanded={monitorPickerOpen}
            aria-haspopup={monitorPickerInteractive ? 'true' : undefined}
            aria-busy={pendingCaptureOutputIndex !== null}
            aria-label="Switch capture display"
            on:click={toggleMonitorPicker}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
              <line x1="8" y1="21" x2="16" y2="21"></line>
              <line x1="12" y1="17" x2="12" y2="21"></line>
            </svg>
          </button>
          {#if monitorPickerPanelVisible}
            <div
              class="monitor-picker-panel"
              role="menu"
              aria-label="Capture display"
              on:animationend={handleOverlayAnimationEnd}
            >
              {#each captureOutputs ?? [] as out}
                <button
                  class="monitor-picker-option"
                  class:selected={activeCaptureOutputIndex === out.index}
                  class:pending={pendingCaptureOutputIndex === out.index}
                  type="button"
                  role="menuitemradio"
                  aria-checked={activeCaptureOutputIndex === out.index}
                  disabled={pendingCaptureOutputIndex !== null ||
                    activeCaptureOutputIndex === out.index}
                  on:click={(e) => {
                    e.stopPropagation();
                    void handleCaptureOutputSelect(out.index);
                  }}
                >
                  <span class="monitor-picker-option-copy">
                    <span class="monitor-picker-option-label">{captureOutputName(out)}</span>
                    {#if captureOutputDetails(out)}
                      <span class="monitor-picker-option-details">{captureOutputDetails(out)}</span>
                    {/if}
                  </span>
                  {#if pendingCaptureOutputIndex === out.index}
                    <span class="monitor-picker-option-check">Switching</span>
                  {:else if activeCaptureOutputIndex === out.index}
                    <span class="monitor-picker-option-check">Active</span>
                  {/if}
                </button>
              {/each}
              {#if captureOutputSwitchError}
                <div class="monitor-picker-option monitor-picker-option--error" role="presentation">
                  {captureOutputSwitchError}
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if activeTab === 'System Shell'}
        {#if activeAgentPlatform === 'linux' && shellSessionInfo}
          <button
            type="button"
            class="nav-aux-icon-button"
            class:active={shellCredentialPanelOpen}
            title="Talos credential"
            aria-label="Talos credential"
            on:click={toggleShellCredentialPanel}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <circle cx="7.5" cy="15.5" r="5.5"></circle>
              <path d="m21 2-9.6 9.6"></path>
              <path d="m15.5 7.5 3 3"></path>
              <path d="m18 5 3 3"></path>
            </svg>
          </button>
        {/if}
        <button
          type="button"
          class="nav-aux-icon-button"
          class:active={shellAssistPanelOpen}
          title="Shell AI Assist"
          aria-label="Shell AI Assist"
          on:click={() => void toggleShellAssistPanel()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="m12 3-1.9 5.1L5 10l5.1 1.9L12 17l1.9-5.1L19 10l-5.1-1.9L12 3Z"></path>
            <path d="M5 3v4"></path>
            <path d="M3 5h4"></path>
            <path d="M19 17v4"></path>
            <path d="M17 19h4"></path>
          </svg>
        </button>
      {/if}
      <div class="connection-info-container">
        <button
          class="connection-info-button"
          class:active={connectionInfoOpen}
          type="button"
          title="Connection Info"
          aria-expanded={connectionInfoOpen}
          aria-haspopup="dialog"
          on:click={toggleConnectionInfo}
        >
          <span
            class="connection-info-indicator"
            class:healthy={getConnectionStatusForKind(activeConnectionKind)}
            class:idle={!getConnectionStatusForKind(activeConnectionKind)}
          ></span>
          <svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M5 12.5a7 7 0 0 1 14 0"></path>
            <path d="M8.5 12.5a3.5 3.5 0 0 1 7 0"></path>
            <circle cx="12" cy="16" r="1.5"></circle>
          </svg>
        </button>
        {#if connectionInfoOpen}
          <div
            class="connection-info-panel"
            role="dialog"
            tabindex="-1"
            aria-label="Connection info"
            on:animationend={handleOverlayAnimationEnd}
          >
            <div class="connection-info-header">
              <div>
                <h3>Connection Info</h3>
                <p>{formatConnectionType(connectionSummary)}</p>
              </div>
              <span class="connection-health-pill">
                {getConnectionHealthLabel(
                  getConnectionStatusForKind(activeConnectionKind),
                  connectionStats?.rttMs,
                  shouldShowConnectionLatencyChart(activeConnectionKind, connectionSummary)
                )}
              </span>
            </div>

            {#if shouldShowConnectionLatencyChart(activeConnectionKind, connectionSummary)}
              <div class="connection-info-chart">
                <div class="connection-info-chart-header">
                  <span>Latency</span>
                  <strong>{formatLatencyMs(connectionStats?.rttMs)}</strong>
                </div>
                <svg
                  viewBox={`0 0 ${CONNECTION_SPARKLINE_WIDTH} ${CONNECTION_SPARKLINE_HEIGHT}`}
                  class="connection-sparkline"
                  aria-hidden="true"
                >
                  <path
                    d={buildLatencySparklinePath(
                      connectionLatencyHistory,
                      CONNECTION_SPARKLINE_WIDTH,
                      CONNECTION_SPARKLINE_HEIGHT
                    )}
                  ></path>
                </svg>
                <div class="connection-chart-summary">
                  <span>Avg {formatLatencyMs(connectionStats?.avgRttMs)}</span>
                  <span>Min {formatLatencyMs(connectionStats?.minRttMs)}</span>
                  <span>Max {formatLatencyMs(connectionStats?.maxRttMs)}</span>
                </div>
              </div>
            {/if}

            <div class="connection-info-grid">
              <div class="connection-info-item">
                <span>Transport</span>
                <strong>{formatTransportLabel(connectionSummary)}</strong>
              </div>
              {#if activeConnectionKind === 'remote_desktop'}
                <div class="connection-info-item">
                  <span>Capture Type</span>
                  <strong>{formatCaptureType(connectionSummary?.captureType)}</strong>
                </div>
              {/if}
              <div class="connection-info-item">
                <span>Encryption</span>
                <strong>{connectionSummary?.encryptionLabel ?? 'Unavailable'}</strong>
              </div>
              <div class="connection-info-item">
                <span>Connect Time</span>
                <strong>{formatDurationMs(connectionSummary?.connectMs)}</strong>
              </div>
              <div class="connection-info-item">
                <span>Remote Address</span>
                <strong class="connection-info-value">{connectionSummary?.remoteAddr ?? 'Unavailable'}</strong>
              </div>
              {#if connectionSummary?.relayTcpMs != null}
                <div class="connection-info-item">
                  <span>Relay TCP</span>
                  <strong>{formatDurationMs(connectionSummary.relayTcpMs)}</strong>
                </div>
              {/if}
              {#if connectionSummary?.relayTlsMs != null}
                <div class="connection-info-item">
                  <span>Relay TLS</span>
                  <strong>{formatDurationMs(connectionSummary.relayTlsMs)}</strong>
                </div>
              {/if}
            </div>

            <div class="connection-info-section">
              <div class="connection-info-section-title">Route</div>
              <div class="connection-route-row">
                <span>Viewer Reflex</span>
                <strong>{formatConnectionEndpoint(connectionSummary?.viewerReflex)}</strong>
              </div>
              <div class="connection-route-row">
                <span>Agent Reflex</span>
                <strong>{formatConnectionEndpoint(connectionSummary?.agentReflex)}</strong>
              </div>
              {#if (connectionSummary?.agentLocalAddrs ?? [])?.length}
                <div class="connection-route-row connection-route-row--stacked">
                  <span>Agent Local</span>
                  <strong>
                    {(connectionSummary?.agentLocalAddrs ?? []).map((addr) => formatLocalAddr(addr)).join(', ')}
                  </strong>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>
      {#if isChatSupported()}
        <button
          class="settings-button"
          type="button"
          aria-label={chatPanelOpen ? 'Hide chat panel' : 'Show chat panel'}
          on:click={toggleChatPanel}
          title={chatPanelOpen ? 'Hide chat panel' : 'Show chat panel'}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M21 15a4 4 0 0 1-4 4H7l-4 4V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"></path>
          </svg>
        </button>
      {/if}
      <button class="settings-button" on:click={toggleSettings} title="Settings">
        <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"></path>
          <circle cx="12" cy="12" r="3"></circle>
        </svg>
      </button>
      {/if}
    </div>
  </nav>

  <main
    class="container"
    class:container--remote={activeTab === 'Remote Desktop'}
    class:container--shell={activeTab === 'System Shell'}
    class:container--file-transfer={activeTab === 'File Transfer'}
    class:container--registry={activeTab === 'Remote Registry'}
    class:container--system-info={activeTab === 'System Info'}
  >
    <!-- svelte-ignore a11y_no_noninteractive_tabindex a11y_no_noninteractive_element_interactions -->
    <div
      class="remote-desktop-frame"
      class:tab-panel-hidden={activeTab !== 'Remote Desktop'}
      bind:this={remoteDesktopFrame}
      tabindex="0"
      role="application"
      on:focus={() => {
        remoteHasFocus = true;
      }}
      on:blur={() => {
        remoteHasFocus = false;
      }}
      on:mousemove={handleMouseMove}
      on:mousedown|preventDefault={(event) => handleMouseButton(event, true)}
      on:mouseup|preventDefault={(event) => handleMouseButton(event, false)}
      on:wheel|preventDefault={handleWheel}
      on:keydown|preventDefault={handleKeyDown}
      on:keyup|preventDefault={handleKeyUp}
      on:paste={handlePaste}
      on:contextmenu|preventDefault={() => {}}
    >
      {#if remoteDesktopFrameImage}
        <img class="remote-desktop-frame-image" src={remoteDesktopFrameImage} alt="" />
      {:else}
        <div class="remote-desktop-placeholder">
          {#if remoteDesktopError}
            <strong>{remoteDesktopError}</strong>
          {:else if remoteDesktopOutput}
            <strong>{remoteDesktopOutput}</strong>
          {:else}
            <strong>{remoteDesktopStatus}</strong>
          {/if}
        </div>
      {/if}
    </div>

    <div
      class="shell-container"
      class:shell-container--assist-open={shellAssistPanelOpen}
      class:tab-panel-hidden={activeTab !== 'System Shell'}
    >
      {#if shellError}
        <div class="shell-status shell-status--error shell-status--centered">{shellError}</div>
      {:else if shellStatus && !shellConnected}
        <div class="shell-status shell-status--centered">{shellStatus}</div>
      {/if}
      <div class="shell-surface">
        <div
          class="shell-terminal"
          bind:this={shellTerminalEl}
          role="application"
          aria-label="System shell terminal"
          on:contextmenu={openShellContextMenu}
        ></div>
      </div>
      {#if shellContextMenu.open}
        <div
          class="shell-context-menu"
          style={`top: ${shellContextMenu.y}px; left: ${shellContextMenu.x}px;`}
          role="menu"
          tabindex="-1"
          aria-label="System shell actions"
          on:click|stopPropagation={() => {}}
          on:keydown|stopPropagation={() => {}}
        >
          <button
            type="button"
            class="shell-context-menu-item"
            role="menuitem"
            disabled={!shellContextMenu.hasSelection}
            on:click={() => void copyShellSelection()}
          >
            <span>Copy</span>
          </button>
          <button
            type="button"
            class="shell-context-menu-item"
            role="menuitem"
            disabled={!shellConnected}
            on:click={() => void pasteShellClipboard()}
          >
            <span>Paste</span>
          </button>
          <div class="shell-context-menu-separator" role="separator"></div>
          <button
            type="button"
            class="shell-context-menu-item"
            role="menuitem"
            disabled={!shellTerminal}
            on:click={selectAllShellText}
          >
            <span>Select all</span>
          </button>
          <button
            type="button"
            class="shell-context-menu-item"
            role="menuitem"
            disabled={!shellConnected}
            on:click={() => void clearShellScreen()}
          >
            <span>Clear screen</span>
          </button>
        </div>
      {/if}
      {#if shellCredentialPanelOpen}
        <aside
          class="ai-assist-sidebar shell-credential-sidebar shell-credential-sidebar--inline"
          aria-label="Talos credential"
          on:animationend={handleOverlayAnimationEnd}
        >
          <div class="settings-header">
            <div>
              <h2 class="settings-title">Talos Credential</h2>
              <p class="ai-assist-subtitle">System shell</p>
            </div>
            <button
              class="settings-close"
              type="button"
              on:click={closeShellCredentialPanel}
              title="Close Talos credential"
              aria-label="Close Talos credential"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>
          </div>

          <div class="shell-credential-content custom-scrollbar">
            {#if linuxShellCredential}
              <div class="shell-credential-card">
                <div class="shell-credential-row">
                  <span class="shell-credential-label">Account</span>
                  <button
                    type="button"
                    class="shell-credential-copy"
                    on:click={() => copyLinuxShellCredential(linuxShellCredential!.username, 'Username')}
                  >
                    {linuxShellCredential.username}
                  </button>
                </div>
                <div class="shell-credential-row shell-credential-row--stacked">
                  <span class="shell-credential-label">Password</span>
                  <code class="shell-credential-password">{linuxShellCredential.password}</code>
                </div>
              </div>
              <div class="shell-credential-actions">
                <button
                  type="button"
                  class="shell-credential-action shell-credential-action--primary"
                  on:click={() => copyLinuxShellCredential(linuxShellCredential!.password, 'Password')}
                >
                  Copy Password
                </button>
                <button
                  type="button"
                  class="shell-credential-action"
                  disabled={!shellConnected}
                  on:click={() => void insertLinuxShellCredentialPassword()}
                >
                  Insert Into Terminal
                </button>
              </div>
            {:else}
              <button
                class="shell-credential-reveal"
                type="button"
                on:click={revealLinuxShellCredential}
                disabled={linuxShellCredentialLoading}
              >
                {linuxShellCredentialLoading ? 'Revealing Password...' : 'Reveal Password for Talos'}
              </button>
            {/if}
            {#if linuxShellCredentialError}
              <div class="shell-credential-status">
                {linuxShellCredentialError}
              </div>
            {/if}
          </div>
        </aside>
      {/if}
      {#if shellAssistPanelOpen}
        <aside
          class="ai-assist-sidebar shell-assist-sidebar shell-assist-sidebar--inline"
          aria-label="Shell AI Assist"
          on:animationend={handleOverlayAnimationEnd}
        >
          <div class="settings-header">
            <div>
              <h2 class="settings-title">Shell AI Assist</h2>
              <p class="ai-assist-subtitle">{activeAgentPlatform}</p>
            </div>
            <button
              class="settings-close"
              type="button"
              on:click={closeShellAssistPanel}
              title="Close Shell AI Assist"
              aria-label="Close Shell AI Assist"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>
          </div>

          <div class="ai-assist-content custom-scrollbar">
            <div
              class="ai-assist-status"
              class:ai-assist-status--ready={shellAssistReady() && !shellAssistInFlight}
              class:ai-assist-status--busy={shellAssistInFlight}
            >
              <span class="ai-assist-status-dot"></span>
              <span>{shellAssistStatus || 'Open a live system shell session first.'}</span>
            </div>

            {#if shellAssistError}
              <div class="ai-assist-error" role="alert">{shellAssistError}</div>
            {/if}

            {#if shellAssistGoal}
              <div class="shell-assist-goal">
                <h3 class="settings-section-title">Goal</h3>
                <p>{shellAssistGoal}</p>
              </div>
            {/if}

            {#if shellAssistTurns.length > 0}
              <div class="shell-assist-turns">
                <h3 class="settings-section-title">Turns</h3>
                {#each shellAssistTurns as turn, index (turn.id)}
                  <div class="shell-assist-turn" class:shell-assist-turn--rejected={!turn.approved}>
                    <div class="shell-assist-turn-header">
                      <span>Turn {index + 1}</span>
                      <strong>{turn.approved ? 'Approved' : 'Rejected'}</strong>
                    </div>
                    <pre class="shell-assist-command shell-assist-command--compact">{turn.command}</pre>
                    {#if turn.output}
                      <p>{turn.output}</p>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}

            {#if shellAssistProposal}
              <div class="shell-assist-proposal">
                <div class="shell-assist-proposal-header">
                  <h3 class="settings-section-title">{shellAssistActionLabel(shellAssistProposal.action)}</h3>
                  {#if shellAssistProposal.action === 'command'}
                    <span>Turn {shellAssistTurns.length + 1}</span>
                  {/if}
                </div>
                {#if shellAssistProposal.action === 'command'}
                  <pre class="shell-assist-command">{shellAssistProposal.command}</pre>
                {/if}
                <h3 class="settings-section-title">Explanation</h3>
                <p>{shellAssistProposal.explanation}</p>
                <h3 class="settings-section-title">Risk</h3>
                <p>{shellAssistProposal.risk}</p>
                {#if shellAssistProposal.message}
                  <h3 class="settings-section-title">Status</h3>
                  <p>{shellAssistProposal.message}</p>
                {/if}
                {#if shellAssistProposal.notes.length > 0}
                  <h3 class="settings-section-title">Notes</h3>
                  <ul class="shell-assist-notes">
                    {#each shellAssistProposal.notes as note, index (`${note}-${index}`)}
                      <li>{note}</li>
                    {/each}
                  </ul>
                {/if}
              </div>
            {:else}
              <div class="ai-assist-empty">
                {shellAssistGoal ? 'No pending turn.' : 'No shell goal is running.'}
              </div>
            {/if}
          </div>

          <form
            class="ai-assist-composer"
            on:submit|preventDefault={() => void sendShellAssistPrompt()}
          >
            <label class="ai-assist-prompt-label" for="shell-assist-prompt">
              {shellAssistProposal?.action === 'needs_input' ? 'Clarification' : 'Goal'}
            </label>
            <textarea
              id="shell-assist-prompt"
              class="ai-assist-prompt"
              bind:value={shellAssistPrompt}
              rows="4"
              placeholder={shellAssistReady()
                ? shellAssistProposal?.action === 'needs_input'
                  ? 'Answer the clarification request...'
                  : shellAssistGoal
                    ? 'Goal is running. Stop it to enter a new goal.'
                    : 'Describe the goal to achieve in the shell...'
                : 'Connect to System Shell to enable AI Assist.'}
              disabled={shellAssistInFlight || (!!shellAssistGoal && shellAssistProposal?.action !== 'needs_input')}
              on:keydown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  void sendShellAssistPrompt();
                }
              }}
            ></textarea>
            <div class="ai-assist-composer-footer">
              <span class="ai-assist-enter-hint">Enter to send, Shift+Enter for newline</span>
              <div class="ai-assist-composer-actions">
                {#if shellAssistProposal}
                  {#if shellAssistProposal.action === 'command'}
                    <button
                      type="button"
                      class="ai-assist-stop"
                      disabled={shellAssistInFlight}
                      on:click={rejectShellAssistCommand}
                    >
                      Reject
                    </button>
                    <button
                      type="button"
                      class="ai-assist-send"
                      disabled={shellAssistInFlight || !shellConnected}
                      on:click={() => void approveShellAssistCommand()}
                    >
                      Approve
                    </button>
                  {:else if shellAssistProposal.action === 'needs_input'}
                    <button
                      type="submit"
                      class="ai-assist-send"
                      disabled={shellAssistInFlight || !shellAssistPrompt.trim() || !shellAssistReady()}
                    >
                      Reply
                    </button>
                  {:else}
                    <button
                      type="button"
                      class="ai-assist-send"
                      disabled={shellAssistInFlight}
                      on:click={stopShellAssistGoal}
                    >
                      Done
                    </button>
                  {/if}
                {:else if shellAssistGoal}
                  <button
                    type="button"
                    class="ai-assist-stop"
                    disabled={shellAssistInFlight}
                    on:click={stopShellAssistGoal}
                  >
                    Stop
                  </button>
                  <button
                    type="button"
                    class="ai-assist-send"
                    disabled={shellAssistInFlight || !shellAssistReady()}
                    on:click={() => void continueShellAssistGoal()}
                  >
                    {shellAssistInFlight ? 'Working...' : 'Continue'}
                  </button>
                {:else}
                  <button
                    type="submit"
                    class="ai-assist-send"
                    disabled={shellAssistInFlight || !shellAssistPrompt.trim() || !shellAssistReady()}
                  >
                    {shellAssistInFlight ? 'Working...' : 'Start'}
                  </button>
                {/if}
              </div>
            </div>
          </form>
        </aside>
      {/if}
    </div>

    <div class="system-info-container custom-scrollbar" class:tab-panel-hidden={activeTab !== 'System Info'}>
      <div class="system-info-header">
        <div>
          <h1 class="system-info-title">System Details</h1>
          <p class="system-info-subtitle">Live host inventory and process telemetry.</p>
        </div>
        <button
          type="button"
          class="system-info-refresh"
          disabled={systemInfoLoading || systemInfoRefreshing || !getSystemInfoAgentContext()}
          on:click={() => void fetchSystemInfo({ refresh: true, background: true })}
        >
          {systemInfoRefreshing ? 'Refreshing...' : 'Refresh'}
        </button>
      </div>

      <div class="system-info-meta-row">
        <span>Last updated: {formatTimestamp(systemInfoLastUpdated)}</span>
        {#if systemInfoData?.refreshed === false}
          <span class="system-info-chip">Showing cached snapshot</span>
        {/if}
      </div>

      {#if systemInfoRefreshError && systemInfoData?.device}
        <div class="system-info-alert system-info-alert--warning">{systemInfoRefreshError}</div>
      {/if}

      {#if systemInfoError && !systemInfoData?.device}
        <div class="system-info-alert system-info-alert--error">{systemInfoError}</div>
      {:else if systemInfoLoading && !systemInfoData?.device}
        <div class="system-info-empty">Loading system details...</div>
      {:else if systemInfoData?.device}
        {@const canRefreshSystemInfo = !!getSystemInfoAgentContext()}
        {#if !canRefreshSystemInfo}
          <div class="system-info-alert system-info-alert--warning">
            Live refresh unavailable. Open a dashboard viewer link once.
          </div>
        {/if}
        {@const device = systemInfoData.device}
        {@const details = getSystemDetailsSource(device)}
        {@const inventory = getSystemInventorySource(device)}
        {@const summarySource = details ?? inventory}
        {@const systemSummary = getSystemSummary(details) ?? getSystemSummary(summarySource)}
        {@const cpuSummary = getCpuSummary(summarySource)}
        {@const memorySummary = getMemorySummary(summarySource)}
        {@const disks = getDisks(summarySource)}
        {@const networks = getNetworks(summarySource)}
        {@const processes = getProcesses(details)}

        <div class="system-info-grid system-info-grid--top">
          <section class="system-info-card">
            <h2>System</h2>
            <div class="system-info-kv">
              <span>Hostname</span>
              <strong>{device.hostname || capabilities?.agentHostname || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Operating System</span>
              <strong>{device.os || capabilities?.agentOs || systemSummary?.osVersion || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Version</span>
              <strong>{device.version || capabilities?.agentVersion || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Primary IP</span>
              <strong>{device.ip || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Platform Name</span>
              <strong>{systemSummary?.name || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Kernel</span>
              <strong>{systemSummary?.kernelVersion || 'Unknown'}</strong>
            </div>
            <div class="system-info-kv">
              <span>Uptime</span>
              <strong>{formatUptime(systemSummary?.uptimeSeconds)}</strong>
            </div>
            <div class="system-info-kv">
              <span>Boot Time</span>
              <strong>{formatBootTime(systemSummary?.bootTime)}</strong>
            </div>
          </section>

          <section class="system-info-card">
            <h2>CPU</h2>
            {#if cpuSummary}
              <p class="system-info-highlight">{cpuSummary.brand}</p>
              <div class="system-info-kv">
                <span>Cores</span>
                <strong>{cpuSummary.cores ?? '—'}</strong>
              </div>
              <div class="system-info-kv">
                <span>Frequency</span>
                <strong>{cpuSummary.frequencyMHz ? `${cpuSummary.frequencyMHz} MHz` : 'Unknown'}</strong>
              </div>
            {:else}
              <p class="system-info-empty-text">CPU details unavailable.</p>
            {/if}
          </section>

          <section class="system-info-card">
            <h2>Memory</h2>
            {#if memorySummary}
              {@const usedBytes = (memorySummary.total ?? 0) - (memorySummary.available ?? 0)}
              <p class="system-info-highlight">{formatBytes(memorySummary.available)} free</p>
              <div class="system-info-kv">
                <span>Total</span>
                <strong>{formatBytes(memorySummary.total)}</strong>
              </div>
              <div class="system-info-kv">
                <span>Used</span>
                <strong>{formatBytes(usedBytes > 0 ? usedBytes : 0)}</strong>
              </div>
              <div class="system-info-kv">
                <span>Usage</span>
                <strong>{formatPercent(memorySummary.usedPercent)}</strong>
              </div>
              <div class="system-info-progress">
                <div
                  class="system-info-progress-fill"
                  style={`width: ${Math.max(0, Math.min(100, memorySummary.usedPercent ?? 0))}%`}
                ></div>
              </div>
            {:else}
              <p class="system-info-empty-text">Memory details unavailable.</p>
            {/if}
          </section>
        </div>

        <div class="system-info-grid system-info-grid--middle">
          <section class="system-info-card">
            <h2>Disks</h2>
            {#if disks.length > 0}
              <div class="system-info-list">
                {#each disks as disk}
                  <div class="system-info-list-item">
                    <div class="system-info-list-header">
                      <strong>{disk.name}</strong>
                      <span>{formatPercent(disk.usedPercent)}</span>
                    </div>
                    <div class="system-info-list-sub">
                      {disk.mount} | {disk.fs}
                    </div>
                    <div class="system-info-list-sub">
                      {formatBytes(disk.available)} free of {formatBytes(disk.total)}
                    </div>
                    <div class="system-info-progress">
                      <div
                        class="system-info-progress-fill"
                        style={`width: ${Math.max(0, Math.min(100, disk.usedPercent ?? 0))}%`}
                      ></div>
                    </div>
                  </div>
                {/each}
              </div>
            {:else}
              <p class="system-info-empty-text">No disk data available.</p>
            {/if}
          </section>

          <section class="system-info-card">
            <h2>Network Interfaces</h2>
            {#if networks.length > 0}
              <div class="system-info-list">
                {#each networks as network}
                  <div class="system-info-list-item">
                    <div class="system-info-list-header">
                      <strong>{network.name}</strong>
                    </div>
                    <div class="system-info-list-sub">
                      IP {network.ips.length > 0 ? network.ips.map((ip) => ip.label).join(', ') : 'Unavailable'}
                    </div>
                    <div class="system-info-list-sub">
                      Gateway {network.gateways.length > 0 ? network.gateways.join(', ') : 'Unavailable'}
                    </div>
                    <div class="system-info-list-sub">
                      DNS {network.dnsServers.length > 0 ? network.dnsServers.join(', ') : 'Unavailable'}
                    </div>
                  </div>
                {/each}
              </div>
            {:else}
              <p class="system-info-empty-text">No network data available.</p>
            {/if}
          </section>
        </div>

        <section class="system-info-card system-info-processes">
          <h2>Top Processes</h2>
          {#if processes.length > 0}
            <div class="system-info-process-list">
              {#each processes.slice(0, 10) as process}
                <div class="system-info-process-row">
                  <div class="system-info-process-name">{getProcessName(process)}</div>
                  <div class="system-info-process-meta">
                    PID {pickNumber(process.pid) ?? '—'} | CPU {formatPercent(pickNumber(process.cpu, process.cpuUsage))} | MEM {formatBytes(pickNumber(process.memory, process.memoryBytes))}
                  </div>
                </div>
              {/each}
            </div>
          {:else}
            <p class="system-info-empty-text">No process data available.</p>
          {/if}
        </section>
      {:else if !getSystemInfoAgentContext()}
        <div class="system-info-empty">
          System info context unavailable. Open a dashboard viewer link once.
        </div>
      {:else}
        <div class="system-info-empty">No system details available yet.</div>
      {/if}
    </div>

    <div class="file-transfer-container file-transfer-scrollable" class:tab-panel-hidden={activeTab !== 'File Transfer'}>
      {#if fileTransferError}
        <div class="file-transfer-alert file-transfer-alert--error">{fileTransferError}</div>
      {/if}

      {#if fileTransferPendingConflict}
        <div class="file-transfer-alert file-transfer-alert--warning">
          <div class="file-transfer-alert-title">Name conflict detected</div>
          <div class="file-transfer-alert-text">
            {fileTransferPendingConflict.conflictPath}: {fileTransferPendingConflict.conflictMessage}
          </div>
          <div class="file-transfer-conflict-actions">
            <button
              type="button"
              class="file-transfer-button file-transfer-button--ghost"
              on:click={() => void resolveFileTransferConflict('skip')}
            >
              Skip
            </button>
            <button
              type="button"
              class="file-transfer-button file-transfer-button--ghost"
              on:click={() => void resolveFileTransferConflict('rename')}
            >
              Rename
            </button>
            <button
              type="button"
              class="file-transfer-button file-transfer-button--secondary"
              on:click={() => void resolveFileTransferConflict('overwrite')}
            >
              Overwrite
            </button>
          </div>
        </div>
      {/if}

      <div class="file-transfer-grid">
        <section class="file-transfer-panel">
          <div class="file-transfer-panel-header">
            <div>
              <h2>Local</h2>
              <p class="file-transfer-panel-subtitle">
                {(fileTransferLocalDirCache[fileTransferLocalPath] ?? []).length} item(s) | {fileTransferLocalSelectedDirs.size + fileTransferLocalSelectedFiles.size} selected
              </p>
              <p class="file-transfer-path">{fileTransferLocalPath}</p>
            </div>
            <div class="file-transfer-panel-actions">
              <button
                type="button"
                class="file-transfer-mini-btn"
                disabled={!fileTransferConnected || !canNavigateUp(fileTransferLocalPath)}
                on:click={() => void selectLocalDir(parentPath(fileTransferLocalPath))}
              >
                Up
              </button>
              <button
                type="button"
                class="file-transfer-mini-btn"
                disabled={!fileTransferConnected}
                on:click={() => void refreshLocalDirCached(fileTransferLocalPath)}
              >
                Refresh
              </button>
            </div>
          </div>
          <div class="file-transfer-list file-transfer-scrollable">
            {#if !fileTransferConnected}
              <div class="file-transfer-empty">
                {fileTransferConnectInFlight
                  ? 'Connecting to local and remote file systems...'
                  : 'Connect to start browsing files.'}
              </div>
            {:else if !(fileTransferLocalBrowseRoot in fileTransferLocalDirCache)}
              <div class="file-transfer-empty">Loading...</div>
            {:else if (fileTransferLocalDirCache[fileTransferLocalBrowseRoot] ?? []).length === 0}
              <div class="file-transfer-empty">This folder is empty.</div>
            {:else}
              {@const localRows = buildFileTransferTreeRows(
                fileTransferLocalBrowseRoot,
                fileTransferLocalDirCache,
                fileTransferLocalExpandedDirs
              )}
              {#each localRows as row (row.entry.path)}
                {@const entry = row.entry}
                {@const modified = formatFileTransferModified(entry.modifiedUnixMs)}
                <div
                  role="treeitem"
                  tabindex="-1"
                  aria-selected={isLocalEntrySelected(entry)}
                  class="file-transfer-row file-transfer-tree-row"
                  class:selected={isLocalEntrySelected(entry)}
                  style={`padding-left: ${8 + Math.min(240, row.depth * 18)}px`}
                  on:contextmenu={(event) => openFileTransferContextMenu(event, 'local', entry)}
                >
                  {#if entry.isDir}
                    <button
                      type="button"
                      class="file-transfer-tree-toggle"
                      aria-label={fileTransferLocalExpandedDirs.has(entry.path)
                        ? 'Collapse folder'
                        : 'Expand folder'}
                      on:click={() => {
                        void setFileTransferLocalDestination(entry.path);
                        void toggleLocalDirExpanded(entry.path);
                      }}
                    >
                      {fileTransferLocalExpandedDirs.has(entry.path) ? '−' : '+'}
                    </button>
                  {:else}
                    <span class="file-transfer-tree-toggle-placeholder" aria-hidden="true"></span>
                  {/if}
                  <input
                    type="checkbox"
                    class="file-transfer-checkbox"
                    checked={isLocalEntrySelected(entry)}
                    disabled={isLocalEntryCheckboxDisabled(entry)}
                    on:change={() => toggleLocalSelection(entry)}
                  />
                  <button
                    type="button"
                    class="file-transfer-entry-button"
                    on:click={() => {
                      if (entry.isDir) {
                        void setFileTransferLocalDestination(entry.path);
                        void toggleLocalDirExpanded(entry.path);
                      } else {
                        toggleLocalSelection(entry);
                      }
                    }}
                  >
                    <span class="file-transfer-entry-name">
                      <span class="file-transfer-entry-type-badge">{entry.isDir ? 'DIR' : 'FILE'}</span>
                      <span>{entry.name}</span>
                      {#if entry.isDir && fileTransferLocalLoadingDirs.has(entry.path)}
                        <span class="file-transfer-entry-loading">...</span>
                      {/if}
                    </span>
                    <span class="file-transfer-entry-meta">
                      {entry.isDir ? 'Folder' : formatBytes(entry.sizeBytes)}
                      {#if modified}
                        | {modified}
                      {/if}
                    </span>
                  </button>
                </div>
              {/each}
            {/if}
          </div>
        </section>

        <section class="file-transfer-panel">
          <div class="file-transfer-panel-header">
            <div>
              <h2>Remote</h2>
              <p class="file-transfer-panel-subtitle">
                {(fileTransferRemoteDirCache[fileTransferRemotePath] ?? []).length} item(s) | {fileTransferRemoteSelectedDirs.size + fileTransferRemoteSelectedFiles.size} selected
              </p>
              <p class="file-transfer-path">{fileTransferRemotePath}</p>
            </div>
            <div class="file-transfer-panel-actions">
              <button
                type="button"
                class="file-transfer-mini-btn"
                disabled={!fileTransferConnected || !canNavigateUp(fileTransferRemotePath)}
                on:click={() => void selectRemoteDir(parentPath(fileTransferRemotePath))}
              >
                Up
              </button>
              <button
                type="button"
                class="file-transfer-mini-btn"
                disabled={!fileTransferConnected}
                on:click={() => void refreshRemoteDirCached(fileTransferRemotePath)}
              >
                Refresh
              </button>
            </div>
          </div>
          <div class="file-transfer-list file-transfer-scrollable">
            {#if !fileTransferConnected}
              <div class="file-transfer-empty">
                {fileTransferConnectInFlight
                  ? 'Connecting to local and remote file systems...'
                  : 'Connect to start browsing files.'}
              </div>
            {:else if !(fileTransferRemoteBrowseRoot in fileTransferRemoteDirCache)}
              <div class="file-transfer-empty">Loading...</div>
            {:else if (fileTransferRemoteDirCache[fileTransferRemoteBrowseRoot] ?? []).length === 0}
              <div class="file-transfer-empty">This folder is empty.</div>
            {:else}
              {@const remoteRows = buildFileTransferTreeRows(
                fileTransferRemoteBrowseRoot,
                fileTransferRemoteDirCache,
                fileTransferRemoteExpandedDirs
              )}
              {#each remoteRows as row (row.entry.path)}
                {@const entry = row.entry}
                {@const modified = formatFileTransferModified(entry.modifiedUnixMs)}
                <div
                  role="treeitem"
                  tabindex="-1"
                  aria-selected={isRemoteEntrySelected(entry)}
                  class="file-transfer-row file-transfer-tree-row"
                  class:selected={isRemoteEntrySelected(entry)}
                  style={`padding-left: ${8 + Math.min(240, row.depth * 18)}px`}
                  on:contextmenu={(event) => openFileTransferContextMenu(event, 'remote', entry)}
                >
                  {#if entry.isDir}
                    <button
                      type="button"
                      class="file-transfer-tree-toggle"
                      aria-label={fileTransferRemoteExpandedDirs.has(entry.path)
                        ? 'Collapse folder'
                        : 'Expand folder'}
                      on:click={() => {
                        void setFileTransferRemoteDestination(entry.path);
                        void toggleRemoteDirExpanded(entry.path);
                      }}
                    >
                      {fileTransferRemoteExpandedDirs.has(entry.path) ? '−' : '+'}
                    </button>
                  {:else}
                    <span class="file-transfer-tree-toggle-placeholder" aria-hidden="true"></span>
                  {/if}
                  <input
                    type="checkbox"
                    class="file-transfer-checkbox"
                    checked={isRemoteEntrySelected(entry)}
                    disabled={isRemoteEntryCheckboxDisabled(entry)}
                    on:change={() => toggleRemoteSelection(entry)}
                  />
                  <button
                    type="button"
                    class="file-transfer-entry-button"
                    on:click={() => {
                      if (entry.isDir) {
                        void setFileTransferRemoteDestination(entry.path);
                        void toggleRemoteDirExpanded(entry.path);
                      } else {
                        toggleRemoteSelection(entry);
                      }
                    }}
                  >
                    <span class="file-transfer-entry-name">
                      <span class="file-transfer-entry-type-badge">{entry.isDir ? 'DIR' : 'FILE'}</span>
                      <span>{entry.name}</span>
                      {#if entry.isDir && fileTransferRemoteLoadingDirs.has(entry.path)}
                        <span class="file-transfer-entry-loading">...</span>
                      {/if}
                    </span>
                    <span class="file-transfer-entry-meta">
                      {entry.isDir ? 'Folder' : formatBytes(entry.sizeBytes)}
                      {#if modified}
                        | {modified}
                      {/if}
                    </span>
                  </button>
                </div>
              {/each}
            {/if}
          </div>
        </section>
      </div>

      <div class="file-transfer-footer">
        <div class="file-transfer-actions">
          <button
            type="button"
            class="file-transfer-button"
            disabled={!fileTransferConnected || (fileTransferLocalSelectedDirs.size + fileTransferLocalSelectedFiles.size) === 0}
            on:click={() => void startFileTransferUpload('prompt')}
          >
            Upload selected to remote
          </button>
          <button
            type="button"
            class="file-transfer-button"
            disabled={!fileTransferConnected || (fileTransferRemoteSelectedDirs.size + fileTransferRemoteSelectedFiles.size) === 0}
            on:click={() => void startFileTransferDownload('prompt')}
          >
            Download selected to local
          </button>
        </div>

        <section class="file-transfer-jobs file-transfer-scrollable">
          <div class="file-transfer-jobs-header">
            <div>
              <h2>Transfer Queue</h2>
              <p class="file-transfer-jobs-subtitle">
                Preparation, transfer, and finalize stages with live updates.
              </p>
            </div>
            <div class="file-transfer-jobs-tools">
              <div class="file-transfer-job-counters">
                <span class="file-transfer-job-counter">{fileTransferRunningJobs} active</span>
                <span class="file-transfer-job-counter">{fileTransferDoneJobs} completed</span>
                <span class="file-transfer-job-counter">{fileTransferFailedJobs} failed</span>
              </div>
              <button
                type="button"
                class="file-transfer-jobs-clear"
                disabled={fileTransferDoneJobs + fileTransferFailedJobs === 0}
                on:click={clearFinishedFileTransferJobs}
              >
                Clear finished
              </button>
            </div>
          </div>

          {#if fileTransferJobs.length === 0}
            <p class="file-transfer-empty-text file-transfer-empty-text--panel">
              No active or completed transfers yet.
            </p>
          {:else}
            {#each sortedFileTransferJobs as job}
              {@const progress = job.bytesTotal > 0
                ? Math.max(0, Math.min(100, (job.bytesDone / job.bytesTotal) * 100))
                : job.status === 'done'
                  ? 100
                  : 0}
              <div
                class="file-transfer-job-row"
                class:is-running={job.status === 'running'}
                class:is-done={job.status === 'done' || job.status === 'cancelled'}
                class:is-error={job.status === 'error'}
              >
                <div class="file-transfer-job-main">
                  <div class="file-transfer-job-title">
                    <span class="file-transfer-job-direction">
                      {job.direction === 'upload' ? 'UPLOAD' : 'DOWNLOAD'}
                    </span>
                    <strong title={job.fileName}>{job.fileName}</strong>
                  </div>
                  {#if job.status === 'running'}
                    <button
                      type="button"
                      class="file-transfer-job-status file-transfer-job-status--action status-running"
                      title="Cancel transfer"
                      on:click={() => void cancelFileTransferJob(job.id)}
                    >
                      <span>running</span>
                      <span class="file-transfer-job-status-cancel" aria-hidden="true">×</span>
                    </button>
                  {:else}
                    <span
                      class="file-transfer-job-status"
                      class:status-done={job.status === 'done' || job.status === 'cancelled'}
                      class:status-error={job.status === 'error'}
                    >
                      {job.status}
                    </span>
                  {/if}
                </div>
                <div class="file-transfer-job-sub">
                  <span>{formatBytes(job.bytesDone)} / {formatBytes(job.bytesTotal)}</span>
                  {#if job.status === 'done'}
                    <span class="file-transfer-job-phase">Completed</span>
                  {:else if job.phase}
                    <span class="file-transfer-job-phase">{formatFileTransferPhase(job.phase)}</span>
                  {/if}
                  {#if job.message}
                    <span class="file-transfer-job-message">{job.message}</span>
                  {/if}
                </div>
                <div class="file-transfer-progress">
                  <div
                    class="file-transfer-progress-fill"
                    class:indeterminate={job.status === 'running' && job.bytesTotal === 0}
                    style={`width: ${progress}%`}
                  ></div>
                </div>
              </div>
            {/each}
          {/if}
        </section>
      </div>

      {#if fileTransferContextMenu.open && fileTransferContextMenu.entry}
        {@const menuEntry = fileTransferContextMenu.entry}
        <div
          class="ft-context-menu"
          style={`top: ${fileTransferContextMenu.y}px; left: ${fileTransferContextMenu.x}px;`}
          role="menu"
          tabindex="-1"
          aria-label="File transfer folder actions"
          on:click|stopPropagation={() => {}}
          on:keydown|stopPropagation={() => {}}
        >
          <div class="ft-context-menu-title" title={menuEntry.path}>
            {menuEntry.name}
          </div>
          <button
            type="button"
            class="ft-context-menu-item"
            role="menuitem"
            disabled={isNonEditableRootFolder(menuEntry)}
            on:click={() => openFileTransferRenameDialog(fileTransferContextMenu.side, menuEntry)}
          >
            Rename
          </button>
          <button
            type="button"
            class="ft-context-menu-item ft-context-menu-item--danger"
            role="menuitem"
            disabled={isNonEditableRootFolder(menuEntry)}
            on:click={() => openFileTransferDeleteDialog(fileTransferContextMenu.side, menuEntry)}
          >
            Delete
          </button>
          <button
            type="button"
            class="ft-context-menu-item"
            role="menuitem"
            on:click={() => void refreshFileTransferFolder()}
          >
            Refresh
          </button>
        </div>
      {/if}

      {#if fileTransferDialogOpen && fileTransferDialogEntry}
        {@const dialogEntry = fileTransferDialogEntry}
        <div
          class="registry-modal-backdrop"
          role="presentation"
          on:click|self={closeFileTransferDialog}
          on:keydown={(e) => e.key === 'Escape' && closeFileTransferDialog()}
        >
          <div
            class="registry-modal"
            role="dialog"
            tabindex="-1"
            aria-modal="true"
            aria-label={fileTransferDialogMode === 'rename'
              ? 'Rename folder'
              : 'Delete folder'}
            on:click|stopPropagation={() => {}}
            on:keydown|stopPropagation={(e) =>
              e.key === 'Escape' && closeFileTransferDialog()}
          >
            <div class="registry-modal-header">
              <div class="registry-modal-title">
                {fileTransferDialogMode === 'rename' ? 'Rename Folder' : 'Delete Folder'}
              </div>
              <button
                class="registry-button registry-button--ghost"
                type="button"
                on:click={closeFileTransferDialog}
                disabled={fileTransferDialogBusy}
              >
                Close
              </button>
            </div>

            <div class="registry-modal-body">
              <div class="registry-form-row">
                <div class="registry-form-label">Path</div>
                <div class="registry-form-static">{dialogEntry.path}</div>
              </div>

              {#if fileTransferDialogMode === 'rename'}
                <div class="registry-form-row">
                  <label class="registry-form-label" for="ft-rename-name">Name</label>
                  <input
                    class="registry-input"
                    id="ft-rename-name"
                    bind:value={fileTransferDialogName}
                    disabled={fileTransferDialogBusy}
                    placeholder="New folder name"
                    on:keydown={(e) => e.key === 'Enter' && void submitFileTransferDialog()}
                  />
                </div>
              {:else}
                <div class="registry-alert registry-alert--warning">
                  This will permanently delete the folder and everything inside it.
                </div>
              {/if}

              {#if fileTransferDialogError}
                <div class="registry-alert registry-alert--error" role="alert">{fileTransferDialogError}</div>
              {/if}
            </div>

            <div class="registry-modal-footer">
              <button
                class="registry-button"
                type="button"
                on:click={closeFileTransferDialog}
                disabled={fileTransferDialogBusy}
              >
                Cancel
              </button>
              <button
                class="registry-button registry-button--primary"
                type="button"
                on:click={() => void submitFileTransferDialog()}
                disabled={fileTransferDialogBusy}
              >
                {#if fileTransferDialogMode === 'rename'}
                  {fileTransferDialogBusy ? 'Renaming...' : 'Rename'}
                {:else}
                  {fileTransferDialogBusy ? 'Deleting...' : 'Delete'}
                {/if}
              </button>
            </div>
          </div>
        </div>
      {/if}
    </div>

    <div class="registry-container" class:tab-panel-hidden={activeTab !== 'Remote Registry'}>
      <RemoteRegistry
        invokeTauri={invokeTauri}
        connected={registryConnected}
        status={registryStatus}
        error={registryError}
        connectInFlight={registryConnectInFlight}
        onConnect={() => void ensureRegistrySessionLive()}
      />
    </div>
  </main>

  {#if appConfirmOpen}
    <div class="registry-modal-backdrop" role="presentation" on:click|self={() => closeAppConfirm(false)}>
      <div class="registry-modal" role="dialog" aria-modal="true" aria-label={appConfirmTitle}>
        <div class="registry-modal-header">
          <div class="registry-modal-title">{appConfirmTitle}</div>
          <button class="registry-button registry-button--ghost" type="button" on:click={() => closeAppConfirm(false)}>
            Close
          </button>
        </div>
        <div class="registry-modal-body">
          <div class="registry-alert registry-alert--warning" style="white-space: pre-wrap;">{appConfirmBody}</div>
        </div>
        <div class="registry-modal-footer">
          {#if !appConfirmHideCancel}
            <button class="registry-button" type="button" on:click={() => closeAppConfirm(false)}>
              {appConfirmCancelLabel}
            </button>
          {/if}
          <button class="registry-button registry-button--primary" type="button" on:click={() => closeAppConfirm(true)}>
            {appConfirmOkLabel}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Session chat sidebar -->
  {#if chatPanelOpen}
    <div
      class="settings-backdrop"
      on:click={closeChatPanel}
      on:keydown={(e) => e.key === 'Escape' && closeChatPanel()}
      role="button"
      tabindex="-1"
      aria-label="Close session chat"
    ></div>

    <aside
      class="chat-sidebar"
      aria-label="Session chat"
      on:animationend={handleOverlayAnimationEnd}
    >
      <div class="settings-header">
        <div>
          <h2 class="settings-title">Session chat</h2>
          <p class="ai-assist-subtitle">Exchange secure messages with the remote user.</p>
        </div>
        <div class="chat-sidebar-header-actions">
          <button
            type="button"
            class="registry-button registry-button--secondary chat-sidebar-disconnect"
            title="Disconnect chat session"
            on:click={() => void disconnectChatSession()}
          >
            Disconnect
          </button>
          <button
            class="settings-close"
            type="button"
            on:click={closeChatPanel}
            title="Close session chat"
            aria-label="Close session chat"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>
      </div>

      <div class="ai-assist-content custom-scrollbar">
        <div
          class="ai-assist-status"
          class:ai-assist-status--ready={chatConnected}
          class:ai-assist-status--busy={!!chatStatus && chatStatus.includes('Connecting')}
        >
          <span class="ai-assist-status-dot"></span>
          <span>{chatStatus || 'Chat idle.'}</span>
        </div>

        {#if chatError}
          <div class="ai-assist-error" role="alert">{chatError}</div>
        {/if}

        <div class="ai-assist-transcript">
          {#if chatMessages.length === 0}
            <div class="ai-assist-empty">No messages yet. Say hello below.</div>
          {:else}
            {#each chatMessages as m (m.id)}
              <div
                class="ai-assist-message"
                class:ai-assist-message--user={m.fromViewer}
              >
                <span>{m.text}</span>
                {#if m.fromViewer && m.state === 'failed'}
                  <small class="chat-message-state">Failed</small>
                {:else if m.fromViewer && m.state === 'sending'}
                  <small class="chat-message-state">Sending</small>
                {/if}
              </div>
            {/each}
          {/if}
        </div>
      </div>

      <form
        class="ai-assist-composer"
        on:submit|preventDefault={() => void sendViewerChatMessage()}
      >
        <label class="ai-assist-prompt-label" for="session-chat-message-input">Message</label>
        <textarea
          id="session-chat-message-input"
          class="ai-assist-prompt"
          bind:value={chatDraft}
          rows="3"
          placeholder={chatConnected
            ? 'Write a message to the remote user…'
            : 'Connect chat to send messages.'}
          disabled={!chatConnected}
          on:keydown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void sendViewerChatMessage();
            }
          }}
        ></textarea>
        <div class="ai-assist-composer-footer">
          <span class="ai-assist-enter-hint">Enter to send, Shift+Enter for newline</span>
          <div class="ai-assist-composer-actions">
            <button type="submit" class="ai-assist-send" disabled={!chatConnected || !chatDraft.trim()}>
              Send
            </button>
          </div>
        </div>
      </form>
    </aside>
  {/if}

  <!-- AI Assist Sidebar -->
  {#if aiAssistPanelOpen}
    <div
      class="settings-backdrop"
      on:click={closeAiAssistPanel}
      on:keydown={(e) => e.key === 'Escape' && closeAiAssistPanel()}
      role="button"
      tabindex="-1"
      aria-label="Close AI Assist"
    ></div>

    <aside
      class="ai-assist-sidebar"
      aria-label="AI Assist"
      on:animationend={handleOverlayAnimationEnd}
    >
      <div class="settings-header">
        <div>
          <h2 class="settings-title">AI Assist</h2>
          <p class="ai-assist-subtitle">Goal-oriented help for the current remote desktop session.</p>
        </div>
        <button
          class="settings-close"
          type="button"
          on:click={closeAiAssistPanel}
          title="Close AI Assist"
          aria-label="Close AI Assist"
        >
          <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="ai-assist-content">
        <div
          class="ai-assist-status"
          class:ai-assist-status--ready={aiAssistReadyForPrompt && !aiAssistInFlight}
          class:ai-assist-status--busy={aiAssistInFlight}
        >
          <span class="ai-assist-status-dot"></span>
          <span>{aiAssistStatus || 'Open a live remote desktop session first.'}</span>
        </div>

        {#if aiAssistCurrentTaskId || aiAssistTaskStatus || aiAssistStepIndex > 0 || aiAssistMaxSteps > 0}
          <div class="ai-assist-progress">
            <span>Step {aiAssistStepIndex || 0}/{aiAssistMaxSteps || '?'}</span>
            <span>{formatAiAssistTaskStatus(aiAssistTaskStatus)}</span>
          </div>
        {/if}

        {#if aiAssistPlanLines.length > 0}
          <div class="ai-assist-plan">
            <h3 class="settings-section-title">Plan</h3>
            <ol class="ai-assist-plan-list">
              {#each aiAssistPlanLines as planLine, index (`${planLine}-${index}`)}
                <li>{planLine}</li>
              {/each}
            </ol>
          </div>
        {/if}

        <div class="ai-assist-transcript" bind:this={aiAssistTranscriptEl}>
          {#if aiAssistLines.length === 0}
            <div class="ai-assist-empty">
              Give AI Assist a concise desktop goal. It will plan, act, re-check the
              screen, and stop when the task completes or needs attention.
            </div>
          {:else}
            {#each aiAssistLines as line, index (index)}
              <div
                class="ai-assist-message"
                class:ai-assist-message--user={line.startsWith('You:')}
                class:ai-assist-message--assistant={line.startsWith('AI')}
              >
                {line}
              </div>
            {/each}
          {/if}
        </div>

        {#if aiAssistActionLines.length > 0}
          <div class="ai-assist-actions">
            <h3 class="settings-section-title">Last Actions</h3>
            <div class="ai-assist-action-list">
              {#each aiAssistActionLines as actionLine, index (index)}
                <div class="ai-assist-action-item">{actionLine}</div>
              {/each}
            </div>
          </div>
        {/if}

        {#if aiAssistError}
          <div class="ai-assist-error" role="alert">
            {aiAssistError}
          </div>
        {/if}
      </div>

      <form
        class="ai-assist-composer"
        on:submit|preventDefault={() => void sendAiAssistMessage()}
      >
        <label class="ai-assist-prompt-label" for="ai-assist-prompt">
          Prompt
        </label>
        <textarea
          id="ai-assist-prompt"
          class="ai-assist-prompt"
          bind:value={aiAssistDraft}
          rows="4"
          placeholder={aiAssistReadyForPrompt
            ? 'Describe the desktop goal to complete...'
            : 'Connect to Remote Desktop to enable AI Assist.'}
          disabled={aiAssistPromptDisabled}
          on:keydown={handleAiAssistPromptKeydown}
        ></textarea>
        <div class="ai-assist-composer-footer">
          <span class="ai-assist-enter-hint">Enter to send, Shift+Enter for newline</span>
          <div class="ai-assist-composer-actions">
            {#if aiAssistInFlight}
              <button
                type="button"
                class="ai-assist-stop"
                disabled={aiAssistStopRequested}
                on:click={requestAiAssistStop}
              >
                {aiAssistStopRequested ? 'Stopping...' : 'Stop'}
              </button>
            {/if}
            <button
              type="submit"
              class="ai-assist-send"
              disabled={!aiAssistCanSend}
            >
              {aiAssistInFlight ? 'Working...' : 'Start'}
            </button>
          </div>
        </div>
      </form>
    </aside>
  {/if}

  <!-- Settings Sidebar -->
  {#if settingsOpen}
    <!-- Backdrop -->
    <div
      class="settings-backdrop"
      on:click={closeSettings}
      on:keydown={(e) => e.key === 'Escape' && closeSettings()}
      role="button"
      tabindex="-1"
      aria-label="Close settings"
    ></div>

    <!-- Sidebar -->
    <aside
      class="settings-sidebar"
      on:animationend={handleOverlayAnimationEnd}
    >
      <div class="settings-header">
        <h2 class="settings-title">Settings</h2>
        <button class="settings-close" on:click={closeSettings} title="Close Settings">
          <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="settings-content">
        <!-- Appearance -->
        <div class="settings-section">
          <h3 class="settings-section-title">Appearance</h3>
          <div class="settings-group">
            <button
              type="button"
              class="theme-toggle-btn"
              on:click={toggleTheme}
              aria-label={isLightMode ? 'Switch to dark mode' : 'Switch to light mode'}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="theme-icon-moon"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>
              <span class="theme-toggle-label">Theme</span>
              <div class="aero-toggle-track">
                <div class="aero-toggle-thumb" class:is-light={isLightMode}></div>
              </div>
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="theme-icon-sun"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
            </button>
          </div>
        </div>

        <div class="settings-section">
          <h3 class="settings-section-title">Updates</h3>
          <div class="settings-group">
            <div class="settings-meta">
              <span class="settings-meta-label">Current Version</span>
              <span class="settings-meta-value">{viewerVersion}</span>
            </div>
            <button
              type="button"
              class="settings-action-button"
              on:click={() => void checkForViewerUpdates()}
              disabled={viewerUpdateCheckInFlight}
            >
              {viewerUpdateCheckInFlight ? 'Checking for updates...' : 'Check for Updates'}
            </button>
            <div class="settings-hint">
              {viewerUpdateStatusMessage ??
                'Checks for a newer viewer build and asks before closing to install it.'}
            </div>
          </div>
        </div>
      </div>
    </aside>
  {/if}
</div>
