import { Prisma } from "@prisma/client";
import { randomUUID } from "crypto";
import { appendCommandCenterMessage } from "./commandCenterConversations";
import { env } from "./env";
import { createLogger } from "./logger";
import { prisma } from "./prisma";
import {
  attachGeneratedSecretsToRunnerJob,
  listGeneratedSecureNotesForJob,
  type GeneratedSecretSummary,
} from "./secureNotes";
import type { CommandCenterConversationContext } from "./commandCenterConversations";

const log = createLogger("api_backend::command_center_ai_runner");

const DEFAULT_RUNNER_WAIT_TIMEOUT_MS = parsePositiveInt(
  process.env.COMMAND_CENTER_AI_RUNNER_WAIT_TIMEOUT_MS,
  75_000,
);
const DEFAULT_RUNNER_POLL_INTERVAL_MS = 1_000;
const APPROVAL_REQUEST_TIMEOUT_MS = 5 * 60 * 1000;
const APPROVAL_GRANT_TTL_MS = 15 * 60 * 1000;
const AI_RUNNER_JOB_TYPE = "desktop_goal";
const AI_RUNNER_SHELL_JOB_TYPE = "shell_goal";
const AI_RUNNER_SCREENSHOT_ARTIFACT_TYPE = "runner-screenshot";
const AI_RUNNER_SHELL_TRANSCRIPT_ARTIFACT_TYPE = "runner-shell-transcript";
const AI_RUNNER_REPLAY_DEFAULT_DELAY_MS = 1_000;
const MAX_ARTIFACT_BASE64_CHARS = 7_900_000;
const COMMAND_APPROVAL_TIMEOUT_MS = 10 * 60 * 1000;
const MAX_COMMAND_OUTPUT_CHARS = 8_000;
const COMMAND_OUTPUT_DELTA_EVENT_TYPE = "command_output_delta";
const COMMAND_OUTPUT_RECENT_MAX_EVENTS = parsePositiveInt(
  process.env.COMMAND_CENTER_AI_RUNNER_OUTPUT_MAX_EVENTS,
  500,
);
const COMMAND_OUTPUT_RECENT_MAX_CHARS = parsePositiveInt(
  process.env.COMMAND_CENTER_AI_RUNNER_OUTPUT_MAX_CHARS,
  256 * 1024,
);
const AI_RUNNER_LEASE_TTL_MS = parsePositiveInt(
  process.env.COMMAND_CENTER_AI_RUNNER_LEASE_TTL_MS,
  45_000,
);
const AI_RUNNER_LEASE_RECONCILE_INTERVAL_MS = parsePositiveInt(
  process.env.COMMAND_CENTER_AI_RUNNER_LEASE_RECONCILE_INTERVAL_MS,
  30_000,
);
const EXPIRED_LEASE_ERROR = "AI runner lease expired before completion.";
const APPROVAL_UNAVAILABLE_PHASE = "approval_unavailable";
const NO_INTERACTIVE_USER_REASON = "no_interactive_user";
const NO_INTERACTIVE_USER_APPROVAL_MESSAGE =
  "Endpoint approval could not be requested because no user is currently logged in on this device. Ask someone to sign in, then retry.";

function parsePositiveInt(value: string | undefined, fallback: number): number {
  const parsed = Number.parseInt(value || "", 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export type CommandCenterMessageAttachment = {
  id: string;
  type: "image";
  mimeType: string;
  name: string;
  artifactId: string;
  width?: number;
  height?: number;
  presentation?: "inline" | "live_frame";
  jobId?: string;
  frameSeq?: number;
  cursor?: {
    visible: boolean;
    x?: number;
    y?: number;
    width: number;
    height: number;
  };
};

export type AiRunnerJobStatus =
  | "queued"
  | "approval_pending"
  | "approval_granted"
  | "approval_denied"
  | "approval_expired"
  | "running"
  | "succeeded"
  | "failed"
  | "stopping"
  | "stopped";

export type AiRunnerJobSummary = {
  id: string;
  organizationId: string;
  userId: string;
  conversationId: string | null;
  agentId: string;
  jobType: string;
  status: AiRunnerJobStatus;
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
};

export type AiRunnerArtifactSummary = {
  id: string;
  jobId: string;
  artifactType: string;
  name: string;
  mimeType: string;
  metadata: unknown | null;
  createdAt: string;
};

export type AiRunnerEventSummary = {
  id: string;
  jobId: string;
  eventKey: string;
  eventType: string;
  runnerId: string | null;
  leaseId: string | null;
  turnIndex: number | null;
  artifactFrameId: string | null;
  commandApprovalId: string | null;
  artifactId: string | null;
  payload: unknown | null;
  createdAt: string;
};

export type AiRunnerCommandOutputDelta = {
  eventId: string;
  jobId: string;
  approvalId: string;
  turnIndex: number | null;
  sequence: number;
  text: string;
  outputOffset: number;
  terminal: boolean;
  createdAt: string;
};

export type AiRunnerConversationStreamSnapshot = {
  jobs: AiRunnerJobDetail[];
  output: AiRunnerCommandOutputDelta[];
};

export type AiRunnerEvidenceSummary = {
  jobId: string;
  jobType: string;
  status: AiRunnerJobStatus;
  shellTranscriptAvailable: boolean;
  desktopReplayAvailable: boolean;
  replayFrameCount: number;
};

export type AiRunnerReplayFrame = {
  artifactId: string;
  frameSeq: number | null;
  width: number | null;
  height: number | null;
  cursor: CommandCenterMessageAttachment["cursor"] | null;
  stepIndex: number | null;
  taskId: string | null;
  displayText: string;
  createdAt: string;
};

export type AiRunnerReplayManifest = {
  jobId: string;
  jobType: string;
  status: AiRunnerJobStatus;
  deviceLabel: string | null;
  goal: string | null;
  startedAt: string | null;
  finishedAt: string | null;
  defaultDelayMs: number;
  frames: AiRunnerReplayFrame[];
};

export type AiRunnerCommandApprovalStatus =
  | "pending"
  | "approved"
  | "denied"
  | "desktop_control_requested"
  | "executing"
  | "executed"
  | "failed"
  | "expired"
  | "policy_blocked";

export type AiRunnerCommandApprovalSummary = {
  id: string;
  jobId: string;
  turnIndex: number;
  status: AiRunnerCommandApprovalStatus;
  command: string;
  explanation: string;
  risk: string;
  notes: string[];
  message: string | null;
  modelResponseId: string | null;
  policyAllowed: boolean | null;
  policyReason: string | null;
  matchedPolicyId: string | null;
  output: string | null;
  outputLength: number | null;
  exitCode: number | null;
  error: string | null;
  messageId: string | null;
  createdAt: string;
  updatedAt: string;
  expiresAt: string | null;
  executedAt: string | null;
};

export type AiRunnerDeviceContext = {
  agentId: string;
  hostname: string | null;
  customerName: string | null;
  siteName: string | null;
  snapshot: {
    collectedAt: string | null;
    ageSeconds: number | null;
  };
  platform: {
    family: "windows" | "macos" | "linux" | "unknown";
    osName: string | null;
    osVersion: string | null;
    architecture: string | null;
    timezone: string | null;
    locale: string | null;
    domain: string | null;
  };
  agent: {
    version: string | null;
    lastSeen: string | null;
  };
  hardware: {
    cpuModel: string | null;
    physicalCores: number | null;
    logicalCores: number | null;
    memoryTotalBytes: number | null;
  };
  state: {
    pendingUpdatesCount: number | null;
    rebootRequired: boolean | null;
  };
  network: {
    primaryIp: string | null;
  };
  shell: {
    runAs: "system" | "root" | "configured_user" | "unknown";
    account: string | null;
    elevated: boolean | null;
    description: string | null;
  };
  security: {
    firewallEnabled: boolean | null;
    secureBoot: boolean | null;
    tpmPresent: boolean | null;
    tpmEnabled: boolean | null;
    antivirusEnabled: boolean | null;
    bitlockerEnabled: boolean | null;
  };
};

type RunnerDb = {
  commandCenterConversation: any;
  commandCenterMessage: any;
  commandCenterAiRunnerJob: any;
  commandCenterAiRunnerArtifact: any;
  commandCenterAiRunnerCommandApproval: any;
  commandCenterAiRunnerApprovalGrant: any;
  commandCenterAiRunnerEvent: any;
  commandPolicy: any;
  organizationMember: any;
  rmmDevice: any;
};

type CreateRunnerJobInput = {
  agentId: string;
  conversationId?: string | null;
  goal?: string | null;
  requesterLabel?: string | null;
  requesterEmail?: string | null;
  organizationName?: string | null;
  jobType?: string | null;
  generatedSecretHandles?: string[] | null;
};

type RunnerCallbackStatusInput = {
  status?: unknown;
  runnerId?: unknown;
  leaseId?: unknown;
  eventKey?: unknown;
  message?: unknown;
  result?: unknown;
  error?: unknown;
};

type RunnerArtifactCallbackInput = {
  runnerId?: unknown;
  leaseId?: unknown;
  eventKey?: unknown;
  artifactType?: unknown;
  name?: unknown;
  mimeType?: unknown;
  contentBase64?: unknown;
  metadata?: unknown;
  appendToChat?: unknown;
  messageContent?: unknown;
  chatPresentation?: unknown;
};

export type AiRunnerLeaseSummary = {
  accepted: boolean;
  reason: string | null;
  job: AiRunnerJobSummary | null;
  leaseId: string | null;
  leaseExpiresAt: string | null;
  cancelRequestedAt: string | null;
};

type RunnerCallbackLeaseInput = {
  runnerId?: unknown;
  leaseId?: unknown;
  eventKey?: unknown;
};

function db(): RunnerDb {
  return prisma as unknown as RunnerDb;
}

function toIso(value: unknown): string {
  return value instanceof Date ? value.toISOString() : new Date(String(value)).toISOString();
}

function asRecord(value: unknown): Record<string, any> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, any>)
    : null;
}

