import { Router } from "express";
import { randomUUID } from "crypto";
import { Prisma } from "@prisma/client";
import { v5 as uuidV5 } from "uuid";
import { prisma } from "../lib/prisma";
import {
  continueAiDesktopTask,
  logAiShellCommandApproved,
  proposeAiShellCommand,
  runAiDesktopActionPoc,
  startAiDesktopTask,
} from "../lib/rmmAiAssist";
import {
  buildAgentHealth,
  readAgentHealthThresholds,
  reconcileHealthAlerts,
  type AgentHealthReason,
  type AgentHealthSummary,
} from "../lib/rmmAgentHealth";
import { requireAuth, AuthedRequest } from "../middleware/auth";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { auditRequest, writeAuditEvent } from "../lib/audit";
import {
  DEVICE_LIST_ALERT_WINDOW_DAYS,
  alertSeverityRank,
  buildDeviceListOrderBy,
  buildDeviceListWhere,
  cleanSavedViewName,
  normalizeDeviceSavedViewState,
  parseDeviceListQuery,
} from "../lib/rmmDeviceList";
import {
  attachRmmServerAuth,
  requireRmmServer,
  RmmServerRequest,
} from "../middleware/rmmServerKey";
import {
  buildUpdateKeyFromParts,
  classifyPatchCategory,
} from "../lib/patchDecisionEngine";
import {
  selectPostPatchRebootLoopFailureKeys,
  shouldClearRebootForFailedPendingUpdates,
} from "../lib/patchRebootLoop";

export const rmmRouter = Router();
rmmRouter.use(attachRmmServerAuth);

/** Namespace for deterministic Unassigned customer IDs (must match talos_server). */
const UNASSIGNED_CUSTOMER_NAMESPACE = "a7c9e2d1-4b3f-4a8e-9e1d-2c5b6a7d8e9f";

function unassignedCustomerId(organizationId: string): string {
  return uuidV5(organizationId, UNASSIGNED_CUSTOMER_NAMESPACE);
}

async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: {
      organization: true,
      user: { select: { id: true, email: true } },
    },
  });
}

function assertUser(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== "user") {
    res.status(403).json({ error: "Machine tokens are not allowed" });
    return false;
  }
  return true;
}

function isAgentAdminRole(role: string) {
  return role === "AGENT_ADMIN" || role === "SUPER_ADMIN";
}

function assertAgentAdmin(membership: { role: string }, res: any) {
  if (!isAgentAdminRole(membership.role)) {
    res.status(403).json({ error: "Only admins can manage devices" });
    return false;
  }
  return true;
}

function readNonEmptyString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function readOptionalNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === "") return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function requireServiceKey(req: RmmServerRequest, res: any): boolean {
  if (req.rmmServer) {
    return true;
  }
  const expected = (
    process.env.SERVICE_KEY ||
    process.env.RMM_TELEMETRY_SERVICE_KEY ||
    ""
  ).trim();
  if (!expected) {
    res
      .status(503)
      .json({
        error: "SERVICE_KEY/RMM_TELEMETRY_SERVICE_KEY is not configured",
      });
    return false;
  }
  const presented = (req.header("x-service-key") || "").trim();
  if (presented !== expected) {
    res.status(401).json({ error: "Unauthorized" });
    return false;
  }
  return true;
}

function readAiDesktopRequestContext(body: any) {
  const screenshotBase64 =
    typeof body?.screenshotBase64 === "string"
      ? body.screenshotBase64.trim()
      : "";
  const sessionId =
    typeof body?.sessionId === "string" ? body.sessionId.trim() : "";
  const sessionToken =
    typeof body?.sessionToken === "string"
      ? body.sessionToken.trim()
      : typeof body?.token === "string"
        ? body.token.trim()
        : "";
  const rmmApiBase =
    typeof body?.rmmApiBase === "string"
      ? body.rmmApiBase.trim()
      : typeof body?.apiBase === "string"
        ? body.apiBase.trim()
        : null;
  const platform =
    typeof body?.platform === "string" && body.platform.trim()
      ? body.platform.trim().toLowerCase()
      : null;
  const width = Number(body?.width);
  const height = Number(body?.height);
  const deviceContext =
    body?.deviceContext && typeof body.deviceContext === "object" && !Array.isArray(body.deviceContext)
      ? body.deviceContext
      : null;
  const jobId = readNonEmptyString(body?.jobId);
  const organizationId = readNonEmptyString(body?.organizationId);
  const userId = readNonEmptyString(body?.userId);
  const conversationId = readNonEmptyString(body?.conversationId);
  const agentId = readNonEmptyString(body?.agentId);
  return {
    screenshotBase64,
    sessionId,
    sessionToken,
    rmmApiBase,
    platform,
    width,
    height,
    deviceContext,
    jobId: jobId || null,
    organizationId: organizationId || null,
    userId: userId || null,
    conversationId: conversationId || null,
    agentId: agentId || null,
  };
}

function readAiShellRequestContext(body: any) {
  const sessionId =
    typeof body?.sessionId === "string" ? body.sessionId.trim() : "";
  const sessionToken =
    typeof body?.sessionToken === "string"
      ? body.sessionToken.trim()
      : typeof body?.token === "string"
        ? body.token.trim()
        : "";
  const rmmApiBase =
    typeof body?.rmmApiBase === "string"
      ? body.rmmApiBase.trim()
      : typeof body?.apiBase === "string"
        ? body.apiBase.trim()
        : null;
  const platform =
    typeof body?.platform === "string" && body.platform.trim()
      ? body.platform.trim().toLowerCase()
      : null;
  const deviceContext =
    body?.deviceContext && typeof body.deviceContext === "object" && !Array.isArray(body.deviceContext)
      ? body.deviceContext
      : null;
  const activeCommand =
    body?.activeCommand && typeof body.activeCommand === "object" && !Array.isArray(body.activeCommand)
      ? body.activeCommand
      : null;
  const jobId = readNonEmptyString(body?.jobId);
  const organizationId = readNonEmptyString(body?.organizationId);
  const userId = readNonEmptyString(body?.userId);
  const conversationId = readNonEmptyString(body?.conversationId);
  const agentId = readNonEmptyString(body?.agentId);
  return {
    sessionId,
    sessionToken,
    rmmApiBase,
    platform,
    deviceContext,
    activeCommand,
    jobId: jobId || null,
    organizationId: organizationId || null,
    userId: userId || null,
    conversationId: conversationId || null,
    agentId: agentId || null,
  };
}

function validateAiDesktopRequestContext(
  context: ReturnType<typeof readAiDesktopRequestContext>,
  res: any,
): boolean {
  if (
    !context.screenshotBase64 ||
    !context.sessionId ||
    !context.sessionToken
  ) {
    res.status(400).json({
      error: "screenshotBase64, sessionId, and sessionToken are required",
    });
    return false;
  }
  if (
    !Number.isFinite(context.width) ||
    !Number.isFinite(context.height) ||
    context.width < 1 ||
    context.height < 1
  ) {
    res
      .status(400)
      .json({ error: "width and height must be positive numbers" });
    return false;
  }
  return true;
}

function sendAiDesktopError(res: any, error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("task not found")) {
    return res.status(404).json({ error: message });
  }
  if (
    message.includes("validation failed") ||
    message.includes("Unsupported computer action") ||
    message.includes("Unsupported desktop action") ||
    message.includes("did not return any computer actions") ||
    message.includes("did not return a Talos desktop step") ||
    message.includes("multi-turn computer use is not enabled yet") ||
    message.includes("multi-turn desktop observation is not enabled yet") ||
    message.includes("without executable actions") ||
    message.includes("exceeding max") ||
    message.includes("repeated screenshots") ||
    message.includes("session does not match") ||
    message.includes("no pending computer action") ||
    message.includes("no pending Talos desktop step") ||
    message.includes("Talos desktop step call")
  ) {
    return res.status(422).json({ error: message });
  }
  if (message.includes("not configured") || message.includes("disabled")) {
    return res.status(503).json({ error: message });
  }
  if (
    message.includes("Remote desktop session validation failed") ||
    message.includes("does not match")
  ) {
    return res.status(401).json({ error: message });
  }
  return res
    .status(500)
    .json({ error: message || "AI desktop action request failed" });
}

function sendAiShellError(res: any, error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (
    message.includes("prompt is required") ||
    message.includes("command is required")
  ) {
    return res.status(400).json({ error: message });
  }
  if (
    message.includes("proposal") ||
    message.includes("invalid JSON") ||
    message.includes("invalid NUL") ||
    message.includes("Unsupported Talos shell") ||
    message.includes("wait proposal")
  ) {
    return res.status(422).json({ error: message });
  }
  if (message.includes("not configured") || message.includes("disabled")) {
    return res.status(503).json({ error: message });
  }
  if (message.includes("Shell session validation failed")) {
    return res.status(401).json({ error: message });
  }
  return res
    .status(500)
    .json({ error: message || "AI shell assist request failed" });
}

rmmRouter.post("/ai/shell-assist", async (req, res) => {
  const prompt =
    typeof req.body?.prompt === "string" ? req.body.prompt.trim() : "";
  const transcript =
    typeof req.body?.transcript === "string" ? req.body.transcript : "";
  const history = Array.isArray(req.body?.history) ? req.body.history : [];
  const context = readAiShellRequestContext(req.body);

  if (!prompt || !context.sessionId || !context.sessionToken) {
    return res.status(400).json({
      error: "prompt, sessionId, and sessionToken are required",
    });
  }

  try {
    const result = await proposeAiShellCommand({
      prompt,
      transcript,
      history,
      sessionId: context.sessionId,
      sessionToken: context.sessionToken,
      rmmApiBase: context.rmmApiBase,
      platform: context.platform,
      deviceContext: context.deviceContext,
      activeCommand: context.activeCommand,
      jobId: context.jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId: context.conversationId,
      agentId: context.agentId,
      generatedSecrets: Array.isArray(req.body?.generatedSecrets) ? req.body.generatedSecrets : [],
    });
    return res.json(result);
  } catch (error) {
    return sendAiShellError(res, error);
  }
});

rmmRouter.post("/ai/shell-assist/approved", async (req, res) => {
  const command =
    typeof req.body?.command === "string" ? req.body.command.trim() : "";
  const responseId =
    typeof req.body?.responseId === "string"
      ? req.body.responseId.trim()
      : null;
  const context = readAiShellRequestContext(req.body);

  if (!command || !context.sessionId || !context.sessionToken) {
    return res.status(400).json({
      error: "command, sessionId, and sessionToken are required",
    });
  }

  try {
    await logAiShellCommandApproved({
      command,
      sessionId: context.sessionId,
      sessionToken: context.sessionToken,
      rmmApiBase: context.rmmApiBase,
      platform: context.platform,
      responseId,
    });
    return res.json({ ok: true });
  } catch (error) {
    return sendAiShellError(res, error);
  }
});

rmmRouter.post("/ai/desktop-task/start", async (req, res) => {
  const goal =
    typeof req.body?.goal === "string"
      ? req.body.goal.trim()
      : typeof req.body?.prompt === "string"
        ? req.body.prompt.trim()
        : "";
  const context = readAiDesktopRequestContext(req.body);

  if (!goal) {
    return res.status(400).json({ error: "goal is required" });
  }
  if (!validateAiDesktopRequestContext(context, res)) {
    return;
  }

  try {
    const result = await startAiDesktopTask({
      goal,
      screenshotBase64: context.screenshotBase64,
      width: Math.round(context.width),
      height: Math.round(context.height),
      sessionId: context.sessionId,
      sessionToken: context.sessionToken,
      rmmApiBase: context.rmmApiBase,
      platform: context.platform,
      deviceContext: context.deviceContext,
      jobId: context.jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId: context.conversationId,
      agentId: context.agentId,
      generatedSecrets: Array.isArray(req.body?.generatedSecrets) ? req.body.generatedSecrets : [],
    });
    return res.json(result);
  } catch (error) {
    return sendAiDesktopError(res, error);
  }
});

rmmRouter.post("/ai/desktop-task/:taskId/continue", async (req, res) => {
  const taskId =
    typeof req.params?.taskId === "string" ? req.params.taskId.trim() : "";
  const context = readAiDesktopRequestContext(req.body);
  if (!taskId) {
    return res.status(400).json({ error: "taskId is required" });
  }
  if (!validateAiDesktopRequestContext(context, res)) {
    return;
  }

  try {
    const result = await continueAiDesktopTask({
      taskId,
      screenshotBase64: context.screenshotBase64,
      width: Math.round(context.width),
      height: Math.round(context.height),
      sessionId: context.sessionId,
      sessionToken: context.sessionToken,
      rmmApiBase: context.rmmApiBase,
      platform: context.platform,
      deviceContext: context.deviceContext,
      jobId: context.jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId: context.conversationId,
      agentId: context.agentId,
      generatedSecrets: Array.isArray(req.body?.generatedSecrets) ? req.body.generatedSecrets : [],
      lastStepResult:
        typeof req.body?.lastStepResult === "string"
          ? req.body.lastStepResult.trim()
          : null,
    });
    return res.json(result);
  } catch (error) {
    return sendAiDesktopError(res, error);
  }
});

rmmRouter.post("/ai/desktop-action", async (req, res) => {
  const prompt =
    typeof req.body?.prompt === "string" ? req.body.prompt.trim() : "";
  const context = readAiDesktopRequestContext(req.body);

  if (
    !prompt ||
    !context.screenshotBase64 ||
    !context.sessionId ||
    !context.sessionToken
  ) {
    return res.status(400).json({
      error:
        "prompt, screenshotBase64, sessionId, and sessionToken are required",
    });
  }
  if (!validateAiDesktopRequestContext(context, res)) {
    return;
  }

  try {
    const result = await runAiDesktopActionPoc({
      prompt,
      screenshotBase64: context.screenshotBase64,
      width: Math.round(context.width),
      height: Math.round(context.height),
      sessionId: context.sessionId,
      sessionToken: context.sessionToken,
      rmmApiBase: context.rmmApiBase,
      platform: context.platform,
      deviceContext: context.deviceContext,
      jobId: context.jobId,
      organizationId: context.organizationId,
      userId: context.userId,
      conversationId: context.conversationId,
      agentId: context.agentId,
      generatedSecrets: Array.isArray(req.body?.generatedSecrets) ? req.body.generatedSecrets : [],
    });
    return res.json(result);
  } catch (error) {
    return sendAiDesktopError(res, error);
  }
});

type TelemetryStateRow = {
  agent_id: string;
  inventory_data: any;
  collected_at: Date | null;
  hostname: string | null;
  pending_updates_count: number | null;
  reboot_required: boolean | null;
  os_name: string | null;
  agent_version: string | null;
};

type DeviceListTelemetrySummaryRow = {
  agent_id: string;
  inventory_data: any | null;
  collected_at: Date | null;
  hostname: string | null;
  pending_updates_count: number | null;
  reboot_required: boolean | null;
  agent_version: string | null;
  os_name: string | null;
  os_version: string | null;
  alert_severity: string | null;
  tag_text: string | null;
};

type DeviceListTelemetrySummary = DeviceListTelemetrySummaryRow;

type DeviceListAlertRankRow = {
  agent_id: string;
  severity_rank: number | null;
};

async function fetchTelemetryStateRows(
  agentIds: string[],
): Promise<Map<string, TelemetryStateRow>> {
  if (agentIds.length === 0) {
    return new Map();
  }
  const rows = await prisma.$queryRaw<TelemetryStateRow[]>(Prisma.sql`
    SELECT
      agent_id,
      inventory_data,
      collected_at,
      hostname,
      pending_updates_count,
      reboot_required,
      os_name,
      agent_version
    FROM rmm_telemetry.device_state
    WHERE agent_id IN (${Prisma.join(agentIds)})
  `);
  return new Map(rows.map((row) => [row.agent_id, row]));
}

async function fetchDeviceListTelemetrySummaries(
  organizationId: string,
  agentIds: string[]
): Promise<Map<string, DeviceListTelemetrySummary>> {
  if (agentIds.length === 0) {
    return new Map();
  }

  const rows = await prisma.$queryRaw<DeviceListTelemetrySummaryRow[]>(Prisma.sql`
    WITH requested AS (
      SELECT unnest(ARRAY[${Prisma.join(agentIds)}]::text[]) AS agent_id
    ),
    alert_rollup AS (
      SELECT
        agent_id,
        MAX(
          CASE lower(severity)
            WHEN 'critical' THEN 4
            WHEN 'error' THEN 3
            WHEN 'warning' THEN 2
            WHEN 'warn' THEN 2
            WHEN 'info' THEN 1
            ELSE 0
          END
        ) AS severity_rank
      FROM rmm_telemetry.device_event
      WHERE organization_id = ${organizationId}
        AND agent_id IN (${Prisma.join(agentIds)})
        AND occurred_at >= NOW() - (${DEVICE_LIST_ALERT_WINDOW_DAYS}::text || ' days')::interval
      GROUP BY agent_id
    ),
    tag_rollup AS (
      SELECT
        agent_id,
        string_agg(DISTINCT fact_value_text, ', ' ORDER BY fact_value_text) AS tag_text
      FROM rmm_telemetry.fact_state_current
      WHERE organization_id = ${organizationId}
        AND agent_id IN (${Prisma.join(agentIds)})
        AND (fact_key ILIKE '%tag%' OR fact_key ILIKE '%group%')
      GROUP BY agent_id
    )
    SELECT
      requested.agent_id,
      state.inventory_data,
      state.collected_at,
      state.hostname,
      state.pending_updates_count,
      state.reboot_required,
      state.agent_version,
      state.os_name,
      state.os_version,
      CASE alert_rollup.severity_rank
        WHEN 4 THEN 'critical'
        WHEN 3 THEN 'error'
        WHEN 2 THEN 'warning'
        WHEN 1 THEN 'info'
        ELSE NULL
      END AS alert_severity,
      tag_rollup.tag_text
    FROM requested
    LEFT JOIN rmm_telemetry.device_state state ON state.agent_id = requested.agent_id
    LEFT JOIN alert_rollup ON alert_rollup.agent_id = requested.agent_id
    LEFT JOIN tag_rollup ON tag_rollup.agent_id = requested.agent_id
  `);

  return new Map(rows.map((row) => [row.agent_id, row]));
}

