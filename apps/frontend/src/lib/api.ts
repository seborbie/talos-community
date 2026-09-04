import axios, { type AxiosError, type AxiosInstance } from 'axios';
import { browser } from '$app/environment';
import { env } from '$env/dynamic/public';
import { resolveRuntimePublicServiceUrls } from '$lib/runtimePublicConfig';
import type {
  User,
  AuthResponse,
  LoginRequest,
  RegisterRequest,
  RegistrationStatus,
  ApiError,
  Organization,
  OrganizationMember,
  OrgRole,
  HaloConfig,
  RmmDevice,
  RmmDeviceListFilters,
  RmmDeviceListQuery,
  RmmDeviceListResponse,
  RmmDeviceSavedView,
  RmmConnectResponse,
  RmmViewerConnectionSummary,
  RmmViewerSessionStatus,
  RmmCommandExecutionLogListResponse,
  RmmReportDataResponse,
  RmmReportDefinition,
  RmmReportDownloadResult,
  RmmReportFilters,
  RmmReportFormat,
  RmmReportFrequency,
  RmmReportId,
  RmmReportRun,
  RmmReportRunCreateResponse,
  RmmReportSchedule,
  RmmDeviceTelemetry,
  RmmTelemetryStatus,
  RmmSnapshotRequestAccepted,
  RmmSnapshotRequestStatus,
  RmmTelemetryEventListResponse,
  RmmTelemetryFactListResponse,
  RmmTelemetryBaselineListResponse,
  RmmTelemetryDecisionListResponse,
  RmmTelemetryAlertListResponse,
  RmmTelemetryAlert,
  RmmTelemetryAlertRuleListResponse,
  RmmTelemetryAlertRule,
  RmmTelemetryRoutingRuleListResponse,
  RmmTelemetryRoutingRule,
  RmmTelemetryRoutingRuleTestResponse,
  RmmTelemetryRoutingAction,
  RmmTelemetryRoutingMatchOperator,
  RmmTelemetryRoutingTestCandidate,
  RmmTelemetryBaselineScopeType,
  RmmTelemetryBaselineScopeCatalogResponse,
  RmmTelemetryScopedBaselineListResponse,
  RmmTelemetryScopedBaselineSummaryResponse,
  RmmTelemetryScopedBaselineDriftListResponse,
  RmmTelemetryStabilityOverrideListResponse,
  RmmTelemetryStabilityOverride,
  RmmTelemetryStabilityOverridePreviewResponse,
  RmmTelemetryIntentListResponse,
  RmmTelemetryIntent,
  RmmTelemetryRemediationJobListResponse,
  PatchApprovalResponse,
  PatchComplianceResponse,
  PatchComplianceStatus,
  PatchDecision,
  PatchInstallResponse,
  PatchManualActionResponse,
  PatchDeviceStateResponse,
  PatchOverviewResponse,
  PatchPolicy,
  PatchPolicyListResponse,
  PatchPolicyScopeType,
  PatchPolicyTargetOsFamily,
  PatchProgressResponse,
  PatchApprovalMode,
  PatchRebootBehavior,
  FeatureUpgradeIsoMediaListResponse,
  FeatureUpgradeIsoStagePreviewResponse,
  FeatureUpgradeIsoStageProgressResponse,
  FeatureUpgradeIsoStageRunResponse,
  FeatureUpgradePreflightPreviewResponse,
  FeatureUpgradePreflightProgressResponse,
  FeatureUpgradePreflightRunResponse,
  FeatureUpgradeStartPreviewResponse,
  FeatureUpgradeStartProgressResponse,
  FeatureUpgradeStartRunResponse,
  Customer,
  Site,
  CommandPolicy,
  CreatePolicyRequest,
  CommandCenterChatRequest,
  CommandCenterChatResponse,
  CommandCenterChatConversationEvent,
  CommandCenterChatDeltaEvent,
  CommandCenterChatStatusEvent,
  CommandCenterAiRunnerJob,
  CommandCenterAiRunnerOutputDelta,
  CommandCenterAiRunnerReplayManifest,
  CommandCenterAiRunnerStreamSnapshot,
  CommandCenterConversationSummary,
  CommandCenterStoredMessage,
  RmmInstallerScopeType,
  RmmInstallerProfile,
  RmmInstallerProfileCreateResponse,
  RmmInstallerDownloadResponse,
  RmmInstallerExeDownloadResult,
  RmmLinuxInstallerResponse,
  RmmMacosInstallerResponse,
  LinuxShellCredential,
  LinuxAgentInstallerInfo,
  MacosPackageInstallerInfo,
  AuditEventListResponse,
  ViewerInstallerInfo,
  ViewerInstallerPlatform,
  SecureNoteCheckResponse,
  SecureNoteRevealResponse
} from '$lib/types';

const { apiUrl: API_BASE_URL, rmmApiUrl: RMM_API_BASE_URL } = resolveRuntimePublicServiceUrls(env);
const RMM_REQUEST_SNAPSHOT_TIMEOUT_MS = 120_000;

// Create axios instance
const api: AxiosInstance = axios.create({
  baseURL: API_BASE_URL,
  timeout: 10000,
  headers: {
    'Content-Type': 'application/json',
  },
});

const rmmApiInstance: AxiosInstance | null = RMM_API_BASE_URL
  ? axios.create({
      baseURL: RMM_API_BASE_URL,
      timeout: 10000,
      headers: {
        'Content-Type': 'application/json',
      },
    })
  : null;

const requireRmmApi = (): AxiosInstance => {
  if (!rmmApiInstance) {
    throw new Error('PUBLIC_RMM_API_URL is not configured');
  }
  return rmmApiInstance;
};

// Request interceptor to add auth token
api.interceptors.request.use(
  (config) => {
    if (browser) {
      const token = localStorage.getItem('token');
      if (token) {
        config.headers.Authorization = `Bearer ${token}`;
      }
    }
    return config;
  },
  (error) => Promise.reject(error)
);

if (rmmApiInstance) {
  rmmApiInstance.interceptors.request.use(
    (config) => {
      if (browser) {
        const token = localStorage.getItem('token');
        if (token) {
          config.headers.Authorization = `Bearer ${token}`;
        }
      }
      return config;
    },
    (error) => Promise.reject(error)
  );
}

const rejectApiError = (error: AxiosError) => {
  const serverData = error.response?.data as any;
  const apiError = new Error(
    serverData?.message || serverData?.error || error.message || 'An error occurred'
  ) as Error & ApiError;
  apiError.statusCode = error.response?.status || 500;
  (apiError as any).data = serverData;

  // Auto-redirect on 401, but not for auth endpoints — a 401 from /auth/login
  // or /auth/register means bad credentials and must propagate to the caller.
  if (error.response?.status === 401 && browser) {
    const url = error.config?.url ?? '';
    if (!url.includes('/auth/')) {
      localStorage.removeItem('token');
      const redirect = `${window.location.pathname}${window.location.search}`;
      window.location.href = `/login?redirect=${encodeURIComponent(redirect)}`;
    }
  }

  return Promise.reject(apiError);
};

// Response interceptor to handle errors
api.interceptors.response.use((response) => response, rejectApiError);

if (rmmApiInstance) {
  rmmApiInstance.interceptors.response.use((response) => response, rejectApiError);
}

// Auth API
export const authApi = {
  getRegistrationStatus: async (): Promise<RegistrationStatus> => {
    const response = await api.get<RegistrationStatus>('/auth/registration-status');
    return response.data;
  },

  login: async (credentials: LoginRequest): Promise<AuthResponse> => {
    const response = await api.post<AuthResponse>('/auth/login', credentials);
    return response.data;
  },
  
  register: async (userData: RegisterRequest): Promise<AuthResponse> => {
    const response = await api.post<AuthResponse>('/auth/register', userData);
    return response.data;
  },
  
  generateMachineToken: async (): Promise<{ token: string }> => {
    const response = await api.post<{ token: string }>('/auth/machine-token');
    return response.data;
  },
};