function numberValue(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function meaningfulStringValue(value: unknown): string | null {
  const text = stringValue(value);
  if (!text) return null;
  const normalized = text.toLowerCase();
  if (["unknown", "none", "n/a", "na", "null", "undefined"].includes(normalized)) return null;
  return text;
}

function firstMeaningfulString(...values: unknown[]): string | null {
  for (const value of values) {
    const text = meaningfulStringValue(value);
    if (text) return text;
  }
  return null;
}

function meaningfulIpValue(value: unknown): string | null {
  const ip = meaningfulStringValue(value);
  if (!ip || ip === "0.0.0.0" || ip === "::" || ip === "::1" || ip.startsWith("127.")) {
    return null;
  }
  return ip;
}

function firstMeaningfulIp(...values: unknown[]): string | null {
  for (const value of values) {
    const ip = meaningfulIpValue(value);
    if (ip) return ip;
  }
  return null;
}

function numberOrNull(value: unknown): number | null {
  if (typeof value === "bigint") {
    const parsed = Number(value);
    return Number.isSafeInteger(parsed) ? parsed : null;
  }
  const parsed = numberValue(value);
  return parsed === undefined ? null : parsed;
}

function booleanValue(value: unknown): boolean | null {
  if (typeof value === "boolean") return value;
  if (typeof value === "number") {
    if (value === 1) return true;
    if (value === 0) return false;
  }
  if (typeof value === "string") {
    const normalized = value.trim().toLowerCase();
    if (["true", "yes", "y", "1", "enabled", "on"].includes(normalized)) return true;
    if (["false", "no", "n", "0", "disabled", "off"].includes(normalized)) return false;
  }
  return null;
}

function valueAtPath(value: unknown, path: string[]): unknown {
  let current: unknown = value;
  for (const segment of path) {
    const record = asRecord(current);
    if (!record || !(segment in record)) return undefined;
    current = record[segment];
  }
  return current;
}

function firstString(...values: unknown[]): string | null {
  for (const value of values) {
    const text = stringValue(value);
    if (text) return text;
  }
  return null;
}

function firstBoolean(...values: unknown[]): boolean | null {
  for (const value of values) {
    const bool = booleanValue(value);
    if (bool !== null) return bool;
  }
  return null;
}

function collectionFromInventory(inventoryData: unknown): unknown {
  return valueAtPath(inventoryData, ["collection"]) ?? inventoryData;
}

function normalizePlatformFamily(value: unknown): AiRunnerDeviceContext["platform"]["family"] {
  const normalized = (stringValue(value) || "").toLowerCase().replace(/[_-]+/g, " ");
  if (normalized.includes("windows")) return "windows";
  if (
    normalized.includes("macos") ||
    normalized.includes("mac os") ||
    normalized.includes("darwin") ||
    normalized.includes("os x")
  ) {
    return "macos";
  }
  if (normalized.includes("linux") || normalized.includes("ubuntu") || normalized.includes("debian")) {
    return "linux";
  }
  return "unknown";
}

function shellContextForPlatform(
  platformFamily: AiRunnerDeviceContext["platform"]["family"],
): AiRunnerDeviceContext["shell"] {
  if (platformFamily === "windows") {
    return {
      runAs: "system",
      account: "NT AUTHORITY\\SYSTEM",
      elevated: true,
      description: "AI shell commands run as the local Windows SYSTEM account, not the signed-in user.",
    };
  }

  if (platformFamily === "macos") {
    return {
      runAs: "root",
      account: "root",
      elevated: true,
      description: "AI shell commands run as root from the Talos LaunchDaemon context, not the console user.",
    };
  }

  if (platformFamily === "linux") {
    return {
      runAs: "configured_user",
      account: null,
      elevated: false,
      description: "AI shell commands run as a configured Linux shell user, not root unless explicitly configured.",
    };
  }

  return {
    runAs: "unknown",
    account: null,
    elevated: null,
    description: "AI shell command identity is unknown for this platform.",
  };
}

function firewallEnabledFromInventory(collection: unknown): boolean | null {
  const enabled = valueAtPath(collection, ["security", "firewall", "enabled"]);
  const direct = booleanValue(enabled);
  if (direct !== null) return direct;
  const enabledRecord = asRecord(enabled);
  if (!enabledRecord) return null;
  let sawAny = false;
  let anyEnabled = false;
  for (const key of ["domain", "private", "public"]) {
    const value = booleanValue(enabledRecord[key]);
    if (value !== null) {
      sawAny = true;
      anyEnabled = anyEnabled || value;
    }
  }
  return sawAny ? anyEnabled : null;
}

function firstInventoryIp(collection: unknown): string | null {
  const adapters = [
    valueAtPath(collection, ["network", "adapters"]),
    valueAtPath(collection, ["network_adapters"]),
    valueAtPath(collection, ["hardware", "network_adapters"]),
  ].find(Array.isArray) as unknown[] | undefined;
  if (!adapters) return null;
  for (const adapter of adapters) {
    const record = asRecord(adapter);
    const ips = Array.isArray(record?.ips) ? record.ips : Array.isArray(record?.ip_addresses) ? record.ip_addresses : [];
    for (const ip of ips) {
      const ipRecord = asRecord(ip);
      const address = firstMeaningfulIp(ipRecord?.address, ip);
      if (address) {
        return address;
      }
    }
  }
  return null;
}

function liveFrameCursorValue(value: unknown): CommandCenterMessageAttachment["cursor"] | undefined {
  const record = asRecord(value);
  if (!record) return undefined;
  const width = numberValue(record.width);
  const height = numberValue(record.height);
  if (width === undefined || height === undefined) return undefined;
  const visible = record.visible === true;
  const cursor: NonNullable<CommandCenterMessageAttachment["cursor"]> = {
    visible,
    width,
    height,
  };
  const x = numberValue(record.x);
  const y = numberValue(record.y);
  if (x !== undefined) cursor.x = x;
  if (y !== undefined) cursor.y = y;
  return cursor;
}

function chatPresentationValue(value: unknown): "message" | "live_frame" | null {
  return value === "message" || value === "live_frame" ? value : null;
}

function dateFromUnixMs(value: unknown): Date | null {
  const number = numberValue(value);
  return number === undefined ? null : new Date(number);
}

function unixMs(date: Date): number {
  return date.getTime();
}

function normalizeStatus(value: unknown): AiRunnerJobStatus {
  const status = typeof value === "string" ? value.trim().toLowerCase() : "";
  if (
    status === "queued" ||
    status === "approval_pending" ||
    status === "approval_granted" ||
    status === "approval_denied" ||
    status === "approval_expired" ||
    status === "running" ||
    status === "succeeded" ||
    status === "failed" ||
    status === "stopping" ||
    status === "stopped"
  ) {
    return status;
  }
  return "running";
}

function isTerminalStatus(status: AiRunnerJobStatus): boolean {
  return (
    status === "succeeded" ||
    status === "failed" ||
    status === "stopped" ||
    status === "approval_denied" ||
    status === "approval_expired"
  );
}

function jsonInput(value: unknown): unknown {
  if (value === undefined) return undefined;
  if (value === null) return Prisma.JsonNull;
  return value;
}

function isUniqueConstraintError(error: unknown): boolean {
  return error instanceof Prisma.PrismaClientKnownRequestError && error.code === "P2002";
}

function activeLeaseId(record: any, now = new Date()): string | null {
  const leaseId = stringValue(record?.leaseId);
  if (!leaseId) return null;
  const expiresAt = record?.leaseExpiresAt instanceof Date ? record.leaseExpiresAt : null;
  if (!expiresAt || expiresAt.getTime() <= now.getTime()) return null;
  return leaseId;
}

function assertCallbackLease(record: any, input: RunnerCallbackLeaseInput) {
  const leaseId = stringValue(input.leaseId);
  const storedLeaseId = stringValue(record?.leaseId);
  if (!storedLeaseId) {
    if (leaseId) {
      throw new Error("AI runner callback lease mismatch");
    }
    return {
      leaseId: null,
      runnerId: stringValue(input.runnerId),
    };
  }
  const active = activeLeaseId(record);
  if (!active) {
    throw new Error("AI runner callback lease expired");
  }
  if (leaseId !== active) {
    throw new Error("AI runner callback lease mismatch");
  }
  return {
    leaseId,
    runnerId: stringValue(input.runnerId),
  };
}

function eventKeyValue(value: unknown, fallback: string): string {
  return typeof value === "string" && value.trim() ? value.trim().slice(0, 500) : fallback;
}

function eventFrameId(metadata: unknown): string | null {
  const record = asRecord(metadata);
  if (!record) return null;
  const frameId =
    stringValue(record.frameId) ||
    stringValue(record.frame_id) ||
    (numberValue(record.frameSeq) !== undefined ? String(Math.trunc(numberValue(record.frameSeq)!)) : null) ||
    (numberValue(record.frame_seq) !== undefined ? String(Math.trunc(numberValue(record.frame_seq)!)) : null);
  return frameId;
}

function eventPayload(input: unknown): unknown {
  const record = asRecord(input);
  if (!record) return jsonInput(input);
  const clone: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(record)) {
    if (key === "contentBase64") {
      clone.contentBase64Length = typeof value === "string" ? value.length : null;
      continue;
    }
    clone[key] = value;
  }
  return jsonInput(clone);
}

async function createAiRunnerEvent(
  runnerDb: RunnerDb,
  job: any,
  input: {
    eventKey: string;
    eventType: string;
    runnerId?: string | null;
    leaseId?: string | null;
    turnIndex?: number | null;
    artifactFrameId?: string | null;
    commandApprovalId?: string | null;
    artifactId?: string | null;
    payload?: unknown;
  },
): Promise<{ record: any; created: boolean }> {
  const existing = await runnerDb.commandCenterAiRunnerEvent.findFirst({
    where: { jobId: job.id, eventKey: input.eventKey },
  });
  if (existing) return { record: existing, created: false };
  try {
    const record = await runnerDb.commandCenterAiRunnerEvent.create({
      data: {
        jobId: job.id,
        organizationId: job.organizationId,
        userId: job.userId,
        conversationId: job.conversationId ?? null,
        agentId: job.agentId,
        eventKey: input.eventKey,
        eventType: input.eventType,
        runnerId: input.runnerId ?? null,
        leaseId: input.leaseId ?? null,
        turnIndex: input.turnIndex ?? null,
        artifactFrameId: input.artifactFrameId ?? null,
        commandApprovalId: input.commandApprovalId ?? null,
        artifactId: input.artifactId ?? null,
        payload: eventPayload(input.payload ?? {}),
      },
    });
    return { record, created: true };
  } catch (error) {
    if (!isUniqueConstraintError(error)) throw error;
    const record = await runnerDb.commandCenterAiRunnerEvent.findFirst({
      where: { jobId: job.id, eventKey: input.eventKey },
    });
    if (record) return { record, created: false };
    throw error;
  }
}