async function fetchDeviceListAlertRanks(
  organizationId: string,
  agentIds: string[]
): Promise<Map<string, number>> {
  if (agentIds.length === 0) {
    return new Map();
  }

  const rows = await prisma.$queryRaw<DeviceListAlertRankRow[]>(Prisma.sql`
    SELECT
      agent_id,
      MAX(
        CASE lower(severity)
          WHEN 'critical' THEN 4
          WHEN 'error' THEN 3
          WHEN 'warning' THEN 2
          WHEN 'warn' THEN 2
          WHEN 'info' THEN 1
          ELSE 0
        END
      ) AS severity_rank
    FROM rmm_telemetry.device_event
    WHERE organization_id = ${organizationId}
      AND agent_id IN (${Prisma.join(agentIds)})
      AND occurred_at >= NOW() - (${DEVICE_LIST_ALERT_WINDOW_DAYS}::text || ' days')::interval
    GROUP BY agent_id
  `);

  return new Map(rows.map((row) => [row.agent_id, Number(row.severity_rank ?? 0)]));
}

type InstalledAppRow = {
  appName: string;
  appNameNorm: string;
  publisher: string | null;
  publisherNorm: string | null;
  version: string | null;
  installDate: string | null;
  sizeBytes: bigint | null;
  source: string | null;
  location: string | null;
  uninstallString: string | null;
  is64Bit: boolean | null;
};

type ServiceRow = {
  serviceName: string;
  serviceNameNorm: string;
  displayName: string;
  displayNameNorm: string;
  status: string;
  startType: string | null;
  account: string | null;
  processId: number | null;
  canStop: boolean | null;
  canPause: boolean | null;
  isCritical: boolean | null;
  description: string | null;
  path: string | null;
};

type StartupItemRow = {
  itemName: string;
  itemNameNorm: string;
  command: string;
  location: string;
  userName: string | null;
  isEnabled: boolean | null;
};

type WindowsFeatureRow = {
  featureName: string;
  featureNameNorm: string;
  displayName: string;
  displayNameNorm: string;
  installState: string | null;
  enabled: boolean | null;
};

type PendingUpdateRow = {
  title: string;
  titleNorm: string;
  description: string | null;
  kbArticle: string | null;
  isMandatory: boolean | null;
  sizeBytes: bigint | null;
  requiresReboot: boolean | null;
};

type InstalledUpdateRow = {
  installedAt: Date | null;
  title: string;
  titleNorm: string;
  kbArticle: string | null;
  operation: string | null;
  result: string | null;
  hresult: number | null;
};

function classifyWindowsUpdateHistoryResult(result: string | null): 'installed' | 'failed' | 'detected' {
  const normalized = normalizeText(result);
  if (normalized.startsWith("succeeded")) return "installed";
  if (normalized === "failed" || normalized === "failure" || normalized === "aborted") return "failed";
  return "detected";
}

function normalizeText(value: unknown): string {
  if (typeof value !== "string") {
    return "";
  }
  return value.trim().toLowerCase();
}

function inferPatchDeviceType(osName: unknown): "server" | "workstation" | "laptop" | "unknown" {
  const os = normalizeText(osName);
  if (!os) return "unknown";
  if (/\bserver\b/.test(os)) return "server";
  if (/\bwindows\b|\bmacos\b|\bubuntu\b|\bdebian\b|\blinux\b/.test(os)) return "workstation";
  return "unknown";
}

function meaningfulText(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const normalized = value.trim();
  if (!normalized) {
    return null;
  }
  const lower = normalized.toLowerCase();
  if (lower === "unknown" || lower === "n/a") {
    return null;
  }
  return normalized;
}

function meaningfulIpText(value: unknown): string | null {
  const normalized = meaningfulText(value);
  if (!normalized || normalized === "0.0.0.0" || normalized === "::") {
    return null;
  }
  return normalized;
}

function firstMeaningfulText(...values: unknown[]): string | null {
  for (const value of values) {
    const normalized = meaningfulText(value);
    if (normalized) return normalized;
  }
  return null;
}

function firstMeaningfulIpText(...values: unknown[]): string | null {
  for (const value of values) {
    const normalized = meaningfulIpText(value);
    if (normalized) return normalized;
  }
  return null;
}

function sameDriveText(left: string | null, right: string | null): boolean {
  const normalizeDrive = (value: string | null) => {
    if (!value) return null;
    const normalized = value.trim().toLowerCase();
    const match = normalized.match(/^([a-z]):/);
    return match ? `${match[1]}:` : normalized;
  };
  const normalizedLeft = normalizeDrive(left);
  const normalizedRight = normalizeDrive(right);
  return Boolean(normalizedLeft && normalizedRight && normalizedLeft === normalizedRight);
}

function asRecord(value: unknown): Record<string, any> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  return value as Record<string, any>;
}

function valueAtPath(value: unknown, path: string[]): unknown {
  let current: unknown = value;
  for (const part of path) {
    const record = asRecord(current);
    if (!record) {
      return undefined;
    }
    current = record[part];
  }
  return current;
}

function arrayAtAnyPath(value: unknown, paths: string[][]): unknown[] {
  for (const path of paths) {
    const candidate = valueAtPath(value, path);
    if (Array.isArray(candidate)) {
      return candidate;
    }
  }
  return [];
}

function textValue(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
  }
  return null;
}

function numberValue(...values: unknown[]): number | null {
  for (const value of values) {
    if (typeof value === "number" && Number.isFinite(value)) {
      return value;
    }
    if (typeof value === "string" && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) {
        return parsed;
      }
    }
  }
  return null;
}

function bigintValue(...values: unknown[]): bigint | null {
  const value = numberValue(...values);
  if (value === null) {
    return null;
  }
  return BigInt(Math.max(0, Math.trunc(value)));
}

function booleanValue(...values: unknown[]): boolean | null {
  for (const value of values) {
    if (typeof value === "boolean") {
      return value;
    }
    if (typeof value === "number" && Number.isFinite(value)) {
      if (value === 1) return true;
      if (value === 0) return false;
    }
    if (typeof value === "string" && value.trim()) {
      const normalized = value.trim().toLowerCase();
      if (["1", "true", "yes", "enabled", "running"].includes(normalized)) {
        return true;
      }
      if (["0", "false", "no", "disabled", "stopped"].includes(normalized)) {
        return false;
      }
    }
  }
  return null;
}

function firstInventoryIp(inventoryData: unknown): string | null {
  const systemIps = arrayAtAnyPath(inventoryData, [
    ["operating_system", "system", "ip_addresses"],
    ["operatingSystem", "system", "ipAddresses"],
    ["system", "ip_addresses"],
    ["system", "ipAddresses"],
  ]);
  for (const ip of systemIps) {
    const candidate = meaningfulIpText(ip);
    if (candidate) return candidate;
  }

  const adapters = arrayAtAnyPath(inventoryData, [
    ["network", "adapters"],
    ["network", "interfaces"],
    ["networks"],
  ]);

  for (const adapter of adapters) {
    const adapterRecord = asRecord(adapter);
    if (!adapterRecord) continue;
    const directIp = firstMeaningfulIpText(
      adapterRecord.ip,
      adapterRecord.ip_address,
      adapterRecord.ipAddress,
      adapterRecord.address,
    );
    if (directIp) return directIp;

    const ips = Array.isArray(adapterRecord.ips)
      ? adapterRecord.ips
      : Array.isArray(adapterRecord.ip_addresses)
        ? adapterRecord.ip_addresses
        : Array.isArray(adapterRecord.ipAddresses)
          ? adapterRecord.ipAddresses
          : [];
    for (const ip of ips) {
      const ipRecord = asRecord(ip);
      const candidate = ipRecord
        ? firstMeaningfulIpText(ipRecord.address, ipRecord.ip, ipRecord.value)
        : meaningfulIpText(ip);
      if (candidate) return candidate;
    }
  }

  return null;
}

function parseInstalledApps(inventoryData: unknown): InstalledAppRow[] {
  const collection = asRecord(inventoryData);
  const installedPrograms = arrayAtAnyPath(collection, [
    ["software", "installed_programs"],
    ["software", "installedPrograms"],
    ["software", "installed_applications"],
    ["software", "installedApplications"],
    ["software", "applications"],
    ["installed_programs"],
    ["installedPrograms"],
    ["installed_applications"],
    ["installedApplications"],
    ["applications"],
  ]);
  const dedupe = new Map<string, InstalledAppRow>();

  for (const app of installedPrograms) {
    const appRecord = asRecord(app);
    if (!appRecord) {
      continue;
    }

    const appName =
      textValue(
        appRecord.name,
        appRecord.app_name,
        appRecord.appName,
        appRecord.display_name,
        appRecord.displayName,
      ) ?? "";
    if (!appName) {
      continue;
    }
    const appNameNorm = normalizeText(appName);
    if (!appNameNorm) {
      continue;
    }

    const publisher = textValue(appRecord.publisher, appRecord.vendor);
    const publisherNorm = publisher ? normalizeText(publisher) : null;
    const version = textValue(appRecord.version, appRecord.displayVersion);
    const installDate = textValue(appRecord.install_date, appRecord.installDate);
    const sizeBytes = bigintValue(appRecord.size_bytes, appRecord.sizeBytes, appRecord.size);
    const source = textValue(appRecord.source, appRecord.package_manager, appRecord.packageManager);
    const location = textValue(appRecord.location, appRecord.install_location, appRecord.installLocation);
    const uninstallString = textValue(appRecord.uninstall_string, appRecord.uninstallString);
    const architecture = textValue(appRecord.architecture, appRecord.arch);
    const is64Bit =
      booleanValue(appRecord.is_64_bit, appRecord.is64Bit) ??
      (architecture ? architecture.includes("64") : null);

    dedupe.set(`${appNameNorm}|${version ?? ""}`, {
      appName,
      appNameNorm,
      publisher,
      publisherNorm,
      version,
      installDate,
      sizeBytes,
      source,
      location,
      uninstallString,
      is64Bit,
    });
  }

  return Array.from(dedupe.values());
}

function parseServices(inventoryData: unknown): ServiceRow[] {
  const collection = asRecord(inventoryData);
  const services = arrayAtAnyPath(collection, [
    ["operating_system", "services", "services"],
    ["operatingSystem", "services", "services"],
    ["services", "services"],
    ["services"],
  ]);
  const dedupe = new Map<string, ServiceRow>();

  for (const service of services) {
    const serviceRecord = asRecord(service);
    if (!serviceRecord) continue;
    const serviceName =
      textValue(serviceRecord.name, serviceRecord.service_name, serviceRecord.serviceName) ?? "";
    if (!serviceName) continue;
    const serviceNameNorm = normalizeText(serviceName);
    if (!serviceNameNorm) continue;
    const displayName =
      textValue(serviceRecord.display_name, serviceRecord.displayName) ?? serviceName;
    const displayNameNorm = normalizeText(displayName);
    const status =
      textValue(serviceRecord.status, serviceRecord.state, serviceRecord.active_state, serviceRecord.activeState) ??
      "unknown";
    dedupe.set(serviceNameNorm, {
      serviceName,
      serviceNameNorm,
      displayName,
      displayNameNorm,
      status,
      startType: textValue(serviceRecord.start_type, serviceRecord.startType, serviceRecord.unit_file_state),
      account: textValue(serviceRecord.account, serviceRecord.user),
      processId: numberValue(serviceRecord.process_id, serviceRecord.processId),
      canStop: booleanValue(serviceRecord.can_stop, serviceRecord.canStop),
      canPause: booleanValue(serviceRecord.can_pause, serviceRecord.canPause),
      isCritical: booleanValue(serviceRecord.is_critical, serviceRecord.isCritical),
      description: textValue(serviceRecord.description),
      path: textValue(serviceRecord.path, serviceRecord.binary_path, serviceRecord.binaryPath),
    });
  }

  return Array.from(dedupe.values());
}

function parseStartupItems(inventoryData: unknown): StartupItemRow[] {
  const collection = asRecord(inventoryData);
  const startupItems = arrayAtAnyPath(collection, [
    ["software", "startup_items"],
    ["software", "startupItems"],
    ["operating_system", "startup_items"],
    ["operatingSystem", "startupItems"],
    ["operating_system", "startup", "items"],
    ["startup_items"],
    ["startupItems"],
  ]);
  const dedupe = new Map<string, StartupItemRow>();

  for (const startupItem of startupItems) {
    const startupItemRecord = asRecord(startupItem);
    if (!startupItemRecord) continue;
    const itemName =
      textValue(startupItemRecord.name, startupItemRecord.item_name, startupItemRecord.itemName) ?? "";
    const command =
      textValue(startupItemRecord.command, startupItemRecord.path, startupItemRecord.target) ?? "";
    const location =
      textValue(startupItemRecord.location, startupItemRecord.source) ?? "";
    if (!itemName || !command || !location) continue;

    const itemNameNorm = normalizeText(itemName);
    if (!itemNameNorm) continue;
    dedupe.set(`${itemNameNorm}|${command}|${location}`, {
      itemName,
      itemNameNorm,
      command,
      location,
      userName: textValue(startupItemRecord.user, startupItemRecord.user_name, startupItemRecord.userName),
      isEnabled: booleanValue(startupItemRecord.is_enabled, startupItemRecord.isEnabled, startupItemRecord.enabled),
    });
  }

  return Array.from(dedupe.values());
}

function parseWindowsFeatures(inventoryData: unknown): WindowsFeatureRow[] {
  const collection = asRecord(inventoryData);
  const features = arrayAtAnyPath(collection, [
    ["software", "features"],
    ["software", "windows_features"],
    ["software", "windowsFeatures"],
    ["operating_system", "windows_features"],
    ["operatingSystem", "windowsFeatures"],
    ["features"],
    ["windows_features"],
    ["windowsFeatures"],
  ]);
  const dedupe = new Map<string, WindowsFeatureRow>();

  for (const feature of features) {
    const featureRecord = asRecord(feature);
    if (!featureRecord) continue;
    const featureName =
      textValue(featureRecord.name, featureRecord.feature_name, featureRecord.featureName) ?? "";
    if (!featureName) continue;
    const featureNameNorm = normalizeText(featureName);
    if (!featureNameNorm) continue;
    const displayName =
      textValue(featureRecord.display_name, featureRecord.displayName) ?? featureName;
    dedupe.set(featureNameNorm, {
      featureName,
      featureNameNorm,
      displayName,
      displayNameNorm: normalizeText(displayName),
      installState: textValue(featureRecord.install_state, featureRecord.installState, featureRecord.state),
      enabled: booleanValue(featureRecord.enabled, featureRecord.is_enabled, featureRecord.isEnabled),
    });
  }

  return Array.from(dedupe.values());
}

function parsePendingUpdates(inventoryData: unknown): PendingUpdateRow[] {
  const collection = asRecord(inventoryData);
  const pendingUpdates = arrayAtAnyPath(collection, [
    ["operating_system", "updates", "software_update", "pending_updates"],
    ["operating_system", "updates", "software_update", "pending"],
    ["operating_system", "updates", "macos_software_update", "pending_updates"],
    ["operating_system", "updates", "macos_software_update", "pending"],
    ["operatingSystem", "updates", "softwareUpdate", "pendingUpdates"],
    ["operatingSystem", "updates", "macosSoftwareUpdate", "pendingUpdates"],
    ["software", "software_updates", "pending_updates"],
    ["software", "softwareUpdates", "pendingUpdates"],
    ["software", "macos_updates", "pending_updates"],
    ["software", "macosUpdates", "pendingUpdates"],
    ["operating_system", "updates", "windows_update", "pending_updates"],
    ["operating_system", "updates", "windows_update", "pending"],
    ["operatingSystem", "updates", "windowsUpdate", "pendingUpdates"],
    ["software", "windows_updates", "pending_updates"],
    ["software", "windowsUpdates", "pendingUpdates"],
    ["updates", "windows_update", "pending_updates"],
    ["updates", "pending_updates"],
    ["pending_updates"],
    ["pendingUpdates"],
  ]);
  const dedupe = new Map<string, PendingUpdateRow>();

  for (const pendingUpdate of pendingUpdates) {
    const pendingUpdateRecord = asRecord(pendingUpdate);
    if (!pendingUpdateRecord) continue;
    const title =
      textValue(pendingUpdateRecord.title, pendingUpdateRecord.name, pendingUpdateRecord.kb) ?? "";
    if (!title) continue;
    const titleNorm = normalizeText(title);
    if (!titleNorm) continue;
    const kbArticle = textValue(pendingUpdateRecord.kb_article, pendingUpdateRecord.kbArticle, pendingUpdateRecord.kb);
    const uniqueKey = `${titleNorm}|${kbArticle ?? ""}`;
    dedupe.set(uniqueKey, {
      title,
      titleNorm,
      description: textValue(pendingUpdateRecord.description),
      kbArticle,
      isMandatory: booleanValue(pendingUpdateRecord.is_mandatory, pendingUpdateRecord.isMandatory),
      sizeBytes: bigintValue(pendingUpdateRecord.size_bytes, pendingUpdateRecord.sizeBytes, pendingUpdateRecord.size),
      requiresReboot: booleanValue(pendingUpdateRecord.requires_reboot, pendingUpdateRecord.requiresReboot),
    });
  }

  return Array.from(dedupe.values());
}