// User API
export const userApi = {
  getProfile: async (): Promise<{ user: User }> => {
    const response = await api.get('/auth/profile');
    return response.data;
  },

  updateProfile: async (data: {
    email?: string;
    currentPassword: string;
    newPassword?: string;
  }): Promise<{ user: User }> => {
    const response = await api.patch('/auth/profile', data);
    return response.data;
  },

  deleteAccount: async (): Promise<void> => {
    await api.delete('/auth/account');
  },
};

// Orgs API
export const orgsApi = {
  getCurrent: async (): Promise<{ organization: Organization; membership: { id: string; role: OrgRole; userId: string; organizationId: string }; user: User } | { error: string; needsOnboarding: boolean }> => {
    const response = await api.get('/orgs/current');
    return response.data;
  },
  onboard: async (data: { name: string; members?: Array<{ email: string; password?: string; role: OrgRole }> }): Promise<{ organization: Organization }> => {
    const response = await api.post('/orgs/onboard', data);
    return response.data;
  },
  listMembers: async (): Promise<OrganizationMember[]> => {
    const response = await api.get('/orgs/members');
    return response.data;
  },
  addMember: async (data: { email: string; password?: string; role: OrgRole }): Promise<OrganizationMember> => {
    const response = await api.post('/orgs/members', data);
    return response.data;
  },
  updateMemberRole: async (memberId: string, role: OrgRole): Promise<OrganizationMember> => {
    const response = await api.patch(`/orgs/members/${memberId}`, { role });
    return response.data;
  },
  removeMember: async (memberId: string): Promise<void> => {
    await api.delete(`/orgs/members/${memberId}`);
  },
  deleteOrganization: async (): Promise<void> => {
    await api.delete('/orgs/account');
  },
  getHaloConfig: async (): Promise<HaloConfig> => {
    const response = await api.get('/orgs/ticketing/halo');
    return response.data;
  },
  updateHaloConfig: async (config: HaloConfig): Promise<{ success: boolean }> => {
    const response = await api.put('/orgs/ticketing/halo', config);
    return response.data;
  },
  clearHaloConfig: async (): Promise<void> => {
    await api.delete('/orgs/ticketing/halo');
  }
};

// Customers API
export const customerApi = {
  getCustomers: async (): Promise<Array<Customer & { deviceCount?: number }>> => {
    try {
      const response = await api.get('/customers');
      return response.data;
    } catch (err: any) {
      throw err;
    }
  },
  getCustomer: async (id: string): Promise<Customer & { deviceCount?: number }> => {
    const response = await api.get(`/customers/${id}`);
    return response.data;
  },
  createCustomer: async (data: { name: string; description?: string | null }): Promise<Customer> => {
    const response = await api.post('/customers', data);
    return response.data;
  },
  updateCustomer: async (id: string, data: { name?: string; description?: string | null }): Promise<Customer> => {
    const response = await api.patch(`/customers/${id}`, data);
    return response.data;
  },
  deleteCustomer: async (id: string): Promise<void> => {
    await api.delete(`/customers/${id}`);
  }
};

// Sites API (sites are under customers)
export const siteApi = {
  getSites: async (customerId?: string): Promise<Array<Site & { deviceCount?: number }>> => {
    const params = customerId ? { customerId } : {};
    const response = await api.get('/sites', { params });
    return response.data;
  },
  getSite: async (id: string): Promise<Site & { deviceCount?: number }> => {
    const response = await api.get(`/sites/${id}`);
    return response.data;
  },
  createSite: async (data: {
    customerId: string;
    name: string;
    timezone?: string | null;
  }): Promise<Site> => {
    const response = await api.post('/sites', data);
    return response.data;
  },
  updateSite: async (
    id: string,
    data: { name?: string; timezone?: string | null }
  ): Promise<Site> => {
    const response = await api.patch(`/sites/${id}`, data);
    return response.data;
  },
  deleteSite: async (id: string): Promise<void> => {
    await api.delete(`/sites/${id}`);
  }
};

// Command policies API
export const policiesApi = {
  listPolicies: async (): Promise<CommandPolicy[]> => {
    try {
      const response = await api.get<CommandPolicy[]>('/policies');
      return response.data;
    } catch (err: any) {
      throw err;
    }
  },
  createPolicy: async (data: CreatePolicyRequest): Promise<CommandPolicy> => {
    const response = await api.post<CommandPolicy>('/policies', data);
    return response.data;
  },
  updatePolicy: async (
    id: string,
    data: Partial<Pick<CommandPolicy, 'policyType' | 'description' | 'reason'>>
  ): Promise<CommandPolicy> => {
    const response = await api.patch<CommandPolicy>(`/policies/${id}`, data);
    return response.data;
  },
  deletePolicy: async (id: string): Promise<void> => {
    await api.delete(`/policies/${id}`);
  }
};