async function linkAiRunnerEvent(
  runnerDb: RunnerDb,
  eventId: string,
  data: { artifactId?: string | null; commandApprovalId?: string | null },
) {
  await runnerDb.commandCenterAiRunnerEvent.update({
    where: { id: eventId },
    data,
  }).catch((error: unknown) => {
    log.warn("AI runner event link failed", {
      eventId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

function toJobSummary(record: any): AiRunnerJobSummary {
  return {
    id: record.id,
    organizationId: record.organizationId,
    userId: record.userId,
    conversationId: record.conversationId ?? null,
    agentId: record.agentId,
    jobType: record.jobType ?? AI_RUNNER_JOB_TYPE,
    status: normalizeStatus(record.status),
    runnerId: record.runnerId ?? null,
    approvalId: record.approvalId ?? null,
    approvalChatSessionId: record.approvalChatSessionId ?? null,
    approvalRequestedAt: record.approvalRequestedAt ? toIso(record.approvalRequestedAt) : null,
    approvalRespondedAt: record.approvalRespondedAt ? toIso(record.approvalRespondedAt) : null,
    approvalExpiresAt: record.approvalExpiresAt ? toIso(record.approvalExpiresAt) : null,
    approvalWindowExpiresAt: record.approvalWindowExpiresAt ? toIso(record.approvalWindowExpiresAt) : null,
    resultMessageId: record.resultMessageId ?? null,
    liveFrameMessageId: record.liveFrameMessageId ?? null,
    result: record.result ?? null,
    error: record.error ?? null,
    createdAt: toIso(record.createdAt),
    updatedAt: toIso(record.updatedAt),
    startedAt: record.startedAt ? toIso(record.startedAt) : null,
    finishedAt: record.finishedAt ? toIso(record.finishedAt) : null,
  };
}

function toArtifactSummary(record: any): AiRunnerArtifactSummary {
  return {
    id: record.id,
    jobId: record.jobId,
    artifactType: record.artifactType,
    name: record.name,
    mimeType: record.mimeType,
    metadata: record.metadata ?? null,
    createdAt: toIso(record.createdAt),
  };
}

function toEventSummary(record: any): AiRunnerEventSummary {
  return {
    id: record.id,
    jobId: record.jobId,
    eventKey: record.eventKey,
    eventType: record.eventType,
    runnerId: record.runnerId ?? null,
    leaseId: record.leaseId ?? null,
    turnIndex: numberOrNull(record.turnIndex),
    artifactFrameId: record.artifactFrameId ?? null,
    commandApprovalId: record.commandApprovalId ?? null,
    artifactId: record.artifactId ?? null,
    payload: record.payload ?? null,
    createdAt: toIso(record.createdAt),
  };
}

function commandOutputDeltaFromEvent(record: any): AiRunnerCommandOutputDelta | null {
  if (record.eventType !== COMMAND_OUTPUT_DELTA_EVENT_TYPE) return null;
  const payload = asRecord(record.payload);
  const approvalId =
    stringValue(payload?.approvalId) ||
    stringValue(payload?.approval_id) ||
    stringValue(record.commandApprovalId);
  const jobId = stringValue(payload?.jobId) || stringValue(payload?.job_id) || stringValue(record.jobId);
  if (!approvalId || !jobId) return null;
  return {
    eventId: record.id,
    jobId,
    approvalId,
    turnIndex: numberOrNull(payload?.turnIndex ?? payload?.turn_index ?? record.turnIndex),
    sequence: Math.max(0, Math.trunc(numberValue(payload?.sequence) ?? 0)),
    text: typeof payload?.text === "string" ? payload.text : "",
    outputOffset: Math.max(0, Math.trunc(numberValue(payload?.outputOffset ?? payload?.output_offset) ?? 0)),
    terminal: payload?.terminal === true,
    createdAt: toIso(record.createdAt),
  };
}

async function pruneCommandOutputDeltaEvents(
  runnerDb: RunnerDb,
  jobId: string,
  approvalId: string,
) {
  const records = await runnerDb.commandCenterAiRunnerEvent.findMany({
    where: {
      jobId,
      eventType: COMMAND_OUTPUT_DELTA_EVENT_TYPE,
      commandApprovalId: approvalId,
    },
    orderBy: { createdAt: "desc" },
    select: { id: true, payload: true },
  });
  if (records.length <= COMMAND_OUTPUT_RECENT_MAX_EVENTS) {
    const total = records.reduce((sum: number, record: any) => {
      const payload = asRecord(record.payload);
      return sum + (typeof payload?.text === "string" ? payload.text.length : 0);
    }, 0);
    if (total <= COMMAND_OUTPUT_RECENT_MAX_CHARS) return;
  }

  const keepIds = new Set<string>();
  let chars = 0;
  for (const record of records) {
    const payload = asRecord(record.payload);
    const textChars = typeof payload?.text === "string" ? payload.text.length : 0;
    if (
      keepIds.size === 0 ||
      (keepIds.size < COMMAND_OUTPUT_RECENT_MAX_EVENTS && chars < COMMAND_OUTPUT_RECENT_MAX_CHARS)
    ) {
      keepIds.add(record.id);
      chars += textChars;
    }
  }
  const deleteIds = records.map((record: any) => record.id).filter((id: string) => !keepIds.has(id));
  if (deleteIds.length === 0) return;
  await runnerDb.commandCenterAiRunnerEvent.deleteMany({
    where: { id: { in: deleteIds } },
  }).catch((error: unknown) => {
    log.warn("AI runner command output pruning failed", {
      jobId,
      approvalId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

function normalizeCommandApprovalStatus(value: unknown): AiRunnerCommandApprovalStatus {
  const status = typeof value === "string" ? value.trim().toLowerCase() : "";
  if (
    status === "pending" ||
    status === "approved" ||
    status === "denied" ||
    status === "desktop_control_requested" ||
    status === "executing" ||
    status === "executed" ||
    status === "failed" ||
    status === "expired" ||
    status === "policy_blocked"
  ) {
    return status;
  }
  return "pending";
}

function stringArrayValue(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => (typeof item === "string" ? item.trim() : ""))
    .filter(Boolean)
    .slice(0, 8);
}

function toCommandApprovalSummary(record: any): AiRunnerCommandApprovalSummary {
  return {
    id: record.id,
    jobId: record.jobId,
    turnIndex: Number(record.turnIndex) || 0,
    status: normalizeCommandApprovalStatus(record.status),
    command: record.command ?? "",
    explanation: record.explanation ?? "",
    risk: record.risk ?? "",
    notes: stringArrayValue(record.notes),
    message: record.message ?? null,
    modelResponseId: record.modelResponseId ?? null,
    policyAllowed: typeof record.policyAllowed === "boolean" ? record.policyAllowed : null,
    policyReason: record.policyReason ?? null,
    matchedPolicyId: record.matchedPolicyId !== null && record.matchedPolicyId !== undefined
      ? record.matchedPolicyId.toString()
      : null,
    output: record.output ?? null,
    outputLength: typeof record.outputLength === "number" ? record.outputLength : null,
    exitCode: typeof record.exitCode === "number" ? record.exitCode : null,
    error: record.error ?? null,
    messageId: record.messageId ?? null,
    createdAt: toIso(record.createdAt),
    updatedAt: toIso(record.updatedAt),
    expiresAt: record.expiresAt ? toIso(record.expiresAt) : null,
    executedAt: record.executedAt ? toIso(record.executedAt) : null,
  };
}

function artifactToAttachment(record: AiRunnerArtifactSummary): CommandCenterMessageAttachment | null {
  if (!record.mimeType.startsWith("image/")) {
    return null;
  }
  const metadata = asRecord(record.metadata);
  return {
    id: record.id,
    type: "image",
    mimeType: record.mimeType,
    name: record.name,
    artifactId: record.id,
    width: numberValue(metadata?.width),
    height: numberValue(metadata?.height),
    presentation: metadata?.presentation === "live_frame" ? "live_frame" : "inline",
    jobId: typeof metadata?.jobId === "string" && metadata.jobId.trim() ? metadata.jobId.trim() : record.jobId,
    frameSeq: numberValue(metadata?.frameSeq),
    cursor: liveFrameCursorValue(metadata?.cursor),
  };
}

function isShellTranscriptArtifact(record: AiRunnerArtifactSummary): boolean {
  return record.artifactType === AI_RUNNER_SHELL_TRANSCRIPT_ARTIFACT_TYPE;
}

function isLiveDesktopFrameArtifact(record: AiRunnerArtifactSummary): boolean {
  if (record.artifactType !== AI_RUNNER_SCREENSHOT_ARTIFACT_TYPE || !record.mimeType.startsWith("image/")) {
    return false;
  }
  const metadata = asRecord(record.metadata);
  return metadata?.source === "live_vp8_relay_stream";
}

function artifactSequence(record: AiRunnerArtifactSummary): number {
  return shellTranscriptSequence(record) ?? Number.MAX_SAFE_INTEGER;
}

function artifactFrameSeq(record: AiRunnerArtifactSummary): number | null {
  const metadata = asRecord(record.metadata);
  const frameSeq = numberValue(metadata?.frameSeq) ?? numberValue(metadata?.frame_seq);
  return frameSeq === undefined ? null : frameSeq;
}

function artifactReplaySortValue(record: AiRunnerArtifactSummary): number {
  const frameSeq = artifactFrameSeq(record);
  return frameSeq === null ? Number.MAX_SAFE_INTEGER : frameSeq;
}

function shellTranscriptSequence(record: AiRunnerArtifactSummary): number | null {
  const metadata = asRecord(record.metadata);
  const sequence = numberValue(metadata?.sequence);
  if (sequence === undefined || !Number.isInteger(sequence) || sequence < 1) return null;
  return sequence;
}

function shellTranscriptTotalChunks(record: AiRunnerArtifactSummary): number | null {
  const metadata = asRecord(record.metadata);
  const totalChunks = numberValue(metadata?.totalChunks);
  if (totalChunks === undefined || !Number.isInteger(totalChunks) || totalChunks < 1) return null;
  return totalChunks;
}

function completeShellTranscriptArtifacts<T extends AiRunnerArtifactSummary>(artifacts: T[]): T[] {
  const transcriptArtifacts = artifacts.filter(isShellTranscriptArtifact);
  if (transcriptArtifacts.length === 0) return [];
  const expectedTotal = transcriptArtifacts.reduce((total, artifact) => {
    const declaredTotal = shellTranscriptTotalChunks(artifact);
    return declaredTotal && declaredTotal > total ? declaredTotal : total;
  }, 0);
  if (expectedTotal === 0) {
    return [...transcriptArtifacts].sort((a, b) => {
      const sequenceDelta = artifactSequence(a) - artifactSequence(b);
      if (sequenceDelta !== 0) return sequenceDelta;
      return new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime();
    });
  }

  const chunksBySequence = new Map<number, T>();
  for (const artifact of transcriptArtifacts) {
    const sequence = shellTranscriptSequence(artifact);
    if (!sequence || sequence > expectedTotal || chunksBySequence.has(sequence)) continue;
    chunksBySequence.set(sequence, artifact);
  }
  if (chunksBySequence.size !== expectedTotal) return [];
  return Array.from({ length: expectedTotal }, (_, index) => chunksBySequence.get(index + 1)!);
}

function artifactEvidenceSummary(job: AiRunnerJobSummary, artifacts: AiRunnerArtifactSummary[]): AiRunnerEvidenceSummary {
  const replayFrameCount = artifacts.filter(isLiveDesktopFrameArtifact).length;
  return {
    jobId: job.id,
    jobType: job.jobType,
    status: job.status,
    shellTranscriptAvailable: completeShellTranscriptArtifacts(artifacts).length > 0,
    desktopReplayAvailable: replayFrameCount > 0,
    replayFrameCount,
  };
}

function hasAiRunnerEvidence(evidence: AiRunnerEvidenceSummary): boolean {
  return evidence.shellTranscriptAvailable || evidence.desktopReplayAvailable;
}

function runnerUrl(): string {
  const url = (env.aiRunnerUrl || "").trim().replace(/\/$/, "");
  if (!url) {
    throw new Error("TALOS_AI_RUNNER_URL is not configured");
  }
  return url;
}

function runnerServiceKey(): string {
  const key = (env.aiRunnerServiceKey || env.serviceKey || "").trim();
  if (!key) {
    throw new Error("TALOS_AI_RUNNER_SERVICE_KEY or SERVICE_KEY is not configured");
  }
  return key;
}

function callbackBaseUrl(): string | null {
  const url = (env.aiRunnerCallbackBaseUrl || "").trim().replace(/\/$/, "");
  return url || null;
}

async function assertDeviceScope(
  context: CommandCenterConversationContext,
  agentId: string,
  runnerDb: RunnerDb = db(),
) {
  const device = await runnerDb.rmmDevice.findFirst({
    where: {
      agentId,
      organizationId: context.organizationId,
    },
    select: {
      agentId: true,
      hostname: true,
      os: true,
      ip: true,
      version: true,
      lastSeen: true,
      aiRunnerAutoApprove: true,
      customerId: true,
      customer: { select: { name: true } },
      site: { select: { name: true } },
      telemetryState: {
        select: {
          collectedAt: true,
          inventoryData: true,
          hostname: true,
          osName: true,
          osVersion: true,
          agentVersion: true,
          cpuModel: true,
          cpuPhysicalCores: true,
          cpuLogicalCores: true,
          memoryTotalBytes: true,
          pendingUpdatesCount: true,
          rebootRequired: true,
        },
      },
    },
  });
  if (!device) {
    throw new Error("Device not found");
  }
  return device;
}

async function findActiveApprovalGrant(
  context: CommandCenterConversationContext,
  agentId: string,
  jobType: string,
  runnerDb: RunnerDb = db(),
) {
  return runnerDb.commandCenterAiRunnerApprovalGrant.findFirst({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      agentId,
      jobType,
      expiresAt: { gt: new Date() },
    },
    orderBy: { expiresAt: "desc" },
  });
}

function deviceLabel(device: { hostname?: string | null; agentId: string; customer?: { name?: string | null } | null }) {
  const hostname = device.hostname?.trim() || device.agentId;
  const customer = device.customer?.name?.trim();
  return customer ? `${hostname} (${customer})` : hostname;
}

export function buildAiRunnerDeviceContextFromDevice(device: any, now = new Date()): AiRunnerDeviceContext {
  const state = device?.telemetryState ?? null;
  const collection = collectionFromInventory(state?.inventoryData ?? null);
  const osSystem =
    asRecord(valueAtPath(collection, ["operating_system", "system"])) ??
    asRecord(valueAtPath(collection, ["operatingSystem", "system"])) ??
    asRecord(valueAtPath(collection, ["system"]));
  const osRecord = asRecord(osSystem?.os);
  const hardwareCpu = asRecord(valueAtPath(collection, ["hardware", "cpu"]));
  const hardwareMemory = asRecord(valueAtPath(collection, ["hardware", "memory"]));
  const tpm = asRecord(valueAtPath(collection, ["hardware", "tpm"]));
  const antivirus = asRecord(valueAtPath(collection, ["security", "antivirus"]));
  const defender = asRecord(valueAtPath(collection, ["security", "antivirus", "windows_defender"]));

  const collectedAt = state?.collectedAt ? toIso(state.collectedAt) : null;
  const collectedDate = state?.collectedAt instanceof Date ? state.collectedAt : collectedAt ? new Date(collectedAt) : null;
  const ageSeconds = collectedDate && Number.isFinite(collectedDate.getTime())
    ? Math.max(0, Math.floor((now.getTime() - collectedDate.getTime()) / 1000))
    : null;
  const osName = firstMeaningfulString(
    state?.osName,
    osSystem?.name,
    osSystem?.os_name,
    osRecord?.name,
    osSystem?.distro,
    device?.os,
  );
  const platformFamily = normalizePlatformFamily(osName);

  return {
    agentId: device.agentId,
    hostname: firstMeaningfulString(state?.hostname, osSystem?.hostname, device.hostname),
    customerName: firstString(device.customer?.name),
    siteName: firstString(device.site?.name),
    snapshot: {
      collectedAt,
      ageSeconds,
    },
    platform: {
      family: platformFamily,
      osName,
      osVersion: firstMeaningfulString(state?.osVersion, osSystem?.version, osSystem?.os_version, osRecord?.version),
      architecture: firstMeaningfulString(
        osSystem?.architecture,
        osSystem?.osArchitecture,
        osRecord?.architecture,
        valueAtPath(collection, ["system", "architecture"]),
        valueAtPath(collection, ["system", "os", "architecture"]),
        hardwareCpu?.architecture,
      ),
      timezone: firstMeaningfulString(osSystem?.timezone, osRecord?.timezone),
      locale: firstMeaningfulString(
        osSystem?.locale,
        osSystem?.language,
        osRecord?.locale,
        osRecord?.language,
        valueAtPath(collection, ["system", "locale"]),
        valueAtPath(collection, ["system", "language"]),
      ),
      domain: firstMeaningfulString(
        osSystem?.domain,
        valueAtPath(collection, ["operating_system", "ad_ds", "domain_name"]),
        valueAtPath(collection, ["operatingSystem", "adDs", "domainName"]),
      ),
    },
    agent: {
      version: firstMeaningfulString(state?.agentVersion, device.version),
      lastSeen: device.lastSeen ? toIso(device.lastSeen) : null,
    },
    hardware: {
      cpuModel: firstMeaningfulString(state?.cpuModel, hardwareCpu?.brand, hardwareCpu?.model),
      physicalCores: numberOrNull(state?.cpuPhysicalCores ?? hardwareCpu?.cores),
      logicalCores: numberOrNull(state?.cpuLogicalCores ?? hardwareCpu?.threads),
      memoryTotalBytes: numberOrNull(state?.memoryTotalBytes ?? hardwareMemory?.total_bytes ?? hardwareMemory?.totalBytes),
    },
    state: {
      pendingUpdatesCount: numberOrNull(state?.pendingUpdatesCount),
      rebootRequired: state?.rebootRequired === undefined ? null : booleanValue(state?.rebootRequired),
    },
    network: {
      primaryIp: firstMeaningfulIp(device.ip, firstInventoryIp(collection)),
    },
    shell: shellContextForPlatform(platformFamily),
    security: {
      firewallEnabled: firewallEnabledFromInventory(collection),
      secureBoot: firstBoolean(valueAtPath(collection, ["hardware", "secure_boot"]), valueAtPath(collection, ["hardware", "secureBoot"])),
      tpmPresent: firstBoolean(tpm?.present),
      tpmEnabled: firstBoolean(tpm?.enabled),
      antivirusEnabled: firstBoolean(defender?.enabled, antivirus?.enabled),
      bitlockerEnabled: firstBoolean(valueAtPath(collection, ["security", "bitlocker", "enabled"])),
    },
  };
}

export async function createAndDispatchAiRunnerJob(
  context: CommandCenterConversationContext,
  input: CreateRunnerJobInput,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerJobSummary> {
  const agentId = input.agentId.trim();
  if (!agentId) {
    throw new Error("agentId is required");
  }
  const requestedJobType = input.jobType?.trim() || AI_RUNNER_SHELL_JOB_TYPE;
  const device = await assertDeviceScope(context, agentId, runnerDb);
  const deviceContext = buildAiRunnerDeviceContextFromDevice(device);
  const grant = await findActiveApprovalGrant(context, agentId, requestedJobType, runnerDb);
  const deviceAutoApproved = device.aiRunnerAutoApprove === true;
  const endpointApprovalAlreadyGranted = Boolean(grant || deviceAutoApproved);
  const approvalId = grant?.approvalId || (deviceAutoApproved ? null : randomUUID());
  const now = new Date();
  const approvalExpiresAt = new Date(now.getTime() + APPROVAL_REQUEST_TIMEOUT_MS);
  const approvalWindowExpiresAt = grant?.expiresAt
    ? new Date(grant.expiresAt)
    : deviceAutoApproved
      ? null
      : new Date(now.getTime() + APPROVAL_GRANT_TTL_MS);

  const record = await runnerDb.commandCenterAiRunnerJob.create({
    data: {
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId: input.conversationId || null,
      agentId,
      goal: input.goal?.trim() || "Perform the requested desktop goal",
      jobType: requestedJobType,
      status: endpointApprovalAlreadyGranted ? "approval_granted" : "approval_pending",
      approvalId,
      approvalRequestedAt: endpointApprovalAlreadyGranted ? null : now,
      approvalRespondedAt: endpointApprovalAlreadyGranted ? now : null,
      approvalExpiresAt: endpointApprovalAlreadyGranted ? null : approvalExpiresAt,
      approvalWindowExpiresAt,
    },
  });
  const job = toJobSummary(record);

  try {
    const generatedSecrets = await attachGeneratedSecretsToRunnerJob({
      organizationId: context.organizationId,
      userId: context.userId,
      jobId: job.id,
      agentId,
      secretHandles: input.generatedSecretHandles ?? [],
    });

    return await dispatchAiRunnerJob(context, job, {
      goal: input.goal || "Perform the requested desktop goal",
      jobType: requestedJobType,
      deviceContext,
      generatedSecrets,
      approvalMode: endpointApprovalAlreadyGranted ? "already_granted" : "request",
      approval: endpointApprovalAlreadyGranted
        ? null
        : {
            approvalId: approvalId!,
            requesterLabel:
              input.requesterLabel?.trim() ||
              input.requesterEmail?.trim() ||
              "A Talos operator",
            requesterEmail: input.requesterEmail?.trim() || null,
            organizationName: input.organizationName?.trim() || null,
            deviceLabel: deviceLabel(device),
            reason: input.goal?.trim() || "Perform the requested desktop goal",
            expiresAtUnixMs: unixMs(approvalExpiresAt),
            approvalWindowExpiresAtUnixMs: unixMs(approvalWindowExpiresAt!),
          },
    }, runnerDb);
  } catch (error) {
    await markAiRunnerJobFailed(
      job.id,
      error instanceof Error ? error.message : String(error),
      runnerDb,
    );
    throw error;
  }
}

async function markAiRunnerJobFailed(jobId: string, error: string, runnerDb: RunnerDb = db()) {
  await runnerDb.commandCenterAiRunnerJob
    .update({
      where: { id: jobId },
      data: {
        status: "failed",
        error,
        finishedAt: new Date(),
      },
    })
    .catch(() => undefined);
}

async function dispatchAiRunnerJob(
  context: CommandCenterConversationContext,
  job: AiRunnerJobSummary,
  options: {
    goal: string;
    jobType: string;
    deviceContext: AiRunnerDeviceContext;
    generatedSecrets: GeneratedSecretSummary[];
    approvalMode: "request" | "already_granted";
    approval: null | {
      approvalId: string;
      requesterLabel: string;
      requesterEmail: string | null;
      organizationName: string | null;
      deviceLabel: string;
      reason: string;
      expiresAtUnixMs: number;
      approvalWindowExpiresAtUnixMs: number;
    };
  },
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerJobSummary> {
  const body = buildAiRunnerJobDispatchBody(context, job, {
    ...options,
    callbackBaseUrl: callbackBaseUrl(),
  });
  await runnerDb.commandCenterAiRunnerJob.update({
    where: { id: job.id },
    data: { dispatchRequest: jsonInput(body) },
  });
  const response = await fetch(`${runnerUrl()}/internal/jobs`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-service-key": runnerServiceKey(),
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`AI runner dispatch failed: ${response.status} ${text}`.trim());
  }

  const data = await response.json().catch(() => ({}));
  const updated = await runnerDb.commandCenterAiRunnerJob.update({
    where: { id: job.id },
    data: {
      status: options.approvalMode === "request" ? "approval_pending" : "running",
      runnerId: typeof data?.runnerId === "string" ? data.runnerId : null,
      startedAt: new Date(),
    },
  });
  return toJobSummary(updated);
}

export function buildAiRunnerJobDispatchBody(
  context: Pick<CommandCenterConversationContext, "organizationId" | "userId">,
  job: Pick<AiRunnerJobSummary, "id" | "conversationId" | "agentId">,
  options: {
    goal: string;
    jobType: string;
    deviceContext: AiRunnerDeviceContext;
    generatedSecrets: GeneratedSecretSummary[];
    approvalMode: "request" | "already_granted";
    approval: null | {
      approvalId: string;
      requesterLabel: string;
      requesterEmail: string | null;
      organizationName: string | null;
      deviceLabel: string;
      reason: string;
      expiresAtUnixMs: number;
      approvalWindowExpiresAtUnixMs: number;
    };
    callbackBaseUrl: string | null;
  },
) {
  return {
    jobId: job.id,
    organizationId: context.organizationId,
    userId: context.userId,
    conversationId: job.conversationId,
    agentId: job.agentId,
    jobType: options.jobType,
    goal: options.goal,
    deviceContext: options.deviceContext,
    generatedSecrets: options.generatedSecrets,
    approvalMode: options.approvalMode,
    approval: options.approval,
    callbackBaseUrl: options.callbackBaseUrl,
  };
}

function toLeaseSummary(record: any | null, input: {
  accepted: boolean;
  reason?: string | null;
  leaseId?: string | null;
  leaseExpiresAt?: Date | null;
}): AiRunnerLeaseSummary {
  return {
    accepted: input.accepted,
    reason: input.reason ?? null,
    job: record ? toJobSummary(record) : null,
    leaseId: input.leaseId ?? stringValue(record?.leaseId),
    leaseExpiresAt: input.leaseExpiresAt
      ? toIso(input.leaseExpiresAt)
      : record?.leaseExpiresAt
        ? toIso(record.leaseExpiresAt)
        : null,
    cancelRequestedAt: record?.cancelRequestedAt ? toIso(record.cancelRequestedAt) : null,
  };
}

export async function acquireAiRunnerJobLease(
  jobId: string,
  runnerIdInput: unknown,
  ttlMs = AI_RUNNER_LEASE_TTL_MS,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerLeaseSummary | null> {
  const runnerId = stringValue(runnerIdInput);
  if (!runnerId) throw new Error("runnerId is required");
  const existing = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!existing) return null;
  const status = normalizeStatus(existing.status);
  if (isTerminalStatus(status)) {
    return toLeaseSummary(existing, { accepted: false, reason: "terminal_status" });
  }
  const now = new Date();
  const existingActiveLease = activeLeaseId(existing, now);
  if (existingActiveLease && existing.leaseOwnerRunnerId === runnerId) {
    return toLeaseSummary(existing, { accepted: true, reason: "already_leased" });
  }
  if (existingActiveLease) {
    return toLeaseSummary(existing, { accepted: false, reason: "lease_active" });
  }

  const leaseId = randomUUID();
  const leaseExpiresAt = new Date(now.getTime() + Math.max(1_000, ttlMs));
  const claim = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobId,
      status: { in: ACTIVE_JOB_STATUSES },
      OR: [
        { leaseId: null },
        { leaseExpiresAt: null },
        { leaseExpiresAt: { lte: now } },
      ],
    },
    data: {
      leaseId,
      leaseOwnerRunnerId: runnerId,
      leaseExpiresAt,
      lastHeartbeatAt: now,
      runnerId,
      retryable: false,
      retryReason: null,
    },
  });
  const record = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (claim.count !== 1) {
    return toLeaseSummary(record, { accepted: false, reason: "lease_active" });
  }
  if (record) {
    await createAiRunnerEvent(runnerDb, record, {
      eventKey: `lease_acquired:${leaseId}`,
      eventType: "lease_acquired",
      runnerId,
      leaseId,
      payload: { leaseExpiresAt: leaseExpiresAt.toISOString() },
    });
  }
  return toLeaseSummary(record, { accepted: true, leaseId, leaseExpiresAt });
}

export async function heartbeatAiRunnerJobLease(
  jobId: string,
  leaseIdInput: unknown,
  runnerIdInput: unknown,
  ttlMs = AI_RUNNER_LEASE_TTL_MS,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerLeaseSummary | null> {
  const leaseId = stringValue(leaseIdInput);
  const runnerId = stringValue(runnerIdInput);
  if (!leaseId) throw new Error("leaseId is required");
  if (!runnerId) throw new Error("runnerId is required");
  const now = new Date();
  const leaseExpiresAt = new Date(now.getTime() + Math.max(1_000, ttlMs));
  const claim = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobId,
      leaseId,
      leaseOwnerRunnerId: runnerId,
      leaseExpiresAt: { gt: now },
      status: { in: ACTIVE_JOB_STATUSES },
    },
    data: {
      leaseExpiresAt,
      lastHeartbeatAt: now,
    },
  });
  const record = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!record) return null;
  if (claim.count !== 1) {
    return toLeaseSummary(record, { accepted: false, reason: "lease_lost" });
  }
  return toLeaseSummary(record, { accepted: true, leaseId, leaseExpiresAt });
}

export async function releaseAiRunnerJobLease(
  jobId: string,
  leaseIdInput: unknown,
  runnerIdInput: unknown,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerLeaseSummary | null> {
  const leaseId = stringValue(leaseIdInput);
  const runnerId = stringValue(runnerIdInput);
  if (!leaseId) throw new Error("leaseId is required");
  if (!runnerId) throw new Error("runnerId is required");
  const existing = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!existing) return null;
  const release = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobId,
      leaseId,
      leaseOwnerRunnerId: runnerId,
    },
    data: {
      leaseId: null,
      leaseOwnerRunnerId: null,
      leaseExpiresAt: null,
    },
  });
  const record = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (release.count !== 1) {
    return toLeaseSummary(record, { accepted: false, reason: "lease_lost" });
  }
  await createAiRunnerEvent(runnerDb, existing, {
    eventKey: `lease_released:${leaseId}`,
    eventType: "lease_released",
    runnerId,
    leaseId,
    payload: {},
  }).catch(() => undefined);
  return toLeaseSummary(record, { accepted: true, reason: "released" });
}