function parseInstalledUpdates(inventoryData: unknown): InstalledUpdateRow[] {
  const collection = asRecord(inventoryData);
  const updateHistory = arrayAtAnyPath(collection, [
    ["operating_system", "updates", "update_history"],
    ["operatingSystem", "updates", "updateHistory"],
    ["updates", "update_history"],
    ["updates", "updateHistory"],
    ["update_history"],
    ["updateHistory"],
  ]);
  const dedupe = new Map<string, InstalledUpdateRow>();

  for (const updateHistoryEntry of updateHistory) {
    const updateHistoryRecord = asRecord(updateHistoryEntry);
    if (!updateHistoryRecord) continue;
    const title =
      textValue(updateHistoryRecord.title, updateHistoryRecord.name, updateHistoryRecord.package) ?? "";
    if (!title) continue;
    const titleNorm = normalizeText(title);
    if (!titleNorm) continue;
    const installedAtRaw =
      textValue(updateHistoryRecord.date, updateHistoryRecord.installed_at, updateHistoryRecord.installedAt);
    const installedAt =
      installedAtRaw && !Number.isNaN(Date.parse(installedAtRaw))
        ? new Date(installedAtRaw)
        : null;
    const kbArticle = textValue(updateHistoryRecord.kb_article, updateHistoryRecord.kbArticle, updateHistoryRecord.kb);
    dedupe.set(`${titleNorm}|${installedAt?.toISOString() ?? ""}|${kbArticle ?? ""}`, {
      installedAt,
      title,
      titleNorm,
      kbArticle,
      operation: textValue(updateHistoryRecord.operation, updateHistoryRecord.action),
      result: textValue(updateHistoryRecord.result, updateHistoryRecord.status),
      hresult: numberValue(updateHistoryRecord.hresult),
    });
  }

  return Array.from(dedupe.values());
}

function uniquePatchUpdatesByUpdateKey<T extends { title: string; kbArticle: string | null }>(
  updates: T[],
): T[] {
  const seen = new Set<string>();
  const unique: T[] = [];
  for (const update of updates) {
    const updateKey = buildUpdateKeyFromParts(update.title, update.kbArticle);
    if (seen.has(updateKey)) {
      continue;
    }
    seen.add(updateKey);
    unique.push(update);
  }
  return unique;
}

function splitTagText(value: string | null | undefined): string[] {
  if (!value) return [];
  return value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
    .slice(0, 20);
}

type HealthSignalRow = {
  agentId: string;
  commandFailureCount: number;
  latestCommandFailureAt: Date | null;
  updaterFailureCount: number;
  latestUpdaterFailureAt: Date | null;
  remediationFailureCount: number;
  latestRemediationFailureAt: Date | null;
};

type HealthAlertRow = {
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
};

type RawCountRow = {
  agent_id: string;
  count: number | bigint | string | null;
  latest_at: Date | null;
};

type RawHealthAlertRow = {
  id: bigint;
  agent_id: string;
  alert_key: string;
  severity: string;
  status: string;
  reason: string;
  detail: string | null;
  first_seen_at: Date;
  last_seen_at: Date;
  resolved_at: Date | null;
  occurrence_count: number | bigint | string;
};

function normalizeCount(value: number | bigint | string | null | undefined): number {
  if (typeof value === "bigint") return Number(value);
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

function readTargetAgentVersion(): string | null {
  const configured =
    process.env.RMM_AGENT_HEALTH_TARGET_VERSION ||
    process.env.RMM_AGENT_LATEST_VERSION ||
    process.env.RMM_AGENT_VERSION ||
    "";
  return configured.trim() || null;
}

function isoDate(value: unknown): string | null {
  if (value instanceof Date) return value.toISOString();
  if (typeof value === "string" && value.trim() && !Number.isNaN(Date.parse(value))) {
    return new Date(value).toISOString();
  }
  return null;
}

async function fetchHealthSignals(agentIds: string[]): Promise<Map<string, HealthSignalRow>> {
  const signals = new Map<string, HealthSignalRow>();
  for (const agentId of agentIds) {
    signals.set(agentId, {
      agentId,
      commandFailureCount: 0,
      latestCommandFailureAt: null,
      updaterFailureCount: 0,
      latestUpdaterFailureAt: null,
      remediationFailureCount: 0,
      latestRemediationFailureAt: null,
    });
  }
  if (agentIds.length === 0) return signals;

  const [commandRows, updaterRows, remediationRows] = await Promise.all([
    prisma.$queryRaw<RawCountRow[]>(Prisma.sql`
      SELECT agent_id, COUNT(*)::int AS count, MAX(created_at) AS latest_at
      FROM public.command_execution_log
      WHERE agent_id IN (${Prisma.join(agentIds)})
        AND created_at >= NOW() - INTERVAL '24 hours'
        AND (was_allowed = false OR (exit_code IS NOT NULL AND exit_code <> 0))
      GROUP BY agent_id
    `),
    prisma.$queryRaw<RawCountRow[]>(Prisma.sql`
      SELECT agent_id, COUNT(*)::int AS count, MAX(occurred_at) AS latest_at
      FROM rmm_telemetry.device_event
      WHERE agent_id IN (${Prisma.join(agentIds)})
        AND occurred_at >= NOW() - INTERVAL '24 hours'
        AND (
          LOWER(event_type) LIKE '%updat%'
          OR LOWER(source) LIKE '%updat%'
          OR LOWER(COALESCE(code, '')) LIKE '%updat%'
          OR LOWER(COALESCE(message, '')) LIKE '%updat%'
        )
        AND (
          LOWER(severity) IN ('critical', 'high', 'error')
          OR LOWER(COALESCE(message, '')) LIKE '%fail%'
          OR LOWER(COALESCE(code, '')) LIKE '%fail%'
        )
      GROUP BY agent_id
    `),
    prisma.$queryRaw<RawCountRow[]>(Prisma.sql`
      SELECT agent_id, COUNT(*)::int AS count, MAX(COALESCE(finished_at, started_at, requested_at)) AS latest_at
      FROM rmm_telemetry.remediation_job
      WHERE agent_id IN (${Prisma.join(agentIds)})
        AND requested_at >= NOW() - INTERVAL '24 hours'
        AND status IN ('failed', 'cancelled')
      GROUP BY agent_id
    `),
  ]);

  for (const row of commandRows) {
    const signal = signals.get(row.agent_id);
    if (!signal) continue;
    signal.commandFailureCount = normalizeCount(row.count);
    signal.latestCommandFailureAt = row.latest_at;
  }
  for (const row of updaterRows) {
    const signal = signals.get(row.agent_id);
    if (!signal) continue;
    signal.updaterFailureCount = normalizeCount(row.count);
    signal.latestUpdaterFailureAt = row.latest_at;
  }
  for (const row of remediationRows) {
    const signal = signals.get(row.agent_id);
    if (!signal) continue;
    signal.remediationFailureCount = normalizeCount(row.count);
    signal.latestRemediationFailureAt = row.latest_at;
  }

  return signals;
}

function buildHealthForDevice(
  device: any,
  telemetryState: TelemetryStateRow | DeviceListTelemetrySummary | null | undefined,
  signals: HealthSignalRow | null | undefined,
  now = new Date(),
): AgentHealthSummary {
  return buildAgentHealth(
    {
      now,
      lastSeenAt: device.lastSeen ?? device.last_seen ?? null,
      websocketStatus: device.websocketStatus ?? device.websocket_status ?? "unknown",
      telemetryCollectedAt: telemetryState?.collected_at ?? null,
      agentVersion: device.version ?? null,
      telemetryAgentVersion: telemetryState?.agent_version ?? null,
      targetAgentVersion: readTargetAgentVersion(),
      rebootRequired: telemetryState?.reboot_required ?? null,
      commandFailureCount: signals?.commandFailureCount ?? 0,
      updaterFailureCount: signals?.updaterFailureCount ?? 0,
      remediationFailureCount: signals?.remediationFailureCount ?? 0,
      latestCommandFailureAt: signals?.latestCommandFailureAt ?? null,
      latestUpdaterFailureAt: signals?.latestUpdaterFailureAt ?? null,
      latestRemediationFailureAt: signals?.latestRemediationFailureAt ?? null,
    },
    readAgentHealthThresholds(),
  );
}

async function fetchHealthAlertRows(agentIds: string[], status = "active"): Promise<Map<string, HealthAlertRow[]>> {
  const result = new Map<string, HealthAlertRow[]>();
  if (agentIds.length === 0) return result;

  const rows = await prisma.$queryRaw<RawHealthAlertRow[]>(Prisma.sql`
    SELECT
      id,
      agent_id,
      alert_key,
      severity,
      status,
      reason,
      detail,
      first_seen_at,
      last_seen_at,
      resolved_at,
      occurrence_count
    FROM public.rmm_agent_health_alert
    WHERE agent_id IN (${Prisma.join(agentIds)})
      AND status = ${status}
    ORDER BY last_seen_at DESC
  `);

  for (const row of rows) {
    const item: HealthAlertRow = {
      id: row.id.toString(),
      agentId: row.agent_id,
      alertKey: row.alert_key,
      severity: row.severity,
      status: row.status,
      reason: row.reason,
      detail: row.detail,
      firstSeenAt: row.first_seen_at.toISOString(),
      lastSeenAt: row.last_seen_at.toISOString(),
      resolvedAt: row.resolved_at ? row.resolved_at.toISOString() : null,
      occurrenceCount: normalizeCount(row.occurrence_count),
    };
    result.set(row.agent_id, [...(result.get(row.agent_id) ?? []), item]);
  }
  return result;
}

async function syncHealthAlertsForDevice(
  organizationId: string,
  agentId: string,
  reasons: AgentHealthReason[],
) {
  const existingRows = await prisma.$queryRaw<Array<{ alert_key: string; status: string }>>(Prisma.sql`
    SELECT alert_key, status
    FROM public.rmm_agent_health_alert
    WHERE organization_id = ${organizationId}
      AND agent_id = ${agentId}
  `);
  const reconciliation = reconcileHealthAlerts(
    existingRows.map((row) => ({ alertKey: row.alert_key, status: row.status })),
    reasons,
  );

  for (const reason of reasons) {
    const recurrenceIncrement = reconciliation.recurringKeys.includes(reason.alertKey) ? 1 : 0;
    await prisma.$executeRaw(Prisma.sql`
      INSERT INTO public.rmm_agent_health_alert
        (
          organization_id,
          agent_id,
          alert_key,
          severity,
          status,
          reason,
          detail,
          first_seen_at,
          last_seen_at,
          resolved_at,
          occurrence_count,
          context_jsonb
        )
      VALUES
        (
          ${organizationId},
          ${agentId},
          ${reason.alertKey},
          ${reason.severity},
          'active',
          ${reason.summary},
          ${reason.detail},
          NOW(),
          NOW(),
          NULL,
          1,
          ${JSON.stringify(reason)}::jsonb
        )
      ON CONFLICT (organization_id, agent_id, alert_key)
      DO UPDATE SET
        severity = EXCLUDED.severity,
        status = 'active',
        reason = EXCLUDED.reason,
        detail = EXCLUDED.detail,
        last_seen_at = NOW(),
        resolved_at = NULL,
        occurrence_count = rmm_agent_health_alert.occurrence_count + ${recurrenceIncrement},
        context_jsonb = EXCLUDED.context_jsonb
    `);
  }

  if (reconciliation.resolveKeys.length > 0) {
    await prisma.$executeRaw(Prisma.sql`
      UPDATE public.rmm_agent_health_alert
      SET
        status = 'resolved',
        resolved_at = NOW(),
        last_seen_at = NOW()
      WHERE organization_id = ${organizationId}
        AND agent_id = ${agentId}
        AND status = 'active'
        AND alert_key IN (${Prisma.join(reconciliation.resolveKeys)})
    `);
  }
}

async function syncHealthAlertsForDevices(
  devices: any[],
  healthByAgentId: Map<string, AgentHealthSummary>,
) {
  for (const device of devices) {
    const health = healthByAgentId.get(device.agentId);
    if (!health) continue;
    await syncHealthAlertsForDevice(device.organizationId, device.agentId, health.reasons);
  }
}

function mapDevice(
  device: any,
  telemetryState?: TelemetryStateRow | DeviceListTelemetrySummary | null,
  health?: AgentHealthSummary | null,
  activeHealthAlerts: HealthAlertRow[] = [],
) {
  const inventoryData = telemetryState?.inventory_data ?? null;
  const inventorySystem =
    asRecord(valueAtPath(inventoryData, ["operating_system", "system"])) ??
    asRecord(valueAtPath(inventoryData, ["system"]));
  const hostname =
    meaningfulText(device.hostname) ??
    meaningfulText(telemetryState?.hostname) ??
    meaningfulText(inventorySystem?.hostname) ??
    device.agentId;
  const os =
    meaningfulText(device.os) ??
    meaningfulText(telemetryState?.os_name) ??
    firstMeaningfulText(
      inventorySystem?.name,
      inventorySystem?.os_name,
      asRecord(inventorySystem?.os)?.name,
      inventorySystem?.distro,
    ) ??
    "unknown";
  const ip = meaningfulIpText(device.ip) ?? firstInventoryIp(inventoryData) ?? "0.0.0.0";
  const version =
    meaningfulText(device.version) ??
    meaningfulText(telemetryState?.agent_version) ??
    null;

  return {
    agentId: device.agentId,
    hostname,
    os,
    ip,
    version,
    lastSeen: device.lastSeen,
    websocketStatus: device.websocketStatus ?? device.websocket_status ?? "unknown",
    websocketConnectedAt: isoDate(device.websocketConnectedAt ?? device.websocket_connected_at),
    websocketDisconnectedAt: isoDate(device.websocketDisconnectedAt ?? device.websocket_disconnected_at),
    lastInventory: inventoryData,
    deviceDetails: inventoryData,
    customerId: device.customerId,
    customerName: device.customer?.name ?? null,
    siteId: device.siteId ?? null,
    siteName: device.site?.name ?? null,
    pendingUpdatesCount: 'pending_updates_count' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).pending_updates_count
      : null,
    rebootRequired: 'reboot_required' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).reboot_required
      : null,
    agentVersion: 'agent_version' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).agent_version
      : device.version ?? null,
    osName: 'os_name' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).os_name
      : null,
    osVersion: 'os_version' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).os_version
      : null,
    alertSeverity: 'alert_severity' in (telemetryState ?? {})
      ? (telemetryState as DeviceListTelemetrySummary).alert_severity
      : null,
    tags: 'tag_text' in (telemetryState ?? {})
      ? splitTagText((telemetryState as DeviceListTelemetrySummary).tag_text)
      : [],
    aiRunnerAutoApprove: Boolean(device.aiRunnerAutoApprove ?? device.ai_runner_auto_approve),
    linuxShellUsername: device.linuxShellUsername ?? null,
    hasLinuxShellCredential: Boolean(device.linuxShellPasswordEnc),
    macosUpdateAccount: device.macosUpdateAccountStatusJson ?? {
      status: device.macosUpdateAccountStatus ?? null,
      required: device.macosUpdateAccountRequired ?? null,
      username: device.macosUpdateAccountUsername ?? null,
      credentialVersion: device.macosUpdateAccountCredentialVersion ?? null,
      generatedUid: device.macosUpdateAccountGeneratedUid ?? null,
      failureCode: device.macosUpdateAccountFailureCode ?? null,
      failureMessage: device.macosUpdateAccountFailureMessage ?? null,
      checkedAt: device.macosUpdateAccountLastVerifiedAt ?? null,
    },
    health: health ?? null,
    activeHealthAlerts,
  };
}

async function getOrCreateUnassigned(organizationId: string) {
  const id = unassignedCustomerId(organizationId);
  const existing = await prisma.customer.findUnique({ where: { id } });
  if (existing) return existing;

  return prisma.customer.create({
    data: {
      id,
      organizationId,
      name: "Unassigned",
      description: "Default holding customer for unassigned devices.",
      isUnassigned: true,
    },
  });
}

function mapDeviceSavedView(view: any) {
  return {
    id: view.id,
    organizationId: view.organizationId,
    userId: view.userId,
    name: view.name,
    filters: view.filters ?? {},
    sortBy: view.sortBy,
    sortDirection: view.sortDirection,
    pageSize: view.pageSize,
    createdAt: view.createdAt,
    updatedAt: view.updatedAt
  };
}

function compareDeviceAlertSeverity(
  a: any,
  b: any,
  alertRanks: Map<string, number>,
  direction: 'asc' | 'desc'
) {
  const aRank = alertRanks.get(a.agentId) ?? alertSeverityRank(null);
  const bRank = alertRanks.get(b.agentId) ?? alertSeverityRank(null);
  if (aRank !== bRank) {
    return direction === 'asc' ? aRank - bRank : bRank - aRank;
  }

  const hostnameCompare = String(a.hostname ?? '').localeCompare(String(b.hostname ?? ''), undefined, {
    sensitivity: 'base'
  });
  if (hostnameCompare !== 0) return hostnameCompare;
  return String(a.agentId ?? '').localeCompare(String(b.agentId ?? ''));
}

// GET /rmm/device-views - saved device table views for the current user/org
rmmRouter.get('/device-views', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const views = await prisma.rmmDeviceSavedView.findMany({
    where: {
      organizationId: membership.organizationId,
      userId: req.jwt!.sub
    },
    orderBy: [
      { updatedAt: 'desc' },
      { name: 'asc' }
    ]
  });

  return res.json({ items: views.map(mapDeviceSavedView) });
});

