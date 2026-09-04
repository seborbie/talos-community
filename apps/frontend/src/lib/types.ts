export interface User {
  id: string;
  email: string;
  createdAt: string;
}

export interface AuthResponse {
  token: string;
  user: User;
}

export interface LoginRequest {
  email: string;
  password: string;
}

export interface RegisterRequest {
  email: string;
  password: string;
}

export interface RegistrationStatus {
  registrationOpen: boolean;
  mode: 'first_user' | 'closed';
}

export interface ApiError {
  message: string;
  statusCode: number;
}

export type OrgRole = 'SUPER_ADMIN' | 'AGENT_ADMIN' | 'VIEWER';

export interface Organization {
  id: string;
  name: string;
  createdAt: string;
}

export interface Customer {
  id: string;
  organizationId: string;
  name: string;
  description?: string | null;
  isUnassigned: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface Site {
  id: string;
  customerId: string;
  customerName?: string;
  name: string;
  timezone?: string | null;
  createdAt: string;
  updatedAt: string;
  deviceCount?: number;
}

export interface CommandPolicy {
  id: string;
  commandName: string;
  scopeType: 'global' | 'organization' | 'customer' | 'role';
  organizationId?: string | null;
  customerId?: string | null;
  roleScope?: OrgRole | null;
  policyType: 'allow' | 'deny';
  description?: string | null;
  reason?: string | null;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export interface CreatePolicyRequest {
  commandName: string;
  scopeType: 'organization' | 'customer' | 'role';
  customerId?: string;
  roleScope?: OrgRole;
  policyType: 'allow' | 'deny';
  description?: string;
  reason?: string;
}

export type CommandCenterChatRole = 'user' | 'assistant';

export interface CommandCenterChatMessage {
  role: CommandCenterChatRole;
  content: string;
}

export interface CommandCenterMessageAttachment {
  id: string;
  type: 'image';
  mimeType: string;
  name: string;
  artifactId: string;
  width?: number;
  height?: number;
  presentation?: 'inline' | 'live_frame';
  jobId?: string;
  frameSeq?: number;
  cursor?: {
    visible: boolean;
    x?: number;
    y?: number;
    width: number;
    height: number;
  };
}

export interface CommandCenterAiRunnerEvidence {
  jobId: string;
  jobType: string;
  status: CommandCenterAiRunnerJobStatus;
  shellTranscriptAvailable: boolean;
  desktopReplayAvailable: boolean;
  replayFrameCount: number;
}

export type CommandCenterCommandApprovalStatus =
  | 'pending'
  | 'approved'
  | 'denied'
  | 'desktop_control_requested'
  | 'executing'
  | 'executed'
  | 'failed'
  | 'expired'
  | 'policy_blocked';

export interface CommandCenterCommandApproval {
  id: string;
  jobId: string;
  turnIndex: number;
  status: CommandCenterCommandApprovalStatus;
  command: string;
  explanation: string;
  risk: string;
  notes: string[];
  message: string | null;
  policyAllowed: boolean | null;
  policyReason: string | null;
  output: string | null;
  outputLength: number | null;
  exitCode: number | null;
  error: string | null;
  updatedAt: string;
}

export interface CommandCenterChatRequest {
  messages: CommandCenterChatMessage[];
  conversationId?: string | null;
}

export interface CommandCenterChatResponse {
  content: string;
  model: string;
  responseId: string | null;
  conversationId: string;
  attachments?: CommandCenterMessageAttachment[];
}

export interface CommandCenterChatStatusEvent {
  phase: 'thinking' | 'tool';
  message: string;
}

export interface CommandCenterChatDeltaEvent {
  delta: string;
}

export interface CommandCenterChatConversationEvent {
  conversationId: string;
}

export interface CommandCenterConversationSummary {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface CommandCenterStoredMessage {
  id: string;
  role: CommandCenterChatRole;
  content: string;
  model: string | null;
  responseId: string | null;
  metadata: unknown | null;
  createdAt: string;
}

export type CommandCenterAiRunnerJobStatus =
  | 'queued'
  | 'approval_pending'
  | 'approval_granted'
  | 'approval_denied'
  | 'approval_expired'
  | 'running'
  | 'succeeded'
  | 'failed'
  | 'stopping'
  | 'stopped';

export interface CommandCenterAiRunnerJob {
  id: string;
  organizationId: string;
  userId: string;
  conversationId: string | null;
  agentId: string;
  jobType: string;
  status: CommandCenterAiRunnerJobStatus;
  runnerId: string | null;
  approvalId: string | null;
  approvalChatSessionId: string | null;
  approvalRequestedAt: string | null;
  approvalRespondedAt: string | null;
  approvalExpiresAt: string | null;
  approvalWindowExpiresAt: string | null;
  resultMessageId: string | null;
  liveFrameMessageId: string | null;
  result: unknown | null;
  error: string | null;
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  deviceLabel: string | null;
  attachments: CommandCenterMessageAttachment[];
  pendingCommandApproval?: CommandCenterCommandApproval | null;
  latestCommandApproval?: CommandCenterCommandApproval | null;
  evidence?: CommandCenterAiRunnerEvidence | null;
}

export interface CommandCenterAiRunnerReplayFrame {
  artifactId: string;
  frameSeq: number | null;
  width: number | null;
  height: number | null;
  cursor: CommandCenterMessageAttachment['cursor'] | null;
  stepIndex: number | null;
  taskId: string | null;
  displayText: string;
  createdAt: string;
}

export interface CommandCenterAiRunnerReplayManifest {
  jobId: string;
  jobType: string;
  status: CommandCenterAiRunnerJobStatus;
  deviceLabel: string | null;
  goal: string | null;
  startedAt: string | null;
  finishedAt: string | null;
  defaultDelayMs: number;
  frames: CommandCenterAiRunnerReplayFrame[];
}

export interface CommandCenterAiRunnerOutputDelta {
  eventId: string;
  jobId: string;
  approvalId: string;
  turnIndex: number | null;
  sequence: number;
  text: string;
  outputOffset: number;
  terminal: boolean;
  createdAt: string;
}

export interface CommandCenterAiRunnerStreamSnapshot {
  jobs: CommandCenterAiRunnerJob[];
  output: CommandCenterAiRunnerOutputDelta[];
}

export interface HaloConfig {
  baseUrl: string;
  clientId: string;
  clientSecret: string;
}

export interface OrganizationMember {
  id: string;
  userId: string;
  organizationId: string;
  role: OrgRole;
  email?: string;
}

export interface RmmDevice {
  agentId: string;
  hostname: string;
  os: string;
  ip: string;
  version?: string | null;
  lastSeen: string;
  websocketStatus?: 'connected' | 'disconnected' | 'unknown' | string | null;
  websocketConnectedAt?: string | null;
  websocketDisconnectedAt?: string | null;
  lastInventory?: Record<string, unknown> | null;
  deviceDetails?: Record<string, unknown> | null;
  customerId?: string | null;
  customerName?: string | null;
  siteId?: string | null;
  siteName?: string | null;
  pendingUpdatesCount?: number | null;
  rebootRequired?: boolean | null;
  agentVersion?: string | null;
  osName?: string | null;
  osVersion?: string | null;
  alertSeverity?: 'info' | 'warning' | 'error' | 'critical' | string | null;
  tags?: string[];
  aiRunnerAutoApprove?: boolean;
  linuxShellUsername?: string | null;
  hasLinuxShellCredential?: boolean;
  macosUpdateAccount?: MacosUpdateAccountStatus | null;
  health?: RmmAgentHealthSummary | null;
  activeHealthAlerts?: RmmAgentHealthAlert[];
}

export type RmmAgentHealthStatus = 'healthy' | 'warning' | 'critical' | 'offline';
export type RmmAgentHealthSeverity = 'info' | 'warning' | 'critical';

export interface RmmAgentHealthReason {
  code: string;
  severity: RmmAgentHealthSeverity | string;
  summary: string;
  detail: string | null;
  observedAt: string | null;
  ageMs: number | null;
  alertKey: string;
}

export interface RmmAgentHealthSummary {
  status: RmmAgentHealthStatus | string;
  severity: RmmAgentHealthSeverity | string;
  summary: string;
  reasons: RmmAgentHealthReason[];
  computedAt: string;
  signals: {
    websocketStatus: 'connected' | 'disconnected' | 'unknown' | string;
    lastSeenAt: string | null;
    telemetryCollectedAt: string | null;
    agentVersion: string | null;
    targetVersion: string | null;
    commandFailureCount: number;
    updaterFailureCount: number;
    remediationFailureCount: number;
  };
}

export interface RmmAgentHealthAlert {
  id: string;
  agentId: string;
  alertKey: string;
  severity: string;
  status: string;
  reason: string;
  detail: string | null;
  firstSeenAt: string;
  lastSeenAt: string;
  resolvedAt: string | null;
  occurrenceCount: number;
}

export type RmmDeviceListSortBy =
  | 'hostname'
  | 'customer'
  | 'site'
  | 'os'
  | 'version'
  | 'lastSeen'
  | 'status'
  | 'pendingUpdates'
  | 'rebootRequired'
  | 'alertSeverity';

export type RmmDeviceListSortDirection = 'asc' | 'desc';
export type RmmDeviceListStatusFilter = 'all' | 'online' | 'offline';
export type RmmDeviceListAlertSeverity = 'info' | 'warning' | 'error' | 'critical';

export interface RmmDeviceListFilters {
  q?: string;
  customerId?: string;
  siteId?: string;
  status: RmmDeviceListStatusFilter;
  os?: string;
  version?: string;
  tag?: string;
  pendingUpdates?: boolean | null;
  rebootRequired?: boolean | null;
  alertSeverity?: RmmDeviceListAlertSeverity | null;
  lastSeenAgeMinutes?: number | null;
}

export interface RmmDeviceListQuery {
  page: number;
  pageSize: number;
  sortBy: RmmDeviceListSortBy;
  sortDirection: RmmDeviceListSortDirection;
  filters: RmmDeviceListFilters;
}

export interface RmmDeviceListResponse extends RmmDeviceListQuery {
  items: RmmDevice[];
  total: number;
}

export interface RmmDeviceSavedView {
  id: string;
  organizationId: string;
  userId: string;
  name: string;
  filters: RmmDeviceListFilters;
  sortBy: RmmDeviceListSortBy;
  sortDirection: RmmDeviceListSortDirection;
  pageSize: number;
  createdAt: string;
  updatedAt: string;
}

export interface MacosVolumeOwnerUser {
  username?: string | null;
  fullName?: string | null;
  generatedUid?: string | null;
  volumeOwner: boolean;
}

export interface MacosUpdateAccountStatus {
  schemaVersion?: number;
  required?: boolean | null;
  status?: string | null;
  username?: string | null;
  isAppleSilicon?: boolean;
  accountPresent?: boolean;
  isAdmin?: boolean;
  isVolumeOwner?: boolean;
  secureTokenEnabled?: boolean;
  credentialAvailable?: boolean;
  credentialVersion?: number | null;
  generatedUid?: string | null;
  expectedGeneratedUid?: string | null;
  discoveredVolumeOwners?: MacosVolumeOwnerUser[];
  failureCode?: string | null;
  failureMessage?: string | null;
  checkedAt?: string | null;
}

export interface LinuxShellCredential {
  agentId: string;
  username: string;
  password: string;
  credentialId?: string | null;
  version?: number | null;
  updatedAt?: string | null;
}

export interface RmmConnectResponse {
  url: string;
  sessionId: string;
}

export type RmmViewerSessionKind =
  | 'remote_desktop'
  | 'shell'
  | 'file_transfer'
  | 'remote_registry'
  | 'chat';

export type RmmViewerSessionState = 'pending' | 'connected';

export interface RmmViewerSessionStatus {
  sessionId: string;
  kind: RmmViewerSessionKind;
  agentId: string;
  userId: string | null;
  userEmail: string | null;
  state: RmmViewerSessionState;
  connected: boolean;
  attached: boolean;
  connectedAt: string | null;
  lastHeartbeatAt: string | null;
}

export interface RmmViewerConnectionSummary {
  sessionId: string;
  kind: RmmViewerSessionKind;
  agentId: string;
  userId: string | null;
  userEmail: string | null;
  connectedAt: string | null;
  lastHeartbeatAt: string | null;
}

export interface RmmCommandExecutionLogEntry {
  id: string;
  createdAt: string;
  customerId: string | null;
  userId: string;
  userEmail: string | null;
  agentId: string;
  command: string;
  wasAllowed: boolean;
  denialReason: string | null;
  matchedPolicyId: string | null;
  executionTimeMs: number | null;
  exitCode: number | null;
  outputLength: number | null;
}

export interface RmmCommandExecutionLogListResponse {
  items: RmmCommandExecutionLogEntry[];
  nextCursor: string | null;
}

export type RmmReportId =
  | 'fleet_health'
  | 'patch_compliance'
  | 'device_inventory'
  | 'software_inventory'
  | 'alert_history'
  | 'uptime_offline'
  | 'command_remediation_outcomes'
  | 'remote_support_activity';

export type RmmReportFormat = 'json' | 'csv' | 'pdf';
export type RmmReportFrequency = 'daily' | 'weekly' | 'monthly';

export interface RmmReportColumn {
  key: string;
  label: string;
}

export interface RmmReportDefinition {
  id: RmmReportId;
  name: string;
  description: string;
  category: 'health' | 'inventory' | 'operations';
  formats: RmmReportFormat[];
  columns: RmmReportColumn[];
}

export interface RmmReportFilters {
  from?: string | null;
  to?: string | null;
  customerId?: string | null;
  siteId?: string | null;
  limit?: number | string | null;
  offlineMinutes?: number | string | null;
}

export interface RmmReportRun {
  id: string;
  organizationId: string;
  reportId: RmmReportId;
  format: RmmReportFormat;
  filters: RmmReportFilters;
  status: string;
  rowCount: number;
  generatedBy: string | null;
  deliveryStatus: string;
  errorMessage: string | null;
  startedAt: string;
  finishedAt: string | null;
  createdAt: string;
}

export interface RmmReportSchedule {
  id: string;
  organizationId: string;
  reportId: RmmReportId;
  name: string;
  format: RmmReportFormat;
  frequency: RmmReportFrequency;
  filters: RmmReportFilters;
  emailTo: string[];
  emailDeliveryStatus: string;
  isEnabled: boolean;
  lastRunAt: string | null;
  nextRunAt: string | null;
  createdBy: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface RmmReportDataResponse {
  definition: RmmReportDefinition;
  filters: RmmReportFilters;
  items: Array<Record<string, unknown>>;
}

export interface RmmReportRunCreateResponse {
  run: RmmReportRun;
  definition: RmmReportDefinition;
  previewRows: Array<Record<string, unknown>>;
  downloadUrl: string | null;
  pdfStubbed: boolean;
}

export interface RmmReportDownloadResult {
  filename: string;
  blob: Blob;
}

export type AuditActorType = 'user' | 'machine' | 'agent' | 'service' | 'system' | 'unknown' | string;
export type AuditResult = 'success' | 'failure' | 'blocked' | string;

export interface AuditEvent {
  id: string;
  organizationId: string | null;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  actorType: AuditActorType;
  userId: string | null;
  userEmail: string | null;
  actionType: string;
  targetType: string;
  targetId: string | null;
  targetName: string | null;
  result: AuditResult;
  statusCode: number | null;
  errorMessage: string | null;
  requestMethod: string | null;
  requestPath: string | null;
  clientIp: string | null;
  userAgent: string | null;
  correlationId: string | null;
  sessionId: string | null;
  metadata: Record<string, unknown>;
  occurredAt: string;
  createdAt: string;
}

export interface AuditEventListResponse {
  items: AuditEvent[];
  nextCursor: string | null;
}

export interface RmmSnapshotRequestAccepted {
  requestId: string;
  status: 'pending' | 'completed' | 'failed' | string;
  message?: string;
  error?: string;
}

export interface RmmSnapshotRequestStatus {
  requestId: string;
  status: 'pending' | 'completed' | 'failed' | string;
}

export interface RmmTelemetryStatus {
  collectedAt: string | null;
}

// Device telemetry (from GET /rmm/devices/:agentId/telemetry)
export interface RmmDeviceState {
  collectedAt: string;
  hostname: string | null;
  osName: string | null;
  osVersion: string | null;
  agentVersion: string | null;
  bootSessionId: string | null;
  cpuModel: string | null;
  cpuPhysicalCores: number | null;
  cpuLogicalCores: number | null;
  cpuBaseMhz: number | null;
  memoryTotalBytes: number | null;
  installedAppsCount: number | null;
  pendingUpdatesCount: number | null;
  rebootRequired: boolean | null;
  inventoryData?: Record<string, unknown> | null;
}

export interface RmmInstalledApp {
  appName: string;
  publisher: string | null;
  version: string | null;
  installDate: string | null;
  sizeBytes: number | null;
  source: string | null;
  location: string | null;
  is64Bit: boolean | null;
}

export interface RmmDeviceService {
  serviceName: string;
  displayName: string;
  status: string;
  startType: string | null;
  account: string | null;
  processId: number | null;
  isCritical: boolean | null;
  description: string | null;
  path: string | null;
}

export interface RmmStartupItem {
  itemName: string;
  command: string;
  location: string;
  userName: string | null;
  isEnabled: boolean | null;
}

export interface RmmWindowsFeature {
  featureName: string;
  displayName: string;
  installState: string | null;
  enabled: boolean | null;
}

export interface RmmPendingUpdate {
  title: string;
  description: string | null;
  kbArticle: string | null;
  isMandatory: boolean | null;
  sizeBytes: number | null;
  requiresReboot: boolean | null;
}

export interface RmmInstalledUpdate {
  installedAt: string | null;
  title: string;
  kbArticle: string | null;
  operation: string | null;
  result: string | null;
  hresult: number | null;
}

export interface RmmDeviceTelemetry {
  deviceState: RmmDeviceState | null;
  installedApps: RmmInstalledApp[];
  services: RmmDeviceService[];
  startupItems: RmmStartupItem[];
  windowsFeatures: RmmWindowsFeature[];
  pendingUpdates: RmmPendingUpdate[];
  installedUpdates: RmmInstalledUpdate[];
}

export interface RmmTelemetryGraphEvent {
  eventId: string;
  occurredAt: string;
  receivedAt: string;
  eventType: string;
  severity: string;
  source: string;
  serviceName: string | null;
  processName: string | null;
  code: string | null;
  message: string | null;
  attributes: Record<string, unknown> | null;
  createdAt: string;
}

export interface RmmTelemetryFact {
  factKey: string;
  factValue: unknown;
  factValueText: string;
  stabilityClass: 'stable' | 'noisy' | string;
  source: string;
  sourceTs: string;
  updatedAt: string;
}

export interface RmmTelemetryBaseline {
  factKey: string;
  promotedValue: unknown;
  candidateValue: unknown;
  candidateCount: number;
  windowCount: number;
  lastChangedAt: string | null;
  updatedAt: string;
}

export type RmmTelemetryBaselineScopeType = 'organization' | 'customer' | 'site' | 'device';

export interface RmmTelemetryScopedBaseline {
  factKey: string;
  promotedValue: unknown;
  candidateValue: unknown;
  candidateCount: number;
  windowCount: number;
  supportCount: number;
  totalCount: number;
  supportRatio: number;
  sampleSize: number;
  confidenceScore: number;
  isStable: boolean;
  lastChangedAt: string | null;
  updatedAt: string;
  effectiveStabilityClass: 'stable' | 'noisy' | 'ignored' | string | null;
  baselineEligible: boolean;
  promotionState: string;
  overrideMatched: boolean;
  overrideId: string | null;
  overridePattern: string | null;
  overrideStabilityClass: 'stable' | 'noisy' | 'ignored' | string | null;
  overrideReason: string | null;
  trustWarnings: string[];
}

export interface RmmTelemetryScopedBaselineScope {
  scopeType: RmmTelemetryBaselineScopeType;
  scopeId: string;
  scopeName: string;
}

export interface RmmTelemetryScopedBaselineListResponse {
  scope: RmmTelemetryScopedBaselineScope;
  items: RmmTelemetryScopedBaseline[];
}

export interface RmmTelemetryBaselineScopeCatalogResponse {
  organization: {
    id: string;
    name: string;
    baselineCount: number;
  };
  customers: Array<{
    id: string;
    name: string;
    deviceCount: number;
    baselineCount: number;
  }>;
  sites: Array<{
    id: string;
    name: string;
    timezone: string | null;
    customerId: string;
    customerName: string;
    deviceCount: number;
    baselineCount: number;
  }>;
  devices: Array<{
    agentId: string;
    hostname: string;
    customerId: string | null;
    customerName: string | null;
    siteId: string | null;
    siteName: string | null;
  }>;
  totals: {
    deviceCount: number;
    customerCount: number;
    siteCount: number;
  };
}

export interface RmmTelemetryScopedBaselineSummaryResponse {
  scope: RmmTelemetryScopedBaselineScope;
  summary: {
    totalFacts: number;
    stableFacts: number;
    unstableFacts: number;
    avgSupportRatio: number;
    avgConfidenceScore: number;
    latestUpdatedAt: string | null;
  };
}

export interface RmmTelemetryScopedBaselineDriftListResponse {
  scope: RmmTelemetryScopedBaselineScope;
  items: Array<{
    agentId: string;
    hostname: string;
    customerId: string | null;
    customerName: string | null;
    siteId: string | null;
    siteName: string | null;
    factKey: string;
    scopeValue: unknown;
    deviceValue: unknown;
    deviceUpdatedAt: string | null;
    scopeUpdatedAt: string;
    scopeSampleSize: number;
    scopeSupportRatio: number;
    scopeConfidenceScore: number;
    scopeIsStable: boolean;
    effectiveStabilityClass: 'stable' | 'noisy' | 'ignored' | string | null;
    baselineEligible: boolean;
    promotionState: string;
    overrideMatched: boolean;
    overrideId: string | null;
    overridePattern: string | null;
    overrideStabilityClass: 'stable' | 'noisy' | 'ignored' | string | null;
    overrideReason: string | null;
    trustWarnings: string[];
  }>;
}

export interface RmmTelemetryDecision {
  id: string;
  domain: string;
  triggerKey: string;
  triggerValue: unknown;
  action: 'ignore' | 'ticket' | 'recommend' | 'auto_remediate' | 'llm_router' | string;
  matchedRuleId: string | null;
  intentId: string | null;
  reason: string | null;
  dedupeKey: string | null;
  source: string;
  sourceTs: string;
  decidedAt: string;
  executionStatus: 'pending' | 'completed' | 'failed' | 'skipped' | string;
  externalRef: string | null;
  outcomeMessage: string | null;
}

export interface RmmTelemetryEventListResponse {
  items: RmmTelemetryGraphEvent[];
}

export interface RmmTelemetryFactListResponse {
  items: RmmTelemetryFact[];
}

export interface RmmTelemetryBaselineListResponse {
  items: RmmTelemetryBaseline[];
}

export interface RmmTelemetryDecisionListResponse {
  items: RmmTelemetryDecision[];
}

export type RmmAlertStatus = 'open' | 'acknowledged' | 'snoozed' | 'resolved' | 'suppressed';
export type RmmAlertSeverity = 'critical' | 'high' | 'medium' | 'low' | 'info';

export interface RmmTelemetryAlert {
  id: string;
  organizationId: string;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  agentId: string;
  hostname: string | null;
  ruleId: string | null;
  status: RmmAlertStatus | string;
  severity: RmmAlertSeverity | string;
  sourceDomain: 'event' | 'baseline' | 'scope_drift' | 'decision' | string;
  sourceKey: string;
  sourceEventId: string | null;
  sourceFactKey: string | null;
  sourceDecisionId: string | null;
  title: string;
  summary: string | null;
  fingerprint: string;
  firstSeenAt: string;
  lastSeenAt: string;
  occurrenceCount: number;
  ownerUserId: string | null;
  ownerEmail: string | null;
  acknowledgedBy: string | null;
  acknowledgedAt: string | null;
  snoozedUntil: string | null;
  resolvedBy: string | null;
  resolvedAt: string | null;
  suppressedUntil: string | null;
  metadata: Record<string, unknown> | null;
  createdAt: string;
  updatedAt: string;
}

export interface RmmTelemetryAlertListResponse {
  items: RmmTelemetryAlert[];
  filters: {
    status: RmmAlertStatus | 'all' | string;
    severity: RmmAlertSeverity | 'all' | string;
  };
}

export interface RmmTelemetryAlertRule {
  id: string;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  name: string;
  triggerDomain: 'event' | 'baseline' | 'scope_drift' | 'decision' | string;
  triggerKey: string;
  matchOperator: RmmTelemetryRoutingMatchOperator | string;
  matchValue: string | null;
  severity: RmmAlertSeverity | string;
  minSeverity: RmmAlertSeverity | string | null;
  dedupeWindowSeconds: number;
  enabled: boolean;
  priority: number;
  notificationChannels: Array<'email' | 'webhook' | 'psa' | string>;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export interface RmmTelemetryAlertRuleListResponse {
  items: RmmTelemetryAlertRule[];
}

export type RmmTelemetryRoutingAction =
  | 'ignore'
  | 'ticket'
  | 'recommend'
  | 'auto_remediate'
  | 'llm_router';

export type RmmTelemetryRoutingMatchOperator =
  | 'equals'
  | 'not_equals'
  | 'contains'
  | 'not_contains'
  | 'starts_with'
  | 'ends_with'
  | 'exists';

export interface RmmTelemetryRoutingRule {
  id: string;
  organizationId: string;
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
  createdAt: string | null;
  updatedAt: string | null;
  specificity: 'agent' | 'site' | 'customer' | 'organization';
  blockedReasons: string[];
  readiness: {
    intentReady: boolean;
    intentRequiresApproval: boolean | null;
    ticketProviderReady: boolean;
    llmRouterEnabled: boolean;
  };
}

export interface RmmTelemetryRoutingRuleListResponse {
  items: RmmTelemetryRoutingRule[];
}

export interface RmmTelemetryRoutingTestCandidate {
  domain: 'baseline' | 'scope_drift' | 'event' | string;
  triggerKey: string;
  currentValue: unknown;
  currentValueText?: string;
  previousValue?: unknown;
  previousValueText?: string | null;
  supportRatio?: number | null;
  confidenceScore?: number | null;
  scopeType?: RmmTelemetryBaselineScopeType | null;
  organizationId?: string | null;
  customerId?: string | null;
  siteId?: string | null;
  agentId?: string | null;
}

export interface RmmTelemetryRoutingRuleTestResponse {
  wouldMatch: boolean;
  cooldownBlocked: boolean;
  action: RmmTelemetryRoutingAction | string;
  dedupeKey: string | null;
  blockedReasons: string[];
  explanation: string[];
  readiness: {
    intentReady: boolean;
    intentRequiresApproval: boolean | null;
    ticketProviderReady: boolean;
    llmRouterEnabled: boolean;
  };
  rule: RmmTelemetryRoutingRule;
  candidate: RmmTelemetryRoutingTestCandidate;
}

export interface RmmTelemetryStabilityOverride {
  id: string;
  factKeyPattern: string;
  stabilityClass: 'stable' | 'noisy' | 'ignored';
  reason: string | null;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
  matchedFactKeyCount: number;
  matchedCurrentFactCount: number;
  matchedScopedBaselineCount: number;
  sampleFactKeys: string[];
}

export interface RmmTelemetryStabilityOverrideListResponse {
  items: RmmTelemetryStabilityOverride[];
}

export interface RmmTelemetryStabilityOverridePreviewResponse {
  factKeyPattern: string;
  matchedFactKeyCount: number;
  matchedCurrentFactCount: number;
  matchedScopedBaselineCount: number;
  items: Array<{
    factKey: string;
    currentFactCount: number;
    scopedBaselineCount: number;
    latestSeenAt: string | null;
  }>;
}

export interface RmmTelemetryIntent {
  id: string;
  name: string;
  description: string | null;
  type: 'hardcoded' | 'ai_planned' | string;
  allowList: string[] | null;
  steps: Array<{ command: string; description?: string; timeout_seconds?: number }> | null;
  aiPrompt: string | null;
  triggerDomain: string | null;
  triggerKey: string | null;
  requiresApproval: boolean;
  maxRetries: number;
  timeoutSeconds: number;
  enabled: boolean;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export interface RmmTelemetryIntentListResponse {
  items: RmmTelemetryIntent[];
}

export interface RmmTelemetryRemediationJob {
  id: string;
  commandId: string | null;
  organizationId: string;
  agentId: string;
  decisionId: string | null;
  intentId: string;
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | string;
  dedupeKey: string | null;
  requestedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  requestedBy: string;
  metadata: Record<string, unknown>;
  steps: Array<{
    id: string;
    stepIndex: number;
    command: string;
    status: string;
    evidence: unknown;
    startedAt: string | null;
    finishedAt: string | null;
  }>;
}

export interface RmmTelemetryRemediationJobListResponse {
  items: RmmTelemetryRemediationJob[];
}

export type PatchPolicyScopeType = 'organization' | 'customer' | 'site' | 'device';
export type PatchPolicyTargetOsFamily = 'all' | 'windows' | 'linux' | 'macos';
export type PatchApprovalMode = 'manual' | 'auto_approve_security' | 'auto_approve_all';
export type PatchRebootBehavior = 'suppress' | 'allow' | 'force';
export type PatchDecision = 'approved' | 'denied' | 'deferred';
export type PatchComplianceStatus = 'compliant' | 'pending' | 'security' | 'critical' | 'reboot_required' | 'unknown';
export type PatchDeviceType = 'server' | 'workstation' | 'laptop' | 'unknown';
export type PatchRing = 'pilot' | 'early' | 'broad' | 'critical_servers' | 'excluded';
export type PatchOverrideAction =
  | 'approve'
  | 'block'
  | 'defer'
  | 'force_install'
  | 'force_scan'
  | 'force_download'
  | 'force_reboot'
  | 'defer_reboot'
  | 'maintenance_mode'
  | 'emergency_approve'
  | 'cancel';

export interface PatchPolicy {
  id: string;
  organizationId: string;
  scopeType: PatchPolicyScopeType;
  scopeKey: string;
  customerId: string | null;
  siteId: string | null;
  agentId: string | null;
  name: string;
  targetOsFamily: PatchPolicyTargetOsFamily;
  approvalMode: PatchApprovalMode;
  maintenanceWindowStart: string | null;
  maintenanceWindowEnd: string | null;
  maintenanceWindowTimezone: string | null;
  rebootBehavior: PatchRebootBehavior;
  deferralDays: number;
  managedMode?: boolean;
  nativeWindowsUpdateControl?: boolean;
  policyConfig?: Record<string, unknown>;
  priority: number;
  enabled: boolean;
  isDefault: boolean;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export interface PatchUpdateSummary {
  title: string;
  titleNorm?: string | null;
  description?: string | null;
  kbArticle?: string | null;
  isMandatory?: boolean | null;
  requiresReboot?: boolean | null;
  sizeBytes?: number | null;
  updateKey: string;
  severity: 'critical' | 'security' | 'other';
  approvalDecision: PatchDecision | null;
  deferUntil: string | null;
}

export interface PatchComplianceItem {
  agentId: string;
  hostname: string;
  os: string;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  lastScanAt: string | null;
  pendingUpdatesCount: number;
  missingCriticalCount: number;
  missingSecurityCount: number;
  rebootRequired: boolean;
  installStatus: string;
  complianceStatus: PatchComplianceStatus;
  effectivePolicy: Pick<
    PatchPolicy,
    | 'id'
    | 'scopeType'
    | 'scopeKey'
    | 'targetOsFamily'
    | 'approvalMode'
    | 'maintenanceWindowStart'
    | 'maintenanceWindowEnd'
    | 'maintenanceWindowTimezone'
    | 'rebootBehavior'
    | 'deferralDays'
    | 'priority'
    | 'enabled'
    | 'isDefault'
  > | null;
  updates: PatchUpdateSummary[];
}

export interface PatchComplianceResponse {
  generatedAt: string;
  totals: {
    devices: number;
    compliant: number;
    pending: number;
    security: number;
    critical: number;
    rebootRequired: number;
    unknown: number;
    pendingUpdates: number;
    missingCritical: number;
    missingSecurity: number;
  };
  items: PatchComplianceItem[];
}

export interface PatchProgressUpdate {
  schemaVersion: number;
  eventType: 'patch.install.progress' | 'patch.scan.progress';
  organizationId: string;
  agentId: string;
  jobId: string;
  commandId: string;
  status: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';
  phase: 'searching' | 'scanning' | 'downloading' | 'installing' | 'finalizing' | 'completed' | 'failed';
  reportedAt: string;
  receivedAt?: string;
  overallPercent: number;
  phasePercent: number;
  currentUpdateIndex: number | null;
  currentUpdatePercent: number | null;
  currentUpdate: {
    updateKey: string | null;
    title: string | null;
    kbArticle: string | null;
    index: number | null;
  } | null;
  updates: Array<{
    updateKey: string;
    title: string;
    kbArticle: string | null;
    index: number;
    state: string;
    percent: number | null;
  }>;
  summary: {
    matched: number;
    downloaded: number;
    installed: number;
    failed: number;
    skipped: number;
    rebootRequired: boolean;
    pendingUpdates?: number | null;
    snapshotRequested?: boolean;
  };
  error?: string | null;
}

export interface PatchProgressResponse {
  items: PatchProgressUpdate[];
}

export interface PatchPolicyListResponse {
  items: PatchPolicy[];
}

export interface PatchApprovalResponse {
  updated: number;
  targetedDevices: number;
  decision: PatchDecision;
}

export interface PatchInstallResponse {
  targetedDevices: number;
  queued: Array<{ agentId: string; remediationJobId: string; remediationCommandId?: string; updateCount: number }>;
  skipped: Array<{ agentId: string; reason: string }>;
}

export interface PatchOverviewDevice {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  lastSeen: string;
  lastScanAt: string | null;
  rebootRequired: boolean;
  deviceType: PatchDeviceType;
  patchRing: PatchRing;
  patchManaged: boolean;
  nativeWindowsUpdateControl: boolean;
  patchMaintenanceModeUntil: string | null;
  patchTags: string[];
  macosUpdateAccount?: MacosUpdateAccountStatus | null;
  serverRoleInventory?: {
    evidencePresent: boolean;
    roles: string[];
    isDomainController: boolean | null;
    details: {
      domainName?: string | null;
      dhcpScopes?: number;
      dnsZones?: number;
      iisSites?: number;
      iisAppPools?: number;
    };
  };
  pendingUpdates: number;
  downloadedUpdates: number;
  failedUpdates: number;
  blockedUpdates: number;
  deferredUpdates: number;
  rebootPendingUpdates: number;
}

export interface PatchOverviewUpdate {
  updateKey: string;
  title: string;
  kbArticle: string | null;
  category: string;
  releaseDate: string | null;
  releaseDateSource: string | null;
  source: string;
  affectedDevices: number;
  associatedDevices: number;
  detectedDevices: number;
  downloadedDevices: number;
  installedDevices: number;
  failedDevices: number;
  blockedDevices: number;
  deferredDevices: number;
  supersededDevices: number;
  affectedAgentIds: string[];
  affectedHostnames: string[];
  associatedAgentIds: string[];
  associatedHostnames: string[];
  customerNames: string[];
  siteNames: string[];
  osFamilies: string[];
  deviceTypes: string[];
  patchRings: string[];
  lastSeenAt: string;
}

export interface PatchDeviceStateUpdate {
  updateKey: string;
  title: string;
  kbArticle: string | null;
  category: string;
  applicabilityState: string;
  approvalState: string;
  lifecycleState: string;
  releaseDate: string | null;
  firstDetectedAt: string;
  lastDetectedAt: string;
  eligibleAt: string | null;
  installDeadlineAt: string | null;
  rebootDeadlineAt: string | null;
  downloadedAt: string | null;
  installedAt: string | null;
  failedAt: string | null;
  failureCode: string | null;
  failureHresult: number | null;
  failureMessage: string | null;
  requiresReboot: boolean | null;
}

export interface PatchTransactionFailure {
  id: string;
  operationId: string;
  action: string;
  reason: string;
  error: string | null;
  phase: string | null;
  packageManager: string | null;
  updateKeyCount: number;
  transactionPackageCount: number | null;
  decidedAt: string;
}

export interface PatchDeviceStateResponse {
  agentId: string;
  generatedAt: string;
  summary: {
    pending: number;
    downloaded: number;
    failed: number;
    installed: number;
    blocked: number;
    deferred: number;
    rebootPending: number;
    transactionFailures?: number;
  };
  updates: PatchDeviceStateUpdate[];
  transactionFailures?: PatchTransactionFailure[];
}

export interface PatchOverride {
  id: string;
  organizationId: string;
  scopeType: 'global' | 'organization' | 'customer' | 'site' | 'group' | 'tag' | 'ring' | 'device';
  scopeKey: string;
  action: PatchOverrideAction;
  operationId: string | null;
  updateKey: string | null;
  kbArticle: string | null;
  category: string | null;
  reason: string | null;
  deferUntil: string | null;
  expiresAt: string | null;
  enabled: boolean;
  createdBy: string;
  createdByEmail?: string | null;
  targetAgentId?: string | null;
  targetHostname?: string | null;
  targetOs?: string | null;
  latestActionType?: string | null;
  latestActionStatus?: string | null;
  latestActionPhase?: string | null;
  latestActionUpdatedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface PatchDecisionLog {
  id: string;
  agentId: string;
  policyId: string | null;
  operationId: string;
  action: string;
  updateKeys: string[];
  decision: string;
  reason: string;
  actorType: 'user' | 'system' | 'agent' | string;
  actorUserId: string | null;
  actorEmail: string | null;
  actionStatus?: string | null;
  actionPhase?: string | null;
  actionUpdatedAt?: string | null;
  details: Record<string, unknown>;
  decidedAt: string;
}

export interface PatchOverviewResponse {
  generatedAt: string;
  summary?: {
    devices: number;
    managed: number;
    pending: number;
    downloaded: number;
    failed: number;
    reboot: number;
  };
  devices: PatchOverviewDevice[];
  updates: PatchOverviewUpdate[];
  policies: PatchPolicy[];
  overrides: PatchOverride[];
  decisions: PatchDecisionLog[];
}

export interface PatchManualActionResponse {
  action: string;
  overrideAction: PatchOverrideAction;
  targetedDevices: number;
  overridesCreated: number;
  overrideIds?: string[];
  cancelled?: boolean;
}

export interface FeatureUpgradeIsoMedia {
  id: string;
  displayName: string;
  osFamily: string;
  product: string;
  version: string;
  edition: string | null;
  architecture: string;
  language: string | null;
  sha256: string | null;
  sizeBytes: number | null;
  containerName: string;
  blobName: string;
  active: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface FeatureUpgradeIsoMediaListResponse {
  items: FeatureUpgradeIsoMedia[];
}

export type FeatureUpgradePreflightCheckStatus = 'passed' | 'failed' | 'warning' | 'skipped' | 'not_applicable' | 'pending';
export type FeatureUpgradePreflightDeviceStatus = 'queued' | 'running' | 'passed' | 'warning' | 'failed' | 'cancelled';

export interface FeatureUpgradePreflightCheckDefinition {
  id: string;
  label: string;
  severity: 'required' | 'warning';
  appliesTo?: string[];
  description?: string;
  requiresFreshSnapshot?: boolean;
}

export interface FeatureUpgradePreflightCheckResult {
  id: string;
  label: string;
  severity: 'required' | 'warning' | string;
  status: FeatureUpgradePreflightCheckStatus;
  message: string;
  description?: string;
  source?: string | null;
  sourceLabel?: string | null;
  sourceUpdatedAt?: string | null;
  requiresFreshSnapshot?: boolean;
  details?: Record<string, unknown> | null;
}

export interface FeatureUpgradePreflightPreviewDevice {
  agentId: string;
  hostname: string;
  os: string;
  osVersion?: string | null;
  snapshotCollectedAt?: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  targetProfile: string;
  checks: FeatureUpgradePreflightCheckResult[];
}

export interface FeatureUpgradePreflightPreviewResponse {
  diskFreeBytesRequired: number;
  devices: FeatureUpgradePreflightPreviewDevice[];
  skipped: Array<{ agentId: string; reason: string }>;
  checks: FeatureUpgradePreflightCheckDefinition[];
}

export interface FeatureUpgradePreflightDeviceProgress {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: FeatureUpgradePreflightDeviceStatus;
  phase: 'queued' | 'checking' | 'completed' | 'failed' | 'cancelled' | string;
  checks: FeatureUpgradePreflightCheckResult[];
  failureSummary: Array<{ id: string | null; label: string | null; message: string | null }>;
  warningSummary: Array<{ id: string | null; label: string | null; message: string | null }>;
  requestedBy: string;
  claimedAt: string | null;
  startedAt: string | null;
  finishedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface FeatureUpgradePreflightRunResponse {
  runId: string;
  targetedDevices: number;
  devices: FeatureUpgradePreflightDeviceProgress[];
}

export interface FeatureUpgradePreflightProgressResponse {
  items: FeatureUpgradePreflightDeviceProgress[];
}

export type FeatureUpgradeIsoStageStatus =
  | 'queued'
  | 'running'
  | 'staged'
  | 'failed'
  | 'cancelled'
  | 'deleted'
  | 'expired'
  | string;

export interface FeatureUpgradeIsoStageProgress {
  schemaVersion: number;
  eventType: 'feature_upgrade.iso.stage.progress' | string;
  organizationId: string;
  agentId: string;
  operationId: string;
  runId: string;
  jobId: string;
  commandId: string;
  isoMedia: {
    id: string;
    displayName: string | null;
    sizeBytes: number | null;
    sha256: string | null;
  } | null;
  status: FeatureUpgradeIsoStageStatus;
  phase: 'queued' | 'requesting_link' | 'downloading' | 'verifying' | 'staged' | 'failed' | 'cleanup_pending' | 'deleted' | 'cancelled' | string;
  reportedAt: string;
  receivedAt?: string;
  overallPercent: number;
  phasePercent: number;
  bytesDownloaded: number | null;
  bytesTotal: number | null;
  bytesPerSecond: number | null;
  stagedAt: string | null;
  expiresAt: string | null;
  cleanedAt: string | null;
  error?: string | null;
}

export interface FeatureUpgradeIsoStageDeviceProgress {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  isoMediaId: string;
  isoDisplayName: string | null;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: FeatureUpgradeIsoStageStatus;
  phase: string;
  progress: FeatureUpgradeIsoStageProgress;
  evidence: Record<string, unknown>;
  errorMessage: string | null;
  sizeBytes: number | null;
  sha256: string | null;
  requestedBy: string;
  claimedAt: string | null;
  startedAt: string | null;
  stagedAt: string | null;
  expiresAt: string | null;
  cleanedAt: string | null;
  finishedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface FeatureUpgradeIsoStagePreviewDevice {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  preflightStatus: FeatureUpgradePreflightDeviceStatus | string | null;
  preflightOperationId: string | null;
  canStage: boolean;
  blockingReasons: string[];
  warnings: string[];
  isoMedia: FeatureUpgradeIsoMedia | null;
  existingStage: FeatureUpgradeIsoStageDeviceProgress | null;
  expectedSizeBytes: number | null;
  estimatedExpiresAt: string;
}

export interface FeatureUpgradeIsoStagePreviewResponse {
  retentionSeconds: number;
  retentionDays: number;
  estimatedExpiresAt: string;
  totalSizeBytes: number;
  devices: FeatureUpgradeIsoStagePreviewDevice[];
  skipped: Array<{ agentId: string; reason: string }>;
}

export interface FeatureUpgradeIsoStageRunResponse {
  runId: string;
  retentionSeconds: number;
  retentionDays: number;
  targetedDevices: number;
  skipped: Array<{ agentId: string; reason: string }>;
  devices: FeatureUpgradeIsoStageDeviceProgress[];
}

export interface FeatureUpgradeIsoStageProgressResponse {
  items: FeatureUpgradeIsoStageDeviceProgress[];
}

export type FeatureUpgradeStartStatus =
  | 'scheduled'
  | 'queued'
  | 'running'
  | 'awaiting_reboot'
  | 'verifying'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | string;

export interface FeatureUpgradeSetupCommandMatrix {
  id: string;
  isoMediaId: string;
  osFamily: string;
  product: string;
  version: string;
  edition: string | null;
  architecture: string;
  language: string | null;
  setupExecutable: string;
  arguments: string[];
  dynamicUpdateMode: string;
  requiresEulaAccept: boolean;
  imageIndexStrategy: string;
  supported: boolean;
  notes: string | null;
  active: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface FeatureUpgradeStartProgress {
  schemaVersion: number;
  eventType: 'feature_upgrade.start.progress' | string;
  organizationId: string;
  agentId: string;
  operationId: string;
  runId: string;
  jobId: string;
  commandId: string;
  isoMedia: {
    id: string;
    displayName: string | null;
    sizeBytes: number | null;
    sha256: string | null;
  } | null;
  status: FeatureUpgradeStartStatus;
  phase: string;
  reportedAt: string;
  receivedAt?: string;
  overallPercent: number;
  phasePercent: number;
  scheduledFor: string | null;
  finalSnapshotAt: string | null;
  setupStartedAt: string | null;
  rebootDetectedAt: string | null;
  verifiedAt: string | null;
  error?: string | null;
}

export interface FeatureUpgradeStartDeviceProgress {
  operationId: string;
  runId: string;
  organizationId: string;
  agentId: string;
  hostname: string | null;
  preflightOperationId: string;
  isoMediaId: string;
  isoDisplayName: string | null;
  setupCommandMatrixId: string;
  sourceOs: string;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  status: FeatureUpgradeStartStatus;
  phase: string;
  progress: FeatureUpgradeStartProgress;
  evidence: Record<string, unknown>;
  failureSummary: Array<{ id: string | null; label: string | null; message: string | null }>;
  errorMessage: string | null;
  sizeBytes: number | null;
  sha256: string | null;
  scheduledFor: string | null;
  requestedBy: string;
  claimedAt: string | null;
  startedAt: string | null;
  finalSnapshotAt: string | null;
  setupStartedAt: string | null;
  rebootDetectedAt: string | null;
  verifiedAt: string | null;
  finishedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface FeatureUpgradeStartPreviewDevice {
  agentId: string;
  hostname: string;
  os: string;
  osVersion: string | null;
  customerId: string | null;
  customerName: string | null;
  siteId: string | null;
  siteName: string | null;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  preflightStatus: FeatureUpgradePreflightDeviceStatus | string | null;
  preflightOperationId: string | null;
  canStart: boolean;
  blockingReasons: string[];
  warnings: string[];
  isoMedia: FeatureUpgradeIsoMedia | null;
  setupCommand: FeatureUpgradeSetupCommandMatrix | null;
  existingStage: FeatureUpgradeIsoStageDeviceProgress | null;
  existingUpgrade: FeatureUpgradeStartDeviceProgress | null;
  expectedSizeBytes: number | null;
  willDownloadIso: boolean;
}

export interface FeatureUpgradeStartPreviewResponse {
  diskFreeBytesRequired: number;
  totalDownloadBytes: number;
  devices: FeatureUpgradeStartPreviewDevice[];
  skipped: Array<{ agentId: string; reason: string }>;
}

export interface FeatureUpgradeStartRunResponse {
  runId: string;
  scheduledFor: string | null;
  targetedDevices: number;
  skipped: Array<{ agentId: string; reason: string }>;
  devices: FeatureUpgradeStartDeviceProgress[];
}

export interface FeatureUpgradeStartProgressResponse {
  items: FeatureUpgradeStartDeviceProgress[];
}

export type RmmInstallerScopeType = 'organization' | 'customer' | 'site';

export interface RmmInstallerTokenSummary {
  id: string;
  tokenPrefix: string;
  expiresAt: string | null;
  maxUses: number | null;
  usedCount: number;
  revokedAt?: string | null;
  createdAt: string;
  lastUsedAt?: string | null;
  token?: string;
}

export interface RmmInstallerProfile {
  id: string;
  name: string;
  scopeType: RmmInstallerScopeType;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  customerName: string | null;
  siteName: string | null;
  expiresAt: string | null;
  maxUses: number | null;
  revokedAt: string | null;
  createdAt: string;
  updatedAt: string;
  latestToken?: RmmInstallerTokenSummary | null;
}

export interface RmmInstallerEnrollmentPayload {
  version: number;
  registrationToken: string;
  organizationId: string;
  customerId: string | null;
  siteId: string | null;
  expiresAt: string | null;
  maxUses: number | null;
  tokenId: string;
  issuedAt: string;
}

export interface RmmInstallerProfileCreateResponse {
  profile: RmmInstallerProfile;
  issuedToken: RmmInstallerTokenSummary;
  bootstrapUrl: string | null;
  downloadExePath?: string;
}

export interface RmmInstallerDownloadResponse {
  profile: RmmInstallerProfile;
  issuedToken: RmmInstallerTokenSummary;
  bootstrapUrl: string | null;
  downloadExePath?: string;
  filename: string;
  enrollmentBlob: string;
  payload: RmmInstallerEnrollmentPayload;
}

export interface RmmLinuxInstallerResponse extends RmmInstallerDownloadResponse {
  linuxScriptPath: string;
  linuxScriptUrl: string;
  linuxShortCode: string;
  linuxShortScriptPath: string;
  linuxShortScriptUrl: string;
  linuxShortScriptExpiresAt: string;
  linuxInstallCommand: string;
  linuxScriptFilename: string;
}

export interface RmmMacosInstallerResponse extends RmmInstallerDownloadResponse {
  macosScriptPath: string;
  macosScriptUrl: string;
  macosShortCode: string;
  macosShortScriptPath: string;
  macosShortScriptUrl: string;
  macosShortScriptExpiresAt: string;
  macosInstallCommand: string;
  macosScriptFilename: string;
}

export interface RmmInstallerExeDownloadResult {
  blob: Blob;
  filename: string;
}

export type ViewerInstallerPlatform = 'windows' | 'macos';

export interface ViewerInstallerArtifact {
  fileName: string;
  sizeBytes: number;
  sha256: string;
}

export interface ViewerInstallerInfo {
  available: boolean;
  platform?: ViewerInstallerPlatform;
  profile: string | null;
  generatedAtUtc: string | null;
  downloadPath: string;
  installer: ViewerInstallerArtifact | null;
  error?: string | null;
}

export interface LinuxAgentArtifact {
  fileName: string;
  sizeBytes: number;
  sha256: string;
}

export interface LinuxAgentInstallerInfo {
  available: boolean;
  downloadPath: string;
  downloadUrl: string | null;
  binary: LinuxAgentArtifact | null;
  error?: string | null;
}

export interface MacosPackageArtifact {
  fileName: string;
  sizeBytes: number;
  sha256: string;
}

export interface MacosPackageInstallerInfo {
  available: boolean;
  downloadPath: string;
  downloadUrl: string | null;
  package: MacosPackageArtifact | null;
  error?: string | null;
}

export type SecureNoteStatus =
  | 'available'
  | 'revealed'
  | 'not_found'
  | 'expired'
  | 'viewed'
  | 'unauthorized'
  | 'invalid'
  | 'error';

export interface SecureNoteCheckResponse {
  status: SecureNoteStatus;
  expiresAt?: string;
  recipientEmail?: string | null;
  error?: string;
}

export interface SecureNoteRevealResponse {
  status: SecureNoteStatus;
  content?: string;
  destroyedAt?: string;
  error?: string;
}