export async function getAiRunnerJob(
  context: CommandCenterConversationContext,
  jobId: string,
): Promise<AiRunnerJobSummary | null> {
  const record = await db().commandCenterAiRunnerJob.findFirst({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  return record ? toJobSummary(record) : null;
}

export async function stopAiRunnerJob(
  context: CommandCenterConversationContext,
  jobId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerJobSummary | null> {
  const existing = await runnerDb.commandCenterAiRunnerJob.findFirst({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!existing) {
    return null;
  }
  const existingStatus = normalizeStatus(existing.status);
  if (isTerminalStatus(existingStatus)) {
    return toJobSummary(existing);
  }

  const now = new Date();
  const stopUpdate = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      status: { in: ACTIVE_JOB_STATUSES },
    },
    data: {
      cancelRequestedAt: now,
      status: "stopping",
    },
  });
  const jobAfterStop = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!jobAfterStop) return null;

  await createAiRunnerEvent(runnerDb, jobAfterStop, {
    eventKey: `stop_requested:${now.toISOString()}`,
    eventType: "stop_requested",
    runnerId: stringValue(jobAfterStop.runnerId),
    leaseId: stringValue(jobAfterStop.leaseId),
    payload: {
      requestedByUserId: context.userId,
      previousStatus: existingStatus,
      updated: stopUpdate.count === 1,
    },
  }).catch((error: unknown) => {
    log.warn("AI runner stop event creation failed", {
      jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
  await settleOpenCommandApprovalsForStoppedJob(runnerDb, jobAfterStop, context.userId);

  let stoppedRemotely = false;
  try {
    const response = await fetch(`${runnerUrl()}/internal/jobs/${encodeURIComponent(jobId)}/stop`, {
      method: "POST",
      headers: { "x-service-key": runnerServiceKey() },
    });
    stoppedRemotely = response.status === 404;
    if (!response.ok && response.status !== 404) {
      const text = await response.text().catch(() => "");
      throw new Error(`AI runner stop failed: ${response.status} ${text}`.trim());
    }
  } catch (error) {
    log.warn("AI runner direct stop request failed after recording cancellation", {
      jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  }

  await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      status: { in: ACTIVE_JOB_STATUSES },
    },
    data: stoppedRemotely
      ? {
          status: "stopped",
          finishedAt: new Date(),
        }
      : {
          status: "stopping",
        },
  });
  const record = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!record) return null;
  return toJobSummary(record);
}

async function settleOpenCommandApprovalsForStoppedJob(
  runnerDb: RunnerDb,
  job: any,
  userId: string,
) {
  const approvals = await runnerDb.commandCenterAiRunnerCommandApproval.findMany({
    where: {
      jobId: job.id,
      organizationId: job.organizationId,
      userId: job.userId,
      status: { in: ["pending", "approved", "executing"] },
    },
  });
  for (const approval of approvals) {
    const status = approval.status === "executing" ? "failed" : "denied";
    const updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
      where: { id: approval.id },
      data: {
        status,
        decidedByUserId: approval.decidedByUserId ?? userId,
        decidedAt: approval.decidedAt ?? new Date(),
        ...(status === "failed"
          ? {
              error: approval.error ?? "Command execution stopped by operator.",
              executedAt: approval.executedAt ?? new Date(),
            }
          : {}),
      },
    });
    await updateCommandApprovalMessage(updated, runnerDb);
  }
  if (approvals.length > 0) {
    await touchAiRunnerJob(job.id, runnerDb);
  }
}