// POST /rmm/device-views - save a named device table view for the current user/org
rmmRouter.post('/device-views', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const name = cleanSavedViewName(req.body?.name);
  if (!name) {
    return res.status(400).json({ error: 'name is required' });
  }

  const state = normalizeDeviceSavedViewState(req.body?.state ?? req.body);

  try {
    const view = await prisma.rmmDeviceSavedView.create({
      data: {
        organizationId: membership.organizationId,
        userId: req.jwt!.sub,
        name,
        filters: state.filters as Prisma.InputJsonValue,
        sortBy: state.sortBy,
        sortDirection: state.sortDirection,
        pageSize: state.pageSize
      }
    });

    return res.status(201).json(mapDeviceSavedView(view));
  } catch (error: any) {
    if (error?.code === 'P2002') {
      return res.status(409).json({ error: 'A saved view with that name already exists' });
    }
    throw error;
  }
});

// PATCH /rmm/device-views/:id - update a saved device table view
rmmRouter.patch('/device-views/:id', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const existing = await prisma.rmmDeviceSavedView.findFirst({
    where: {
      id: req.params.id,
      organizationId: membership.organizationId,
      userId: req.jwt!.sub
    }
  });
  if (!existing) {
    return res.status(404).json({ error: 'Saved view not found' });
  }

  const data: Prisma.RmmDeviceSavedViewUpdateInput = {};
  if (req.body?.name !== undefined) {
    const name = cleanSavedViewName(req.body.name);
    if (!name) return res.status(400).json({ error: 'name must not be empty' });
    data.name = name;
  }

  if (req.body?.state !== undefined || req.body?.filters !== undefined) {
    const state = normalizeDeviceSavedViewState(req.body?.state ?? req.body);
    data.filters = state.filters as Prisma.InputJsonValue;
    data.sortBy = state.sortBy;
    data.sortDirection = state.sortDirection;
    data.pageSize = state.pageSize;
  }

  try {
    const view = await prisma.rmmDeviceSavedView.update({
      where: { id: existing.id },
      data
    });
    return res.json(mapDeviceSavedView(view));
  } catch (error: any) {
    if (error?.code === 'P2002') {
      return res.status(409).json({ error: 'A saved view with that name already exists' });
    }
    throw error;
  }
});

// DELETE /rmm/device-views/:id - delete a saved device table view
rmmRouter.delete('/device-views/:id', requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

  const existing = await prisma.rmmDeviceSavedView.findFirst({
    where: {
      id: req.params.id,
      organizationId: membership.organizationId,
      userId: req.jwt!.sub
    },
    select: { id: true }
  });
  if (!existing) {
    return res.status(404).json({ error: 'Saved view not found' });
  }

  await prisma.rmmDeviceSavedView.delete({ where: { id: existing.id } });
  return res.status(204).end();
});

// GET /rmm/devices - list devices for current org
rmmRouter.get("/devices", requireAuth, async (req: AuthedRequest, res) => {
  if (!assertUser(req, res)) return;
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership)
    return res
      .status(404)
      .json({ error: "No organization", needsOnboarding: true });

  const queryKeys = Object.keys(req.query ?? {});
  const hasListControls = queryKeys.some((key) =>
    [
      "page",
      "pageSize",
      "sortBy",
      "sortDirection",
      "sortDir",
      "q",
      "search",
      "customerId",
      "customer",
      "siteId",
      "site",
      "status",
      "os",
      "version",
      "agentVersion",
      "tag",
      "tags",
      "group",
      "tagGroup",
      "pendingUpdates",
      "rebootRequired",
      "alertSeverity",
      "lastSeenAgeMinutes",
      "lastSeenAge",
    ].includes(key),
  );
  const rawLegacyLimit = req.query?.limit ? Number(req.query.limit) : 200;
  const legacyPageSize = Number.isFinite(rawLegacyLimit)
    ? Math.min(Math.max(Math.trunc(rawLegacyLimit), 1), 500)
    : 200;
  const query = hasListControls
    ? parseDeviceListQuery(req.query as Record<string, unknown>)
    : parseDeviceListQuery({ pageSize: legacyPageSize });
  const where = buildDeviceListWhere({
    organizationId: membership.organizationId,
    unassignedCustomerId: unassignedCustomerId(membership.organizationId),
    filters: query.filters
  });
  const orderBy = buildDeviceListOrderBy(query.sortBy, query.sortDirection);

  let total: number;
  let devices: any[];

  if (query.sortBy === 'alertSeverity') {
    const allDevices = await prisma.rmmDevice.findMany({
      where,
      include: {
        customer: true,
        site: true
      },
      orderBy: [{ hostname: 'asc' }, { agentId: 'asc' }]
    });
    total = allDevices.length;
    const alertRanks = await fetchDeviceListAlertRanks(
      membership.organizationId,
      allDevices.map((device) => device.agentId)
    );
    devices = allDevices
      .sort((a, b) => compareDeviceAlertSeverity(a, b, alertRanks, query.sortDirection))
      .slice((query.page - 1) * query.pageSize, query.page * query.pageSize);
  } else {
    [total, devices] = await prisma.$transaction([
      prisma.rmmDevice.count({ where }),
      prisma.rmmDevice.findMany({
        where,
        include: {
          customer: true,
          site: true
        },
        orderBy,
        skip: (query.page - 1) * query.pageSize,
        take: query.pageSize
      })
    ]);
  }

  const agentIds = devices.map((device) => device.agentId);
  const [telemetrySummaries, healthSignals] = await Promise.all([
    fetchDeviceListTelemetrySummaries(
      membership.organizationId,
      agentIds
    ),
    fetchHealthSignals(agentIds),
  ]);
  const now = new Date();
  const healthByAgentId = new Map(
    devices.map((device) => [
      device.agentId,
      buildHealthForDevice(
        device,
        telemetrySummaries.get(device.agentId),
        healthSignals.get(device.agentId),
        now,
      ),
    ]),
  );
  await syncHealthAlertsForDevices(devices, healthByAgentId);
  const alertRows = await fetchHealthAlertRows(agentIds);
  const items = devices.map((device) => mapDevice(
    device,
    telemetrySummaries.get(device.agentId),
    healthByAgentId.get(device.agentId),
    alertRows.get(device.agentId) ?? [],
  ));

  if (!hasListControls) {
    return res.json(items);
  }

  res.json({
    items,
    total,
    page: query.page,
    pageSize: query.pageSize,
    sortBy: query.sortBy,
    sortDirection: query.sortDirection,
    filters: query.filters
  });
});

// GET /rmm/devices/:agentId - get single device for current org
rmmRouter.get(
  "/devices/:agentId",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });

    const device = await (prisma.rmmDevice as any).findFirst({
      where: {
        agentId: req.params.agentId,
        organizationId: membership.organizationId,
      },
      include: { customer: true, site: true },
    });

    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    const [telemetryStateRows, healthSignals] = await Promise.all([
      fetchTelemetryStateRows([device.agentId]),
      fetchHealthSignals([device.agentId]),
    ]);
    const health = buildHealthForDevice(
      device,
      telemetryStateRows.get(device.agentId),
      healthSignals.get(device.agentId),
      new Date(),
    );
    await syncHealthAlertsForDevice(device.organizationId, device.agentId, health.reasons);
    const alertRows = await fetchHealthAlertRows([device.agentId]);
    res.json(mapDevice(
      device,
      telemetryStateRows.get(device.agentId),
      health,
      alertRows.get(device.agentId) ?? [],
    ));
  },
);

// PATCH /rmm/devices/:agentId/settings - update admin-managed device settings
rmmRouter.patch(
  "/devices/:agentId/settings",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!assertAgentAdmin(membership, res)) return;

    if (typeof req.body?.aiRunnerAutoApprove !== "boolean") {
      return res.status(400).json({ error: "aiRunnerAutoApprove must be a boolean" });
    }

    const existing = await prisma.rmmDevice.findFirst({
      where: {
        agentId: req.params.agentId,
        organizationId: membership.organizationId,
      },
      include: { customer: true, site: true },
    });
    if (!existing) {
      return res.status(404).json({ error: "Device not found" });
    }

    const previousAiRunnerAutoApprove = Boolean(existing.aiRunnerAutoApprove);
    const nextAiRunnerAutoApprove = req.body.aiRunnerAutoApprove;
    const device = await prisma.rmmDevice.update({
      where: { agentId: existing.agentId },
      data: { aiRunnerAutoApprove: nextAiRunnerAutoApprove },
      include: { customer: true, site: true },
    });

    await writeAuditEvent(auditRequest(req, {
      organizationId: membership.organizationId,
      customerId: device.customerId ?? null,
      siteId: device.siteId ?? null,
      agentId: device.agentId,
      actorType: "user",
      userId: membership.userId,
      userEmail: membership.user?.email ?? null,
      actionType: "device.settings.update",
      targetType: "rmm_device",
      targetId: device.agentId,
      targetName: device.hostname,
      result: "success",
      metadata: {
        aiRunnerAutoApprove: {
          previous: previousAiRunnerAutoApprove,
          next: nextAiRunnerAutoApprove,
        },
      },
    }));

    const [telemetryStateRows, healthSignals] = await Promise.all([
      fetchTelemetryStateRows([device.agentId]),
      fetchHealthSignals([device.agentId]),
    ]);
    const health = buildHealthForDevice(
      device,
      telemetryStateRows.get(device.agentId),
      healthSignals.get(device.agentId),
      new Date(),
    );
    await syncHealthAlertsForDevice(device.organizationId, device.agentId, health.reasons);
    const alertRows = await fetchHealthAlertRows([device.agentId]);
    return res.json(mapDevice(
      device,
      telemetryStateRows.get(device.agentId),
      health,
      alertRows.get(device.agentId) ?? [],
    ));
  },
);

// GET /rmm/devices/:agentId/linux-shell-credential - reveal Linux sudo credential
rmmRouter.get(
  "/devices/:agentId/linux-shell-credential",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!isAgentAdminRole(membership.role)) {
      return res.status(403).json({ error: "Only admins can reveal device credentials" });
    }

    const device = await (prisma.rmmDevice as any).findFirst({
      where: {
        agentId: req.params.agentId,
        organizationId: membership.organizationId,
      },
      select: {
        agentId: true,
        os: true,
        linuxShellUsername: true,
        linuxShellPasswordEnc: true,
        linuxShellCredentialId: true,
        linuxShellCredentialVersion: true,
        linuxShellPasswordUpdatedAt: true,
      },
    });

    if (!device) return res.status(404).json({ error: "Device not found" });
    if (!device.linuxShellUsername || !device.linuxShellPasswordEnc) {
      return res.status(404).json({ error: "Linux shell credential is not available for this device" });
    }

    const password = decryptSecret(device.linuxShellPasswordEnc);
    if (!password) {
      return res.status(500).json({ error: "Linux shell credential could not be decrypted" });
    }

    res.json({
      agentId: device.agentId,
      username: device.linuxShellUsername,
      password,
      credentialId: device.linuxShellCredentialId,
      version: device.linuxShellCredentialVersion,
      updatedAt: device.linuxShellPasswordUpdatedAt?.toISOString() ?? null,
    });
  },
);

// GET /rmm/devices/:agentId/telemetry - full telemetry breakdown for device (apps, services, updates, etc.)
rmmRouter.get(
  "/devices/:agentId/telemetry",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });

    const agentId = req.params.agentId ? String(req.params.agentId) : "";
    if (!agentId.trim()) {
      return res.status(400).json({ error: "agentId is required" });
    }

    const device = await prisma.rmmDevice.findFirst({
      where: {
        agentId,
        organizationId: membership.organizationId,
      },
      include: {
        telemetryState: true,
        telemetryInstalledApps: true,
        telemetryServices: true,
        telemetryStartupItems: true,
        telemetryWindowsFeatures: true,
        telemetryPendingUpdates: true,
        telemetryInstalledUpdates: true,
      },
    });

    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    const state = device.telemetryState;
    const deviceState = state
      ? {
          collectedAt: state.collectedAt.toISOString(),
          hostname: meaningfulText(state.hostname) ?? device.hostname,
          osName: meaningfulText(state.osName) ?? device.os,
          osVersion: state.osVersion,
          agentVersion:
            meaningfulText(state.agentVersion) ?? device.version ?? null,
          bootSessionId: state.bootSessionId,
          cpuModel: state.cpuModel,
          cpuPhysicalCores: state.cpuPhysicalCores,
          cpuLogicalCores: state.cpuLogicalCores,
          cpuBaseMhz: state.cpuBaseMhz,
          memoryTotalBytes:
            state.memoryTotalBytes != null
              ? Number(state.memoryTotalBytes)
              : null,
          installedAppsCount: state.installedAppsCount,
          pendingUpdatesCount: state.pendingUpdatesCount,
          rebootRequired: state.rebootRequired,
          inventoryData: state.inventoryData,
        }
      : null;

    const mapApp = (row: {
      appName: string;
      publisher: string | null;
      version: string | null;
      installDate: string | null;
      sizeBytes: bigint | null;
      source: string | null;
      location: string | null;
      is64Bit: boolean | null;
    }) => ({
      appName: row.appName,
      publisher: row.publisher,
      version: row.version,
      installDate: row.installDate,
      sizeBytes: row.sizeBytes != null ? Number(row.sizeBytes) : null,
      source: row.source,
      location: row.location,
      is64Bit: row.is64Bit,
    });

    const mapService = (row: {
      serviceName: string;
      displayName: string;
      status: string;
      startType: string | null;
      account: string | null;
      processId: number | null;
      isCritical: boolean | null;
      description: string | null;
      path: string | null;
    }) => ({
      serviceName: row.serviceName,
      displayName: row.displayName,
      status: row.status,
      startType: row.startType,
      account: row.account,
      processId: row.processId,
      isCritical: row.isCritical,
      description: row.description,
      path: row.path,
    });

    const mapStartupItem = (row: {
      itemName: string;
      command: string;
      location: string;
      userName: string | null;
      isEnabled: boolean | null;
    }) => ({
      itemName: row.itemName,
      command: row.command,
      location: row.location,
      userName: row.userName,
      isEnabled: row.isEnabled,
    });

    const mapWindowsFeature = (row: {
      featureName: string;
      displayName: string;
      installState: string | null;
      enabled: boolean | null;
    }) => ({
      featureName: row.featureName,
      displayName: row.displayName,
      installState: row.installState,
      enabled: row.enabled,
    });

    const mapPendingUpdate = (row: {
      title: string;
      description: string | null;
      kbArticle: string | null;
      isMandatory: boolean | null;
      sizeBytes: bigint | null;
      requiresReboot: boolean | null;
    }) => ({
      title: row.title,
      description: row.description,
      kbArticle: row.kbArticle,
      isMandatory: row.isMandatory,
      sizeBytes: row.sizeBytes != null ? Number(row.sizeBytes) : null,
      requiresReboot: row.requiresReboot,
    });

    const mapInstalledUpdate = (row: {
      installedAt: Date | null;
      title: string;
      kbArticle: string | null;
      operation: string | null;
      result: string | null;
      hresult: number | null;
    }) => ({
      installedAt: row.installedAt ? row.installedAt.toISOString() : null,
      title: row.title,
      kbArticle: row.kbArticle,
      operation: row.operation,
      result: row.result,
      hresult: row.hresult,
    });

    const stateInventory = state?.inventoryData ?? null;
    const installedApps =
      device.telemetryInstalledApps.length > 0
        ? device.telemetryInstalledApps
        : parseInstalledApps(stateInventory);
    const services =
      device.telemetryServices.length > 0
        ? device.telemetryServices
        : parseServices(stateInventory);
    const startupItems =
      device.telemetryStartupItems.length > 0
        ? device.telemetryStartupItems
        : parseStartupItems(stateInventory);
    const windowsFeatures =
      device.telemetryWindowsFeatures.length > 0
        ? device.telemetryWindowsFeatures
        : parseWindowsFeatures(stateInventory);
    const pendingUpdates =
      device.telemetryPendingUpdates.length > 0
        ? device.telemetryPendingUpdates
        : parsePendingUpdates(stateInventory);
    const installedUpdates =
      device.telemetryInstalledUpdates.length > 0
        ? device.telemetryInstalledUpdates
        : parseInstalledUpdates(stateInventory);

    res.json({
      deviceState,
      installedApps: installedApps.map(mapApp),
      services: services.map(mapService),
      startupItems: startupItems.map(mapStartupItem),
      windowsFeatures: windowsFeatures.map(mapWindowsFeature),
      pendingUpdates: pendingUpdates.map(mapPendingUpdate),
      installedUpdates: installedUpdates.map(mapInstalledUpdate),
    });
  },
);