export const commandCenterApi = {
  listConversations: async (): Promise<CommandCenterConversationSummary[]> => {
    const response = await api.get<{ items: CommandCenterConversationSummary[] }>(
      '/command-center/conversations'
    );
    return response.data.items;
  },
  getConversationMessages: async (conversationId: string): Promise<CommandCenterStoredMessage[]> => {
    const response = await api.get<{ items: CommandCenterStoredMessage[] }>(
      `/command-center/conversations/${conversationId}/messages`
    );
    return response.data.items;
  },
  createConversation: async (data: { title?: string | null } = {}): Promise<CommandCenterConversationSummary> => {
    const response = await api.post<{ conversation: CommandCenterConversationSummary }>(
      '/command-center/conversations',
      data
    );
    return response.data.conversation;
  },
  deleteConversation: async (conversationId: string): Promise<void> => {
    await api.delete(`/command-center/conversations/${conversationId}`);
  },
  listAiRunnerJobs: async (options: { conversationId?: string | null; active?: boolean } = {}): Promise<CommandCenterAiRunnerJob[]> => {
    const params = new URLSearchParams();
    if (options.conversationId) params.set('conversationId', options.conversationId);
    if (options.active) params.set('active', 'true');
    const query = params.toString();
    const response = await api.get<{ items: CommandCenterAiRunnerJob[] }>(
      `/command-center/ai-runner/jobs${query ? `?${query}` : ''}`
    );
    return response.data.items;
  },
  getAiRunnerJob: async (jobId: string): Promise<CommandCenterAiRunnerJob> => {
    const response = await api.get<{ job: CommandCenterAiRunnerJob }>(
      `/command-center/ai-runner/jobs/${encodeURIComponent(jobId)}`
    );
    return response.data.job;
  },
  getAiRunnerReplay: async (jobId: string): Promise<CommandCenterAiRunnerReplayManifest> => {
    const response = await api.get<{ replay: CommandCenterAiRunnerReplayManifest }>(
      `/command-center/ai-runner/jobs/${encodeURIComponent(jobId)}/replay`
    );
    return response.data.replay;
  },
  downloadShellTranscript: async (jobId: string): Promise<Blob> => {
    const token = browser ? localStorage.getItem('token') : null;
    const response = await fetch(
      `${API_BASE_URL.replace(/\/$/, '')}/command-center/ai-runner/jobs/${encodeURIComponent(jobId)}/shell-transcript`,
      {
        headers: {
          ...(token ? { Authorization: `Bearer ${token}` } : {})
        }
      }
    );
    if (!response.ok) {
      throw new Error(response.statusText || 'Shell transcript download failed');
    }
    return response.blob();
  },
  stopAiRunnerJobsForConversation: async (conversationId: string): Promise<CommandCenterAiRunnerJob[]> => {
    const response = await api.post<{ items: CommandCenterAiRunnerJob[] }>(
      `/command-center/conversations/${encodeURIComponent(conversationId)}/ai-runner/stop`
    );
    return response.data.items;
  },
  stopAiRunnerJob: async (jobId: string): Promise<CommandCenterAiRunnerJob> => {
    const response = await api.post<{ job: CommandCenterAiRunnerJob }>(
      `/command-center/ai-runner/jobs/${encodeURIComponent(jobId)}/stop`
    );
    return response.data.job;
  },
  streamAiRunnerConversation: async (
    conversationId: string,
    handlers: {
      onSnapshot?: (event: CommandCenterAiRunnerStreamSnapshot) => void;
      onJobs?: (event: { jobs: CommandCenterAiRunnerJob[] }) => void;
      onOutput?: (event: CommandCenterAiRunnerOutputDelta) => void;
      onHeartbeat?: (event: { at?: string }) => void;
      signal?: AbortSignal;
    } = {}
  ): Promise<void> => {
    const token = browser ? localStorage.getItem('token') : null;
    const response = await fetch(
      `${API_BASE_URL.replace(/\/$/, '')}/command-center/conversations/${encodeURIComponent(conversationId)}/ai-runner/stream`,
      {
        headers: {
          ...(token ? { Authorization: `Bearer ${token}` } : {})
        },
        signal: handlers.signal
      }
    );
    if (!response.ok) {
      let message = response.statusText || 'AI runner stream failed';
      try {
        const serverData = await response.json();
        message = serverData?.message || serverData?.error || message;
      } catch {
        // Keep the status text fallback.
      }
      const apiError = new Error(message) as Error & ApiError;
      apiError.statusCode = response.status;
      throw apiError;
    }
    if (!response.body) {
      throw new Error('AI runner stream did not include a response body');
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    const processEvent = (chunk: string) => {
      const lines = chunk.split(/\r?\n/);
      let event = 'message';
      const dataLines: string[] = [];
      for (const line of lines) {
        if (line.startsWith('event:')) {
          event = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          dataLines.push(line.slice(5).trimStart());
        }
      }
      if (dataLines.length === 0) return;
      const payload = JSON.parse(dataLines.join('\n'));
      if (event === 'snapshot') {
        handlers.onSnapshot?.(payload as CommandCenterAiRunnerStreamSnapshot);
      } else if (event === 'jobs') {
        handlers.onJobs?.(payload as { jobs: CommandCenterAiRunnerJob[] });
      } else if (event === 'command_output_delta') {
        handlers.onOutput?.(payload as CommandCenterAiRunnerOutputDelta);
      } else if (event === 'heartbeat') {
        handlers.onHeartbeat?.(payload as { at?: string });
      } else if (event === 'error') {
        throw new Error(payload?.error || 'AI runner stream failed');
      }
    };

    for (;;) {
      const { value, done } = await reader.read();
      buffer += decoder.decode(value ?? new Uint8Array(), { stream: !done });
      const events = buffer.split(/\n\n/);
      buffer = events.pop() ?? '';
      for (const event of events) {
        if (event.trim()) {
          processEvent(event);
        }
      }
      if (done) break;
    }
    if (buffer.trim()) {
      processEvent(buffer);
    }
  },
  approveCommandApproval: async (approvalId: string): Promise<void> => {
    await api.post(`/command-center/ai-runner/command-approvals/${encodeURIComponent(approvalId)}/approve`);
  },
  denyCommandApproval: async (approvalId: string): Promise<void> => {
    await api.post(`/command-center/ai-runner/command-approvals/${encodeURIComponent(approvalId)}/deny`);
  },
  denyCommandApprovalAndUseDesktopControl: async (approvalId: string): Promise<void> => {
    await api.post(
      `/command-center/ai-runner/command-approvals/${encodeURIComponent(approvalId)}/deny-and-use-desktop-control`
    );
  },
  getArtifactContent: async (artifactId: string): Promise<Blob> => {
    const token = browser ? localStorage.getItem('token') : null;
    const response = await fetch(
      `${API_BASE_URL.replace(/\/$/, '')}/command-center/artifacts/${encodeURIComponent(artifactId)}/content`,
      {
        headers: {
          ...(token ? { Authorization: `Bearer ${token}` } : {})
        }
      }
    );
    if (!response.ok) {
      throw new Error(response.statusText || 'Artifact download failed');
    }
    return response.blob();
  },
  chat: async (data: CommandCenterChatRequest): Promise<CommandCenterChatResponse> => {
    const response = await api.post<CommandCenterChatResponse>('/command-center/chat', data, {
      timeout: 120000
    });
    return response.data;
  },
  streamChat: async (
    data: CommandCenterChatRequest,
	    handlers: {
	      onDelta?: (event: CommandCenterChatDeltaEvent) => void;
	      onStatus?: (event: CommandCenterChatStatusEvent) => void;
	      onConversation?: (event: CommandCenterChatConversationEvent) => void;
	      signal?: AbortSignal;
	    } = {}
	  ): Promise<CommandCenterChatResponse> => {
    const token = browser ? localStorage.getItem('token') : null;
    const response = await fetch(`${API_BASE_URL.replace(/\/$/, '')}/command-center/chat/stream`, {
      method: 'POST',
	      headers: {
	        'Content-Type': 'application/json',
	        ...(token ? { Authorization: `Bearer ${token}` } : {})
	      },
	      signal: handlers.signal,
	      body: JSON.stringify(data)
	    });

    if (!response.ok) {
      let message = response.statusText || 'Command Center stream failed';
      try {
        const serverData = await response.json();
        message = serverData?.message || serverData?.error || message;
      } catch {
        // Keep the status text fallback.
      }
      const apiError = new Error(message) as Error & ApiError;
      apiError.statusCode = response.status;
      throw apiError;
    }

    if (!response.body) {
      throw new Error('Command Center stream did not include a response body');
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    let finalResponse: CommandCenterChatResponse | null = null;

    const processEvent = (chunk: string) => {
      const lines = chunk.split(/\r?\n/);
      let event = 'message';
      const dataLines: string[] = [];
      for (const line of lines) {
        if (line.startsWith('event:')) {
          event = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          dataLines.push(line.slice(5).trimStart());
        }
      }
      if (dataLines.length === 0) return;
      const payload = JSON.parse(dataLines.join('\n'));
      if (event === 'status') {
        handlers.onStatus?.(payload as CommandCenterChatStatusEvent);
      } else if (event === 'delta') {
        handlers.onDelta?.(payload as CommandCenterChatDeltaEvent);
      } else if (event === 'conversation') {
        handlers.onConversation?.(payload as CommandCenterChatConversationEvent);
      } else if (event === 'final') {
        finalResponse = payload as CommandCenterChatResponse;
      } else if (event === 'error') {
        throw new Error(payload?.error || 'Command Center stream failed');
      }
    };

    for (;;) {
      const { value, done } = await reader.read();
      buffer += decoder.decode(value ?? new Uint8Array(), { stream: !done });
      const events = buffer.split(/\n\n/);
      buffer = events.pop() ?? '';
      for (const event of events) {
        if (event.trim()) {
          processEvent(event);
        }
      }
      if (done) break;
    }

    if (buffer.trim()) {
      processEvent(buffer);
    }

    if (!finalResponse) {
      throw new Error('Command Center stream ended before a final response');
    }
    return finalResponse;
  }
};

export const auditApi = {
  listEvents: async (options?: {
    limit?: number;
    cursor?: string | null;
    q?: string;
    actionType?: string;
    result?: string;
    agentId?: string;
    userId?: string;
    customerId?: string;
    siteId?: string;
    from?: string;
    to?: string;
  }): Promise<AuditEventListResponse> => {
    const response = await api.get<AuditEventListResponse>('/audit/events', {
      params: {
        limit: options?.limit,
        cursor: options?.cursor ?? undefined,
        q: options?.q?.trim() || undefined,
        actionType: options?.actionType?.trim() || undefined,
        result: options?.result && options.result !== 'all' ? options.result : undefined,
        agentId: options?.agentId?.trim() || undefined,
        userId: options?.userId?.trim() || undefined,
        customerId: options?.customerId?.trim() || undefined,
        siteId: options?.siteId?.trim() || undefined,
        from: options?.from || undefined,
        to: options?.to || undefined
      },
      timeout: 15000
    });
    return response.data;
  },
  exportCsv: async (options?: {
    q?: string;
    actionType?: string;
    result?: string;
    agentId?: string;
    userId?: string;
    customerId?: string;
    siteId?: string;
    from?: string;
    to?: string;
  }): Promise<Blob> => {
    const response = await api.get<Blob>('/audit/events', {
      params: {
        ...options,
        result: options?.result && options.result !== 'all' ? options.result : undefined,
        format: 'csv',
        limit: 1000
      },
      responseType: 'blob',
      timeout: 20000
    });
    return response.data;
  }
};

// RMM API (Rust server)
const toDeviceListParams = (query: Partial<RmmDeviceListQuery>) => {
  const filters = (query.filters ?? {}) as Partial<RmmDeviceListFilters>;
  return {
    page: query.page,
    pageSize: query.pageSize,
    sortBy: query.sortBy,
    sortDirection: query.sortDirection,
    q: filters.q || undefined,
    customerId: filters.customerId || undefined,
    siteId: filters.siteId || undefined,
    status: filters.status && filters.status !== 'all' ? filters.status : undefined,
    os: filters.os || undefined,
    version: filters.version || undefined,
    tag: filters.tag || undefined,
    pendingUpdates: filters.pendingUpdates === null || filters.pendingUpdates === undefined ? undefined : String(filters.pendingUpdates),
    rebootRequired: filters.rebootRequired === null || filters.rebootRequired === undefined ? undefined : String(filters.rebootRequired),
    alertSeverity: filters.alertSeverity || undefined,
    lastSeenAgeMinutes: filters.lastSeenAgeMinutes || undefined
  };
};

export const rmmApi = {
  getDeviceList: async (query: RmmDeviceListQuery): Promise<RmmDeviceListResponse> => {
    const response = await api.get<RmmDeviceListResponse>('/rmm/devices', {
      params: toDeviceListParams(query)
    });
    return response.data;
  },
  getDevices: async (): Promise<RmmDevice[]> => {
    const response = await api.get<RmmDevice[] | RmmDeviceListResponse>('/rmm/devices');
    return Array.isArray(response.data) ? response.data : response.data.items;
  },
  getDeviceSavedViews: async (): Promise<RmmDeviceSavedView[]> => {
    const response = await api.get<{ items: RmmDeviceSavedView[] }>('/rmm/device-views');
    return response.data.items;
  },
  createDeviceSavedView: async (data: {
    name: string;
    state: Pick<RmmDeviceListQuery, 'filters' | 'sortBy' | 'sortDirection' | 'pageSize'>;
  }): Promise<RmmDeviceSavedView> => {
    const response = await api.post<RmmDeviceSavedView>('/rmm/device-views', data);
    return response.data;
  },
  updateDeviceSavedView: async (
    id: string,
    data: Partial<{
      name: string;
      state: Pick<RmmDeviceListQuery, 'filters' | 'sortBy' | 'sortDirection' | 'pageSize'>;
    }>
  ): Promise<RmmDeviceSavedView> => {
    const response = await api.patch<RmmDeviceSavedView>(`/rmm/device-views/${id}`, data);
    return response.data;
  },
  deleteDeviceSavedView: async (id: string): Promise<void> => {
    await api.delete(`/rmm/device-views/${id}`);
  },
  connectDevice: async (agentId: string): Promise<RmmConnectResponse> => {
    const client = requireRmmApi();
    const response = await client.post<RmmConnectResponse>(`/api/rmm/devices/${agentId}/connect`);
    return response.data;
  },
  connectFileTransfer: async (agentId: string): Promise<RmmConnectResponse> => {
    const client = requireRmmApi();
    const response = await client.post<RmmConnectResponse>(
      `/api/rmm/devices/${agentId}/connect-file-transfer`
    );
    return response.data;
  },
  connectShell: async (agentId: string): Promise<RmmConnectResponse> => {
    const client = requireRmmApi();
    const response = await client.post<RmmConnectResponse>(`/api/rmm/devices/${agentId}/connect-shell`);
    return response.data;
  },
  connectRegistry: async (agentId: string): Promise<RmmConnectResponse> => {
    const client = requireRmmApi();
    const response = await client.post<RmmConnectResponse>(
      `/api/rmm/devices/${agentId}/connect-registry`
    );
    return response.data;
  },
  getViewerSessionStatus: async (sessionId: string): Promise<RmmViewerSessionStatus> => {
    const client = requireRmmApi();
    const response = await client.get<RmmViewerSessionStatus>(
      `/api/rmm/viewer-session/${encodeURIComponent(sessionId)}/status`
    );
    return response.data;
  },
  getViewerConnections: async (agentId?: string): Promise<RmmViewerConnectionSummary[]> => {
    const client = requireRmmApi();
    const response = await client.get<RmmViewerConnectionSummary[]>('/api/rmm/viewer-connections', {
      params: agentId ? { agentId } : undefined,
    });
    return response.data;
  },
  getDevice: async (agentId: string): Promise<RmmDevice> => {
    const response = await api.get<RmmDevice>(`/rmm/devices/${agentId}`);
    return response.data;
  },
  updateDeviceSettings: async (
    agentId: string,
    data: { aiRunnerAutoApprove: boolean }
  ): Promise<RmmDevice> => {
    const response = await api.patch<RmmDevice>(`/rmm/devices/${agentId}/settings`, data);
    return response.data;
  },
  getLinuxShellCredential: async (agentId: string): Promise<LinuxShellCredential> => {
    const response = await api.get<LinuxShellCredential>(
      `/rmm/devices/${agentId}/linux-shell-credential`
    );
    return response.data;
  },
  getDeviceTelemetry: async (agentId: string): Promise<RmmDeviceTelemetry> => {
    const response = await api.get<RmmDeviceTelemetry>(`/rmm/devices/${agentId}/telemetry`);
    return response.data;
  },
  getCommandExecutionLogs: async (
    agentId: string,
    options?: {
      limit?: number;
      cursor?: string | null;
      q?: string;
      allowed?: 'all' | 'allowed' | 'blocked';
    }
  ): Promise<RmmCommandExecutionLogListResponse> => {
    const allowed =
      options?.allowed === 'allowed' ? '1' : options?.allowed === 'blocked' ? '0' : undefined;
    const q = options?.q?.trim() ? options.q.trim() : undefined;
    const response = await api.get<RmmCommandExecutionLogListResponse>(
      `/rmm/devices/${agentId}/command-log`,
      {
        params: {
          limit: options?.limit,
          cursor: options?.cursor ?? undefined,
          q,
          allowed
        },
        timeout: 15000
      }
    );
    return response.data;
  },
  deleteDevice: async (agentId: string): Promise<{ deleted: number }> => {
    const response = await api.delete<{ deleted: number }>(`/rmm/devices/${agentId}`);
    return response.data;
  },
  bulkDeleteDevices: async (deviceIds: string[]): Promise<{ deleted: number }> => {
    const response = await api.post<{ deleted: number }>('/rmm/devices/bulk-delete', {
      deviceIds
    });
    return response.data;
  },
  bulkUpdateCustomer: async (deviceIds: string[], customerId: string): Promise<{ updated: number }> => {
    const response = await api.post<{ updated: number }>(
      '/rmm/devices/bulk-update-customer',
      { deviceIds, customerId }
    );
    return response.data;
  },
  bulkUpdateSite: async (
    deviceIds: string[],
    siteId: string | null
  ): Promise<{ updated: number }> => {
    const response = await api.post<{ updated: number }>(
      '/rmm/devices/bulk-update-site',
      { deviceIds, siteId }
    );
    return response.data;
  },
  executeScript: async (
    agentId: string,
    script: string
  ): Promise<{ output: string; exit_code: number | null }> => {
    const client = requireRmmApi();
    const response = await client.post(
      `/api/rmm/devices/${agentId}/execute-script`,
      { script },
      { timeout: 25000 }
    );
    return response.data;
  },
  fetchDeviceDetails: async (agentId: string): Promise<RmmDevice> => {
    const client = requireRmmApi();
    const response = await client.post<RmmDevice>(`/api/rmm/devices/${agentId}/fetch-details`);
    return response.data;
  },
  requestSnapshot: async (agentId: string): Promise<RmmSnapshotRequestAccepted> => {
    const client = requireRmmApi();
    const response = await client.post<RmmSnapshotRequestAccepted>(
      `/api/rmm/devices/${agentId}/request-snapshot`,
      {},
      { validateStatus: () => true, timeout: RMM_REQUEST_SNAPSHOT_TIMEOUT_MS }
    );
    if (response.status === 202) return response.data;
    if (response.status === 400) throw new Error('Snapshot limited to once every 30 seconds');
    if (response.status === 404) throw new Error('Agent not connected');
    throw new Error(response.data?.message ?? response.data?.error ?? 'Request failed');
  },
  getSnapshotRequestStatus: async (
    agentId: string,
    requestId: string
  ): Promise<RmmSnapshotRequestStatus> => {
    const response = await api.get<RmmSnapshotRequestStatus>(
      `/rmm/devices/${agentId}/snapshot-requests/${requestId}`
    );
    return response.data;
  },
  getTelemetryStatus: async (agentId: string): Promise<RmmTelemetryStatus> => {
    const response = await api.get<RmmTelemetryStatus>(
      `/rmm/devices/${agentId}/telemetry/status`
    );
    return response.data;
  },
  getTelemetryEvents: async (
    agentId: string,
    limit = 200
  ): Promise<RmmTelemetryEventListResponse> => {
    const response = await api.get<RmmTelemetryEventListResponse>(
      `/rmm/telemetry/read/events/${agentId}`,
      { params: { limit } }
    );
    return response.data;
  },
  getTelemetryFacts: async (agentId: string): Promise<RmmTelemetryFactListResponse> => {
    const response = await api.get<RmmTelemetryFactListResponse>(
      `/rmm/telemetry/read/facts/${agentId}`
    );
    return response.data;
  },
  getTelemetryBaselines: async (agentId: string): Promise<RmmTelemetryBaselineListResponse> => {
    const response = await api.get<RmmTelemetryBaselineListResponse>(
      `/rmm/telemetry/read/baselines/${agentId}`
    );
    return response.data;
  },
  getTelemetryBaselineScopes: async (
    deviceLimit = 300
  ): Promise<RmmTelemetryBaselineScopeCatalogResponse> => {
    const response = await api.get<RmmTelemetryBaselineScopeCatalogResponse>(
      '/rmm/telemetry/read/baselines/scopes',
      { params: { deviceLimit } }
    );
    return response.data;
  },
  getTelemetryScopedBaselines: async (
    scopeType: RmmTelemetryBaselineScopeType,
    scopeId: string,
    options?: {
      factKey?: string;
      onlyUnstable?: boolean;
      limit?: number;
    }
  ): Promise<RmmTelemetryScopedBaselineListResponse> => {
    const response = await api.get<RmmTelemetryScopedBaselineListResponse>(
      '/rmm/telemetry/read/baselines/scope',
      {
        params: {
          scopeType,
          scopeId,
          factKey: options?.factKey?.trim() || undefined,
          onlyUnstable: options?.onlyUnstable ? 'true' : undefined,
          limit: options?.limit
        }
      }
    );
    return response.data;
  },
  getTelemetryScopedBaselineSummary: async (
    scopeType: RmmTelemetryBaselineScopeType,
    scopeId: string
  ): Promise<RmmTelemetryScopedBaselineSummaryResponse> => {
    const response = await api.get<RmmTelemetryScopedBaselineSummaryResponse>(
      `/rmm/telemetry/read/baselines/scope/${scopeType}/${encodeURIComponent(scopeId)}/summary`
    );
    return response.data;
  },
  getTelemetryScopedBaselineDrift: async (
    scopeType: Exclude<RmmTelemetryBaselineScopeType, 'device'>,
    scopeId: string,
    options?: {
      factKey?: string;
      limit?: number;
    }
  ): Promise<RmmTelemetryScopedBaselineDriftListResponse> => {
    const response = await api.get<RmmTelemetryScopedBaselineDriftListResponse>(
      `/rmm/telemetry/read/baselines/scope/${scopeType}/${encodeURIComponent(scopeId)}/drift`,
      {
        params: {
          factKey: options?.factKey?.trim() || undefined,
          limit: options?.limit
        }
      }
    );
    return response.data;
  },
  getTelemetryDecisions: async (
    agentId: string,
    limit = 200,
    matchedRuleId?: string
  ): Promise<RmmTelemetryDecisionListResponse> => {
    const response = await api.get<RmmTelemetryDecisionListResponse>(
      `/rmm/telemetry/read/decisions/${agentId}`,
      { params: { limit, matchedRuleId } }
    );
    return response.data;
  },
  getAlerts: async (options?: {
    status?: string;
    severity?: string;
    agentId?: string;
    customerId?: string;
    siteId?: string;
    q?: string;
    limit?: number;
  }): Promise<RmmTelemetryAlertListResponse> => {
    const response = await api.get<RmmTelemetryAlertListResponse>(
      '/rmm/telemetry/read/alerts',
      { params: options }
    );
    return response.data;
  },
  acknowledgeAlert: async (id: string): Promise<RmmTelemetryAlert> => {
    const response = await api.post<RmmTelemetryAlert>(
      `/rmm/telemetry/alerts/${id}/acknowledge`,
      {}
    );
    return response.data;
  },
  snoozeAlert: async (id: string, minutes = 60): Promise<RmmTelemetryAlert> => {
    const response = await api.post<RmmTelemetryAlert>(
      `/rmm/telemetry/alerts/${id}/snooze`,
      { minutes }
    );
    return response.data;
  },
  resolveAlert: async (id: string): Promise<RmmTelemetryAlert> => {
    const response = await api.post<RmmTelemetryAlert>(
      `/rmm/telemetry/alerts/${id}/resolve`,
      {}
    );
    return response.data;
  },
  suppressAlert: async (id: string, minutes = 1440): Promise<RmmTelemetryAlert> => {
    const response = await api.post<RmmTelemetryAlert>(
      `/rmm/telemetry/alerts/${id}/suppress`,
      { minutes }
    );
    return response.data;
  },
  getAlertRules: async (options?: {
    enabled?: boolean;
    triggerDomain?: string;
  }): Promise<RmmTelemetryAlertRuleListResponse> => {
    const response = await api.get<RmmTelemetryAlertRuleListResponse>(
      '/rmm/telemetry/read/alert-rules',
      { params: options }
    );
    return response.data;
  },
  createAlertRule: async (data: {
    name: string;
    customerId?: string | null;
    siteId?: string | null;
    agentId?: string | null;
    triggerDomain: 'event' | 'baseline' | 'scope_drift' | 'decision' | string;
    triggerKey: string;
    matchOperator?: string;
    matchValue?: string | null;
    severity?: string;
    minSeverity?: string | null;
    dedupeWindowSeconds?: number;
    enabled?: boolean;
    priority?: number;
    notificationChannels?: string[];
  }): Promise<RmmTelemetryAlertRule> => {
    const response = await api.post<RmmTelemetryAlertRule>(
      '/rmm/telemetry/alert-rules',
      data
    );
    return response.data;
  },
  getRoutingRules: async (options?: {
    enabled?: boolean;
    triggerDomain?: string;
    action?: string;
  }): Promise<RmmTelemetryRoutingRuleListResponse> => {
    const response = await api.get<RmmTelemetryRoutingRuleListResponse>(
      '/rmm/telemetry/read/routing-rules',
      { params: options }
    );
    return response.data;
  },
  createRoutingRule: async (data: {
    customerId?: string | null;
    siteId?: string | null;
    agentId?: string | null;
    triggerDomain: 'baseline' | 'scope_drift' | 'event' | string;
    triggerKey: string;
    matchOperator?: RmmTelemetryRoutingMatchOperator | string;
    matchValue?: string | null;
    previousMatchOperator?: RmmTelemetryRoutingMatchOperator | string | null;
    previousMatchValue?: string | null;
    minSupportRatio?: number | null;
    minConfidenceScore?: number | null;
    scopeTypeFilter?: RmmTelemetryBaselineScopeType | string | null;
    action: RmmTelemetryRoutingAction | string;
    intentId?: string | null;
    cooldownSeconds?: number;
    enabled?: boolean;
    priority?: number;
  }): Promise<RmmTelemetryRoutingRule> => {
    const response = await api.post<RmmTelemetryRoutingRule>(
      '/rmm/telemetry/routing-rules',
      data
    );
    return response.data;
  },
  updateRoutingRule: async (id: string, data: Partial<{
    customerId: string | null;
    siteId: string | null;
    agentId: string | null;
    triggerDomain: 'baseline' | 'scope_drift' | 'event' | string;
    triggerKey: string;
    matchOperator: RmmTelemetryRoutingMatchOperator | string;
    matchValue: string | null;
    previousMatchOperator: RmmTelemetryRoutingMatchOperator | string | null;
    previousMatchValue: string | null;
    minSupportRatio: number | null;
    minConfidenceScore: number | null;
    scopeTypeFilter: RmmTelemetryBaselineScopeType | string | null;
    action: RmmTelemetryRoutingAction | string;
    intentId: string | null;
    cooldownSeconds: number;
    enabled: boolean;
    priority: number;
  }>): Promise<RmmTelemetryRoutingRule> => {
    const response = await api.patch<RmmTelemetryRoutingRule>(
      `/rmm/telemetry/routing-rules/${id}`,
      data
    );
    return response.data;
  },
  deleteRoutingRule: async (id: string): Promise<void> => {
    await api.delete(`/rmm/telemetry/routing-rules/${id}`);
  },
  enableRoutingRule: async (id: string): Promise<RmmTelemetryRoutingRule> => {
    const response = await api.post<RmmTelemetryRoutingRule>(
      `/rmm/telemetry/routing-rules/${id}/enable`,
      {}
    );
    return response.data;
  },
  disableRoutingRule: async (id: string): Promise<RmmTelemetryRoutingRule> => {
    const response = await api.post<RmmTelemetryRoutingRule>(
      `/rmm/telemetry/routing-rules/${id}/disable`,
      {}
    );
    return response.data;
  },
  testRoutingRule: async (data: {
    ruleId?: string;
    rule?: Partial<{
      customerId: string | null;
      siteId: string | null;
      agentId: string | null;
      triggerDomain: 'baseline' | 'scope_drift' | 'event' | string;
      triggerKey: string;
      matchOperator: RmmTelemetryRoutingMatchOperator | string;
      matchValue: string | null;
      previousMatchOperator: RmmTelemetryRoutingMatchOperator | string | null;
      previousMatchValue: string | null;
      minSupportRatio: number | null;
      minConfidenceScore: number | null;
      scopeTypeFilter: RmmTelemetryBaselineScopeType | string | null;
      action: RmmTelemetryRoutingAction | string;
      intentId: string | null;
      cooldownSeconds: number;
      enabled: boolean;
      priority: number;
    }>;
    candidate: RmmTelemetryRoutingTestCandidate;
  }): Promise<RmmTelemetryRoutingRuleTestResponse> => {
    const response = await api.post<RmmTelemetryRoutingRuleTestResponse>(
      '/rmm/telemetry/routing-rules/test',
      data
    );
    return response.data;
  },
  getStabilityOverrides: async (): Promise<RmmTelemetryStabilityOverrideListResponse> => {
    const response = await api.get<RmmTelemetryStabilityOverrideListResponse>(
      '/rmm/telemetry/read/stability-overrides'
    );
    return response.data;
  },
  createStabilityOverride: async (data: {
    factKeyPattern: string;
    stabilityClass: 'stable' | 'noisy' | 'ignored';
    reason?: string;
  }): Promise<RmmTelemetryStabilityOverride> => {
    const response = await api.post<RmmTelemetryStabilityOverride>(
      '/rmm/telemetry/stability-overrides',
      data
    );
    return response.data;
  },
  updateStabilityOverride: async (id: string, data: Partial<{
    factKeyPattern: string;
    stabilityClass: 'stable' | 'noisy' | 'ignored';
    reason: string | null;
  }>): Promise<RmmTelemetryStabilityOverride> => {
    const response = await api.patch<RmmTelemetryStabilityOverride>(
      `/rmm/telemetry/stability-overrides/${id}`,
      data
    );
    return response.data;
  },
  deleteStabilityOverride: async (id: string): Promise<void> => {
    await api.delete(`/rmm/telemetry/stability-overrides/${id}`);
  },
  previewStabilityOverride: async (
    factKeyPattern: string,
    limit = 8
  ): Promise<RmmTelemetryStabilityOverridePreviewResponse> => {
    const response = await api.get<RmmTelemetryStabilityOverridePreviewResponse>(
      '/rmm/telemetry/read/stability-overrides/preview',
      { params: { factKeyPattern, limit } }
    );
    return response.data;
  },
  getIntents: async (): Promise<RmmTelemetryIntentListResponse> => {
    const response = await api.get<RmmTelemetryIntentListResponse>(
      '/rmm/telemetry/read/intents'
    );
    return response.data;
  },
  createIntent: async (data: {
    name: string;
    description?: string;
    type?: string;
    allowList?: string[];
    steps?: Array<{ command: string; description?: string; timeout_seconds?: number }>;
    aiPrompt?: string;
    triggerDomain?: string;
    triggerKey?: string;
    requiresApproval?: boolean;
    maxRetries?: number;
    timeoutSeconds?: number;
    enabled?: boolean;
  }): Promise<RmmTelemetryIntent> => {
    const response = await api.post<RmmTelemetryIntent>(
      '/rmm/telemetry/intents',
      data
    );
    return response.data;
  },
  updateIntent: async (id: string, data: Partial<{
    name: string;
    description: string | null;
    type: string;
    allowList: string[] | null;
    steps: Array<{ command: string; description?: string; timeout_seconds?: number }> | null;
    aiPrompt: string | null;
    triggerDomain: string | null;
    triggerKey: string | null;
    requiresApproval: boolean;
    maxRetries: number;
    timeoutSeconds: number;
    enabled: boolean;
  }>): Promise<RmmTelemetryIntent> => {
    const response = await api.patch<RmmTelemetryIntent>(
      `/rmm/telemetry/intents/${id}`,
      data
    );
    return response.data;
  },
  deleteIntent: async (id: string): Promise<void> => {
    await api.delete(`/rmm/telemetry/intents/${id}`);
  },
  getRemediationJobs: async (options?: {
    status?: string;
    limit?: number;
  }): Promise<RmmTelemetryRemediationJobListResponse> => {
    const response = await api.get<RmmTelemetryRemediationJobListResponse>(
      '/rmm/telemetry/read/remediation/jobs',
      { params: options }
    );
    return response.data;
  },
};

export const patchApi = {
  getOverview: async (): Promise<PatchOverviewResponse> => {
    const response = await api.get<PatchOverviewResponse>('/rmm/patches/overview');
    return response.data;
  },
  getDeviceState: async (agentId: string): Promise<PatchDeviceStateResponse> => {
    const response = await api.get<PatchDeviceStateResponse>(`/rmm/patches/devices/${agentId}/state`);
    return response.data;
  },
  getCompliance: async (options?: {
    customerId?: string;
    siteId?: string;
    complianceStatus?: PatchComplianceStatus | 'all';
  }): Promise<PatchComplianceResponse> => {
    const response = await api.get<PatchComplianceResponse>('/rmm/patches/compliance', {
      params: {
        customerId: options?.customerId && options.customerId !== 'all' ? options.customerId : undefined,
        siteId: options?.siteId && options.siteId !== 'all' ? options.siteId : undefined,
        complianceStatus:
          options?.complianceStatus && options.complianceStatus !== 'all'
            ? options.complianceStatus
            : undefined
      }
    });
    return response.data;
  },
  listPolicies: async (): Promise<PatchPolicy[]> => {
    const response = await api.get<PatchPolicyListResponse>('/rmm/patches/policies');
    return response.data.items;
  },
  savePolicy: async (data: {
    scopeType: PatchPolicyScopeType;
    scopeId?: string;
    customerId?: string | null;
    siteId?: string | null;
    agentId?: string | null;
    name: string;
    targetOsFamily?: PatchPolicyTargetOsFamily;
    approvalMode: PatchApprovalMode;
    maintenanceWindowStart?: string | null;
    maintenanceWindowEnd?: string | null;
    maintenanceWindowTimezone?: string | null;
    rebootBehavior: PatchRebootBehavior;
    deferralDays: number;
    managedMode?: boolean;
    nativeWindowsUpdateControl?: boolean;
    policyConfig?: Record<string, unknown>;
    priority?: number;
    enabled: boolean;
  }): Promise<PatchPolicy> => {
    const response = await api.post<PatchPolicy>('/rmm/patches/policies', data);
    return response.data;
  },
  updatePolicy: async (id: string, data: {
    scopeType?: PatchPolicyScopeType;
    scopeId?: string;
    customerId?: string | null;
    siteId?: string | null;
    agentId?: string | null;
    name?: string;
    targetOsFamily?: PatchPolicyTargetOsFamily;
    approvalMode?: PatchApprovalMode;
    maintenanceWindowStart?: string | null;
    maintenanceWindowEnd?: string | null;
    maintenanceWindowTimezone?: string | null;
    rebootBehavior?: PatchRebootBehavior;
    deferralDays?: number;
    managedMode?: boolean;
    nativeWindowsUpdateControl?: boolean;
    policyConfig?: Record<string, unknown>;
    priority?: number;
    enabled?: boolean;
  }): Promise<PatchPolicy> => {
    const response = await api.patch<PatchPolicy>(`/rmm/patches/policies/${id}`, data);
    return response.data;
  },
  deletePolicy: async (id: string): Promise<void> => {
    await api.delete(`/rmm/patches/policies/${id}`);
  },
  approveUpdates: async (data: {
    agentIds?: string[];
    filters?: {
      customerId?: string;
      siteId?: string;
      complianceStatus?: PatchComplianceStatus | 'all';
    };
    updateKeys?: string[];
    decision: PatchDecision;
    reason?: string;
    deferUntil?: string | null;
  }): Promise<PatchApprovalResponse> => {
    const response = await api.post<PatchApprovalResponse>('/rmm/patches/approvals', data);
    return response.data;
  },
  installUpdates: async (data: {
    agentIds?: string[];
    filters?: {
      customerId?: string;
      siteId?: string;
      complianceStatus?: PatchComplianceStatus | 'all';
    };
    updateKeys?: string[];
  }): Promise<PatchInstallResponse> => {
    const response = await api.post<PatchInstallResponse>('/rmm/patches/install', data, {
      timeout: 20000
    });
    return response.data;
  },
  queryProgress: async (agentIds: string[]): Promise<PatchProgressResponse> => {
    const response = await api.post<PatchProgressResponse>('/rmm/patches/progress/query', { agentIds });
    return response.data;
  },
  runAction: async (data: {
    action: string;
    agentIds?: string[];
    updateKeys?: string[];
    kbArticle?: string | null;
    category?: string | null;
    reason?: string;
    deferUntil?: string | null;
    expiresAt?: string | null;
    scopeType?: string;
    scopeKey?: string;
  }): Promise<PatchManualActionResponse> => {
    const response = await api.post<PatchManualActionResponse>('/rmm/patches/actions', data, {
      timeout: 20000
    });
    return response.data;
  }
};

export const featureUpgradeApi = {
  listIsoMedia: async (): Promise<FeatureUpgradeIsoMediaListResponse> => {
    const response = await api.get<FeatureUpgradeIsoMediaListResponse>('/rmm/feature-upgrades/iso-media');
    return response.data;
  },
  previewPreflight: async (agentIds: string[]): Promise<FeatureUpgradePreflightPreviewResponse> => {
    const response = await api.post<FeatureUpgradePreflightPreviewResponse>('/rmm/feature-upgrades/preflight/preview', { agentIds });
    return response.data;
  },
  runPreflight: async (agentIds: string[]): Promise<FeatureUpgradePreflightRunResponse> => {
    const response = await api.post<FeatureUpgradePreflightRunResponse>('/rmm/feature-upgrades/preflight-runs', { agentIds }, {
      timeout: 20000
    });
    return response.data;
  },
  queryPreflightProgress: async (agentIds: string[]): Promise<FeatureUpgradePreflightProgressResponse> => {
    const response = await api.post<FeatureUpgradePreflightProgressResponse>('/rmm/feature-upgrades/preflight/progress/query', { agentIds });
    return response.data;
  },
  previewStageIso: async (agentIds: string[]): Promise<FeatureUpgradeIsoStagePreviewResponse> => {
    const response = await api.post<FeatureUpgradeIsoStagePreviewResponse>('/rmm/feature-upgrades/stage-iso/preview', { agentIds });
    return response.data;
  },
  runStageIso: async (agentIds: string[]): Promise<FeatureUpgradeIsoStageRunResponse> => {
    const response = await api.post<FeatureUpgradeIsoStageRunResponse>('/rmm/feature-upgrades/stage-iso-runs', { agentIds }, {
      timeout: 20000
    });
    return response.data;
  },
  queryStageIsoProgress: async (agentIds: string[]): Promise<FeatureUpgradeIsoStageProgressResponse> => {
    const response = await api.post<FeatureUpgradeIsoStageProgressResponse>('/rmm/feature-upgrades/stage-iso/progress/query', { agentIds });
    return response.data;
  },
  previewStart: async (agentIds: string[]): Promise<FeatureUpgradeStartPreviewResponse> => {
    const response = await api.post<FeatureUpgradeStartPreviewResponse>('/rmm/feature-upgrades/start/preview', { agentIds });
    return response.data;
  },
  runStart: async (agentIds: string[], scheduledFor?: string | null): Promise<FeatureUpgradeStartRunResponse> => {
    const response = await api.post<FeatureUpgradeStartRunResponse>('/rmm/feature-upgrades/start-runs', { agentIds, scheduledFor: scheduledFor ?? null }, {
      timeout: 20000
    });
    return response.data;
  },
  queryStartProgress: async (agentIds: string[]): Promise<FeatureUpgradeStartProgressResponse> => {
    const response = await api.post<FeatureUpgradeStartProgressResponse>('/rmm/feature-upgrades/start/progress/query', { agentIds });
    return response.data;
  }
};

const parseDownloadFilename = (headerValue: string | null | undefined, fallback: string): string => {
  if (!headerValue) return fallback;

  const utf8Match = headerValue.match(/filename\*=UTF-8''([^;]+)/i);
  if (utf8Match?.[1]) {
    try {
      return decodeURIComponent(utf8Match[1]);
    } catch {
      // Fall through to plain filename parsing.
    }
  }

  const plainMatch = headerValue.match(/filename="?([^\";]+)"?/i);
  if (plainMatch?.[1]) {
    return plainMatch[1];
  }
  return fallback;
};

export const reportApi = {
  listDefinitions: async (): Promise<RmmReportDefinition[]> => {
    const response = await api.get<{ items: RmmReportDefinition[] }>('/rmm/reports/definitions');
    return response.data.items;
  },
  generateReport: async (
    reportId: RmmReportId,
    filters?: RmmReportFilters
  ): Promise<RmmReportDataResponse> => {
    const response = await api.get<RmmReportDataResponse>(`/rmm/reports/${reportId}`, {
      params: filters,
      timeout: 20000
    });
    return response.data;
  },
  downloadCsv: async (
    reportId: RmmReportId,
    filters?: RmmReportFilters
  ): Promise<RmmReportDownloadResult> => {
    const response = await api.get<ArrayBuffer>(`/rmm/reports/${reportId}/export.csv`, {
      params: filters,
      responseType: 'arraybuffer',
      timeout: 30000
    });
    const filename = parseDownloadFilename(
      (response.headers['content-disposition'] as string | undefined) ?? null,
      `talos-${reportId}.csv`
    );
    const mimeType = (response.headers['content-type'] as string | undefined) || 'text/csv';
    return {
      filename,
      blob: new Blob([response.data], { type: mimeType })
    };
  },
  listRuns: async (reportId?: RmmReportId): Promise<RmmReportRun[]> => {
    const response = await api.get<{ items: RmmReportRun[] }>('/rmm/reports/runs', {
      params: reportId ? { reportId } : undefined
    });
    return response.data.items;
  },
  createRun: async (data: {
    reportId: RmmReportId;
    format: RmmReportFormat;
    filters?: RmmReportFilters;
  }): Promise<RmmReportRunCreateResponse> => {
    const response = await api.post<RmmReportRunCreateResponse>('/rmm/reports/runs', data, {
      timeout: 30000
    });
    return response.data;
  },
  downloadRunCsv: async (runId: string): Promise<RmmReportDownloadResult> => {
    const response = await api.get<ArrayBuffer>(`/rmm/reports/runs/${runId}/export.csv`, {
      responseType: 'arraybuffer',
      timeout: 30000
    });
    const filename = parseDownloadFilename(
      (response.headers['content-disposition'] as string | undefined) ?? null,
      `talos-report-${runId}.csv`
    );
    const mimeType = (response.headers['content-type'] as string | undefined) || 'text/csv';
    return {
      filename,
      blob: new Blob([response.data], { type: mimeType })
    };
  },
  listSchedules: async (): Promise<RmmReportSchedule[]> => {
    const response = await api.get<{ items: RmmReportSchedule[] }>('/rmm/reports/schedules');
    return response.data.items;
  },
  createSchedule: async (data: {
    name?: string;
    reportId: RmmReportId;
    format: RmmReportFormat;
    frequency: RmmReportFrequency;
    filters?: RmmReportFilters;
    emailTo?: string[];
  }): Promise<RmmReportSchedule> => {
    const response = await api.post<RmmReportSchedule>('/rmm/reports/schedules', data);
    return response.data;
  },
  deleteSchedule: async (id: string): Promise<void> => {
    await api.delete(`/rmm/reports/schedules/${id}`);
  }
};

export const installerApi = {
  listProfiles: async (params?: {
    scopeType?: RmmInstallerScopeType;
    customerId?: string;
    siteId?: string;
  }): Promise<RmmInstallerProfile[]> => {
    const response = await api.get<RmmInstallerProfile[]>('/rmm/installers/profiles', {
      params: {
        scopeType: params?.scopeType,
        customerId: params?.customerId,
        siteId: params?.siteId
      }
    });
    return response.data;
  },
  createProfile: async (data: {
    name?: string;
    scopeType: RmmInstallerScopeType;
    customerId?: string;
    siteId?: string;
    expiresAt?: string | null;
    maxUses?: number | null;
  }): Promise<RmmInstallerProfileCreateResponse> => {
    const response = await api.post<RmmInstallerProfileCreateResponse>('/rmm/installers/profiles', data);
    return response.data;
  },
  issueDownload: async (
    profileId: string,
    data?: {
      expiresAt?: string | null;
      maxUses?: number | null;
    }
  ): Promise<RmmInstallerDownloadResponse> => {
    const response = await api.post<RmmInstallerDownloadResponse>(
      `/rmm/installers/profiles/${profileId}/download`,
      data ?? {}
    );
    return response.data;
  },
  issueLinuxInstaller: async (
    profileId: string,
    data?: {
      expiresAt?: string | null;
      maxUses?: number | null;
    }
  ): Promise<RmmLinuxInstallerResponse> => {
    const response = await api.post<RmmLinuxInstallerResponse>(
      `/rmm/installers/profiles/${profileId}/linux-install`,
      data ?? {}
    );
    return response.data;
  },
  issueMacosInstaller: async (
    profileId: string,
    data?: {
      expiresAt?: string | null;
      maxUses?: number | null;
    }
  ): Promise<RmmMacosInstallerResponse> => {
    const response = await api.post<RmmMacosInstallerResponse>(
      `/rmm/installers/profiles/${profileId}/macos-install`,
      data ?? {}
    );
    return response.data;
  },
  getViewerInstallerInfo: async (platform: ViewerInstallerPlatform = 'windows'): Promise<ViewerInstallerInfo> => {
    const response = await api.get<ViewerInstallerInfo>('/rmm/installers/viewer', {
      params: { platform }
    });
    return response.data;
  },
  getLinuxAgentInfo: async (): Promise<LinuxAgentInstallerInfo> => {
    const response = await api.get<LinuxAgentInstallerInfo>('/rmm/installers/linux/agent');
    return response.data;
  },
  getMacosPackageInfo: async (): Promise<MacosPackageInstallerInfo> => {
    const response = await api.get<MacosPackageInstallerInfo>('/rmm/installers/macos/package');
    return response.data;
  },
  downloadMacosPackage: async (
    profileId: string,
    data?: {
      expiresAt?: string | null;
      maxUses?: number | null;
    }
  ): Promise<RmmInstallerExeDownloadResult> => {
    const response = await api.post<ArrayBuffer>(
      `/rmm/installers/profiles/${profileId}/download-macos-pkg`,
      data ?? {},
      { responseType: 'arraybuffer' }
    );

    const filenameHeader =
      (response.headers['x-installer-filename'] as string | undefined) ||
      (response.headers['content-disposition'] as string | undefined);
    const filename = parseDownloadFilename(filenameHeader, 'Talos.Agent.macos-universal.pkg');
    const mimeType =
      (response.headers['content-type'] as string | undefined) ||
      'application/octet-stream';

    return {
      filename,
      blob: new Blob([response.data], { type: mimeType })
    };
  },
  downloadViewerInstaller: async (
    platform: ViewerInstallerPlatform = 'windows'
  ): Promise<RmmInstallerExeDownloadResult> => {
    const response = await api.get<ArrayBuffer>('/rmm/installers/viewer/download', {
      responseType: 'arraybuffer',
      params: { platform }
    });

    const filenameHeader =
      (response.headers['x-installer-filename'] as string | undefined) ||
      (response.headers['content-disposition'] as string | undefined);
    const fallbackFilename =
      platform === 'macos' ? 'Talos.Viewer.macos.pkg' : 'talos-viewer-installer.msi';
    const filename = parseDownloadFilename(filenameHeader, fallbackFilename);
    const mimeType =
      (response.headers['content-type'] as string | undefined) ||
      (platform === 'macos' ? 'application/octet-stream' : 'application/x-msi');

    return {
      filename,
      blob: new Blob([response.data], { type: mimeType })
    };
  },
  revokeProfile: async (profileId: string): Promise<RmmInstallerProfile & { revoked: boolean }> => {
    const response = await api.post<RmmInstallerProfile & { revoked: boolean }>(
      `/rmm/installers/profiles/${profileId}/revoke`,
      {}
    );
    return response.data;
  }
};

// Auth utilities
export const authUtils = {
  setToken: (token: string) => {
    if (browser) {
      localStorage.setItem('token', token);
    }
  },
  
  getToken: (): string | null => {
    if (browser) {
      return localStorage.getItem('token');
    }
    return null;
  },
  
  removeToken: () => {
    if (browser) {
      localStorage.removeItem('token');
    }
  },
  
  isAuthenticated: (): boolean => {
    return !!authUtils.getToken();
  },
};

export const secureNotesApi = {
  check: async (code: string): Promise<SecureNoteCheckResponse> => {
    try {
      const response = await api.get<SecureNoteCheckResponse>(
        `/secure-notes/${encodeURIComponent(code)}/check`
      );
      return response.data;
    } catch (error: any) {
      const status = error?.statusCode;
      const message = error?.message || 'Secure note unavailable';
      const responseStatus = error?.data?.status;
      if (status === 403) return { status: responseStatus || 'unauthorized', error: message };
      if (status === 404) return { status: responseStatus || 'not_found', error: message };
      if (status === 400) return { status: responseStatus || 'invalid', error: message };
      throw error;
    }
  },
  reveal: async (code: string): Promise<SecureNoteRevealResponse> => {
    try {
      const response = await api.post<SecureNoteRevealResponse>(
        `/secure-notes/${encodeURIComponent(code)}/reveal`
      );
      return response.data;
    } catch (error: any) {
      const status = error?.statusCode;
      const message = error?.message || 'Secure note unavailable';
      const responseStatus = error?.data?.status;
      if (status === 403) return { status: responseStatus || 'unauthorized', error: message };
      if (status === 404) return { status: responseStatus || 'not_found', error: message };
      if (status === 400) return { status: responseStatus || 'invalid', error: message };
      throw error;
    }
  },
};

export default api;