export async function stopAiRunnerJobsForConversation(
  context: CommandCenterConversationContext,
  conversationId: string,
): Promise<AiRunnerJobDetail[]> {
  const conversation = await db().commandCenterConversation.findFirst({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    select: { id: true },
  });
  if (!conversation) {
    return [];
  }

  const jobs = await db().commandCenterAiRunnerJob.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId,
      status: { in: ACTIVE_JOB_STATUSES },
    },
    orderBy: { updatedAt: "desc" },
  });
  for (const job of jobs) {
    await stopAiRunnerJob(context, job.id).catch((error: unknown) => {
      log.warn("AI runner conversation stop failed for job", {
        jobId: job.id,
        conversationId,
        error: error instanceof Error ? error.message : String(error),
      });
    });
  }
  const stopped = await db().commandCenterAiRunnerJob.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      id: { in: jobs.map((job: any) => job.id) },
    },
    orderBy: { updatedAt: "desc" },
  });
  return attachJobDetails(context, stopped.map(toJobSummary));
}

export async function updateAiRunnerJobStatusFromCallback(
  jobId: string,
  input: RunnerCallbackStatusInput,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerJobSummary | null> {
  const existing = await runnerDb.commandCenterAiRunnerJob.findUnique({
    where: { id: jobId },
  });
  if (!existing) {
    return null;
  }
  const status = normalizeStatus(input.status);
  const lease = assertCallbackLease(existing, input);
  const eventKey = eventKeyValue(input.eventKey, `status:${status}:legacy`);
  const event = await createAiRunnerEvent(runnerDb, existing, {
    eventKey,
    eventType: "status",
    runnerId: lease.runnerId,
    leaseId: lease.leaseId,
    payload: input,
  });
  if (!event.created) {
    return toJobSummary(existing);
  }
  const existingStatus = normalizeStatus(existing.status);
  const existingTerminal = isTerminalStatus(existingStatus);
  const nextTerminal = isTerminalStatus(status);
  if (existingTerminal) {
    return toJobSummary(existing);
  }
  if (existing.cancelRequestedAt && nextTerminal && status !== "stopped") {
    const record = await runnerDb.commandCenterAiRunnerJob.update({
      where: { id: jobId },
      data: {
        status: "stopped",
        error: typeof input.error === "string" && input.error.trim() ? input.error.trim() : existing.error,
        finishedAt: existing.finishedAt ?? new Date(),
      },
    });
    return toJobSummary(record);
  }
  const resultRecord = asRecord(input.result);
  const approvalId = stringValue(resultRecord?.approvalId) || stringValue(resultRecord?.approval_id) || existing.approvalId;
  const approvalChatSessionId =
    stringValue(resultRecord?.approvalChatSessionId) ||
    stringValue(resultRecord?.approval_chat_session_id) ||
    existing.approvalChatSessionId;
  const approvalExpiresAt =
    dateFromUnixMs(resultRecord?.approvalExpiresAtUnixMs) ||
    dateFromUnixMs(resultRecord?.approval_expires_at_unix_ms) ||
    existing.approvalExpiresAt;
  const approvalWindowExpiresAt =
    dateFromUnixMs(resultRecord?.approvalWindowExpiresAtUnixMs) ||
    dateFromUnixMs(resultRecord?.approval_window_expires_at_unix_ms) ||
    existing.approvalWindowExpiresAt;
  const approvalRespondedAt =
    status === "approval_granted" || status === "approval_denied"
      ? new Date()
      : existing.approvalRespondedAt;

  const record = await runnerDb.commandCenterAiRunnerJob.update({
    where: { id: jobId },
    data: {
      status,
      runnerId: typeof input.runnerId === "string" ? input.runnerId : existing.runnerId,
      approvalId,
      approvalChatSessionId,
      approvalExpiresAt,
      approvalWindowExpiresAt,
      approvalRespondedAt,
      result: jsonInput(input.result),
      error: typeof input.error === "string" && input.error.trim() ? input.error.trim() : null,
      startedAt: existing.startedAt ?? new Date(),
      finishedAt: nextTerminal ? new Date() : existing.finishedAt,
    },
  });
  if (status === "approval_granted" && approvalId && approvalWindowExpiresAt) {
    await runnerDb.commandCenterAiRunnerApprovalGrant.create({
      data: {
        organizationId: record.organizationId,
        userId: record.userId,
        agentId: record.agentId,
        jobType: record.jobType ?? AI_RUNNER_JOB_TYPE,
        approvalId,
        jobId: record.id,
        expiresAt: approvalWindowExpiresAt,
      },
    }).catch((error: unknown) => {
      log.warn("AI runner approval grant creation failed", {
        jobId: record.id,
        error: error instanceof Error ? error.message : String(error),
      });
    });
  }
  if (nextTerminal) {
    await appendAssistantResultMessageIfNeeded(record, runnerDb);
  }
  return toJobSummary(record);
}

function secureNoteLinksMarkdown(
  notes: Awaited<ReturnType<typeof listGeneratedSecureNotesForJob>>,
): string {
  if (notes.length === 0) return "";
  const lines = notes.map((note, index) => {
    const purpose = note.purpose
      ?.trim()
      .replace(/[\r\n\t]+/g, " ")
      .replace(/[[\]()`<>]/g, "")
      .slice(0, 120);
    const expires = note.expiresAt ? `, expires ${note.expiresAt}` : "";
    const label = purpose || `Generated secret ${index + 1}`;
    return `- ${label}: ${note.secureNoteUrl}${expires}`;
  });
  return ["", "Generated secure notes:", ...lines].join("\n");
}

function approvalUnavailableNoInteractiveUserMessage(resultRecord: Record<string, any> | null): string | null {
  if (
    stringValue(resultRecord?.phase) !== APPROVAL_UNAVAILABLE_PHASE ||
    stringValue(resultRecord?.reason) !== NO_INTERACTIVE_USER_REASON
  ) {
    return null;
  }
  return (
    stringValue(resultRecord?.summary) ||
    stringValue(resultRecord?.message) ||
    NO_INTERACTIVE_USER_APPROVAL_MESSAGE
  );
}

async function appendAssistantResultMessageIfNeeded(jobRecord: any, runnerDb: RunnerDb = db()) {
  if (!jobRecord.conversationId || jobRecord.resultMessageId) {
    return;
  }
  const artifacts = await runnerDb.commandCenterAiRunnerArtifact.findMany({
    where: {
      jobId: jobRecord.id,
      organizationId: jobRecord.organizationId,
      userId: jobRecord.userId,
    },
    orderBy: { createdAt: "asc" },
  }).catch((error: unknown) => {
    log.warn("AI runner evidence artifact lookup failed", {
      jobId: jobRecord.id,
      error: error instanceof Error ? error.message : String(error),
    });
    return [];
  });
  const evidence = artifactEvidenceSummary(toJobSummary(jobRecord), artifacts.map(toArtifactSummary));
  const generatedNotes = await listGeneratedSecureNotesForJob(jobRecord.id).catch((error: unknown) => {
    log.warn("AI runner secure note link lookup failed", {
      jobId: jobRecord.id,
      error: error instanceof Error ? error.message : String(error),
    });
    return [];
  });
  const status = normalizeStatus(jobRecord.status);
  const resultRecord = asRecord(jobRecord.result);
  const approvalUnavailableMessage = approvalUnavailableNoInteractiveUserMessage(resultRecord);
  if (
    status !== "succeeded" &&
    generatedNotes.length === 0 &&
    !hasAiRunnerEvidence(evidence) &&
    !approvalUnavailableMessage
  ) {
    return;
  }
  const claimId = `pending:${randomUUID()}`;
  const claim = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: jobRecord.id,
      resultMessageId: null,
    },
    data: {
      resultMessageId: claimId,
    },
  });
  if (claim.count !== 1) {
    return;
  }
  const summary = [
    approvalUnavailableMessage ||
      stringValue(resultRecord?.summary) ||
      stringValue(resultRecord?.message) ||
      (status === "succeeded" ? "Desktop goal finished." : jobRecord.error || "AI runner stopped before completion."),
    secureNoteLinksMarkdown(generatedNotes),
  ].filter((value) => value.trim()).join("\n\n");
  const message = await appendCommandCenterMessage(
    { organizationId: jobRecord.organizationId, userId: jobRecord.userId },
    jobRecord.conversationId,
    {
      role: "assistant",
      content: summary,
      model: "talos-ai-runner",
      responseId: jobRecord.id,
      metadata: { aiRunnerJob: evidence },
    },
    runnerDb as any,
  ).catch(async (error: unknown) => {
    await clearResultMessageClaim(jobRecord.id, claimId, runnerDb);
    throw error;
  });
  if (!message) {
    log.warn("AI runner result message append skipped; conversation unavailable", {
      jobId: jobRecord.id,
      conversationId: jobRecord.conversationId,
    });
    await clearResultMessageClaim(jobRecord.id, claimId, runnerDb);
    return;
  }
  await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: { id: jobRecord.id, resultMessageId: claimId },
    data: { resultMessageId: message.id },
  }).catch(async (error: unknown) => {
    log.warn("AI runner result message id update failed", {
      jobId: jobRecord.id,
      messageId: message.id,
      error: error instanceof Error ? error.message : String(error),
    });
    await clearResultMessageClaim(jobRecord.id, claimId, runnerDb);
  });
}

async function clearResultMessageClaim(jobId: string, claimId: string, runnerDb: RunnerDb = db()) {
  await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: { id: jobId, resultMessageId: claimId },
    data: { resultMessageId: null },
  }).catch((error: unknown) => {
    log.warn("AI runner result message claim cleanup failed", {
      jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

function commandApprovalMessageContent(approval: AiRunnerCommandApprovalSummary): string {
  const executedLabel =
    approval.exitCode === null || approval.exitCode === undefined
      ? "Command completed."
      : approval.exitCode === 0
        ? "Command completed."
        : "Command completed with a non-zero exit code.";
  const statusLabel: Record<AiRunnerCommandApprovalStatus, string> = {
    pending: "Command approval requested.",
    approved: "Command approved. Waiting for execution.",
    denied: "Command denied. The shell runner was stopped.",
    desktop_control_requested: "Desktop control requested. Transferring the runner to the desktop.",
    executing: "Approved command is running.",
    executed: executedLabel,
    failed: "Command execution failed.",
    expired: "Command approval expired.",
    policy_blocked: "Command blocked by policy.",
  };
  return [
    statusLabel[approval.status],
    "",
    "```",
    approval.command,
    "```",
    "",
    approval.message || approval.explanation,
  ].join("\n");
}

function commandApprovalMessageMetadata(approval: AiRunnerCommandApprovalSummary) {
  return {
    commandApproval: approval,
  };
}

async function updateCommandApprovalMessage(record: any, runnerDb: RunnerDb = db()) {
  const approval = toCommandApprovalSummary(record);
  if (!approval.messageId) return;
  await runnerDb.commandCenterMessage.updateMany({
    where: { id: approval.messageId },
    data: {
      content: commandApprovalMessageContent(approval),
      metadata: commandApprovalMessageMetadata(approval),
    },
  }).catch((error: unknown) => {
    log.warn("AI runner command approval message update failed", {
      approvalId: approval.id,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

async function touchAiRunnerJob(jobId: string, runnerDb: RunnerDb = db()) {
  await runnerDb.commandCenterAiRunnerJob.update({
    where: { id: jobId },
    data: { updatedAt: new Date() },
  }).catch((error: unknown) => {
    log.warn("AI runner job touch failed", {
      jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

function truncateCommandOutput(value: unknown): { output: string | null; outputLength: number | null } {
  if (typeof value !== "string") return { output: null, outputLength: null };
  const outputLength = value.length;
  if (value.length <= MAX_COMMAND_OUTPUT_CHARS) return { output: value, outputLength };
  return {
    output: `${value.slice(0, MAX_COMMAND_OUTPUT_CHARS)}\n...output truncated...`,
    outputLength,
  };
}

export async function createAiRunnerCommandApprovalFromCallback(
  jobId: string,
  input: {
    turnIndex?: unknown;
    command?: unknown;
    explanation?: unknown;
    risk?: unknown;
    notes?: unknown;
    message?: unknown;
    modelResponseId?: unknown;
    runnerId?: unknown;
    leaseId?: unknown;
    eventKey?: unknown;
  },
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandApprovalSummary | null> {
  const job = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!job) return null;
  const lease = assertCallbackLease(job, input);
  const turnIndex = Math.max(0, Math.trunc(numberValue(input.turnIndex) ?? 0));
  const command = stringValue(input.command) ?? "";
  const explanation = stringValue(input.explanation) ?? "";
  const risk = stringValue(input.risk) ?? "";
  if (!command || !explanation || !risk) {
    throw new Error("command, explanation, and risk are required");
  }
  const eventKey = eventKeyValue(input.eventKey, `command_proposal:${turnIndex}`);
  const event = await createAiRunnerEvent(runnerDb, job, {
    eventKey,
    eventType: "command_proposal",
    runnerId: lease.runnerId,
    leaseId: lease.leaseId,
    turnIndex,
    payload: input,
  });
  if (!event.created) {
    if (event.record.commandApprovalId) {
      const existingByEvent = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
        where: { id: event.record.commandApprovalId, jobId },
      });
      if (existingByEvent) return toCommandApprovalSummary(existingByEvent);
    }
  }
  const existing = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: { jobId, turnIndex },
  });
  if (existing) {
    await linkAiRunnerEvent(runnerDb, event.record.id, { commandApprovalId: existing.id });
    return toCommandApprovalSummary(existing);
  }

  const expiresAt = new Date(Date.now() + COMMAND_APPROVAL_TIMEOUT_MS);
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.create({
    data: {
      jobId,
      organizationId: job.organizationId,
      userId: job.userId,
      conversationId: job.conversationId ?? null,
      agentId: job.agentId,
      turnIndex,
      command,
      explanation,
      risk,
      notes: jsonInput(stringArrayValue(input.notes)),
      message: stringValue(input.message),
      modelResponseId: stringValue(input.modelResponseId),
      policyAllowed: null,
      policyReason: null,
      matchedPolicyId: null,
      expiresAt,
      status: "pending",
    },
  });
  await linkAiRunnerEvent(runnerDb, event.record.id, { commandApprovalId: record.id });
  let updated = record;
  if (job.conversationId) {
    const approval = toCommandApprovalSummary(record);
    const message = await appendCommandCenterMessage(
      { organizationId: job.organizationId, userId: job.userId },
      job.conversationId,
      {
        role: "assistant",
        content: commandApprovalMessageContent(approval),
        model: "talos-ai-runner",
        responseId: `${jobId}:command:${turnIndex}`,
        metadata: commandApprovalMessageMetadata(approval),
      },
      runnerDb as any,
    );
    if (message) {
      updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
        where: { id: record.id },
        data: { messageId: message.id },
      });
      await updateCommandApprovalMessage(updated, runnerDb);
    }
  }
  await touchAiRunnerJob(jobId, runnerDb);
  return toCommandApprovalSummary(updated);
}

export async function getAiRunnerCommandApprovalFromCallback(
  jobId: string,
  approvalId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandApprovalSummary | null> {
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: { id: approvalId, jobId },
  });
  if (!record) return null;
  const expiresAt = record.expiresAt instanceof Date ? record.expiresAt : null;
  if (record.status === "pending" && expiresAt && expiresAt.getTime() <= Date.now()) {
    const updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
      where: { id: approvalId },
      data: { status: "expired" },
    });
    await updateCommandApprovalMessage(updated, runnerDb);
    await touchAiRunnerJob(jobId, runnerDb);
    return toCommandApprovalSummary(updated);
  }
  return toCommandApprovalSummary(record);
}

export async function updateAiRunnerCommandApprovalExecutionFromCallback(
  jobId: string,
  approvalId: string,
  input: {
    status?: unknown;
    output?: unknown;
    exitCode?: unknown;
    error?: unknown;
    runnerId?: unknown;
    leaseId?: unknown;
    eventKey?: unknown;
  },
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandApprovalSummary | null> {
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: { id: approvalId, jobId },
  });
  if (!record) return null;
  const job = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!job) return null;
  const lease = assertCallbackLease(job, input);
  const requestedStatus = normalizeCommandApprovalStatus(input.status);
  const status =
    requestedStatus === "executing" || requestedStatus === "executed" || requestedStatus === "failed"
      ? requestedStatus
      : "executed";
  const eventKey = eventKeyValue(input.eventKey, `command_result:${approvalId}:${status}`);
  const event = await createAiRunnerEvent(runnerDb, job, {
    eventKey,
    eventType: "command_result",
    runnerId: lease.runnerId,
    leaseId: lease.leaseId,
    turnIndex: Number(record.turnIndex) || 0,
    commandApprovalId: approvalId,
    payload: input,
  });
  if (!event.created) {
    return toCommandApprovalSummary(record);
  }
  const { output, outputLength } = truncateCommandOutput(input.output);
  const exitCode = numberValue(input.exitCode);
  const updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
    where: { id: approvalId },
    data: {
      status,
      ...(output !== null ? { output } : {}),
      ...(outputLength !== null ? { outputLength } : {}),
      exitCode: exitCode === undefined ? null : Math.trunc(exitCode),
      error: stringValue(input.error),
      executedAt: status === "executed" || status === "failed" ? new Date() : record.executedAt,
    },
  });
  await updateCommandApprovalMessage(updated, runnerDb);
  await touchAiRunnerJob(jobId, runnerDb);
  return toCommandApprovalSummary(updated);
}

export async function approveAiRunnerCommandApproval(
  context: CommandCenterConversationContext,
  approvalId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandApprovalSummary | null> {
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: {
      id: approvalId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!record) return null;
  if (record.status !== "pending") return toCommandApprovalSummary(record);
  const updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
    where: { id: approvalId },
    data: {
      status: "approved",
      decidedByUserId: context.userId,
      decidedAt: new Date(),
    },
  });
  await updateCommandApprovalMessage(updated, runnerDb);
  await touchAiRunnerJob(updated.jobId, runnerDb);
  return toCommandApprovalSummary(updated);
}