// POST /rmm/devices/:agentId/macos-update-account-status - store non-secret macOS update account status (internal)
rmmRouter.post(
  "/devices/:agentId/macos-update-account-status",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId;
    const status = readNonEmptyString(req.body?.status);
    const username = readNonEmptyString(req.body?.username);
    const checkedAtRaw = readNonEmptyString(req.body?.checkedAt);
    const checkedAt = checkedAtRaw ? new Date(checkedAtRaw) : new Date();
    const credentialVersionRaw = Number(req.body?.credentialVersion);
    const discoveredVolumeOwners = Array.isArray(req.body?.discoveredVolumeOwners)
      ? req.body.discoveredVolumeOwners.map((owner: any) => ({
          username: readNonEmptyString(owner?.username) || null,
          fullName: readNonEmptyString(owner?.fullName) || null,
          generatedUid: readNonEmptyString(owner?.generatedUid) || null,
          volumeOwner: owner?.volumeOwner === true,
        }))
      : [];

    if (!status) {
      return res.status(400).json({ error: "status is required" });
    }
    if (Number.isNaN(checkedAt.getTime())) {
      return res.status(400).json({ error: "checkedAt must be a valid timestamp" });
    }
    const credentialVersion = Number.isFinite(credentialVersionRaw) ? Math.trunc(credentialVersionRaw) : null;
    const storedStatus = {
      schemaVersion: Number.isFinite(Number(req.body?.schemaVersion)) ? Math.trunc(Number(req.body.schemaVersion)) : 1,
      required: typeof req.body?.required === "boolean" ? req.body.required : null,
      status,
      username: username || null,
      isAppleSilicon: typeof req.body?.isAppleSilicon === "boolean" ? req.body.isAppleSilicon : null,
      accountPresent: typeof req.body?.accountPresent === "boolean" ? req.body.accountPresent : null,
      isAdmin: typeof req.body?.isAdmin === "boolean" ? req.body.isAdmin : null,
      isVolumeOwner: typeof req.body?.isVolumeOwner === "boolean" ? req.body.isVolumeOwner : null,
      secureTokenEnabled: typeof req.body?.secureTokenEnabled === "boolean" ? req.body.secureTokenEnabled : null,
      credentialAvailable: typeof req.body?.credentialAvailable === "boolean" ? req.body.credentialAvailable : null,
      credentialVersion,
      generatedUid: readNonEmptyString(req.body?.generatedUid) || null,
      expectedGeneratedUid: readNonEmptyString(req.body?.expectedGeneratedUid) || null,
      discoveredVolumeOwners,
      failureCode: readNonEmptyString(req.body?.failureCode) || null,
      failureMessage: readNonEmptyString(req.body?.failureMessage) || null,
      checkedAt: checkedAt.toISOString(),
    };

    await (prisma.rmmDevice as any).update({
      where: { agentId },
      data: {
        macosUpdateAccountStatus: status,
        macosUpdateAccountRequired: storedStatus.required,
        macosUpdateAccountUsername: username || null,
        macosUpdateAccountCredentialVersion: credentialVersion,
        macosUpdateAccountGeneratedUid: storedStatus.generatedUid,
        macosUpdateAccountFailureCode: storedStatus.failureCode,
        macosUpdateAccountFailureMessage: storedStatus.failureMessage,
        macosUpdateAccountLastVerifiedAt: checkedAt,
        macosUpdateAccountStatusJson: storedStatus,
      },
    });

    res.json({ accepted: true, storedAt: new Date().toISOString() });
  },
);

// POST /rmm/devices/:agentId/linux-shell-credential - store generated credential (internal)
rmmRouter.post(
  "/devices/:agentId/linux-shell-credential",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId;
    const username = readNonEmptyString(req.body?.username);
    const password = readNonEmptyString(req.body?.password);
    const credentialId = readNonEmptyString(req.body?.credentialId);
    const versionRaw = Number(req.body?.version ?? 1);
    const generatedAtRaw = readNonEmptyString(req.body?.generatedAt);
    const generatedAt = generatedAtRaw ? new Date(generatedAtRaw) : new Date();

    if (!username || !password || !credentialId) {
      return res.status(400).json({ error: "username, password, and credentialId are required" });
    }
    if (!Number.isFinite(versionRaw) || versionRaw < 1) {
      return res.status(400).json({ error: "version must be a positive number" });
    }
    if (Number.isNaN(generatedAt.getTime())) {
      return res.status(400).json({ error: "generatedAt must be a valid timestamp" });
    }

    const passwordEnc = encryptSecret(password);
    await (prisma.rmmDevice as any).update({
      where: { agentId },
      data: {
        linuxShellUsername: username,
        linuxShellPasswordEnc: passwordEnc,
        linuxShellCredentialId: credentialId,
        linuxShellCredentialVersion: Math.trunc(versionRaw),
        linuxShellPasswordUpdatedAt: generatedAt,
      },
    });

    res.json({
      accepted: true,
      credentialId,
      storedAt: new Date().toISOString(),
    });
  },
);

// POST /rmm/devices/:agentId/linux-shell-credential/reveal-for-user - reveal for viewer session (internal)
rmmRouter.post(
  "/devices/:agentId/linux-shell-credential/reveal-for-user",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId;
    const userId = readNonEmptyString(req.body?.userId);
    if (!userId) return res.status(400).json({ error: "userId is required" });

    const device = await (prisma.rmmDevice as any).findUnique({
      where: { agentId },
      select: {
        agentId: true,
        organizationId: true,
        linuxShellUsername: true,
        linuxShellPasswordEnc: true,
        linuxShellCredentialId: true,
        linuxShellCredentialVersion: true,
        linuxShellPasswordUpdatedAt: true,
      },
    });
    if (!device) return res.status(404).json({ error: "Device not found" });

    const membership = await prisma.organizationMember.findFirst({
      where: { userId, organizationId: device.organizationId },
      select: { role: true },
    });
    if (!membership || !isAgentAdminRole(membership.role)) {
      return res.status(403).json({ error: "Only admins can reveal device credentials" });
    }
    if (!device.linuxShellUsername || !device.linuxShellPasswordEnc) {
      return res.status(404).json({ error: "Linux shell credential is not available for this device" });
    }

    const password = decryptSecret(device.linuxShellPasswordEnc);
    if (!password) {
      return res.status(500).json({ error: "Linux shell credential could not be decrypted" });
    }

    res.json({
      agentId: device.agentId,
      username: device.linuxShellUsername,
      password,
      credentialId: device.linuxShellCredentialId,
      version: device.linuxShellCredentialVersion,
      updatedAt: device.linuxShellPasswordUpdatedAt?.toISOString() ?? null,
    });
  },
);

// GET /rmm/devices/:agentId/telemetry/status - lightweight polling status for latest collectedAt
rmmRouter.get(
  "/devices/:agentId/telemetry/status",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });

    const agentId = req.params.agentId ? String(req.params.agentId) : "";
    if (!agentId.trim()) {
      return res.status(400).json({ error: "agentId is required" });
    }

    const device = await prisma.rmmDevice.findFirst({
      where: {
        agentId,
        organizationId: membership.organizationId,
      },
      select: { agentId: true },
    });

    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    const state = await prisma.rmmTelemetryDeviceState.findUnique({
      where: { agentId },
      select: { collectedAt: true },
    });

    return res.json({
      collectedAt: state?.collectedAt ? state.collectedAt.toISOString() : null,
    });
  },
);

// GET /rmm/devices/:agentId/command-log - command execution audit trail for device
rmmRouter.get(
  "/devices/:agentId/command-log",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });

    const agentId = req.params.agentId ? String(req.params.agentId) : "";
    if (!agentId.trim()) {
      return res.status(400).json({ error: "agentId is required" });
    }

    // Make sure the device is in the caller's org.
    const device = await prisma.rmmDevice.findFirst({
      where: {
        agentId,
        organizationId: membership.organizationId,
      },
      select: { agentId: true },
    });
    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    const rawLimit = req.query?.limit ? Number(req.query.limit) : 50;
    const limit = Number.isFinite(rawLimit)
      ? Math.min(Math.max(rawLimit, 1), 200)
      : 50;

    const q = typeof req.query?.q === "string" ? req.query.q.trim() : "";
    const query = q.length > 200 ? q.slice(0, 200) : q;

    const allowedRaw =
      typeof req.query?.allowed === "string" ? req.query.allowed.trim() : "";
    const allowed =
      allowedRaw === "1" || allowedRaw.toLowerCase() === "true"
        ? true
        : allowedRaw === "0" || allowedRaw.toLowerCase() === "false"
          ? false
          : null;

    const cursorRaw =
      typeof req.query?.cursor === "string" ? req.query.cursor.trim() : "";
    let cursor: bigint | null = null;
    if (cursorRaw) {
      try {
        cursor = BigInt(cursorRaw);
      } catch {
        return res
          .status(400)
          .json({ error: "cursor must be a bigint string" });
      }
    }

    const logs = await prisma.commandExecutionLog.findMany({
      where: {
        organizationId: membership.organizationId,
        agentId,
        ...(cursor ? { id: { lt: cursor } } : {}),
        ...(allowed === null ? {} : { wasAllowed: allowed }),
        ...(query ? { command: { contains: query, mode: "insensitive" } } : {}),
      },
      orderBy: { id: "desc" },
      take: limit,
    });

    const userIds = Array.from(
      new Set(logs.map((log) => log.userId).filter(Boolean)),
    );
    const users =
      userIds.length > 0
        ? await prisma.user.findMany({
            where: { id: { in: userIds } },
            select: { id: true, email: true },
          })
        : [];
    const userEmailById = new Map(users.map((user) => [user.id, user.email]));

    const items = logs.map((log) => ({
      id: log.id.toString(),
      createdAt: log.createdAt.toISOString(),
      customerId: log.customerId,
      userId: log.userId,
      userEmail: userEmailById.get(log.userId) ?? null,
      agentId: log.agentId,
      command: log.command,
      wasAllowed: log.wasAllowed,
      denialReason: log.denialReason,
      matchedPolicyId: log.matchedPolicyId
        ? log.matchedPolicyId.toString()
        : null,
      executionTimeMs: log.executionTimeMs,
      exitCode: log.exitCode,
      outputLength: log.outputLength,
    }));

    const nextCursor =
      logs.length === limit ? logs[logs.length - 1]!.id.toString() : null;
    res.json({ items, nextCursor });
  },
);

// DELETE /rmm/devices/:agentId - delete device for current org
rmmRouter.delete(
  "/devices/:agentId",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!assertAgentAdmin(membership, res)) return;

    const result = await prisma.rmmDevice.deleteMany({
      where: {
        agentId: req.params.agentId,
        organizationId: membership.organizationId,
      },
    });

    if (result.count === 0) {
      return res.status(404).json({ error: "Device not found" });
    }

    res.json({ deleted: result.count });
  },
);

// POST /rmm/devices/bulk-delete - delete many devices for current org
rmmRouter.post(
  "/devices/bulk-delete",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!assertAgentAdmin(membership, res)) return;

    const deviceIds = Array.isArray(req.body?.deviceIds)
      ? req.body.deviceIds
      : [];
    if (deviceIds.length === 0) {
      return res.status(400).json({ error: "deviceIds is required" });
    }

    const result = await prisma.rmmDevice.deleteMany({
      where: {
        agentId: { in: deviceIds },
        organizationId: membership.organizationId,
      },
    });

    res.json({ deleted: result.count });
  },
);

// POST /rmm/devices/bulk-update-customer - update customer for many devices
rmmRouter.post(
  "/devices/bulk-update-customer",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!assertAgentAdmin(membership, res)) return;

    const deviceIds = Array.isArray(req.body?.deviceIds)
      ? req.body.deviceIds
      : [];
    const customerId = req.body?.customerId ? String(req.body.customerId) : "";
    if (deviceIds.length === 0) {
      return res.status(400).json({ error: "deviceIds is required" });
    }
    if (!customerId.trim()) {
      return res.status(400).json({ error: "customerId is required" });
    }

    const customer = await prisma.customer.findUnique({
      where: { id: customerId },
    });
    if (!customer || customer.organizationId !== membership.organizationId) {
      return res.status(404).json({ error: "Customer not found" });
    }

    const result = await prisma.rmmDevice.updateMany({
      where: {
        agentId: { in: deviceIds },
        organizationId: membership.organizationId,
      },
      data: {
        customerId: customer.id,
        siteId: null,
      },
    });

    res.json({ updated: result.count });
  },
);

// POST /rmm/devices/bulk-update-site - update site for many devices (and customer when assigning to a site)
rmmRouter.post(
  "/devices/bulk-update-site",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });
    if (!assertAgentAdmin(membership, res)) return;

    const deviceIds = Array.isArray(req.body?.deviceIds)
      ? req.body.deviceIds
      : [];
    const siteIdParam = req.body?.siteId;
    const siteId =
      siteIdParam === null || siteIdParam === undefined
        ? null
        : String(siteIdParam).trim() || null;

    if (deviceIds.length === 0) {
      return res.status(400).json({ error: "deviceIds is required" });
    }

    let data:
      | { siteId: null; customerId?: string }
      | { siteId: string; customerId: string };
    if (siteId) {
      const site = await prisma.rmmSite.findFirst({
        where: {
          id: siteId,
          customer: { organizationId: membership.organizationId },
        },
        include: { customer: true },
      });
      if (!site) return res.status(404).json({ error: "Site not found" });
      data = { siteId: site.id, customerId: site.customerId };
    } else {
      data = { siteId: null };
    }

    const result = await prisma.rmmDevice.updateMany({
      where: {
        agentId: { in: deviceIds },
        organizationId: membership.organizationId,
      },
      data,
    });

    res.json({ updated: result.count });
  },
);

// POST /rmm/devices - upsert device (internal)
rmmRouter.post(
  "/devices",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.body?.agentId ? String(req.body.agentId) : "";
    const organizationIdInput = req.body?.organizationId
      ? String(req.body.organizationId).trim()
      : "";
    const hostname = req.body?.hostname ? String(req.body.hostname) : "";
    const os = req.body?.os ? String(req.body.os) : "";
    const ip = req.body?.ip ? String(req.body.ip) : "";
    const version = req.body?.version ? String(req.body.version) : null;

    if (!agentId || !hostname || !os || !ip) {
      return res
        .status(400)
        .json({ error: "agentId, hostname, os, and ip are required" });
    }

    const existing = await prisma.rmmDevice.findUnique({
      where: { agentId },
      select: { organizationId: true },
    });
    const organizationId =
      organizationIdInput || existing?.organizationId || "";
    if (!organizationId) {
      return res
        .status(400)
        .json({ error: "organizationId is required for new devices" });
    }
    if (
      existing?.organizationId &&
      organizationIdInput &&
      existing.organizationId !== organizationIdInput
    ) {
      return res
        .status(409)
        .json({
          error: "organizationId cannot be changed for an existing device",
        });
    }

    const now = new Date();
    const device = await prisma.rmmDevice.upsert({
      where: { agentId },
      create: {
        agentId,
        organizationId,
        hostname,
        os,
        ip,
        version,
        lastSeen: now,
      },
      update: {
        hostname,
        os,
        ip,
        version,
        lastSeen: now,
      },
      include: { customer: true, site: true },
    });

    const telemetryStateRows = await fetchTelemetryStateRows([device.agentId]);
    res.json(mapDevice(device, telemetryStateRows.get(device.agentId)));
  },
);

// POST /rmm/devices/:agentId/connection-status - websocket presence from talos_server (internal)
rmmRouter.post(
  "/devices/:agentId/connection-status",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId ? String(req.params.agentId).trim() : "";
    const organizationId = req.body?.organizationId ? String(req.body.organizationId).trim() : "";
    const rawStatus = req.body?.status ? String(req.body.status).trim().toLowerCase() : "";
    const status = rawStatus === "connected" || rawStatus === "disconnected" ? rawStatus : null;
    const observedAtRaw = req.body?.observedAt ? String(req.body.observedAt).trim() : "";
    const observedAt =
      observedAtRaw && !Number.isNaN(Date.parse(observedAtRaw))
        ? new Date(observedAtRaw)
        : new Date();
    const version = req.body?.version ? String(req.body.version).trim() || null : null;

    if (!agentId || !status) {
      return res.status(400).json({ error: "agentId and status(connected|disconnected) are required" });
    }

    const rows = await prisma.$queryRaw<any[]>(Prisma.sql`
      UPDATE public.rmm_devices
      SET
        websocket_status = ${status},
        websocket_connected_at = CASE
          WHEN ${status} = 'connected' THEN ${observedAt}
          ELSE websocket_connected_at
        END,
        websocket_disconnected_at = CASE
          WHEN ${status} = 'disconnected' THEN ${observedAt}
          ELSE websocket_disconnected_at
        END,
        last_seen = CASE
          WHEN ${status} = 'connected' AND last_seen < ${observedAt} THEN ${observedAt}
          ELSE last_seen
        END,
        version = COALESCE(${version}, version),
        updated_at = NOW()
      WHERE agent_id = ${agentId}
        AND (${organizationId} = '' OR organization_id = ${organizationId})
      RETURNING
        agent_id AS "agentId",
        organization_id AS "organizationId",
        hostname,
        os,
        ip,
        version,
        last_seen AS "lastSeen",
        websocket_status AS "websocketStatus",
        websocket_connected_at AS "websocketConnectedAt",
	        websocket_disconnected_at AS "websocketDisconnectedAt",
	        customer_id AS "customerId",
	        site_id AS "siteId",
	        ai_runner_auto_approve AS "aiRunnerAutoApprove"
	    `);

    const device = rows[0];
    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    const [telemetryStateRows, healthSignals] = await Promise.all([
      fetchTelemetryStateRows([device.agentId]),
      fetchHealthSignals([device.agentId]),
    ]);
    const health = buildHealthForDevice(
      device,
      telemetryStateRows.get(device.agentId),
      healthSignals.get(device.agentId),
      new Date(),
    );
    await syncHealthAlertsForDevice(device.organizationId, device.agentId, health.reasons);
    const alertRows = await fetchHealthAlertRows([device.agentId]);

    return res.json({
      updated: true,
      status,
      device: mapDevice(
        device,
        telemetryStateRows.get(device.agentId),
        health,
        alertRows.get(device.agentId) ?? [],
      ),
    });
  },
);

// PATCH /rmm/devices/:agentId - update device fields (internal)
rmmRouter.patch(
  "/devices/:agentId",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId;
    const lastInventory = req.body?.lastInventory ?? req.body?.last_inventory;
    const deviceDetails = req.body?.deviceDetails ?? req.body?.device_details;
    const inventoryData = lastInventory ?? deviceDetails ?? null;
    const lastSeen = req.body?.lastSeen
      ? new Date(req.body.lastSeen)
      : new Date();

    if (lastInventory === undefined && deviceDetails === undefined) {
      return res
        .status(400)
        .json({ error: "lastInventory or deviceDetails is required" });
    }

    const device = await prisma.rmmDevice.update({
      where: { agentId },
      data: { lastSeen },
      include: { customer: true, site: true },
    });

    await prisma.$executeRaw(
      Prisma.sql`
      INSERT INTO rmm_telemetry.device_state
        (organization_id, agent_id, collected_at, inventory_data, blob_container, blob_name, blob_content_encoding, blob_size_bytes, updated_at)
      VALUES
        (${device.organizationId}, ${agentId}, ${lastSeen}, ${inventoryData}::jsonb, 'inline', 'device_patch', null, null, NOW())
      ON CONFLICT (agent_id)
      DO UPDATE SET
        organization_id = EXCLUDED.organization_id,
        collected_at = GREATEST(rmm_telemetry.device_state.collected_at, EXCLUDED.collected_at),
        inventory_data = COALESCE(EXCLUDED.inventory_data, rmm_telemetry.device_state.inventory_data),
        updated_at = NOW()
      WHERE rmm_telemetry.device_state.blob_name IS NULL
        OR rmm_telemetry.device_state.blob_name IN ('device_patch', 'inventory_update')
    `,
    );

    const telemetryStateRows = await fetchTelemetryStateRows([device.agentId]);
    res.json(mapDevice(device, telemetryStateRows.get(device.agentId)));
  },
);

// POST /rmm/devices/:agentId/inventory - upsert inventory + snapshot (internal)
rmmRouter.post(
  "/devices/:agentId/inventory",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId;
    const organizationIdInput = req.body?.organizationId
      ? String(req.body.organizationId).trim()
      : "";
    const hostname = req.body?.hostname ? String(req.body.hostname) : "";
    const os = req.body?.os ? String(req.body.os) : "";
    const ip = req.body?.ip ? String(req.body.ip) : "";
    const version = req.body?.version ? String(req.body.version) : null;
    const inventory = req.body?.inventory;

    if (!agentId || !hostname || !os || !ip || inventory === undefined) {
      return res
        .status(400)
        .json({
          error: "agentId, hostname, os, ip, and inventory are required",
        });
    }

    const existing = await prisma.rmmDevice.findUnique({
      where: { agentId },
      select: { organizationId: true },
    });
    const organizationId =
      organizationIdInput || existing?.organizationId || "";
    if (!organizationId) {
      return res
        .status(400)
        .json({ error: "organizationId is required for new devices" });
    }
    if (
      existing?.organizationId &&
      organizationIdInput &&
      existing.organizationId !== organizationIdInput
    ) {
      return res
        .status(409)
        .json({
          error: "organizationId cannot be changed for an existing device",
        });
    }

    const now = new Date();
    const device = await prisma.rmmDevice.upsert({
      where: { agentId },
      create: {
        agentId,
        organizationId,
        hostname,
        os,
        ip,
        version,
        lastSeen: now,
      },
      update: {
        hostname,
        os,
        ip,
        version,
        lastSeen: now,
      },
      include: { customer: true, site: true },
    });

    await prisma.$executeRaw(
      Prisma.sql`
      INSERT INTO rmm_telemetry.device_state
        (organization_id, agent_id, collected_at, inventory_data, installed_apps_count, blob_container, blob_name, blob_content_encoding, blob_size_bytes, updated_at)
      VALUES
        (${device.organizationId}, ${agentId}, ${now}, ${inventory}::jsonb, ${parseInstalledApps(inventory).length}, 'inline', 'inventory_update', null, null, NOW())
      ON CONFLICT (agent_id)
      DO UPDATE SET
        organization_id = EXCLUDED.organization_id,
        collected_at = GREATEST(rmm_telemetry.device_state.collected_at, EXCLUDED.collected_at),
        inventory_data = EXCLUDED.inventory_data,
        installed_apps_count = EXCLUDED.installed_apps_count,
        updated_at = NOW()
      WHERE rmm_telemetry.device_state.blob_name IS NULL
        OR rmm_telemetry.device_state.blob_name IN ('device_patch', 'inventory_update')
    `,
    );

    const telemetryStateRows = await fetchTelemetryStateRows([device.agentId]);
    res.json(mapDevice(device, telemetryStateRows.get(device.agentId)));
  },
);