export async function denyAiRunnerCommandApproval(
  context: CommandCenterConversationContext,
  approvalId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandApprovalSummary | null> {
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: {
      id: approvalId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!record) return null;
  if (record.status !== "pending" && record.status !== "approved") return toCommandApprovalSummary(record);
  const updated = await runnerDb.commandCenterAiRunnerCommandApproval.update({
    where: { id: approvalId },
    data: {
      status: "denied",
      decidedByUserId: context.userId,
      decidedAt: new Date(),
    },
  });
  await updateCommandApprovalMessage(updated, runnerDb);
  await touchAiRunnerJob(updated.jobId, runnerDb);
  await stopAiRunnerJob(context, updated.jobId).catch((error: unknown) => {
    log.warn("AI runner stop after command denial failed", {
      approvalId,
      jobId: updated.jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
  return toCommandApprovalSummary(updated);
}

export async function denyAiRunnerCommandApprovalAndUseDesktopControl(
  context: CommandCenterConversationContext,
  approvalId: string,
  runnerDb: RunnerDb = db(),
): Promise<{ approval: AiRunnerCommandApprovalSummary; job: AiRunnerJobSummary | null } | null> {
  const record = await runnerDb.commandCenterAiRunnerCommandApproval.findFirst({
    where: {
      id: approvalId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!record) return null;

  const sourceJob = await runnerDb.commandCenterAiRunnerJob.findFirst({
    where: {
      id: record.jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!sourceJob) return null;

  const previousJobType = sourceJob.jobType ?? AI_RUNNER_SHELL_JOB_TYPE;
  if (
    previousJobType !== AI_RUNNER_SHELL_JOB_TYPE ||
    !ACTIVE_JOB_STATUSES.includes(normalizeStatus(sourceJob.status)) ||
    (record.status !== "pending" && record.status !== "approved")
  ) {
    return { approval: toCommandApprovalSummary(record), job: toJobSummary(sourceJob) };
  }

  const now = new Date();
  const updatedApproval = await runnerDb.commandCenterAiRunnerCommandApproval.update({
    where: { id: approvalId },
    data: {
      status: "desktop_control_requested",
      decidedByUserId: context.userId,
      decidedAt: now,
    },
  });
  await updateCommandApprovalMessage(updatedApproval, runnerDb);

  const updatedJob = await runnerDb.commandCenterAiRunnerJob.update({
    where: { id: sourceJob.id },
    data: {
      jobType: AI_RUNNER_JOB_TYPE,
      status: "running",
      error: null,
    },
  });

  await createAiRunnerEvent(runnerDb, updatedJob, {
    eventKey: `desktop_control_requested:${approvalId}`,
    eventType: "desktop_control_requested",
    runnerId: stringValue(updatedJob.runnerId),
    leaseId: stringValue(updatedJob.leaseId),
    turnIndex: Number(record.turnIndex) || 0,
    commandApprovalId: approvalId,
    payload: {
      requestedByUserId: context.userId,
      previousJobType,
      command: record.command,
    },
  }).catch((error: unknown) => {
    log.warn("AI runner desktop-control transfer event creation failed", {
      approvalId,
      jobId: sourceJob.id,
      error: error instanceof Error ? error.message : String(error),
    });
  });

  return {
    approval: toCommandApprovalSummary(updatedApproval),
    job: toJobSummary(updatedJob),
  };
}

function liveFrameMessageContent(input: RunnerArtifactCallbackInput): string {
  return typeof input.messageContent === "string" && input.messageContent.trim()
    ? input.messageContent.trim()
    : "Desktop view updated.";
}

function liveFrameMessageMetadata(attachment: CommandCenterMessageAttachment) {
  return {
    attachments: [
      {
        ...attachment,
        presentation: "live_frame",
      },
    ],
  };
}

async function updateLiveFrameMessage(
  runnerDb: RunnerDb,
  job: any,
  liveFrameMessageId: string,
  attachment: CommandCenterMessageAttachment,
  content: string,
): Promise<boolean> {
  if (!job.conversationId) {
    return false;
  }
  const update = await runnerDb.commandCenterMessage.updateMany({
    where: {
      id: liveFrameMessageId,
      conversationId: job.conversationId,
    },
    data: {
      content,
      metadata: liveFrameMessageMetadata(attachment),
    },
  });
  if (update.count !== 1) {
    return false;
  }
  await runnerDb.commandCenterConversation.update({
    where: { id: job.conversationId },
    data: { updatedAt: new Date() },
  }).catch((error: unknown) => {
    log.warn("AI runner live frame conversation timestamp update failed", {
      jobId: job.id,
      conversationId: job.conversationId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
  return true;
}

async function clearLiveFrameMessageClaim(runnerDb: RunnerDb, jobId: string, claimId: string) {
  await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: { id: jobId, liveFrameMessageId: claimId },
    data: { liveFrameMessageId: null },
  }).catch((error: unknown) => {
    log.warn("AI runner live frame message claim cleanup failed", {
      jobId,
      error: error instanceof Error ? error.message : String(error),
    });
  });
}

async function upsertLiveFrameMessageForArtifact(
  runnerDb: RunnerDb,
  job: any,
  artifact: AiRunnerArtifactSummary,
  attachment: CommandCenterMessageAttachment,
  input: RunnerArtifactCallbackInput,
) {
  if (!job.conversationId) {
    return;
  }

  const content = liveFrameMessageContent(input);
  const existingMessageId = typeof job.liveFrameMessageId === "string" ? job.liveFrameMessageId : null;
  if (existingMessageId && !existingMessageId.startsWith("pending:")) {
    const updated = await updateLiveFrameMessage(runnerDb, job, existingMessageId, attachment, content);
    if (updated) {
      return;
    }
    await runnerDb.commandCenterAiRunnerJob.updateMany({
      where: { id: job.id, liveFrameMessageId: existingMessageId },
      data: { liveFrameMessageId: null },
    });
  }

  const claimId = `pending:${randomUUID()}`;
  const claim = await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: {
      id: job.id,
      liveFrameMessageId: null,
    },
    data: {
      liveFrameMessageId: claimId,
    },
  });
  if (claim.count !== 1) {
    const latest = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: job.id } });
    const latestMessageId = typeof latest?.liveFrameMessageId === "string" ? latest.liveFrameMessageId : null;
    if (latestMessageId && !latestMessageId.startsWith("pending:")) {
      await updateLiveFrameMessage(runnerDb, latest, latestMessageId, attachment, content);
    }
    return;
  }

  const message = await appendCommandCenterMessage(
    { organizationId: job.organizationId, userId: job.userId },
    job.conversationId,
    {
      role: "assistant",
      content,
      model: "talos-ai-runner",
      responseId: `${job.id}:live-frame`,
      metadata: liveFrameMessageMetadata(attachment),
    },
    runnerDb as any,
  ).catch(async (error: unknown) => {
    await clearLiveFrameMessageClaim(runnerDb, job.id, claimId);
    throw error;
  });
  if (!message) {
    log.warn("AI runner live frame message append skipped; conversation unavailable", {
      jobId: job.id,
      artifactId: artifact.id,
      conversationId: job.conversationId,
    });
    await clearLiveFrameMessageClaim(runnerDb, job.id, claimId);
    return;
  }

  await runnerDb.commandCenterAiRunnerJob.updateMany({
    where: { id: job.id, liveFrameMessageId: claimId },
    data: { liveFrameMessageId: message.id },
  }).catch(async (error: unknown) => {
    log.warn("AI runner live frame message id update failed", {
      jobId: job.id,
      artifactId: artifact.id,
      messageId: message.id,
      error: error instanceof Error ? error.message : String(error),
    });
    await clearLiveFrameMessageClaim(runnerDb, job.id, claimId);
  });
}

export async function appendAiRunnerArtifactFromCallback(
  jobId: string,
  input: RunnerArtifactCallbackInput,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerArtifactSummary | null> {
  const job = await runnerDb.commandCenterAiRunnerJob.findUnique({
    where: { id: jobId },
  });
  if (!job) {
    return null;
  }
  const lease = assertCallbackLease(job, input);
  const contentBase64 = typeof input.contentBase64 === "string" ? input.contentBase64.trim() : "";
  if (!contentBase64) {
    throw new Error("contentBase64 is required");
  }
  if (contentBase64.length > MAX_ARTIFACT_BASE64_CHARS) {
    throw new Error("artifact content is too large");
  }
  const chatPresentation = chatPresentationValue(input.chatPresentation);
  const rawMetadata = asRecord(input.metadata);
  const artifactType = typeof input.artifactType === "string" ? input.artifactType : "runner-artifact";
  const name = typeof input.name === "string" && input.name.trim() ? input.name.trim() : "runner-artifact";
  const artifactFrameId = eventFrameId(input.metadata);
  const eventKey = eventKeyValue(
    input.eventKey,
    `artifact:${artifactType}:${artifactFrameId || name}`,
  );
  const event = await createAiRunnerEvent(runnerDb, job, {
    eventKey,
    eventType: "artifact",
    runnerId: lease.runnerId,
    leaseId: lease.leaseId,
    artifactFrameId,
    payload: input,
  });
  if (!event.created) {
    if (event.record.artifactId) {
      const existingArtifact = await runnerDb.commandCenterAiRunnerArtifact.findFirst({
        where: { id: event.record.artifactId, jobId },
      });
      if (existingArtifact) return toArtifactSummary(existingArtifact);
    }
    const fallback = await runnerDb.commandCenterAiRunnerArtifact.findFirst({
      where: { jobId, name },
      orderBy: { createdAt: "desc" },
    });
    return fallback ? toArtifactSummary(fallback) : null;
  }
  const metadata =
    chatPresentation === "live_frame"
      ? {
          ...(rawMetadata ?? {}),
          presentation: "live_frame",
          jobId,
        }
      : input.metadata;
  const record = await runnerDb.commandCenterAiRunnerArtifact.create({
    data: {
      jobId,
      organizationId: job.organizationId,
      userId: job.userId,
      artifactType,
      name,
      mimeType: typeof input.mimeType === "string" && input.mimeType.trim() ? input.mimeType.trim() : "application/octet-stream",
      contentBase64,
      metadata: jsonInput(metadata),
    },
  });
  await linkAiRunnerEvent(runnerDb, event.record.id, { artifactId: record.id });
  const artifact = toArtifactSummary(record);
  if (chatPresentation === "live_frame" && job.conversationId) {
    const attachment = artifactToAttachment(artifact);
    if (attachment) {
      await upsertLiveFrameMessageForArtifact(runnerDb, job, artifact, attachment, input).catch((error: unknown) => {
        log.warn("AI runner live frame message update failed", {
          jobId,
          artifactId: artifact.id,
          error: error instanceof Error ? error.message : String(error),
        });
      });
    }
  } else if (input.appendToChat === true && job.conversationId) {
    const attachment = artifactToAttachment(artifact);
    if (attachment) {
      const content =
        typeof input.messageContent === "string" && input.messageContent.trim()
          ? input.messageContent.trim()
          : "Desktop view updated.";
      await appendCommandCenterMessage(
        { organizationId: job.organizationId, userId: job.userId },
        job.conversationId,
        {
          role: "assistant",
          content,
          model: "talos-ai-runner",
          responseId: `${jobId}:${artifact.id}`,
          metadata: { attachments: [attachment] },
        },
      ).catch((error: unknown) => {
        log.warn("AI runner artifact chat append failed", {
          jobId,
          artifactId: artifact.id,
          error: error instanceof Error ? error.message : String(error),
        });
      });
    }
  }
  return artifact;
}

export async function listAiRunnerArtifactsForJob(
  context: CommandCenterConversationContext,
  jobId: string,
): Promise<AiRunnerArtifactSummary[]> {
  const job = await getAiRunnerJob(context, jobId);
  if (!job) {
    return [];
  }
  const records = await db().commandCenterAiRunnerArtifact.findMany({
    where: {
      jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    orderBy: { createdAt: "asc" },
  });
  return records.map(toArtifactSummary);
}

export async function waitForAiRunnerJob(
  context: CommandCenterConversationContext,
  jobId: string,
  timeoutMs = DEFAULT_RUNNER_WAIT_TIMEOUT_MS,
  abortSignal?: AbortSignal,
): Promise<{
  job: AiRunnerJobSummary;
  artifacts: AiRunnerArtifactSummary[];
  attachments: CommandCenterMessageAttachment[];
  timedOut: boolean;
}> {
  const deadline = Date.now() + Math.max(1_000, timeoutMs);
  for (;;) {
    throwIfAiRunnerWaitAborted(abortSignal);
    const job = await getAiRunnerJob(context, jobId);
    if (!job) {
      throw new Error("AI runner job not found");
    }
    if (isTerminalStatus(job.status)) {
      const artifacts = await listAiRunnerArtifactsForJob(context, jobId);
      return {
        job,
        artifacts,
        attachments: artifacts.map(artifactToAttachment).filter(Boolean) as CommandCenterMessageAttachment[],
        timedOut: false,
      };
    }
    if (Date.now() >= deadline) {
      return {
        job,
        artifacts: [],
        attachments: [],
        timedOut: true,
      };
    }
    await waitForAiRunnerPollInterval(DEFAULT_RUNNER_POLL_INTERVAL_MS, abortSignal);
  }
}

function throwIfAiRunnerWaitAborted(abortSignal?: AbortSignal) {
  if (abortSignal?.aborted) {
    throw new Error("AI runner wait stopped by operator");
  }
}

function waitForAiRunnerPollInterval(ms: number, abortSignal?: AbortSignal): Promise<void> {
  if (!abortSignal) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      abortSignal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timeout);
      reject(new Error("AI runner wait stopped by operator"));
    };
    if (abortSignal.aborted) {
      onAbort();
      return;
    }
    abortSignal.addEventListener("abort", onAbort, { once: true });
  });
}

const ACTIVE_JOB_STATUSES: AiRunnerJobStatus[] = [
  "queued",
  "approval_pending",
  "approval_granted",
  "running",
  "stopping",
];

export async function appendAiRunnerEventFromCallback(
  jobId: string,
  input: {
    eventKey?: unknown;
    eventType?: unknown;
    runnerId?: unknown;
    leaseId?: unknown;
    turnIndex?: unknown;
    artifactFrameId?: unknown;
    commandApprovalId?: unknown;
    artifactId?: unknown;
    payload?: unknown;
  },
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerEventSummary | null> {
  const job = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: jobId } });
  if (!job) return null;
  const lease = assertCallbackLease(job, input);
  const eventType = stringValue(input.eventType) || "runner_event";
  const eventKey = eventKeyValue(input.eventKey, `${eventType}:${Date.now()}`);
  const payload = asRecord(input.payload);
  const outputTerminal = payload?.terminal === true;
  const event = await createAiRunnerEvent(runnerDb, job, {
    eventKey,
    eventType,
    runnerId: lease.runnerId,
    leaseId: lease.leaseId,
    turnIndex: numberOrNull(input.turnIndex),
    artifactFrameId: stringValue(input.artifactFrameId),
    commandApprovalId: stringValue(input.commandApprovalId),
    artifactId: stringValue(input.artifactId),
    payload: input.payload ?? input,
  });
  if (event.created && eventType === COMMAND_OUTPUT_DELTA_EVENT_TYPE) {
    const approvalId =
      stringValue(input.commandApprovalId) ||
      stringValue(payload?.approvalId) ||
      stringValue(payload?.approval_id);
    const sequence = numberValue(payload?.sequence);
    if (approvalId && (outputTerminal || sequence === undefined || Math.trunc(sequence) % 20 === 0)) {
      await pruneCommandOutputDeltaEvents(runnerDb, jobId, approvalId);
    }
  }
  if (eventType !== COMMAND_OUTPUT_DELTA_EVENT_TYPE || outputTerminal) {
    await touchAiRunnerJob(jobId, runnerDb);
  }
  return toEventSummary(event.record);
}

function aiRunnerReconcilerEnabled(): boolean {
  const raw = String(process.env.COMMAND_CENTER_AI_RUNNER_LEASE_RECONCILER_ENABLED || "true")
    .trim()
    .toLowerCase();
  return raw !== "0" && raw !== "false" && raw !== "off";
}

async function cleanupAiRunnerSession(event: any) {
  const payload = asRecord(event.payload);
  const sessionId = stringValue(payload?.sessionId) || stringValue(payload?.session_id);
  const kind = stringValue(payload?.kind);
  const agentId = stringValue(payload?.agentId) || stringValue(payload?.agent_id) || stringValue(event.agentId);
  const baseUrl = (env.rmmServerUrl || "").trim().replace(/\/+$/, "");
  const serverKey = (env.rmmServerApiKey || "").trim();
  if (!sessionId || !kind || !agentId || !baseUrl || !serverKey) return;
  const response = await fetch(`${baseUrl}/api/rmm/internal/ai-runner/sessions/cleanup`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-rmm-server-key": serverKey,
    },
    body: JSON.stringify({ sessionId, kind, agentId }),
  }).catch((error: unknown) => {
    log.warn("AI runner stale session cleanup request failed", {
      sessionId,
      kind,
      agentId,
      error: error instanceof Error ? error.message : String(error),
    });
    return null;
  });
  if (response && !response.ok) {
    const body = await response.text().catch(() => "");
    log.warn("AI runner stale session cleanup request rejected", {
      sessionId,
      kind,
      agentId,
      status: response.status,
      body,
    });
  }
}