// POST /rmm/telemetry/snapshots/upsert - consumer writes blob-backed telemetry into DB (internal)
rmmRouter.post("/telemetry/snapshots/upsert", async (req, res) => {
  if (!requireServiceKey(req, res)) {
    return;
  }

  const agentId = req.body?.agentId ? String(req.body.agentId).trim() : "";
  const organizationIdInput = req.body?.organizationId
    ? String(req.body.organizationId).trim()
    : req.body?.organization_id
      ? String(req.body.organization_id).trim()
      : "";
  const collectedAt = req.body?.collectedAt
    ? String(req.body.collectedAt).trim()
    : "";
  const receivedAt = req.body?.receivedAt
    ? String(req.body.receivedAt).trim()
    : "";
  const snapshot = req.body?.snapshot;
  const blobContainerRaw = req.body?.blobContainer
    ? String(req.body.blobContainer).trim()
    : "";
  const blobNameRaw = req.body?.blobName ? String(req.body.blobName).trim() : "";
  const blobContentEncoding = req.body?.blobContentEncoding
    ? String(req.body.blobContentEncoding).trim()
    : null;
  const blobSizeBytes =
    req.body?.blobSizeBytes !== undefined
      ? Number(req.body.blobSizeBytes)
      : null;
  const snapshotRequestId = req.body?.snapshotRequestId
    ? String(req.body.snapshotRequestId).trim()
    : null;
  const requestedHostname = meaningfulText(req.body?.hostname);
  const requestedOs = meaningfulText(req.body?.os);
  const requestedIp = meaningfulIpText(req.body?.ip);
  const requestedVersion = meaningfulText(req.body?.version);

  if (
    !agentId ||
    !organizationIdInput ||
    !collectedAt ||
    snapshot === undefined
  ) {
    return res.status(400).json({
      error:
        "agentId, organizationId, collectedAt, and snapshot are required",
    });
  }
  if (Number.isNaN(Date.parse(collectedAt))) {
    return res
      .status(400)
      .json({
        error: "collectedAt must be a valid ISO-8601 timestamp",
      });
  }
  if (receivedAt && Number.isNaN(Date.parse(receivedAt))) {
    return res
      .status(400)
      .json({
        error: "receivedAt must be a valid ISO-8601 timestamp when provided",
      });
  }
  if (
    typeof snapshot !== "object" ||
    snapshot === null ||
    Array.isArray(snapshot)
  ) {
    return res.status(400).json({ error: "snapshot must be an object" });
  }
  if (
    blobSizeBytes !== null &&
    (!Number.isFinite(blobSizeBytes) || blobSizeBytes < 0)
  ) {
    return res
      .status(400)
      .json({
        error: "blobSizeBytes must be a non-negative number when provided",
      });
  }

  const collectedAtDate = new Date(collectedAt);
  const receivedAtDate =
    receivedAt && !Number.isNaN(Date.parse(receivedAt))
      ? new Date(receivedAt)
      : new Date();
  const blobContainer = blobContainerRaw || "inline-snapshot";
  const blobName =
    blobNameRaw ||
    `inline/${agentId}/${collectedAtDate.toISOString().replace(/[:.]/g, "-")}.json`;
  const inlineSnapshotSizeBytes =
    blobSizeBytes === null && blobContainerRaw === "" && blobNameRaw === ""
      ? Buffer.byteLength(JSON.stringify(snapshot), "utf8")
      : blobSizeBytes;
  const snapshotRecord = asRecord(snapshot);
  const nestedSnapshotRecord = asRecord(snapshotRecord?.snapshot);
  const collectionPayload =
    asRecord(snapshotRecord?.collection) ??
    asRecord(nestedSnapshotRecord?.collection) ??
    snapshotRecord;
  const normalizedCollectionPayload =
    asRecord(collectionPayload?.inventory) ?? collectionPayload;
  const snapshotMetadata =
    asRecord(snapshotRecord?.metadata) ?? asRecord(nestedSnapshotRecord?.metadata);
  const osSystem = asRecord(
    asRecord(normalizedCollectionPayload?.operating_system)?.system,
  ) ?? asRecord(normalizedCollectionPayload?.system);
  const snapshotHostname = firstMeaningfulText(
    snapshotMetadata?.device_name,
    snapshotMetadata?.hostname,
    osSystem?.hostname,
  );
  const snapshotIp = firstInventoryIp(normalizedCollectionPayload);
  const cpuData =
    asRecord(asRecord(normalizedCollectionPayload?.hardware)?.cpu) ??
    asRecord(normalizedCollectionPayload?.cpu);
  const memoryData =
    asRecord(asRecord(normalizedCollectionPayload?.hardware)?.memory) ??
    asRecord(normalizedCollectionPayload?.memory);
  const platformUpdates =
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "software_updates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "softwareUpdates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "macos_updates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "macosUpdates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operating_system", "updates", "software_update"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operating_system", "updates", "macos_software_update"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operatingSystem", "updates", "softwareUpdate"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operatingSystem", "updates", "macosSoftwareUpdate"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "windows_updates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["software", "windowsUpdates"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operating_system", "updates", "windows_update"])) ??
    asRecord(valueAtPath(normalizedCollectionPayload, ["operatingSystem", "updates", "windowsUpdate"]));
  const bitlockerData = asRecord(valueAtPath(normalizedCollectionPayload, ["security", "bitlocker"]));
  const bitlockerEnabled = booleanValue(bitlockerData?.enabled);
  const bitlockerVolumes = Array.isArray(bitlockerData?.volumes) ? bitlockerData.volumes.map(asRecord).filter(Boolean) : [];
  const bitlockerSystemVolume =
    bitlockerVolumes.find((volume) => sameDriveText(textValue(volume?.drive_letter, volume?.driveLetter), "C:")) ??
    bitlockerVolumes[0] ??
    null;
  const bitlockerProtectionStatus = textValue(
    bitlockerSystemVolume?.protection_status,
    bitlockerSystemVolume?.protectionStatus,
  );
  const installedApps = parseInstalledApps(normalizedCollectionPayload);
  const services = parseServices(normalizedCollectionPayload);
  const startupItems = parseStartupItems(normalizedCollectionPayload);
  const windowsFeatures = parseWindowsFeatures(normalizedCollectionPayload);
  const pendingUpdates = parsePendingUpdates(normalizedCollectionPayload);
  const installedUpdates = parseInstalledUpdates(normalizedCollectionPayload);
  const installedAppsCount = installedApps.length;
  const pendingUpdatesCount =
    numberValue(platformUpdates?.pending_count, platformUpdates?.pendingCount) ??
    pendingUpdates.length;
  console.info("rmm snapshot patch ingestion parsed", {
    agentId,
    parsedPendingUpdates: pendingUpdates.length,
    reportedPendingUpdatesCount: pendingUpdatesCount,
    destination: "rmm_patch_update_catalog",
  });
  if (pendingUpdatesCount > 0 && pendingUpdates.length === 0) {
    console.warn("rmm snapshot reported pending updates but parser found no update rows", {
      agentId,
      reportedPendingUpdatesCount: pendingUpdatesCount,
      updateSummaryKeys: platformUpdates ? Object.keys(platformUpdates) : [],
    });
  }
  const rebootRequired = booleanValue(
    platformUpdates?.pending_reboot,
    platformUpdates?.pendingReboot,
    platformUpdates?.reboot_required,
    platformUpdates?.rebootRequired,
  );
  const osName =
    typeof osSystem?.name === "string"
      ? osSystem.name
      : typeof osSystem?.os_name === "string"
        ? osSystem.os_name
        : typeof asRecord(osSystem?.os)?.name === "string"
          ? asRecord(osSystem?.os)?.name
          : typeof osSystem?.distro === "string"
            ? osSystem.distro
            : null;
  const osVersion =
    typeof osSystem?.version === "string"
      ? osSystem.version
      : typeof osSystem?.os_version === "string"
        ? osSystem.os_version
        : typeof asRecord(osSystem?.os)?.version === "string"
          ? asRecord(osSystem?.os)?.version
          : null;
  const osBuildVersion = firstMeaningfulText(
    osSystem?.build,
    osSystem?.buildVersion,
    osSystem?.build_version,
    asRecord(osSystem?.os)?.build,
    asRecord(osSystem?.os)?.buildVersion,
    asRecord(osSystem?.os)?.build_version,
    valueAtPath(normalizedCollectionPayload, ["system", "build"]),
    valueAtPath(normalizedCollectionPayload, ["system", "buildVersion"]),
    valueAtPath(normalizedCollectionPayload, ["system", "kernel_version"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "build"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "buildVersion"]),
  );
  const osArchitecture = firstMeaningfulText(
    osSystem?.architecture,
    osSystem?.osArchitecture,
    asRecord(osSystem?.os)?.architecture,
    valueAtPath(normalizedCollectionPayload, ["system", "architecture"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "architecture"]),
    valueAtPath(normalizedCollectionPayload, ["hardware", "cpu", "architecture"]),
  );
  const osEdition = firstMeaningfulText(
    osSystem?.edition,
    asRecord(osSystem?.os)?.edition,
    valueAtPath(normalizedCollectionPayload, ["system", "edition"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "edition"]),
  );
  const osLocale = firstMeaningfulText(
    osSystem?.locale,
    osSystem?.language,
    asRecord(osSystem?.os)?.locale,
    asRecord(osSystem?.os)?.language,
    valueAtPath(normalizedCollectionPayload, ["system", "locale"]),
    valueAtPath(normalizedCollectionPayload, ["system", "language"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "locale"]),
    valueAtPath(normalizedCollectionPayload, ["system", "os", "language"]),
  );
  const agentVersion =
    typeof snapshotMetadata?.agent_version === "string"
      ? snapshotMetadata.agent_version
      : requestedVersion;
  const bootSessionId =
    typeof snapshotMetadata?.boot_session_id === "string"
      ? snapshotMetadata.boot_session_id
      : null;
  const cpuModel = typeof cpuData?.name === "string" ? cpuData.name : null;
  const cpuPhysicalCores = Number.isFinite(Number(cpuData?.cores))
    ? Number(cpuData?.cores)
    : null;
  const cpuLogicalCores = Number.isFinite(Number(cpuData?.logical_cores))
    ? Number(cpuData?.logical_cores)
    : null;
  const cpuBaseMhz = Number.isFinite(Number(cpuData?.frequency_mhz))
    ? Number(cpuData?.frequency_mhz)
    : null;
  const memoryTotalBytes = Number.isFinite(Number(memoryData?.total_bytes))
    ? BigInt(Number(memoryData?.total_bytes))
    : null;
  const stateHostname = requestedHostname ?? snapshotHostname;

  const result = await prisma.$transaction(async (tx) => {
    const existing = await tx.rmmDevice.findUnique({
      where: { agentId },
      select: {
        organizationId: true,
        hostname: true,
        os: true,
        ip: true,
        version: true,
        deviceType: true,
        deviceTypeSource: true,
      },
    });
    const previousDeviceStateRows = await tx.$queryRaw<
      Array<{
        collectedAt: Date | null;
        bootSessionId: string | null;
        rebootRequired: boolean | null;
      }>
    >(
      Prisma.sql`
        SELECT
          collected_at AS "collectedAt",
          boot_session_id AS "bootSessionId",
          reboot_required AS "rebootRequired"
        FROM rmm_telemetry.device_state
        WHERE agent_id = ${agentId}
        LIMIT 1
      `,
    );
    const previousDeviceState = previousDeviceStateRows[0] ?? null;
    if (
      existing?.organizationId &&
      existing.organizationId !== organizationIdInput
    ) {
      throw new Error("organizationId mismatch for agent");
    }
    const deviceHostname =
      requestedHostname ??
      snapshotHostname ??
      meaningfulText(existing?.hostname) ??
      agentId;
    const deviceOs =
      requestedOs ??
      meaningfulText(osName) ??
      meaningfulText(existing?.os) ??
      "unknown";
    const deviceIp =
      requestedIp ??
      snapshotIp ??
      meaningfulIpText(existing?.ip) ??
      "0.0.0.0";
    const deviceVersion =
      requestedVersion ??
      meaningfulText(agentVersion) ??
      meaningfulText(existing?.version) ??
      null;
    const inferredDeviceType = inferPatchDeviceType(deviceOs);
    const deviceType =
      existing?.deviceTypeSource === "manual"
        ? existing.deviceType
        : inferredDeviceType;

    await tx.rmmDevice.upsert({
      where: { agentId },
      create: {
        agentId,
        organizationId: organizationIdInput,
        hostname: deviceHostname,
        os: deviceOs,
        ip: deviceIp,
        version: deviceVersion,
        lastSeen: collectedAtDate,
        deviceType,
        deviceTypeSource: "auto",
        patchRing: deviceType === "server" ? "critical_servers" : "broad",
        patchManaged: true,
        nativeWindowsUpdateControl: true,
      },
      update: {
        hostname: deviceHostname,
        os: deviceOs,
        ip: deviceIp,
        version: deviceVersion,
        lastSeen: collectedAtDate,
        ...(existing?.deviceTypeSource === "manual"
          ? {}
          : {
              deviceType,
              deviceTypeSource: "auto",
            }),
      },
    });

    const inserted = await tx.$executeRaw(
      Prisma.sql`
        INSERT INTO rmm_telemetry.snapshot_ingest
          (organization_id, agent_id, collected_at, received_at, blob_container, blob_name, blob_content_encoding, blob_size_bytes)
        VALUES
          (${organizationIdInput}, ${agentId}, ${collectedAtDate}, ${receivedAtDate}, ${blobContainer}, ${blobName}, ${blobContentEncoding}, ${inlineSnapshotSizeBytes})
        ON CONFLICT (agent_id, collected_at) DO NOTHING
      `,
    );

    const appliedRows = await tx.$queryRaw<{ agent_id: string }[]>(
      Prisma.sql`
        INSERT INTO rmm_telemetry.device_state
          (
            agent_id, collected_at, inventory_data, hostname, os_name, os_version, agent_version, boot_session_id,
            organization_id,
            cpu_model, cpu_physical_cores, cpu_logical_cores, cpu_base_mhz, memory_total_bytes,
            installed_apps_count, pending_updates_count, reboot_required,
            blob_container, blob_name, blob_content_encoding, blob_size_bytes, updated_at
          )
        VALUES
          (
            ${agentId}, ${collectedAtDate}, ${normalizedCollectionPayload}::jsonb, ${stateHostname}, ${osName}, ${osVersion}, ${agentVersion}, ${bootSessionId},
            ${organizationIdInput},
            ${cpuModel}, ${cpuPhysicalCores}, ${cpuLogicalCores}, ${cpuBaseMhz}, ${memoryTotalBytes},
            ${installedAppsCount}, ${pendingUpdatesCount}, ${rebootRequired},
            ${blobContainer}, ${blobName}, ${blobContentEncoding}, ${inlineSnapshotSizeBytes}, NOW()
          )
        ON CONFLICT (agent_id)
        DO UPDATE SET
          collected_at = EXCLUDED.collected_at,
          inventory_data = EXCLUDED.inventory_data,
          hostname = EXCLUDED.hostname,
          os_name = EXCLUDED.os_name,
          os_version = EXCLUDED.os_version,
          agent_version = EXCLUDED.agent_version,
          boot_session_id = EXCLUDED.boot_session_id,
          organization_id = EXCLUDED.organization_id,
          cpu_model = EXCLUDED.cpu_model,
          cpu_physical_cores = EXCLUDED.cpu_physical_cores,
          cpu_logical_cores = EXCLUDED.cpu_logical_cores,
          cpu_base_mhz = EXCLUDED.cpu_base_mhz,
          memory_total_bytes = EXCLUDED.memory_total_bytes,
          installed_apps_count = EXCLUDED.installed_apps_count,
          pending_updates_count = EXCLUDED.pending_updates_count,
          reboot_required = EXCLUDED.reboot_required,
          blob_container = EXCLUDED.blob_container,
          blob_name = EXCLUDED.blob_name,
          blob_content_encoding = EXCLUDED.blob_content_encoding,
          blob_size_bytes = EXCLUDED.blob_size_bytes,
          updated_at = NOW()
        WHERE EXCLUDED.collected_at >= rmm_telemetry.device_state.collected_at
          OR rmm_telemetry.device_state.blob_name IS NULL
          OR rmm_telemetry.device_state.blob_name IN ('device_patch', 'inventory_update')
        RETURNING agent_id
      `,
    );

    if (appliedRows.length > 0) {
      const osFactValues = [
        ["os.name", osName],
        ["os.version", osVersion],
        ["os.architecture", osArchitecture],
        ["os.edition", osEdition],
        ["os.locale", osLocale],
        ["os.language", osLocale],
        ["security.bitlocker_enabled", bitlockerEnabled],
        ["security.bitlocker_protection_status", bitlockerProtectionStatus],
      ].filter(
        (entry): entry is [string, string | boolean] =>
          (typeof entry[1] === "string" && entry[1].trim().length > 0) || typeof entry[1] === "boolean",
      );

      if (osFactValues.length > 0) {
        const factValues = Prisma.join(
          osFactValues.map(([factKey, factValue]) => {
            const factJson = JSON.stringify(factValue);
            return Prisma.sql`
              (
                ${organizationIdInput}, ${agentId}, ${factKey}, ${factJson}::jsonb, ${factJson},
                'stable', 'full_snapshot', ${collectedAtDate}, NOW()
              )
            `;
          }),
        );
        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.fact_state_current
              (organization_id, agent_id, fact_key, fact_value, fact_value_text, stability_class, source, source_ts, updated_at)
            VALUES ${factValues}
            ON CONFLICT (agent_id, fact_key)
            DO UPDATE SET
              organization_id = EXCLUDED.organization_id,
              fact_value = EXCLUDED.fact_value,
              fact_value_text = EXCLUDED.fact_value_text,
              stability_class = EXCLUDED.stability_class,
              source = EXCLUDED.source,
              source_ts = EXCLUDED.source_ts,
              updated_at = NOW()
          `,
        );
      }

      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_installed_app WHERE agent_id = ${agentId}`,
      );
      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_service WHERE agent_id = ${agentId}`,
      );
      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_startup_item WHERE agent_id = ${agentId}`,
      );
      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_windows_feature WHERE agent_id = ${agentId}`,
      );
      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_pending_update WHERE agent_id = ${agentId}`,
      );
      await tx.$executeRaw(
        Prisma.sql`DELETE FROM rmm_telemetry.device_installed_update WHERE agent_id = ${agentId}`,
      );

      if (installedApps.length > 0) {
        const values = Prisma.join(
          installedApps.map(
            (app) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${app.appName}, ${app.appNameNorm}, ${app.publisher}, ${app.publisherNorm},
              ${organizationIdInput},
              ${app.version}, ${app.installDate}, ${app.sizeBytes}, ${app.source}, ${app.location}, ${app.uninstallString}, ${app.is64Bit}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_installed_app
              (
                agent_id, collected_at, app_name, app_name_norm, publisher, publisher_norm, organization_id, version,
                install_date, size_bytes, source, location, uninstall_string, is_64_bit, updated_at
              )
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );
      }

      if (services.length > 0) {
        const values = Prisma.join(
          services.map(
            (service) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${service.serviceName}, ${service.serviceNameNorm}, ${service.displayName},
              ${organizationIdInput},
              ${service.displayNameNorm}, ${service.status}, ${service.startType}, ${service.account}, ${service.processId},
              ${service.canStop}, ${service.canPause}, ${service.isCritical}, ${service.description}, ${service.path}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_service
              (
                agent_id, collected_at, service_name, service_name_norm, display_name, organization_id, display_name_norm, status,
                start_type, account, process_id, can_stop, can_pause, is_critical, description, path, updated_at
              )
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );
      }

      if (startupItems.length > 0) {
        const values = Prisma.join(
          startupItems.map(
            (startupItem) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${startupItem.itemName}, ${startupItem.itemNameNorm},
              ${organizationIdInput},
              ${startupItem.command}, ${startupItem.location}, ${startupItem.userName}, ${startupItem.isEnabled}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_startup_item
              (agent_id, collected_at, item_name, item_name_norm, organization_id, command, location, user_name, is_enabled, updated_at)
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );
      }

      if (windowsFeatures.length > 0) {
        const values = Prisma.join(
          windowsFeatures.map(
            (feature) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${feature.featureName}, ${feature.featureNameNorm},
              ${feature.displayName},
              ${organizationIdInput}, ${feature.displayNameNorm}, ${feature.installState}, ${feature.enabled}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_windows_feature
              (
                agent_id, collected_at, feature_name, feature_name_norm, display_name,
                organization_id, display_name_norm, install_state, enabled, updated_at
              )
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );
      }

      const pendingPatchUpdates = uniquePatchUpdatesByUpdateKey(pendingUpdates);
      const pendingPatchUpdateKeys = new Set(
        pendingPatchUpdates.map((pendingUpdate) =>
          buildUpdateKeyFromParts(pendingUpdate.title, pendingUpdate.kbArticle),
        ),
      );
      const pendingRebootPatchUpdateKeys = pendingPatchUpdates
        .filter((pendingUpdate) => pendingUpdate.requiresReboot === true)
        .map((pendingUpdate) =>
          buildUpdateKeyFromParts(pendingUpdate.title, pendingUpdate.kbArticle),
        );
      let clearRebootRequiredForFailedPendingUpdates = false;

      if (pendingUpdates.length > 0) {
        let postPatchRebootFailedUpdateKeys: string[] = [];
        if (pendingRebootPatchUpdateKeys.length > 0 && previousDeviceState?.collectedAt) {
          const previousRebootUpdateRows = await tx.$queryRaw<Array<{ updateKey: string }>>(
            Prisma.sql`
              SELECT update_key AS "updateKey"
              FROM public.rmm_patch_device_update_state
              WHERE organization_id = ${organizationIdInput}
                AND agent_id = ${agentId}
                AND update_key IN (${Prisma.join(pendingRebootPatchUpdateKeys)})
                AND requires_reboot = TRUE
                AND applicability_state = 'applicable'
                AND lifecycle_state NOT IN ('failed', 'superseded')
            `,
          );
          const patchRebootIntentRows = await tx.$queryRaw<Array<{ hadIntent: boolean }>>(
            Prisma.sql`
              SELECT EXISTS (
                SELECT 1
                FROM public.rmm_patch_decision_log
                WHERE organization_id = ${organizationIdInput}
                  AND agent_id = ${agentId}
                  AND action = 'reboot'
                  AND decision = 'authorized'
                  AND decided_at >= ${previousDeviceState.collectedAt}
                  AND decided_at <= ${collectedAtDate}
                UNION ALL
                SELECT 1
                FROM public.rmm_patch_action
                WHERE organization_id = ${organizationIdInput}
                  AND agent_id = ${agentId}
                  AND action_type = 'reboot'
                  AND status IN ('running', 'completed')
                  AND updated_at >= ${previousDeviceState.collectedAt}
                  AND updated_at <= ${collectedAtDate}
                UNION ALL
                SELECT 1
                FROM public.rmm_patch_action
                WHERE organization_id = ${organizationIdInput}
                  AND agent_id = ${agentId}
                  AND action_type = 'install'
                  AND evidence_jsonb #>> '{actionResult,rebootScheduled}' = 'true'
                  AND updated_at >= ${previousDeviceState.collectedAt}
                  AND updated_at <= ${collectedAtDate}
              ) AS "hadIntent"
            `,
          );

          postPatchRebootFailedUpdateKeys = selectPostPatchRebootLoopFailureKeys({
            previousBootSessionId: previousDeviceState.bootSessionId,
            currentBootSessionId: bootSessionId,
            previousRebootRequired: previousDeviceState.rebootRequired,
            currentRebootRequired: rebootRequired,
            hadPatchRebootIntent: patchRebootIntentRows[0]?.hadIntent === true,
            pendingRebootUpdateKeys: pendingRebootPatchUpdateKeys,
            previousRebootUpdateKeys: previousRebootUpdateRows.map((row) => row.updateKey),
          });
        }

        const values = Prisma.join(
          pendingUpdates.map(
            (pendingUpdate) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${pendingUpdate.title}, ${pendingUpdate.titleNorm},
              ${organizationIdInput},
              ${pendingUpdate.description}, ${pendingUpdate.kbArticle}, ${pendingUpdate.isMandatory},
              ${pendingUpdate.sizeBytes}, ${pendingUpdate.requiresReboot}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_pending_update
              (
                agent_id, collected_at, title, title_norm, organization_id, description, kb_article,
                is_mandatory, size_bytes, requires_reboot, updated_at
              )
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );

        const catalogValues = Prisma.join(
          pendingPatchUpdates.map((pendingUpdate) => {
            const updateKey = buildUpdateKeyFromParts(pendingUpdate.title, pendingUpdate.kbArticle);
            const category = classifyPatchCategory({
              title: pendingUpdate.title,
              kbArticle: pendingUpdate.kbArticle,
            });
            return Prisma.sql`
              (
                ${randomUUID()}, ${organizationIdInput}, ${updateKey}, ${pendingUpdate.title}, ${pendingUpdate.titleNorm},
                ${pendingUpdate.kbArticle}, ${category}, ${collectedAtDate}, ${collectedAtDate}, NOW()
              )
            `;
          }),
        );

        const catalogRows = await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO public.rmm_patch_update_catalog
              (
                id, organization_id, update_key, title, title_norm, kb_article,
                category, first_seen_at, last_seen_at, updated_at
              )
            VALUES ${catalogValues}
            ON CONFLICT (organization_id, update_key)
            DO UPDATE SET
              title = EXCLUDED.title,
              title_norm = EXCLUDED.title_norm,
              kb_article = EXCLUDED.kb_article,
              category = EXCLUDED.category,
              last_seen_at = EXCLUDED.last_seen_at,
              updated_at = NOW()
          `,
        );

        const stateValues = Prisma.join(
          pendingPatchUpdates.map((pendingUpdate) => {
            const updateKey = buildUpdateKeyFromParts(pendingUpdate.title, pendingUpdate.kbArticle);
            const category = classifyPatchCategory({
              title: pendingUpdate.title,
              kbArticle: pendingUpdate.kbArticle,
            });
            return Prisma.sql`
              (
                ${randomUUID()}, ${organizationIdInput}, ${agentId}, ${updateKey},
                ${pendingUpdate.title}, ${pendingUpdate.titleNorm}, ${pendingUpdate.kbArticle}, ${category},
                'applicable', 'detected', 'detected', ${collectedAtDate}, ${collectedAtDate},
                ${pendingUpdate.requiresReboot}, ${JSON.stringify({
                  source: "snapshot",
                  description: pendingUpdate.description,
                  sizeBytes: pendingUpdate.sizeBytes === null ? null : Number(pendingUpdate.sizeBytes),
                  isMandatory: pendingUpdate.isMandatory,
                })}::jsonb,
                NOW()
              )
            `;
          }),
        );

        const stateRows = await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO public.rmm_patch_device_update_state
              (
                id, organization_id, agent_id, update_key, title, title_norm, kb_article, category,
                applicability_state, approval_state, lifecycle_state, first_detected_at, last_detected_at,
                requires_reboot, metadata_jsonb, updated_at
              )
            VALUES ${stateValues}
            ON CONFLICT (organization_id, agent_id, update_key)
            DO UPDATE SET
              title = EXCLUDED.title,
              title_norm = EXCLUDED.title_norm,
              kb_article = EXCLUDED.kb_article,
              category = EXCLUDED.category,
              applicability_state = 'applicable',
              approval_state = CASE
                WHEN public.rmm_patch_device_update_state.approval_state IN ('blocked', 'deferred', 'approved', 'emergency_approved')
                  THEN public.rmm_patch_device_update_state.approval_state
                ELSE EXCLUDED.approval_state
              END,
              lifecycle_state = CASE
                WHEN public.rmm_patch_device_update_state.lifecycle_state IN ('installed', 'failed')
                  THEN public.rmm_patch_device_update_state.lifecycle_state
                ELSE EXCLUDED.lifecycle_state
              END,
              last_detected_at = EXCLUDED.last_detected_at,
              requires_reboot = EXCLUDED.requires_reboot,
              metadata_jsonb = EXCLUDED.metadata_jsonb,
              updated_at = NOW()
          `,
        );
        console.info("rmm patch catalog projection applied", {
          agentId,
          parsedPendingUpdates: pendingUpdates.length,
          uniquePendingUpdateKeys: pendingPatchUpdates.length,
          catalogRows,
          stateRows,
        });

        if (postPatchRebootFailedUpdateKeys.length > 0) {
          let postPatchSameVersionFailedUpdateKeys: string[] = [];
          if (osVersion) {
            const installEvidenceRows = await tx.$queryRaw<Array<{
              updateKey: string;
              productVersion: string | null;
              buildVersion: string | null;
            }>>(Prisma.sql`
              SELECT DISTINCT
                jsonb_array_elements_text(update_keys_jsonb) AS "updateKey",
                evidence_jsonb #>> '{actionResult,preflight,swVers,productversion}' AS "productVersion",
                evidence_jsonb #>> '{actionResult,preflight,swVers,buildversion}' AS "buildVersion"
              FROM public.rmm_patch_action
              WHERE organization_id = ${organizationIdInput}
                AND agent_id = ${agentId}
                AND action_type = 'install'
                AND status = 'completed'
                AND updated_at >= ${previousDeviceState?.collectedAt}
                AND updated_at <= ${collectedAtDate}
            `);
            const pendingFailureKeys = new Set(postPatchRebootFailedUpdateKeys);
            postPatchSameVersionFailedUpdateKeys = installEvidenceRows
              .filter((row) => pendingFailureKeys.has(row.updateKey))
              .filter((row) => {
                if (!row.productVersion || row.productVersion !== osVersion) {
                  return false;
                }
                return row.buildVersion ? row.buildVersion === osBuildVersion : true;
              })
              .map((row) => row.updateKey);
          }
          const sameVersionKeySet = new Set(postPatchSameVersionFailedUpdateKeys);
          const genericPostRebootFailedUpdateKeys = postPatchRebootFailedUpdateKeys.filter(
            (updateKey) => !sameVersionKeySet.has(updateKey),
          );

          if (postPatchSameVersionFailedUpdateKeys.length > 0) {
            await tx.$executeRaw(
              Prisma.sql`
                UPDATE public.rmm_patch_device_update_state
                SET lifecycle_state = 'failed',
                    failed_at = COALESCE(failed_at, ${collectedAtDate}),
                    failure_code = 'post_reboot_same_version',
                    failure_message = 'Mac returned from a Talos-managed patch reboot on the same macOS version and the update is still pending.',
                    metadata_jsonb = COALESCE(metadata_jsonb, '{}'::jsonb) || ${JSON.stringify({
                      source: "post_patch_reboot_loop_guard",
                      reason: "same_version",
                      observedAt: collectedAtDate.toISOString(),
                      currentOsVersion: osVersion,
                      currentOsBuildVersion: osBuildVersion,
                      previousBootSessionId: previousDeviceState?.bootSessionId ?? null,
                      currentBootSessionId: bootSessionId,
                    })}::jsonb,
                    updated_at = NOW()
                WHERE organization_id = ${organizationIdInput}
                  AND agent_id = ${agentId}
                  AND update_key IN (${Prisma.join(postPatchSameVersionFailedUpdateKeys)})
              `,
            );
          }

          if (genericPostRebootFailedUpdateKeys.length > 0) {
            await tx.$executeRaw(
              Prisma.sql`
                UPDATE public.rmm_patch_device_update_state
                SET lifecycle_state = 'failed',
                    failed_at = COALESCE(failed_at, ${collectedAtDate}),
                    failure_code = 'post_reboot_still_pending',
                    failure_message = 'Patch remained pending after a Talos-managed patch reboot; suppressing repeat reboot loop.',
                    metadata_jsonb = COALESCE(metadata_jsonb, '{}'::jsonb) || ${JSON.stringify({
                      source: "post_patch_reboot_loop_guard",
                      observedAt: collectedAtDate.toISOString(),
                      previousBootSessionId: previousDeviceState?.bootSessionId ?? null,
                      currentBootSessionId: bootSessionId,
                    })}::jsonb,
                    updated_at = NOW()
                WHERE organization_id = ${organizationIdInput}
                  AND agent_id = ${agentId}
                  AND update_key IN (${Prisma.join(genericPostRebootFailedUpdateKeys)})
              `,
            );
          }
          console.warn("rmm patch reboot loop guard marked updates failed", {
            agentId,
            updateKeys: postPatchRebootFailedUpdateKeys,
            sameVersionUpdateKeys: postPatchSameVersionFailedUpdateKeys,
            previousBootSessionId: previousDeviceState?.bootSessionId ?? null,
            currentBootSessionId: bootSessionId,
          });
        }

        if (pendingRebootPatchUpdateKeys.length > 0) {
          const failedPostRebootRows = await tx.$queryRaw<Array<{ updateKey: string }>>(
            Prisma.sql`
              SELECT update_key AS "updateKey"
              FROM public.rmm_patch_device_update_state
              WHERE organization_id = ${organizationIdInput}
                AND agent_id = ${agentId}
                AND update_key IN (${Prisma.join(pendingRebootPatchUpdateKeys)})
                AND lifecycle_state = 'failed'
                AND failure_code IN ('post_reboot_still_pending', 'post_reboot_same_version')
            `,
          );
          clearRebootRequiredForFailedPendingUpdates = shouldClearRebootForFailedPendingUpdates(
            pendingRebootPatchUpdateKeys,
            failedPostRebootRows.map((row) => row.updateKey),
          );
        }

        await tx.$executeRaw(
          Prisma.sql`
            UPDATE public.rmm_patch_device_update_state
            SET applicability_state = 'not_applicable',
                lifecycle_state = CASE
                  WHEN lifecycle_state IN ('installed', 'superseded') THEN lifecycle_state
                  WHEN lifecycle_state = 'failed' AND COALESCE(metadata_jsonb->>'source', '') <> 'update_history' THEN lifecycle_state
                  ELSE 'superseded'
                END,
                updated_at = NOW()
            WHERE organization_id = ${organizationIdInput}
              AND agent_id = ${agentId}
              AND update_key NOT IN (${Prisma.join(pendingPatchUpdates.map((pendingUpdate) => buildUpdateKeyFromParts(pendingUpdate.title, pendingUpdate.kbArticle)))})
              AND lifecycle_state <> 'installed'
              AND NOT (lifecycle_state = 'failed' AND COALESCE(metadata_jsonb->>'source', '') <> 'update_history')
          `,
        );
      } else if ((pendingUpdatesCount ?? 0) === 0) {
        await tx.$executeRaw(
          Prisma.sql`
            UPDATE public.rmm_patch_device_update_state
            SET applicability_state = 'not_applicable',
                lifecycle_state = CASE
                  WHEN lifecycle_state IN ('installed', 'superseded') THEN lifecycle_state
                  WHEN lifecycle_state = 'failed' AND COALESCE(metadata_jsonb->>'source', '') <> 'update_history' THEN lifecycle_state
                  ELSE 'superseded'
                END,
                updated_at = NOW()
            WHERE organization_id = ${organizationIdInput}
              AND agent_id = ${agentId}
              AND lifecycle_state <> 'installed'
              AND NOT (lifecycle_state = 'failed' AND COALESCE(metadata_jsonb->>'source', '') <> 'update_history')
          `,
        );
      } else {
        console.warn("rmm patch state not superseded because snapshot reported pending updates without parseable rows", {
          agentId,
          reportedPendingUpdatesCount: pendingUpdatesCount,
        });
      }

      if (clearRebootRequiredForFailedPendingUpdates) {
        await tx.$executeRaw(
          Prisma.sql`
            UPDATE rmm_telemetry.device_state
            SET reboot_required = FALSE,
                updated_at = NOW()
            WHERE agent_id = ${agentId}
              AND organization_id = ${organizationIdInput}
          `,
        );
      }

      if (installedUpdates.length > 0) {
        const installedPatchUpdates = uniquePatchUpdatesByUpdateKey(installedUpdates);
        const installedPatchStateUpdates = installedPatchUpdates.filter((installedUpdate) => {
          const resultState = classifyWindowsUpdateHistoryResult(installedUpdate.result);
          if (resultState !== "failed") return true;
          return pendingPatchUpdateKeys.has(
            buildUpdateKeyFromParts(installedUpdate.title, installedUpdate.kbArticle),
          );
        });
        const values = Prisma.join(
          installedUpdates.map(
            (installedUpdate) => Prisma.sql`
            (
              ${agentId}, ${collectedAtDate}, ${installedUpdate.installedAt}, ${installedUpdate.title}, ${installedUpdate.titleNorm},
              ${organizationIdInput},
              ${installedUpdate.kbArticle}, ${installedUpdate.operation}, ${installedUpdate.result}, ${installedUpdate.hresult}, NOW()
            )
          `,
          ),
        );

        await tx.$executeRaw(
          Prisma.sql`
            INSERT INTO rmm_telemetry.device_installed_update
              (
                agent_id, collected_at, installed_at, title, title_norm, organization_id, kb_article,
                operation, result, hresult, updated_at
              )
            VALUES ${values}
            ON CONFLICT DO NOTHING
          `,
        );

        if (installedPatchStateUpdates.length > 0) {
          const installedStateValues = Prisma.join(
            installedPatchStateUpdates.map((installedUpdate) => {
              const updateKey = buildUpdateKeyFromParts(installedUpdate.title, installedUpdate.kbArticle);
              const category = classifyPatchCategory({
                title: installedUpdate.title,
                kbArticle: installedUpdate.kbArticle,
              });
              return Prisma.sql`
              (
                ${randomUUID()}, ${organizationIdInput}, ${updateKey}, ${installedUpdate.title}, ${installedUpdate.titleNorm},
                ${installedUpdate.kbArticle}, ${category}, ${collectedAtDate}, ${collectedAtDate}, NOW()
              )
            `;
            }),
          );

          await tx.$executeRaw(
            Prisma.sql`
            INSERT INTO public.rmm_patch_update_catalog
              (
                id, organization_id, update_key, title, title_norm, kb_article,
                category, first_seen_at, last_seen_at, updated_at
              )
            VALUES ${installedStateValues}
            ON CONFLICT (organization_id, update_key)
            DO UPDATE SET
              title = EXCLUDED.title,
              title_norm = EXCLUDED.title_norm,
              kb_article = EXCLUDED.kb_article,
              category = EXCLUDED.category,
              last_seen_at = EXCLUDED.last_seen_at,
              updated_at = NOW()
          `,
          );

          const deviceInstalledValues = Prisma.join(
            installedPatchStateUpdates.map((installedUpdate) => {
              const updateKey = buildUpdateKeyFromParts(installedUpdate.title, installedUpdate.kbArticle);
              const category = classifyPatchCategory({
                title: installedUpdate.title,
                kbArticle: installedUpdate.kbArticle,
              });
              const resultState = classifyWindowsUpdateHistoryResult(installedUpdate.result);
              const installed = resultState === "installed";
              const failed = resultState === "failed";
              return Prisma.sql`
              (
                ${randomUUID()}, ${organizationIdInput}, ${agentId}, ${updateKey},
                ${installedUpdate.title}, ${installedUpdate.titleNorm}, ${installedUpdate.kbArticle}, ${category},
                ${installed ? "installed" : "detected"}, ${resultState},
                ${collectedAtDate}, ${collectedAtDate}, ${installed ? installedUpdate.installedAt : null},
                ${failed ? collectedAtDate : null}, ${failed ? installedUpdate.hresult : null},
                ${failed ? installedUpdate.result : null}, ${JSON.stringify({
                  source: "update_history",
                  operation: installedUpdate.operation,
                  result: installedUpdate.result,
                })}::jsonb,
                NOW()
              )
            `;
            }),
          );

          await tx.$executeRaw(
            Prisma.sql`
            INSERT INTO public.rmm_patch_device_update_state
              (
                id, organization_id, agent_id, update_key, title, title_norm, kb_article, category,
                approval_state, lifecycle_state, first_detected_at, last_detected_at, installed_at,
                failed_at, failure_hresult, failure_message, metadata_jsonb, updated_at
              )
            VALUES ${deviceInstalledValues}
            ON CONFLICT (organization_id, agent_id, update_key)
            DO UPDATE SET
              title = EXCLUDED.title,
              title_norm = EXCLUDED.title_norm,
              kb_article = EXCLUDED.kb_article,
              category = EXCLUDED.category,
              applicability_state = 'applicable',
              approval_state = EXCLUDED.approval_state,
              lifecycle_state = EXCLUDED.lifecycle_state,
              last_detected_at = EXCLUDED.last_detected_at,
              installed_at = COALESCE(EXCLUDED.installed_at, public.rmm_patch_device_update_state.installed_at),
              failed_at = EXCLUDED.failed_at,
              failure_hresult = EXCLUDED.failure_hresult,
              failure_message = EXCLUDED.failure_message,
              metadata_jsonb = EXCLUDED.metadata_jsonb,
              updated_at = NOW()
          `,
          );
        }
      }
    }

    return { inserted };
  });

  if (snapshotRequestId && snapshotRequestId.length > 0) {
    await prisma.rmmTelemetrySnapshotRequest.upsert({
      where: {
        agentId_requestId: { agentId, requestId: snapshotRequestId },
      },
      create: {
        organizationId: organizationIdInput,
        agentId,
        requestId: snapshotRequestId,
        status: "completed",
      },
      update: { status: "completed", updatedAt: new Date() },
    });
  }

  return res.status(202).json({
    accepted: true,
    duplicate: result.inserted === 0,
  });
});