async function cleanupExpiredLeaseSessions(job: any, runnerDb: RunnerDb) {
  const events = await runnerDb.commandCenterAiRunnerEvent.findMany({
    where: {
      jobId: job.id,
      eventType: "session_started",
    },
    orderBy: { createdAt: "asc" },
  });
  for (const event of events) {
    await cleanupAiRunnerSession(event);
  }
}

export async function reconcileExpiredAiRunnerJobLeases(
  now = new Date(),
  runnerDb: RunnerDb = db(),
): Promise<number> {
  const expired = await runnerDb.commandCenterAiRunnerJob.findMany({
    where: {
      status: { in: ACTIVE_JOB_STATUSES },
      leaseId: { not: null },
      leaseExpiresAt: { lte: now },
    },
    orderBy: { leaseExpiresAt: "asc" },
    take: 50,
  });
  let reconciled = 0;
  for (const job of expired) {
    const leaseId = stringValue(job.leaseId);
    if (!leaseId) continue;
    await createAiRunnerEvent(runnerDb, job, {
      eventKey: `lease_expired:${leaseId}`,
      eventType: "lease_expired",
      runnerId: stringValue(job.leaseOwnerRunnerId) || stringValue(job.runnerId),
      leaseId,
      payload: {
        leaseExpiresAt: job.leaseExpiresAt ? toIso(job.leaseExpiresAt) : null,
        lastHeartbeatAt: job.lastHeartbeatAt ? toIso(job.lastHeartbeatAt) : null,
      },
    }).catch((error: unknown) => {
      log.warn("AI runner lease expiry event failed", {
        jobId: job.id,
        error: error instanceof Error ? error.message : String(error),
      });
    });
    await cleanupExpiredLeaseSessions(job, runnerDb);
    const update = await runnerDb.commandCenterAiRunnerJob.updateMany({
      where: {
        id: job.id,
        leaseId,
        status: { in: ACTIVE_JOB_STATUSES },
        leaseExpiresAt: { lte: now },
      },
      data: {
        status: "failed",
        error: EXPIRED_LEASE_ERROR,
        finishedAt: now,
        retryable: true,
        retryReason: "lease_expired",
        leaseId: null,
        leaseOwnerRunnerId: null,
        leaseExpiresAt: null,
      },
    });
    if (update.count === 1) {
      reconciled += 1;
      const updated = await runnerDb.commandCenterAiRunnerJob.findUnique({ where: { id: job.id } });
      if (updated) {
        await appendAssistantResultMessageIfNeeded(updated, runnerDb);
      }
    }
  }
  return reconciled;
}

let aiRunnerLeaseReconciler: NodeJS.Timeout | null = null;

export function startAiRunnerLeaseReconciler() {
  if (!aiRunnerReconcilerEnabled() || aiRunnerLeaseReconciler) return;
  aiRunnerLeaseReconciler = setInterval(() => {
    reconcileExpiredAiRunnerJobLeases().catch((error: unknown) => {
      log.warn("AI runner lease reconciliation failed", {
        error: error instanceof Error ? error.message : String(error),
      });
    });
  }, AI_RUNNER_LEASE_RECONCILE_INTERVAL_MS);
  aiRunnerLeaseReconciler.unref?.();
}

export type AiRunnerJobDetail = AiRunnerJobSummary & {
  deviceLabel: string | null;
  attachments: CommandCenterMessageAttachment[];
  pendingCommandApproval: AiRunnerCommandApprovalSummary | null;
  latestCommandApproval: AiRunnerCommandApprovalSummary | null;
  evidence: AiRunnerEvidenceSummary;
};

async function attachJobDetails(
  context: CommandCenterConversationContext,
  jobs: AiRunnerJobSummary[],
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerJobDetail[]> {
  if (jobs.length === 0) return [];
  const agentIds = [...new Set(jobs.map((job) => job.agentId))];
  const devices = await runnerDb.rmmDevice.findMany({
    where: {
      organizationId: context.organizationId,
      agentId: { in: agentIds },
    },
    select: {
      agentId: true,
      hostname: true,
      customer: { select: { name: true } },
    },
  });
  const deviceByAgent = new Map<
    string,
    { agentId: string; hostname?: string | null; customer?: { name?: string | null } | null }
  >(devices.map((device: { agentId: string; hostname?: string | null; customer?: { name?: string | null } | null }) => [device.agentId, device]));
  const artifacts = await runnerDb.commandCenterAiRunnerArtifact.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      jobId: { in: jobs.map((job) => job.id) },
    },
    orderBy: { createdAt: "asc" },
  });
  const attachmentsByJob = new Map<string, CommandCenterMessageAttachment[]>();
  const artifactsByJob = new Map<string, AiRunnerArtifactSummary[]>();
  for (const artifact of artifacts.map(toArtifactSummary)) {
    const jobArtifacts = artifactsByJob.get(artifact.jobId) ?? [];
    jobArtifacts.push(artifact);
    artifactsByJob.set(artifact.jobId, jobArtifacts);
    const attachment = artifactToAttachment(artifact);
    if (!attachment) continue;
    const items = attachmentsByJob.get(artifact.jobId) ?? [];
    items.push(attachment);
    attachmentsByJob.set(artifact.jobId, items);
  }
  const pendingCommandApprovalRecords = await runnerDb.commandCenterAiRunnerCommandApproval.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      jobId: { in: jobs.map((job) => job.id) },
      status: { in: ["pending", "approved", "desktop_control_requested", "executing", "policy_blocked"] },
    },
    orderBy: [{ jobId: "asc" }, { turnIndex: "desc" }],
  });
  const approvalByJob = new Map<string, AiRunnerCommandApprovalSummary>();
  for (const approval of pendingCommandApprovalRecords.map(toCommandApprovalSummary)) {
    if (!approvalByJob.has(approval.jobId)) {
      approvalByJob.set(approval.jobId, approval);
    }
  }
  const latestCommandApprovalRecords = await runnerDb.commandCenterAiRunnerCommandApproval.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      jobId: { in: jobs.map((job) => job.id) },
    },
    orderBy: [{ jobId: "asc" }, { turnIndex: "desc" }, { updatedAt: "desc" }],
  });
  const latestApprovalByJob = new Map<string, AiRunnerCommandApprovalSummary>();
  for (const approval of latestCommandApprovalRecords.map(toCommandApprovalSummary)) {
    if (!latestApprovalByJob.has(approval.jobId)) {
      latestApprovalByJob.set(approval.jobId, approval);
    }
  }
  return jobs.map((job) => {
    const device = deviceByAgent.get(job.agentId);
    return {
      ...job,
      deviceLabel: device ? deviceLabel(device) : null,
      attachments: attachmentsByJob.get(job.id) ?? [],
      pendingCommandApproval: approvalByJob.get(job.id) ?? null,
      latestCommandApproval: latestApprovalByJob.get(job.id) ?? null,
      evidence: artifactEvidenceSummary(job, artifactsByJob.get(job.id) ?? []),
    };
  });
}

export async function listAiRunnerJobs(
  context: CommandCenterConversationContext,
  options: { conversationId?: string | null; active?: boolean } = {},
): Promise<AiRunnerJobDetail[]> {
  const records = await db().commandCenterAiRunnerJob.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      ...(options.conversationId ? { conversationId: options.conversationId } : {}),
      ...(options.active ? { status: { in: ACTIVE_JOB_STATUSES } } : {}),
    },
    orderBy: { updatedAt: "desc" },
    take: 25,
  });
  return attachJobDetails(context, records.map(toJobSummary));
}

export async function listAiRunnerCommandOutputDeltas(
  context: CommandCenterConversationContext,
  conversationId: string,
  options: { after?: Date | null; take?: number } = {},
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerCommandOutputDelta[]> {
  const records = await runnerDb.commandCenterAiRunnerEvent.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId,
      eventType: COMMAND_OUTPUT_DELTA_EVENT_TYPE,
      ...(options.after ? { createdAt: { gt: options.after } } : {}),
    },
    orderBy: { createdAt: "asc" },
    take: Math.max(1, Math.min(Math.trunc(options.take ?? 1_000), 2_000)),
  });
  return (records
    .map(commandOutputDeltaFromEvent)
    .filter(Boolean) as AiRunnerCommandOutputDelta[])
    .sort((a, b) => {
      const timeDelta = new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime();
      if (timeDelta !== 0) return timeDelta;
      const approvalDelta = a.approvalId.localeCompare(b.approvalId);
      if (approvalDelta !== 0) return approvalDelta;
      return a.sequence - b.sequence;
    });
}

export async function getAiRunnerConversationStreamSnapshot(
  context: CommandCenterConversationContext,
  conversationId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerConversationStreamSnapshot | null> {
  const conversation = await runnerDb.commandCenterConversation.findFirst({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    select: { id: true },
  });
  if (!conversation) return null;
  const jobs = await listAiRunnerJobs(context, { conversationId });
  const output = await listAiRunnerCommandOutputDeltas(
    context,
    conversationId,
    { take: Math.max(COMMAND_OUTPUT_RECENT_MAX_EVENTS, 1_000) },
    runnerDb,
  );
  return { jobs, output };
}

export async function getAiRunnerJobDetail(
  context: CommandCenterConversationContext,
  jobId: string,
): Promise<AiRunnerJobDetail | null> {
  const job = await getAiRunnerJob(context, jobId);
  if (!job) return null;
  const [detail] = await attachJobDetails(context, [job]);
  return detail ?? null;
}

export async function readAiRunnerShellTranscript(
  context: CommandCenterConversationContext,
  jobId: string,
  runnerDb: RunnerDb = db(),
): Promise<{ buffer: Buffer; mimeType: string; name: string } | null> {
  const job = await runnerDb.commandCenterAiRunnerJob.findFirst({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!job) return null;
  const records = await runnerDb.commandCenterAiRunnerArtifact.findMany({
    where: {
      jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      artifactType: AI_RUNNER_SHELL_TRANSCRIPT_ARTIFACT_TYPE,
    },
    orderBy: { createdAt: "asc" },
  });
  const transcriptRecords = completeShellTranscriptArtifacts(
    records.map((record: any) => ({
      ...toArtifactSummary(record),
      contentBase64: record.contentBase64,
    })),
  );
  if (transcriptRecords.length === 0) {
    return null;
  }
  return {
    buffer: Buffer.concat(
      transcriptRecords.map((record: AiRunnerArtifactSummary & { contentBase64?: unknown }) =>
        Buffer.from(typeof record.contentBase64 === "string" ? record.contentBase64 : "", "base64"),
      ),
    ),
    mimeType: "text/plain; charset=utf-8",
    name: `shell-transcript-${jobId}.txt`,
  };
}

export async function getAiRunnerReplayManifest(
  context: CommandCenterConversationContext,
  jobId: string,
  runnerDb: RunnerDb = db(),
): Promise<AiRunnerReplayManifest | null> {
  const jobRecord = await runnerDb.commandCenterAiRunnerJob.findFirst({
    where: {
      id: jobId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!jobRecord) return null;
  const [detail] = await attachJobDetails(context, [toJobSummary(jobRecord)], runnerDb);
  const artifactRecords = await runnerDb.commandCenterAiRunnerArtifact.findMany({
    where: {
      jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      artifactType: AI_RUNNER_SCREENSHOT_ARTIFACT_TYPE,
    },
    orderBy: { createdAt: "asc" },
  });
  const eventRecords = await runnerDb.commandCenterAiRunnerEvent.findMany({
    where: {
      jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      eventType: "artifact",
    },
    orderBy: { createdAt: "asc" },
  });
  const eventTextByArtifactId = new Map<string, string>();
  for (const event of eventRecords) {
    const artifactId = stringValue(event.artifactId);
    if (!artifactId || eventTextByArtifactId.has(artifactId)) continue;
    const payload = asRecord(event.payload);
    const displayText = stringValue(payload?.messageContent);
    if (displayText) {
      eventTextByArtifactId.set(artifactId, displayText);
    }
  }

  const frames = artifactRecords
    .map(toArtifactSummary)
    .filter(isLiveDesktopFrameArtifact)
    .sort((a: AiRunnerArtifactSummary, b: AiRunnerArtifactSummary) => {
      const frameDelta = artifactReplaySortValue(a) - artifactReplaySortValue(b);
      if (frameDelta !== 0) return frameDelta;
      return new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime();
    })
    .map((artifact: AiRunnerArtifactSummary): AiRunnerReplayFrame => {
      const metadata = asRecord(artifact.metadata);
      const width = numberValue(metadata?.width);
      const height = numberValue(metadata?.height);
      const stepIndex = numberValue(metadata?.stepIndex);
      return {
        artifactId: artifact.id,
        frameSeq: artifactFrameSeq(artifact),
        width: width === undefined ? null : width,
        height: height === undefined ? null : height,
        cursor: liveFrameCursorValue(metadata?.cursor) ?? null,
        stepIndex: stepIndex === undefined ? null : stepIndex,
        taskId: stringValue(metadata?.taskId),
        displayText:
          stringValue(metadata?.displayText) ||
          eventTextByArtifactId.get(artifact.id) ||
          "Desktop view updated.",
        createdAt: artifact.createdAt,
      };
    });

  return {
    jobId,
    jobType: detail?.jobType ?? jobRecord.jobType ?? AI_RUNNER_JOB_TYPE,
    status: detail?.status ?? normalizeStatus(jobRecord.status),
    deviceLabel: detail?.deviceLabel ?? null,
    goal: stringValue(jobRecord.goal),
    startedAt: detail?.startedAt ?? (jobRecord.startedAt ? toIso(jobRecord.startedAt) : null),
    finishedAt: detail?.finishedAt ?? (jobRecord.finishedAt ? toIso(jobRecord.finishedAt) : null),
    defaultDelayMs: AI_RUNNER_REPLAY_DEFAULT_DELAY_MS,
    frames,
  };
}

export async function readAiRunnerArtifactContent(
  context: CommandCenterConversationContext,
  artifactId: string,
): Promise<{ buffer: Buffer; mimeType: string; name: string } | null> {
  const record = await db().commandCenterAiRunnerArtifact.findFirst({
    where: {
      id: artifactId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  if (!record) {
    return null;
  }
  return {
    buffer: Buffer.from(record.contentBase64, "base64"),
    mimeType: record.mimeType,
    name: record.name,
  };
}

export function collectAiRunnerAttachments(
  artifacts: AiRunnerArtifactSummary[],
): CommandCenterMessageAttachment[] {
  return artifacts.map(artifactToAttachment).filter(Boolean) as CommandCenterMessageAttachment[];
}

export function logAiRunnerCallbackError(error: unknown) {
  log.warn("AI runner callback failed", {
    error: error instanceof Error ? error.message : String(error),
  });
}