// GET /rmm/devices/:agentId/scope - organization/customer scope (internal)
rmmRouter.get(
  "/devices/:agentId/scope",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const device = await prisma.rmmDevice.findUnique({
      where: { agentId: req.params.agentId },
      include: { customer: true, site: true },
    });

    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    res.json({
      organizationId: device.organizationId,
      customerId: device.customerId ?? null,
      siteId: device.siteId ?? null,
    });
  },
);

// POST /rmm/devices/:agentId/snapshot-requests - register pending snapshot request (internal, called by RMM server)
rmmRouter.post(
  "/devices/:agentId/snapshot-requests",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const agentId = req.params.agentId ? String(req.params.agentId).trim() : "";
    const requestId = req.body?.requestId
      ? String(req.body.requestId).trim()
      : "";
    const statusRaw = req.body?.status
      ? String(req.body.status).trim().toLowerCase()
      : "pending";
    const status =
      statusRaw === "completed" || statusRaw === "failed"
        ? statusRaw
        : "pending";
    if (!agentId || !requestId) {
      return res
        .status(400)
        .json({ error: "agentId and requestId are required" });
    }
    const device = await prisma.rmmDevice.findUnique({
      where: { agentId },
      select: { organizationId: true },
    });
    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }
    await prisma.rmmTelemetrySnapshotRequest.upsert({
      where: {
        agentId_requestId: { agentId, requestId },
      },
      create: {
        organizationId: device.organizationId,
        agentId,
        requestId,
        status,
      },
      update: { status, updatedAt: new Date() },
    });
    return res.status(201).json({ requestId, status });
  },
);

// GET /rmm/devices/:agentId/snapshot-requests/:requestId - poll snapshot request status (auth)
rmmRouter.get(
  "/devices/:agentId/snapshot-requests/:requestId",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership)
      return res
        .status(404)
        .json({ error: "No organization", needsOnboarding: true });

    const agentId = req.params.agentId ? String(req.params.agentId) : "";
    const requestId = req.params.requestId ? String(req.params.requestId) : "";
    if (!agentId || !requestId)
      return res
        .status(400)
        .json({ error: "agentId and requestId are required" });

    const device = await prisma.rmmDevice.findFirst({
      where: {
        agentId,
        organizationId: membership.organizationId,
      },
    });
    if (!device) return res.status(404).json({ error: "Device not found" });

    const row = await prisma.rmmTelemetrySnapshotRequest.findFirst({
      where: {
        agentId,
        requestId,
        organizationId: membership.organizationId,
      },
    });
    if (!row)
      return res.status(404).json({ error: "Snapshot request not found" });

    return res.json({ requestId, status: row.status });
  },
);

// POST /rmm/devices/:agentId/ensure-org - ensure device assigned to org (internal)
rmmRouter.post(
  "/devices/:agentId/ensure-org",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const organizationId = req.body?.organizationId
      ? String(req.body.organizationId)
      : "";
    if (!organizationId) {
      return res.status(400).json({ error: "organizationId is required" });
    }

    const device = await prisma.rmmDevice.findUnique({
      where: { agentId: req.params.agentId },
    });

    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }

    if (device.organizationId !== organizationId) {
      return res
        .status(409)
        .json({
          error: "device is already assigned to a different organization",
        });
    }

    if (device.customerId && device.customerId.trim()) {
      return res.json({ updated: false, customerId: device.customerId });
    }

    const unassigned = await getOrCreateUnassigned(organizationId);
    const updated = await prisma.rmmDevice.update({
      where: { agentId: device.agentId },
      data: { organizationId, customerId: unassigned.id },
    });

    res.json({ updated: true, customerId: updated.customerId });
  },
);

// POST /rmm/command-log - create command execution log (internal)
rmmRouter.post(
  "/command-log",
  requireRmmServer,
  async (req: RmmServerRequest, res) => {
    const organizationId = req.body?.organizationId
      ? String(req.body.organizationId)
      : "";
    const userId = req.body?.userId ? String(req.body.userId) : "";
    const userEmail = req.body?.userEmail ? String(req.body.userEmail) : null;
    const agentId = req.body?.agentId ? String(req.body.agentId) : "";
    const command = req.body?.command ? String(req.body.command) : "";
    const wasAllowed = Boolean(req.body?.wasAllowed);

    if (!organizationId || !userId || !agentId || !command) {
      return res
        .status(400)
        .json({
          error: "organizationId, userId, agentId, and command are required",
        });
    }

    const device = await prisma.rmmDevice.findUnique({
      where: { agentId },
      select: { organizationId: true, customerId: true, siteId: true },
    });
    if (!device) {
      return res.status(404).json({ error: "Device not found" });
    }
    if (device.organizationId !== organizationId) {
      return res.status(409).json({ error: "Device organization mismatch" });
    }

    const customerIdInput = req.body?.customerId ? String(req.body.customerId) : null;
    if (customerIdInput && customerIdInput !== (device.customerId ?? "")) {
      return res.status(409).json({ error: "Device customer mismatch" });
    }

    const siteIdInput = req.body?.siteId ? String(req.body.siteId) : null;
    if (siteIdInput && siteIdInput !== (device.siteId ?? "")) {
      return res.status(409).json({ error: "Device site mismatch" });
    }

    const customerId = device.customerId ?? null;
    const siteId = device.siteId ?? null;
    const denialReason = req.body?.denialReason
      ? String(req.body.denialReason)
      : null;
    const executionTimeMs = readOptionalNumber(req.body?.executionTimeMs);
    const outputLength = readOptionalNumber(req.body?.outputLength);
    const exitCode = readOptionalNumber(req.body?.exitCode);
    const requestId = req.body?.requestId ? String(req.body.requestId) : null;

    const log = await prisma.$transaction(async (tx) => {
      const row = await tx.commandExecutionLog.create({
        data: {
          organizationId,
          customerId,
          userId,
          agentId,
          command,
          wasAllowed,
          denialReason,
          matchedPolicyId: req.body?.matchedPolicyId
            ? BigInt(req.body.matchedPolicyId)
            : null,
          executionTimeMs,
          exitCode,
          outputLength,
        },
      });

      await writeAuditEvent(
        auditRequest(req, {
          organizationId,
          customerId,
          siteId,
          agentId,
          actorType: "user",
          userId,
          userEmail,
          actionType: "command.execute",
          targetType: "rmm_device",
          targetId: agentId,
          targetName: command.split(/\s+/)[0] || command,
          result: wasAllowed ? (exitCode !== null && exitCode !== 0 ? "failure" : "success") : "blocked",
          statusCode: wasAllowed ? null : 403,
          errorMessage: denialReason,
          ...(requestId ? { correlationId: requestId } : {}),
          metadata: {
            command,
            wasAllowed,
            matchedPolicyId: req.body?.matchedPolicyId ? String(req.body.matchedPolicyId) : null,
            executionTimeMs,
            outputLength,
            exitCode,
          },
        }),
        tx,
      );

      return row;
    });

    res.json({ id: log.id.toString() });
  },
);
