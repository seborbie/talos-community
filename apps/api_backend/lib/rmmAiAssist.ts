import OpenAI from "openai";
import { createHash, randomUUID } from "crypto";
import { createLogger } from "./logger";
import { createGeneratedSecret, type GeneratedSecretSummary, type SecretSurface } from "./secureNotes";
import type { AiRunnerDeviceContext } from "./commandCenterAiRunner";

const log = createLogger("api_backend::rmm_ai_assist");
const DEFAULT_OPENAI_MODEL = "gpt-5.5";
const DEFAULT_OPENAI_REASONING_EFFORT = "none" as const;
const DEFAULT_SESSION_VERIFY_TIMEOUT_MS = 8_000;
const OPENAI_RESPONSE_TIMEOUT_MS = 90_000;
const DEFAULT_WAIT_ACTION_MS = 1_000;
const MAX_WAIT_ACTION_MS = 30_000;
const SHELL_WAIT_DEFAULT_MS = 10_000;
const SHELL_WAIT_MIN_MS = 1_000;
const SHELL_WAIT_MAX_MS = 60_000;
const DEFAULT_TASK_MAX_STEPS = 12;
const DEFAULT_TASK_MAX_ACTIONS_PER_STEP = 8;
const TASK_TTL_MS = 30 * 60 * 1_000;
const TALOS_DESKTOP_STEP_TOOL_NAME = "talos_desktop_step";
const TALOS_SHELL_COMMAND_TOOL_NAME = "talos_shell_command_proposal";
const TALOS_GENERATED_SECRET_TOOL_NAME = "talos_create_generated_secret";
const MAX_SHELL_TRANSCRIPT_CHARS = 12_000;

export type AiDesktopAction =
  | {
      type: "move";
      x: number;
      y: number;
      keys: string[];
    }
  | {
      type: "click" | "double_click";
      x: number;
      y: number;
      button: "left" | "right" | "middle";
      keys: string[];
    }
  | {
      type: "scroll";
      x: number;
      y: number;
      scrollX: number;
      scrollY: number;
      keys: string[];
    }
  | {
      type: "drag";
      path: AiDesktopPoint[];
      button: "left" | "right" | "middle";
      keys: string[];
    }
  | {
      type: "type";
      text: string;
    }
  | {
      type: "inject_secret";
      secretHandle: string;
    }
  | {
      type: "keypress";
      keys: string[];
    }
  | {
      type: "wait";
      ms: number;
    };

export type AiDesktopPoint = {
  x: number;
  y: number;
};

export type AiDesktopActionResponse = {
  assistantMessage: string;
  actions: AiDesktopAction[];
  responseId: string | null;
  generatedSecrets?: GeneratedSecretSummary[];
};

export type AiDesktopActionRequest = {
  prompt: string;
  screenshotBase64: string;
  width: number;
  height: number;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  platform?: string | null;
  deviceContext?: AiRunnerDeviceContext | null;
  jobId?: string | null;
  organizationId?: string | null;
  userId?: string | null;
  conversationId?: string | null;
  agentId?: string | null;
  generatedSecrets?: GeneratedSecretSummary[] | null;
};

export type AiDesktopTaskStatus =
  | "running"
  | "complete"
  | "failed"
  | "needs_approval";

export type AiDesktopTaskStartRequest = {
  goal: string;
  screenshotBase64: string;
  width: number;
  height: number;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  platform?: string | null;
  deviceContext?: AiRunnerDeviceContext | null;
  jobId?: string | null;
  organizationId?: string | null;
  userId?: string | null;
  conversationId?: string | null;
  agentId?: string | null;
  generatedSecrets?: GeneratedSecretSummary[] | null;
};

export type AiDesktopTaskContinueRequest = {
  taskId: string;
  screenshotBase64: string;
  width: number;
  height: number;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  platform?: string | null;
  deviceContext?: AiRunnerDeviceContext | null;
  lastStepResult?: string | null;
  jobId?: string | null;
  organizationId?: string | null;
  userId?: string | null;
  conversationId?: string | null;
  agentId?: string | null;
  generatedSecrets?: GeneratedSecretSummary[] | null;
};

export type AiDesktopTaskStepResponse = {
  taskId: string;
  status: AiDesktopTaskStatus;
  plan: string[];
  assistantMessage: string;
  actions: AiDesktopAction[];
  responseId: string | null;
  stepIndex: number;
  maxSteps: number;
  generatedSecrets?: GeneratedSecretSummary[];
};

export type AiShellAssistRequest = {
  prompt: string;
  transcript?: string | null;
  history?: AiShellAssistHistoryEntry[] | null;
  activeCommand?: AiShellAssistActiveCommand | null;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  platform?: string | null;
  deviceContext?: AiRunnerDeviceContext | null;
  jobId?: string | null;
  organizationId?: string | null;
  userId?: string | null;
  conversationId?: string | null;
  agentId?: string | null;
  generatedSecrets?: GeneratedSecretSummary[] | null;
};

export type AiShellAssistAction = "command" | "wait" | "interrupt" | "done" | "needs_input" | "open_desktop";

export type AiShellAssistActiveCommand = {
  command: string;
  approvalId: string;
  turnIndex: number;
  elapsedMs: number;
  checkpointCount: number;
  recentOutput: string;
  remainingMs: number;
};

export type AiShellAssistHistoryEntry = {
  command: string;
  approved?: boolean | null;
  output?: string | null;
  responseId?: string | null;
};

export type AiShellAssistResponse = {
  action: AiShellAssistAction;
  command: string;
  explanation: string;
  risk: string;
  notes: string[];
  message: string;
  waitMs: number;
  responseId: string | null;
  generatedSecrets?: GeneratedSecretSummary[];
};

export type AiShellApprovalRequest = {
  command: string;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  platform?: string | null;
  responseId?: string | null;
};

type TalosDesktopStepCall = {
  callId: string;
  args: TalosDesktopStepArguments;
};

type TalosDesktopStepArguments = {
  status?: AiDesktopTaskStatus;
  plan?: string[];
  message?: string;
  actions?: any[];
};

type AiDesktopTaskState = {
  taskId: string;
  goal: string;
  sessionId: string;
  sessionToken: string;
  rmmApiBase: string | null;
  rmmServerBase: string;
  platform: AiAssistPlatform;
  deviceContext: AiRunnerDeviceContext | null;
  jobId: string | null;
  organizationId: string | null;
  userId: string | null;
  conversationId: string | null;
  agentId: string | null;
  generatedSecrets: GeneratedSecretSummary[];
  model: string;
  responseId: string | null;
  pendingToolCallId: string | null;
  plan: string[];
  status: AiDesktopTaskStatus;
  stepIndex: number;
  maxSteps: number;
  createdAt: number;
  updatedAt: number;
};

type ParsedTaskText = {
  status?: AiDesktopTaskStatus;
  plan?: string[];
  message?: string;
};

type AiAssistPlatform = "windows" | "linux" | "macos" | "unknown";

let cachedClient: OpenAI | null = null;
const aiDesktopTasks = new Map<string, AiDesktopTaskState>();

function parseBool(value: string | undefined, defaultValue: boolean): boolean {
  if (value === undefined) {
    return defaultValue;
  }
  const normalized = value.trim().toLowerCase();
  if (!normalized) {
    return defaultValue;
  }
  return ["1", "true", "yes", "y", "on"].includes(normalized);
}

function aiAssistEnabled(): boolean {
  return parseBool(process.env.RMM_AI_ASSIST_ENABLED, false);
}

function parsePositiveInt(
  value: string | undefined,
  defaultValue: number,
): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 1) {
    return defaultValue;
  }
  return Math.floor(parsed);
}

function getOpenAIReasoningEffort() {
  const raw = (
    process.env.OPENAI_REASONING_EFFORT || DEFAULT_OPENAI_REASONING_EFFORT
  )
    .trim()
    .toLowerCase();
  switch (raw) {
    case "minimal":
      return "minimal" as const;
    case "low":
      return "low" as const;
    case "high":
      return "high" as const;
    case "xhigh":
      return "xhigh" as const;
    case "none":
      return "none" as const;
    case "medium":
      return "medium" as const;
    default:
      return DEFAULT_OPENAI_REASONING_EFFORT;
  }
}

function withOpenAIReasoning<T extends object>(
  params: T,
): T & {
  reasoning: {
    effort: "none" | "minimal" | "low" | "medium" | "high" | "xhigh";
  };
} {
  return {
    ...params,
    reasoning: {
      effort: getOpenAIReasoningEffort(),
    },
  };
}

function taskMaxSteps(): number {
  return parsePositiveInt(
    process.env.RMM_AI_ASSIST_MAX_STEPS,
    DEFAULT_TASK_MAX_STEPS,
  );
}

function taskMaxActionsPerStep(): number {
  return parsePositiveInt(
    process.env.RMM_AI_ASSIST_MAX_ACTIONS_PER_STEP,
    DEFAULT_TASK_MAX_ACTIONS_PER_STEP,
  );
}

function cleanupExpiredTasks(now = Date.now()): void {
  for (const [taskId, task] of aiDesktopTasks) {
    if (now - task.updatedAt > TASK_TTL_MS) {
      aiDesktopTasks.delete(taskId);
    }
  }
}

function normalizeBaseUrl(raw: string | null | undefined): string | null {
  if (!raw || !raw.trim()) {
    return null;
  }
  try {
    const url = new URL(raw.trim());
    if (url.protocol === "ws:") {
      url.protocol = "http:";
    } else if (url.protocol === "wss:") {
      url.protocol = "https:";
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    url.pathname = "";
    url.hash = "";
    url.search = "";
    return url.toString().replace(/\/+$/, "");
  } catch {
    return null;
  }
}

function normalizeAssistPlatform(value: string | null | undefined): AiAssistPlatform {
  const normalized = (value || "").trim().toLowerCase().replace(/[_-]+/g, " ");
  if (!normalized) {
    return "unknown";
  }
  if (normalized.includes("windows")) {
    return "windows";
  }
  if (normalized.includes("linux")) {
    return "linux";
  }
  if (
    normalized.includes("macos") ||
    normalized.includes("mac os") ||
    normalized.includes("darwin") ||
    normalized.includes("os x") ||
    normalized === "mac"
  ) {
    return "macos";
  }
  return "unknown";
}

function resolveConfiguredRmmServerBase(): string | null {
  return normalizeBaseUrl(
    process.env.PUBLIC_RMM_API_URL ||
      process.env.RMM_INSTALLER_SERVER_URL ||
      process.env.RMM_SERVER_URL ||
      null,
  );
}

async function fetchWithTimeout(
  url: string,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

async function verifyRemoteDesktopSession(
  sessionId: string,
  sessionToken: string,
  requestBase: string | null,
): Promise<string> {
  const configuredBase = resolveConfiguredRmmServerBase();
  const requestedBase = normalizeBaseUrl(requestBase);
  const resolvedBase = requestedBase ?? configuredBase;
  if (!resolvedBase) {
    throw new Error("RMM server base URL is not configured");
  }
  if (configuredBase && requestedBase && configuredBase !== requestedBase) {
    log.warn(
      "desktop action request used viewer-supplied rmmApiBase that differs from configured base",
      {
        configured_rmm_api_base: configuredBase,
        requested_rmm_api_base: requestedBase,
      },
    );
  }

  const verifyUrl = `${resolvedBase}/api/rmm/session/${encodeURIComponent(sessionId)}/capabilities?token=${encodeURIComponent(sessionToken)}`;
  const response = await fetchWithTimeout(
    verifyUrl,
    DEFAULT_SESSION_VERIFY_TIMEOUT_MS,
  );
  if (!response.ok) {
    throw new Error(
      `Remote desktop session validation failed (${response.status})`,
    );
  }
  return resolvedBase;
}

async function verifyShellSession(
  sessionId: string,
  sessionToken: string,
  requestBase: string | null,
): Promise<string> {
  const configuredBase = resolveConfiguredRmmServerBase();
  const requestedBase = normalizeBaseUrl(requestBase);
  const resolvedBase = requestedBase ?? configuredBase;
  if (!resolvedBase) {
    throw new Error("RMM server base URL is not configured");
  }
  if (configuredBase && requestedBase && configuredBase !== requestedBase) {
    log.warn(
      "shell assist request used viewer-supplied rmmApiBase that differs from configured base",
      {
        configured_rmm_api_base: configuredBase,
        requested_rmm_api_base: requestedBase,
      },
    );
  }

  const verifyUrl = `${resolvedBase}/api/rmm/shell/session/${encodeURIComponent(sessionId)}/capabilities?token=${encodeURIComponent(sessionToken)}`;
  const response = await fetchWithTimeout(
    verifyUrl,
    DEFAULT_SESSION_VERIFY_TIMEOUT_MS,
  );
  if (!response.ok) {
    throw new Error(`Shell session validation failed (${response.status})`);
  }
  return resolvedBase;
}

function getOpenAiClient(): OpenAI {
  const apiKey = (process.env.OPENAI_API_KEY || "").trim();
  if (!apiKey) {
    throw new Error("OPENAI_API_KEY is not configured");
  }
  if (!cachedClient) {
    cachedClient = new OpenAI({
      apiKey,
      timeout: OPENAI_RESPONSE_TIMEOUT_MS,
    });
  }
  return cachedClient;
}

const TALOS_DESKTOP_STEP_TOOL: any = {
  type: "function",
  name: TALOS_DESKTOP_STEP_TOOL_NAME,
  description:
    "Return the next Talos remote desktop step as a status, plan, message, and ordered batch of desktop actions.",
  strict: true,
  parameters: {
    type: "object",
    properties: {
      status: {
        type: "string",
        enum: ["running", "complete", "failed", "needs_approval"],
      },
      plan: {
        type: "array",
        items: { type: "string" },
      },
      message: {
        type: "string",
      },
      actions: {
        type: "array",
        items: {
          type: "object",
          properties: {
            type: {
              type: "string",
              enum: [
                "move",
                "click",
                "double_click",
                "drag",
                "scroll",
                "type",
                "inject_secret",
                "keypress",
                "screenshot",
                "wait",
              ],
            },
            x: { type: ["number", "null"] },
            y: { type: ["number", "null"] },
            button: {
              type: "string",
              enum: [
                "left",
                "right",
                "middle",
                "wheel",
                "back",
                "forward",
                "none",
              ],
            },
            keys: {
              type: "array",
              items: { type: "string" },
            },
            scrollX: { type: ["number", "null"] },
            scrollY: { type: ["number", "null"] },
            path: {
              type: "array",
              items: {
                type: "object",
                properties: {
                  x: { type: "number" },
                  y: { type: "number" },
                },
                required: ["x", "y"],
                additionalProperties: false,
              },
            },
            text: { type: "string" },
            secretHandle: { type: "string" },
            ms: { type: ["number", "null"] },
          },
          required: [
            "type",
            "x",
            "y",
            "button",
            "keys",
            "scrollX",
            "scrollY",
            "path",
            "text",
            "secretHandle",
            "ms",
          ],
          additionalProperties: false,
        },
      },
    },
    required: ["status", "plan", "message", "actions"],
    additionalProperties: false,
  },
};

const TALOS_SHELL_COMMAND_TOOL: any = {
  type: "function",
  name: TALOS_SHELL_COMMAND_TOOL_NAME,
  description:
    "Return the next approval-gated shell agent turn for a Talos system shell session.",
  strict: true,
  parameters: {
    type: "object",
    properties: {
      action: {
        type: "string",
        enum: ["command", "wait", "interrupt", "done", "needs_input", "open_desktop"],
        description:
          "Use command when another shell command should be proposed, wait when the active command should keep running, interrupt when the active command should be stopped with Ctrl+C, done when the goal is complete, needs_input when the operator must clarify something, or open_desktop when the goal cannot continue through shell alone.",
      },
      command: {
        type: "string",
        description:
          "The exact command to send to the terminal if the operator approves it. Use an empty string unless action is command.",
      },
      explanation: {
        type: "string",
        description:
          "Briefly explain the next turn: what the command does, why no command is needed, or what clarification is needed.",
      },
      risk: {
        type: "string",
        description:
          "Brief risk note for command actions, or 'No command risk' when no command is proposed.",
      },
      notes: {
        type: "array",
        items: { type: "string" },
        description: "Optional concise caveats or follow-up checks.",
      },
      message: {
        type: "string",
        description:
          "Short operator-facing status or completion message for this turn.",
      },
      waitMs: {
        type: "number",
        description:
          `For wait actions, the requested additional wait in milliseconds, clamped to ${SHELL_WAIT_MIN_MS}-${SHELL_WAIT_MAX_MS}. Use 0 for every non-wait action.`,
      },
    },
    required: ["action", "command", "explanation", "risk", "notes", "message", "waitMs"],
    additionalProperties: false,
  },
};

const TALOS_GENERATED_SECRET_TOOL: any = {
  type: "function",
  name: TALOS_GENERATED_SECRET_TOOL_NAME,
  description:
    "Generate a password secret without revealing plaintext. Use this when a shell or desktop task needs a new password; then reference the returned shellReference or secretHandle in the next Talos step.",
  strict: true,
  parameters: {
    type: "object",
    properties: {
      kind: {
        type: "string",
        enum: ["password"],
        description: "Only password is supported.",
      },
      purpose: {
        type: "string",
        description: "Short reason for the generated password.",
      },
      surface: {
        type: "string",
        enum: ["shell", "desktop", "note_only"],
        description: "Where the generated password will be used.",
      },
      minWordLength: { type: ["number", "null"] },
      maxWordLength: { type: ["number", "null"] },
      maxPasswordLength: { type: ["number", "null"] },
    },
    required: [
      "kind",
      "purpose",
      "surface",
      "minWordLength",
      "maxWordLength",
      "maxPasswordLength",
    ],
    additionalProperties: false,
  },
};

function talosDesktopTools(): any[] {
  return [TALOS_DESKTOP_STEP_TOOL, TALOS_GENERATED_SECRET_TOOL];
}

function talosDesktopToolChoice(): any {
  return "auto";
}

function talosShellTools(): any[] {
  return [TALOS_SHELL_COMMAND_TOOL, TALOS_GENERATED_SECRET_TOOL];
}

function talosShellToolChoice(): any {
  return "auto";
}

function buildScreenshotMessage(text: string, screenshotBase64: string): any {
  return {
    role: "user",
    content: [
      { type: "input_text", text },
      {
        type: "input_image",
        image_url: `data:image/png;base64,${screenshotBase64}`,
        detail: "original",
      },
    ],
  };
}

function desktopPlatformLabel(platform: AiAssistPlatform): string {
  switch (platform) {
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    case "macos":
      return "macOS";
    default:
      return "the current";
  }
}

function desktopPlatformGuidance(platform: AiAssistPlatform): string[] {
  if (platform === "macos") {
    return [
      "The target desktop is macOS. Do not assume Windows-specific UI, shortcuts, menus, taskbar, Start menu, or PowerShell.",
      "For macOS shortcuts, use COMMAND/CMD for the Command key and OPTION/ALT for the Option key in keypress keys.",
      "Prefer macOS UI conventions such as the menu bar, Dock, Finder, Safari, and Spotlight when they are visible or relevant.",
    ];
  }
  if (platform === "linux") {
    return [
      "The target desktop is Linux. Do not assume Windows-specific UI, shortcuts, menus, taskbar, Start menu, or PowerShell.",
      "Prefer Linux desktop conventions and visible UI state over assumptions about a specific distribution.",
    ];
  }
  if (platform === "windows") {
    return [
      "The target desktop is Windows. Use Windows UI conventions and keyboard names when shortcuts are needed.",
    ];
  }
  return [
    "The target desktop platform is unknown. Base actions only on visible UI state and avoid platform-specific assumptions unless the screenshot confirms them.",
  ];
}

function platformFromDeviceContext(deviceContext?: AiRunnerDeviceContext | null): AiAssistPlatform | null {
  const family = deviceContext?.platform?.family;
  if (family === "windows" || family === "linux" || family === "macos" || family === "unknown") {
    return family;
  }
  return null;
}

function effectiveAssistPlatform(
  platform: string | null | undefined,
  deviceContext?: AiRunnerDeviceContext | null,
): AiAssistPlatform {
  const fromContext = platformFromDeviceContext(deviceContext);
  return fromContext && fromContext !== "unknown" ? fromContext : normalizeAssistPlatform(platform);
}

function formatBytes(value: number | null | undefined): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  const gib = value / (1024 ** 3);
  if (gib >= 1) {
    return `${Math.round(gib * 10) / 10} GiB`;
  }
  const mib = value / (1024 ** 2);
  return `${Math.round(mib)} MiB`;
}

function formatAgeSeconds(value: number | null | undefined): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    return null;
  }
  if (value < 90) return `${Math.round(value)} seconds old`;
  const minutes = Math.round(value / 60);
  if (minutes < 90) return `${minutes} minutes old`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} hours old`;
  const days = Math.round(hours / 24);
  return `${days} days old`;
}

function boolLabel(value: boolean | null | undefined): string | null {
  if (value === true) return "yes";
  if (value === false) return "no";
  return null;
}

function compactLine(label: string, values: Array<string | null | undefined>): string | null {
  const text = values.filter((value): value is string => Boolean(value && value.trim())).join(", ");
  return text ? `- ${label}: ${text}` : null;
}

function formatShellContextLine(shell: any): string | null {
  const description = typeof shell?.description === "string" ? shell.description.trim() : "";
  if (description) return `- Shell: ${description}`;
  return compactLine("Shell", [
    typeof shell?.account === "string" && shell.account.trim() ? `account ${shell.account.trim()}` : null,
    typeof shell?.runAs === "string" && shell.runAs.trim()
      ? `run as ${shell.runAs.trim().replace(/_/g, " ")}`
      : null,
    typeof shell?.elevated === "boolean" ? `elevated: ${boolLabel(shell.elevated)}` : null,
  ]);
}

export function formatDeviceContextForPrompt(deviceContext?: AiRunnerDeviceContext | null): string[] {
  if (!deviceContext) return [];
  const raw = deviceContext as any;
  const snapshot = raw.snapshot ?? {};
  const platform = raw.platform ?? {};
  const agent = raw.agent ?? {};
  const hardware = raw.hardware ?? {};
  const state = raw.state ?? {};
  const network = raw.network ?? {};
  const shell = raw.shell ?? {};
  const securityContext = raw.security ?? {};
  const lines: Array<string | null> = ["Target device context:"];
  const snapshotAge = formatAgeSeconds(snapshot.ageSeconds);
  const memory = formatBytes(hardware.memoryTotalBytes);
  const cores =
    (hardware.physicalCores !== null && hardware.physicalCores !== undefined) ||
    (hardware.logicalCores !== null && hardware.logicalCores !== undefined)
      ? `${hardware.physicalCores ?? "unknown"} physical / ${hardware.logicalCores ?? "unknown"} logical cores`
      : null;
  const rebootRequired = boolLabel(state.rebootRequired);
  const security = [
    ["firewall", securityContext.firewallEnabled],
    ["secure boot", securityContext.secureBoot],
    ["TPM present", securityContext.tpmPresent],
    ["TPM enabled", securityContext.tpmEnabled],
    ["antivirus", securityContext.antivirusEnabled],
    ["BitLocker", securityContext.bitlockerEnabled],
  ]
    .map(([label, value]) => {
      const bool = boolLabel(value as boolean | null);
      return bool ? `${label}: ${bool}` : null;
    })
    .filter((value): value is string => Boolean(value));

  lines.push(
    compactLine("Device", [
      raw.hostname || raw.agentId,
      raw.customerName,
      raw.siteName,
    ]),
    compactLine("Snapshot", [
      snapshot.collectedAt,
      snapshotAge,
    ]),
    compactLine("OS", [
      platform.osName,
      platform.osVersion,
      platform.architecture,
      platform.locale,
      platform.timezone,
    ]),
    formatShellContextLine(shell),
    compactLine("Agent", [agent.version, agent.lastSeen ? `last seen ${agent.lastSeen}` : null]),
    compactLine("Hardware", [hardware.cpuModel, cores, memory]),
    compactLine("State", [
      state.pendingUpdatesCount !== null && state.pendingUpdatesCount !== undefined
        ? `${state.pendingUpdatesCount} pending update${state.pendingUpdatesCount === 1 ? "" : "s"}`
        : null,
      rebootRequired ? `reboot required: ${rebootRequired}` : null,
    ]),
    compactLine("Network", [network.primaryIp, platform.domain]),
    security.length > 0 ? `- Security: ${security.join(", ")}` : null,
  );

  return lines.filter((line): line is string => Boolean(line));
}

function generatedSecretSummariesForPrompt(
  secrets: GeneratedSecretSummary[] | null | undefined,
  surface: "shell" | "desktop",
): string[] {
  const usable = (secrets ?? []).filter((secret) =>
    surface === "shell" ? Boolean(secret.shellReference) : Boolean(secret.secretHandle),
  );
  if (usable.length === 0) return [];
  const lines = [
    surface === "shell"
      ? "Generated secrets already available for this shell job:"
      : "Generated secrets already available for this desktop job:",
  ];
  for (const secret of usable) {
    const parts = [
      `handle ${secret.secretHandle}`,
      secret.shellReference ? `shellReference ${secret.shellReference}` : null,
      secret.desktopReference ? `desktopReference ${secret.desktopReference}` : null,
      secret.purpose ? `purpose ${secret.purpose}` : null,
      `secureNote ${secret.secureNoteUrl}`,
      `expires ${secret.expiresAt}`,
    ].filter((value): value is string => Boolean(value));
    lines.push(`- ${parts.join(", ")}`);
  }
  return lines;
}

function mergeGeneratedSecretSummaries(
  existing: GeneratedSecretSummary[],
  incoming: GeneratedSecretSummary[] | null | undefined,
): GeneratedSecretSummary[] {
  const byHandle = new Map(existing.map((secret) => [secret.secretHandle, secret]));
  for (const secret of incoming ?? []) {
    if (secret?.secretHandle && !byHandle.has(secret.secretHandle)) {
      byHandle.set(secret.secretHandle, secret);
    }
  }
  return [...byHandle.values()];
}

export function buildPrompt(prompt: string, platform: string | null = null): string {
  const normalizedPlatform = normalizeAssistPlatform(platform);
  const platformLabel = desktopPlatformLabel(normalizedPlatform);
  return [
    `You are controlling ${platformLabel} remote desktop for a single-turn proof of concept.`,
    "Use the Talos desktop harness function for UI interaction.",
    "The current screenshot is attached. Return only the immediate next batch of UI actions for the visible screen.",
    "Do not plan beyond the current screen.",
    "Supported actions are move, click, double_click, drag, scroll, type, inject_secret, keypress, screenshot, and wait.",
    `If you need a generated password, call ${TALOS_GENERATED_SECRET_TOOL_NAME} with surface=desktop first. Then focus the password field and use an inject_secret action with the returned secretHandle; never type password text yourself.`,
    "For unused action fields, use null for numbers, none for button, empty arrays for keys/path, empty text, and an empty secretHandle.",
    `For wait actions, set ms to the actual duration needed in milliseconds, capped at ${MAX_WAIT_ACTION_MS}.`,
    "Use screenshot only when another observation is required before any safe executable action.",
    "When a screenshot is returned in a function output, treat that image as the latest desktop state for your next action.",
    `Target platform: ${normalizedPlatform}.`,
    ...desktopPlatformGuidance(normalizedPlatform),
    `User request: ${prompt}`,
  ].join("\n");
}

export function buildTaskPrompt(
  goal: string,
  platform: string | null = null,
  deviceContext: AiRunnerDeviceContext | null = null,
  generatedSecrets: GeneratedSecretSummary[] | null = null,
): string {
  const normalizedPlatform = effectiveAssistPlatform(platform, deviceContext);
  const platformLabel = desktopPlatformLabel(normalizedPlatform);
  return [
    `You are Talos AI Assist controlling ${platformLabel} remote desktop for a GUI-only proof of concept.`,
    "Your job is to complete the user goal by repeatedly observing screenshots and using the Talos desktop harness function.",
    "Create and maintain a concise plan. Prefer small batches of GUI actions, then wait for a fresh screenshot before continuing.",
    "Use GUI desktop actions only. Do not assume access to shell, files, registry, APIs, or remote execution tools.",
    "Supported actions are move, click, double_click, drag, scroll, type, inject_secret, keypress, screenshot, and wait.",
    `If the task needs a generated password, call ${TALOS_GENERATED_SECRET_TOOL_NAME} with surface=desktop before the Talos desktop step. Then click/focus the target password field and use inject_secret with the returned secretHandle; never place password text in a type action.`,
    "For unused action fields, use null for numbers, none for button, empty arrays for keys/path, empty text, and an empty secretHandle.",
    `For wait actions, set ms to the actual duration needed in milliseconds, capped at ${MAX_WAIT_ACTION_MS}.`,
    "Do not add wait actions after routine input unless elapsed time is necessary for the UI to change.",
    "Use screenshot only when another observation is required before any safe executable action.",
    "When a screenshot is returned in a function output, treat that image as the latest desktop state after your previous actions.",
    "After each batch of actions, stop and wait for the next screenshot observation.",
    "Return complete only after the latest screenshot visibly confirms that the goal has been achieved.",
    "If you cannot continue safely or need a human decision, stop and explain why.",
    `Always call ${TALOS_DESKTOP_STEP_TOOL_NAME}; do not return free-form text for task state.`,
    `Target platform: ${normalizedPlatform}.`,
    ...formatDeviceContextForPrompt(deviceContext),
    ...generatedSecretSummariesForPrompt(generatedSecrets, "desktop"),
    ...desktopPlatformGuidance(normalizedPlatform),
    `User goal: ${goal}`,
  ].join("\n");
}

function normalizeKeys(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((item) => (typeof item === "string" ? item.trim().toUpperCase() : ""))
    .filter((item) => item.length > 0);
}

function clampCoordinate(value: unknown, maxExclusive: number): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new Error("desktop action is missing coordinates");
  }
  const rounded = Math.round(parsed);
  return Math.max(0, Math.min(maxExclusive - 1, rounded));
}

function normalizeButton(value: unknown): "left" | "right" | "middle" {
  const normalized =
    typeof value === "string" ? value.trim().toLowerCase() : "left";
  if (!normalized || normalized === "none" || normalized === "left") {
    return "left";
  }
  if (normalized === "right" || normalized === "middle") {
    return normalized;
  }
  if (normalized === "wheel") {
    return "middle";
  }
  if (normalized === "back" || normalized === "forward") {
    throw new Error(`Unsupported desktop action button: ${normalized}`);
  }
  throw new Error(`Unsupported desktop action button: ${normalized}`);
}

function normalizeScrollDelta(value: unknown): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    return 0;
  }
  return Math.round(parsed);
}

function normalizeWaitMs(value: unknown): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new Error("wait action is missing ms");
  }
  return Math.max(0, Math.min(MAX_WAIT_ACTION_MS, Math.round(parsed)));
}

function pickFirstDefined(...values: unknown[]): unknown {
  return values.find((value) => value !== undefined && value !== null);
}

function normalizePoint(
  raw: any,
  width: number,
  height: number,
): AiDesktopPoint {
  return {
    x: clampCoordinate(raw?.x, width),
    y: clampCoordinate(raw?.y, height),
  };
}

function normalizeDragPath(
  raw: any,
  width: number,
  height: number,
): AiDesktopPoint[] {
  const rawPath = Array.isArray(raw?.path)
    ? (raw.path as unknown[])
    : Array.isArray(raw?.points)
      ? (raw.points as unknown[])
      : Array.isArray(raw?.coordinates)
        ? (raw.coordinates as unknown[])
        : null;

  if (rawPath) {
    const path = rawPath.map((point) => normalizePoint(point, width, height));
    if (path.length >= 2) {
      return path;
    }
  }

  const start = raw?.start ?? raw?.from;
  const end = raw?.end ?? raw?.to;
  if (start && end) {
    return [
      normalizePoint(start, width, height),
      normalizePoint(end, width, height),
    ];
  }

  const startX = raw?.startX ?? raw?.start_x ?? raw?.x;
  const startY = raw?.startY ?? raw?.start_y ?? raw?.y;
  const endX = raw?.endX ?? raw?.end_x ?? raw?.destX ?? raw?.dest_x;
  const endY = raw?.endY ?? raw?.end_y ?? raw?.destY ?? raw?.dest_y;
  if (endX !== undefined && endY !== undefined) {
    return [
      { x: clampCoordinate(startX, width), y: clampCoordinate(startY, height) },
      { x: clampCoordinate(endX, width), y: clampCoordinate(endY, height) },
    ];
  }

  throw new Error("drag action is missing a path");
}

function summarizeActions(actions: AiDesktopAction[]): string {
  if (actions.length === 0) {
    return "The AI did not return any executable actions.";
  }
  const labels = actions.map((action) => action.type.replace("_", " "));
  return `AI prepared ${actions.length} action${actions.length === 1 ? "" : "s"}: ${labels.join(", ")}.`;
}

function parseTalosDesktopStepArguments(
  raw: unknown,
): TalosDesktopStepArguments {
  if (typeof raw !== "string" || !raw.trim()) {
    throw new Error("Talos desktop step call is missing arguments");
  }
  try {
    const parsed = JSON.parse(raw);
    return {
      status: normalizeTaskStatus(parsed?.status),
      plan: normalizePlan(parsed?.plan),
      message:
        typeof parsed?.message === "string" ? parsed.message.trim() : undefined,
      actions: Array.isArray(parsed?.actions) ? parsed.actions : undefined,
    };
  } catch {
    throw new Error("Talos desktop step call returned invalid JSON arguments");
  }
}

function extractTalosDesktopStepCall(
  response: any,
): TalosDesktopStepCall | null {
  const output = Array.isArray(response?.output) ? response.output : [];
  for (const item of output) {
    if (
      item?.type === "function_call" &&
      item.name === TALOS_DESKTOP_STEP_TOOL_NAME &&
      typeof item.call_id === "string"
    ) {
      return {
        callId: item.call_id,
        args: parseTalosDesktopStepArguments(item.arguments),
      };
    }
  }
  return null;
}

function extractOutputText(response: any): string {
  if (typeof response?.output_text === "string") {
    return response.output_text.trim();
  }
  const output = Array.isArray(response?.output) ? response.output : [];
  const chunks: string[] = [];
  for (const item of output) {
    if (typeof item?.content === "string") {
      chunks.push(item.content);
    }
    if (Array.isArray(item?.content)) {
      for (const content of item.content) {
        if (typeof content?.text === "string") {
          chunks.push(content.text);
        }
      }
    }
  }
  return chunks.join("\n").trim();
}

function normalizeShellNotes(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((item) => (typeof item === "string" ? item.trim() : ""))
    .filter((item) => item.length > 0)
    .slice(0, 6);
}

function normalizeShellAssistAction(value: unknown): AiShellAssistAction {
  const normalized =
    typeof value === "string" ? value.trim().toLowerCase() : "";
  if (
    normalized === "command" ||
    normalized === "wait" ||
    normalized === "interrupt" ||
    normalized === "done" ||
    normalized === "needs_input" ||
    normalized === "open_desktop"
  ) {
    return normalized;
  }
  throw new Error(`Unsupported Talos shell command proposal action: ${normalized || "(empty)"}`);
}

function normalizeShellWaitMs(value: unknown, action: AiShellAssistAction): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error("Talos shell proposal must include numeric waitMs");
  }
  if (action !== "wait") {
    if (value !== 0) {
      throw new Error("Talos shell non-wait proposal must set waitMs to 0");
    }
    return 0;
  }
  if (value <= 0) {
    throw new Error("Talos shell wait proposal must include a positive waitMs");
  }
  return Math.max(SHELL_WAIT_MIN_MS, Math.min(SHELL_WAIT_MAX_MS, Math.round(value)));
}

function normalizeShellAssistHistory(
  value: unknown,
): AiShellAssistHistoryEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const history: AiShellAssistHistoryEntry[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== "object") {
      continue;
    }
    const raw = entry as Record<string, unknown>;
    const command =
      typeof raw.command === "string" ? raw.command.trim().slice(0, 4_000) : "";
    if (!command) {
      continue;
    }
    history.push({
      command,
      approved: raw.approved === false ? false : true,
      output:
        typeof raw.output === "string"
          ? raw.output.trim().slice(-4_000)
          : null,
      responseId:
        typeof raw.responseId === "string" && raw.responseId.trim()
          ? raw.responseId.trim()
          : null,
    });
  }
  return history.slice(-8);
}

function formatShellAssistHistory(
  history: AiShellAssistHistoryEntry[],
): string {
  if (history.length === 0) {
    return "(no previous approved AI shell commands for this goal)";
  }
  return history
    .map((entry, index) => {
      const output = entry.output?.trim()
        ? entry.output.trim()
        : "(no captured output)";
      return [
        `Turn ${index + 1}`,
        `Approved: ${entry.approved === false ? "no" : "yes"}`,
        `Command: ${entry.command}`,
        "Observed output:",
        output,
      ].join("\n");
    })
    .join("\n\n");
}

function normalizeShellAssistActiveCommand(value: unknown): AiShellAssistActiveCommand | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const raw = value as Record<string, unknown>;
  const command = typeof raw.command === "string" ? raw.command.trim().slice(0, 4_000) : "";
  const approvalId = typeof raw.approvalId === "string" ? raw.approvalId.trim().slice(0, 200) : "";
  const turnIndex = Number(raw.turnIndex);
  const elapsedMs = Number(raw.elapsedMs);
  const checkpointCount = Number(raw.checkpointCount);
  const remainingMs = Number(raw.remainingMs);
  if (!command || !approvalId || !Number.isFinite(turnIndex)) {
    return null;
  }
  return {
    command,
    approvalId,
    turnIndex: Math.max(0, Math.trunc(turnIndex)),
    elapsedMs: Number.isFinite(elapsedMs) ? Math.max(0, Math.trunc(elapsedMs)) : 0,
    checkpointCount: Number.isFinite(checkpointCount) ? Math.max(0, Math.trunc(checkpointCount)) : 0,
    recentOutput:
      typeof raw.recentOutput === "string"
        ? raw.recentOutput.trim().slice(-4_000)
        : "",
    remainingMs: Number.isFinite(remainingMs) ? Math.max(0, Math.trunc(remainingMs)) : 0,
  };
}

function formatDurationMs(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0s";
  if (value < 1_000) return `${Math.round(value)}ms`;
  const seconds = value / 1_000;
  return seconds >= 60 ? `${Math.round(seconds / 60)}m ${Math.round(seconds % 60)}s` : `${Math.round(seconds)}s`;
}

function formatShellAssistActiveCommand(value: unknown): string {
  const active = normalizeShellAssistActiveCommand(value);
  if (!active) {
    return "(no command is currently running)";
  }
  return [
    `Approval: ${active.approvalId}`,
    `Turn: ${active.turnIndex + 1}`,
    `Elapsed: ${formatDurationMs(active.elapsedMs)}`,
    `Checkpoints: ${active.checkpointCount}`,
    `Remaining hard-timeout budget: ${formatDurationMs(active.remainingMs)}`,
    `Command: ${active.command}`,
    "Recent output:",
    active.recentOutput || "(no recent output yet)",
  ].join("\n");
}

export function parseTalosShellCommandArguments(
  raw: unknown,
): Omit<AiShellAssistResponse, "responseId"> {
  if (typeof raw !== "string" || !raw.trim()) {
    throw new Error("Talos shell command proposal is missing arguments");
  }
  let parsed: any;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(
      "Talos shell command proposal returned invalid JSON arguments",
    );
  }

  const command =
    typeof parsed?.command === "string" ? parsed.command.trim() : "";
  const action = normalizeShellAssistAction(parsed?.action);
  const waitMs = normalizeShellWaitMs(parsed?.waitMs, action);
  const explanation =
    typeof parsed?.explanation === "string" ? parsed.explanation.trim() : "";
  const risk = typeof parsed?.risk === "string" ? parsed.risk.trim() : "";
  const message =
    typeof parsed?.message === "string" && parsed.message.trim()
      ? parsed.message.trim()
      : explanation;
  if (action === "command" && !command) {
    throw new Error("Talos shell command proposal did not include a command");
  }
  if (action === "wait" && command) {
    throw new Error("Talos shell wait proposal must leave command empty");
  }
  if (action !== "command" && command) {
    throw new Error("Talos shell non-command proposal must leave command empty");
  }
  if (command.includes("\0")) {
    throw new Error(
      "Talos shell command proposal contains an invalid NUL byte",
    );
  }
  if (!explanation || !risk) {
    throw new Error(
      "Talos shell command proposal must include explanation and risk",
    );
  }
  return {
    action,
    command,
    explanation,
    risk,
    notes: normalizeShellNotes(parsed?.notes),
    message,
    waitMs,
  };
}

function extractTalosShellCommandProposal(
  response: any,
): AiShellAssistResponse {
  const output = Array.isArray(response?.output) ? response.output : [];
  for (const item of output) {
    if (
      item?.type === "function_call" &&
      item.name === TALOS_SHELL_COMMAND_TOOL_NAME
    ) {
      return {
        ...parseTalosShellCommandArguments(item.arguments),
        responseId: typeof response?.id === "string" ? response.id : null,
      };
    }
  }
  throw new Error("AI did not return a Talos shell command proposal");
}

export function buildShellAssistPrompt(
  request: AiShellAssistRequest,
  transcript: string,
  history: AiShellAssistHistoryEntry[],
): string {
  const platform = effectiveAssistPlatform(request.platform, request.deviceContext);
  return [
    "You are Talos Shell AI Assist for an RMM operator.",
    "Operate like a turn-based terminal coding/ops agent inside an existing interactive system shell.",
    "Your job is to advance the operator's goal one shell command at a time.",
    "Return exactly one turn by calling the Talos shell command proposal function.",
    "If another command is needed, set action=command and provide only that next command.",
    "If the transcript and prior turns show the goal is complete, set action=done and leave command empty.",
    "Do not repeat a command that already appears as an approved prior turn unless its observed output clearly failed to answer the goal.",
    "When the operator explicitly asks to run exactly one command, return action=done after that approved command has executed and produced relevant output.",
    "If the goal cannot safely continue without operator input, set action=needs_input and leave command empty.",
    "If the goal requires visible desktop interaction that cannot reasonably be completed in shell, set action=open_desktop and leave command empty.",
    "The viewer will show command turns to the operator, and a command is only sent after explicit approval.",
    `After approval, the viewer observes terminal output in a view-only terminal. If an approved command is still running, you may set action=wait with command empty and waitMs set to the additional wait you need; use ${SHELL_WAIT_DEFAULT_MS} when unsure.`,
    "If an active command is stuck, waiting for stdin, clearly wrong, or should be stopped before replanning, set action=interrupt with command empty and waitMs set to 0. Only use action=interrupt when an active command checkpoint is present.",
    `For wait actions, choose waitMs between ${SHELL_WAIT_MIN_MS} and ${SHELL_WAIT_MAX_MS}. For every non-wait action, set waitMs to 0.`,
    "Only use wait when the active command is plausibly still progressing. If output shows an stdin prompt, missing parameter prompt, password prompt, menu, confirmation prompt, or stalled installer, do not wait; choose interrupt, a recovery command, needs_input, or open_desktop.",
    "Prefer noninteractive flags for installers and remediations. Do not propose commands that require stdin prompts, menus, or hidden password entry unless the operator explicitly asks for that interaction.",
    "Do not claim a command has run unless it appears in prior turn history or the transcript.",
    "Do not pack a multi-step plan into one command just to avoid approvals. Use one focused command per turn.",
    "Prefer read-only or narrowly scoped commands. Avoid destructive changes, credential exposure, broad package installs, or long-running commands unless the operator clearly asks for them.",
    "If a safer inspection command should precede a risky change, propose the inspection command first.",
    "Use platform-appropriate commands. For Linux/macOS prefer POSIX shell syntax; for Windows prefer PowerShell/cmd syntax based on the visible shell.",
    `If a command needs a newly generated password, call ${TALOS_GENERATED_SECRET_TOOL_NAME} with surface=shell first. Use the returned shellReference in the command, and never write or ask for the password plaintext.`,
    "For Windows password-reset commands that use generated secrets, use PowerShell syntax and SecureString-compatible cmdlets. The returned shellReference is already a SecureString variable; pass it directly to SecureString-compatible parameters and do not wrap it with ConvertTo-SecureString.",
    "For Linux/macOS generated secrets, use the returned shellReference as a shell variable reference.",
    "Do not repeat secrets from terminal output in explanations, notes, or messages.",
    `Target platform: ${platform}.`,
    ...formatDeviceContextForPrompt(request.deviceContext),
    ...generatedSecretSummariesForPrompt(request.generatedSecrets, "shell"),
    "Prior approved AI shell turns for this goal follow:",
    "--- prior turns ---",
    formatShellAssistHistory(history),
    "--- end prior turns ---",
    "Active approved command checkpoint follows:",
    "--- active command ---",
    formatShellAssistActiveCommand(request.activeCommand),
    "--- end active command ---",
    "Recent terminal transcript follows. It may be incomplete and may contain prompts or command output; do not repeat secrets.",
    "--- transcript ---",
    transcript || "(no recent terminal transcript)",
    "--- end transcript ---",
    `Operator request: ${request.prompt}`,
  ].join("\n");
}

function normalizeTaskStatus(value: unknown): AiDesktopTaskStatus | undefined {
  const normalized =
    typeof value === "string" ? value.trim().toLowerCase() : "";
  if (
    normalized === "running" ||
    normalized === "complete" ||
    normalized === "failed" ||
    normalized === "needs_approval"
  ) {
    return normalized;
  }
  return undefined;
}

function normalizePlan(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const plan = value
    .map((item) => (typeof item === "string" ? item.trim() : ""))
    .filter((item) => item.length > 0)
    .slice(0, 12);
  return plan.length > 0 ? plan : undefined;
}

function parseTaskText(text: string): ParsedTaskText {
  const trimmed = text.trim();
  if (!trimmed) {
    return {};
  }

  const parseJson = (raw: string): ParsedTaskText | null => {
    try {
      const parsed = JSON.parse(raw);
      return {
        status: normalizeTaskStatus(parsed?.status),
        plan: normalizePlan(parsed?.plan),
        message:
          typeof parsed?.message === "string"
            ? parsed.message.trim()
            : undefined,
      };
    } catch {
      return null;
    }
  };

  const direct = parseJson(trimmed);
  if (direct) {
    return direct;
  }

  const firstBrace = trimmed.indexOf("{");
  const lastBrace = trimmed.lastIndexOf("}");
  if (firstBrace >= 0 && lastBrace > firstBrace) {
    const embedded = parseJson(trimmed.slice(firstBrace, lastBrace + 1));
    if (embedded) {
      return embedded;
    }
  }

  const lower = trimmed.toLowerCase();
  const status: AiDesktopTaskStatus =
    lower.includes("needs approval") || lower.includes("need approval")
      ? "needs_approval"
      : lower.includes("fail") ||
          lower.includes("cannot") ||
          lower.includes("can't")
        ? "failed"
        : "complete";
  return { status, message: trimmed };
}

function extractAssistantMessage(
  response: any,
  normalizedActions: AiDesktopAction[],
): string {
  const outputText = extractOutputText(response);
  if (outputText) {
    const parsed = parseTaskText(outputText);
    return parsed.message || outputText;
  }
  return summarizeActions(normalizedActions);
}

function normalizeTalosDesktopAction(
  raw: any,
  width: number,
  height: number,
): AiDesktopAction {
  const type =
    typeof raw?.type === "string" ? raw.type.trim().toLowerCase() : "";
  switch (type) {
    case "move":
      return {
        type: "move",
        x: clampCoordinate(raw.x, width),
        y: clampCoordinate(raw.y, height),
        keys: normalizeKeys(raw.keys),
      };
    case "click":
    case "double_click":
      return {
        type,
        x: clampCoordinate(raw.x, width),
        y: clampCoordinate(raw.y, height),
        button: normalizeButton(raw.button),
        keys: normalizeKeys(raw.keys),
      };
    case "scroll":
      return {
        type: "scroll",
        x: clampCoordinate(raw.x, width),
        y: clampCoordinate(raw.y, height),
        scrollX: normalizeScrollDelta(
          pickFirstDefined(
            raw.scrollX,
            raw.scroll_x,
            raw.deltaX,
            raw.delta_x,
            raw.dx,
          ),
        ),
        scrollY: normalizeScrollDelta(
          pickFirstDefined(
            raw.scrollY,
            raw.scroll_y,
            raw.deltaY,
            raw.delta_y,
            raw.dy,
            raw.amount,
            raw.delta,
          ),
        ),
        keys: normalizeKeys(raw.keys),
      };
    case "drag":
      return {
        type: "drag",
        path: normalizeDragPath(raw, width, height),
        button: normalizeButton(raw.button),
        keys: normalizeKeys(raw.keys),
      };
    case "type": {
      const text = typeof raw?.text === "string" ? raw.text : "";
      if (!text) {
        throw new Error("type action is missing text");
      }
      return { type: "type", text };
    }
    case "inject_secret": {
      const secretHandle =
        typeof raw?.secretHandle === "string"
          ? raw.secretHandle.trim()
          : typeof raw?.secret_handle === "string"
            ? raw.secret_handle.trim()
            : "";
      if (!/^sec_[a-z0-9]{16}$/.test(secretHandle)) {
        throw new Error("inject_secret action is missing a valid secretHandle");
      }
      return { type: "inject_secret", secretHandle };
    }
    case "keypress": {
      const keys = normalizeKeys(raw.keys);
      if (keys.length === 0) {
        throw new Error("keypress action is missing keys");
      }
      return { type: "keypress", keys };
    }
    case "wait":
      return {
        type: "wait",
        ms: normalizeWaitMs(
          raw.ms ?? raw.durationMs ?? raw.duration_ms ?? DEFAULT_WAIT_ACTION_MS,
        ),
      };
    default:
      throw new Error(`Unsupported desktop action: ${type || "unknown"}`);
  }
}

function actionableDesktopActions(actions: any[]): any[] {
  return actions.filter((action) => action?.type !== "screenshot");
}

function desktopActionsRequestScreenshot(actions: any[]): boolean {
  return actions.some((action) => action?.type === "screenshot");
}

function buildFunctionCallOutput(callId: string, output: unknown): any {
  return {
    type: "function_call_output",
    call_id: callId,
    output:
      typeof output === "string" || Array.isArray(output)
        ? output
      : JSON.stringify(output),
  };
}

type GeneratedSecretContext = {
  jobId?: string | null;
  organizationId?: string | null;
  userId?: string | null;
  agentId?: string | null;
};

function generatedSecretToolCalls(response: any): Array<{ callId: string; args: Record<string, unknown> }> {
  const output = Array.isArray(response?.output) ? response.output : [];
  const calls: Array<{ callId: string; args: Record<string, unknown> }> = [];
  for (const item of output) {
    if (
      item?.type === "function_call" &&
      item.name === TALOS_GENERATED_SECRET_TOOL_NAME &&
      typeof item.call_id === "string"
    ) {
      try {
        const parsed = typeof item.arguments === "string" ? JSON.parse(item.arguments) : item.arguments;
        calls.push({
          callId: item.call_id,
          args: parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {},
        });
      } catch {
        calls.push({ callId: item.call_id, args: { parseError: "invalid JSON arguments" } });
      }
    }
  }
  return calls;
}

function secretSurfaceValue(value: unknown): SecretSurface | null {
  return value === "shell" || value === "desktop" || value === "note_only" ? value : null;
}

function positiveIntOrUndefined(value: unknown): number | undefined {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? Math.trunc(parsed) : undefined;
}

async function executeGeneratedSecretTool(
  args: Record<string, unknown>,
  context: GeneratedSecretContext,
): Promise<{ ok: boolean; result?: GeneratedSecretSummary; error?: string }> {
  if (typeof args.parseError === "string") {
    return { ok: false, error: args.parseError };
  }
  if (args.kind !== "password") {
    return { ok: false, error: "Only password secret generation is supported" };
  }
  const surface = secretSurfaceValue(args.surface);
  if (!surface) {
    return { ok: false, error: "surface must be shell, desktop, or note_only" };
  }
  if (!context.organizationId || !context.userId) {
    return { ok: false, error: "Secret generation requires AI runner organization and user context" };
  }
  try {
    const result = await createGeneratedSecret({
      organizationId: context.organizationId,
      userId: context.userId,
      kind: "password",
      surface,
      purpose: typeof args.purpose === "string" && args.purpose.trim() ? args.purpose.trim() : "Generated Talos secret",
      jobId: context.jobId ?? null,
      agentId: context.agentId ?? null,
      passwordOptions: {
        minWordLength: positiveIntOrUndefined(args.minWordLength),
        maxWordLength: positiveIntOrUndefined(args.maxWordLength),
        maxPasswordLength: positiveIntOrUndefined(args.maxPasswordLength),
      },
    });
    return { ok: true, result };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}

async function resolveGeneratedSecretToolCalls(
  client: OpenAI,
  model: string,
  response: any,
  tools: any[],
  context: GeneratedSecretContext,
  generatedSecrets: GeneratedSecretSummary[],
  logContext: Record<string, unknown>,
): Promise<any> {
  let current = response;
  for (let round = 0; round < 4; round += 1) {
    const calls = generatedSecretToolCalls(current);
    if (calls.length === 0) {
      return current;
    }
    if (typeof current?.id !== "string") {
      throw new Error("OpenAI secret tool response did not include a response id");
    }
    const outputs = [];
    for (const call of calls) {
      const output = await executeGeneratedSecretTool(call.args, context);
      if (output.result) {
        generatedSecrets.push(output.result);
      }
      outputs.push(buildFunctionCallOutput(call.callId, output));
    }
    const previousResponseId = current.id;
    current = await streamRawOpenAiResponse(
      client,
      withOpenAIReasoning({
        model,
        tools,
        tool_choice: "auto",
        previous_response_id: previousResponseId,
        input: outputs,
      }),
      {
        ...logContext,
        operation: `${String(logContext.operation || "ai_assist")}_secret_tool`,
        model,
        previous_response_id: previousResponseId,
        generated_secret_calls: calls.length,
      },
    );
  }
  throw new Error("OpenAI repeatedly requested generated secrets without returning a Talos step");
}

function stringifyRawOpenAi(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch (error) {
    return JSON.stringify({
      serializationError: error instanceof Error ? error.message : String(error),
      fallback: String(value),
    });
  }
}

function logRawOpenAiResponse(
  phase: string,
  response: unknown,
  fields: Record<string, unknown> = {},
): void {
  const record = response && typeof response === "object" ? (response as any) : null;
  log.debug("raw OpenAI final response", {
    phase,
    ...fields,
    response_id: typeof record?.id === "string" ? record.id : null,
    raw_json: stringifyRawOpenAi(response),
  });
}

function logRawOpenAiEvent(
  phase: string,
  event: unknown,
  fields: Record<string, unknown> = {},
): void {
  log.debug("raw OpenAI response event", {
    phase,
    ...fields,
    raw_json: stringifyRawOpenAi(event),
  });
}

async function streamRawOpenAiResponse(
  client: OpenAI,
  params: Record<string, unknown>,
  debug: Record<string, unknown>,
): Promise<any> {
  const stream: any = client.responses.stream(params);
  let eventIndex = 0;
  for await (const event of stream) {
    eventIndex += 1;
    logRawOpenAiEvent("stream_event", event, {
      ...debug,
      event_index: eventIndex,
      event_type: typeof event?.type === "string" ? event.type : null,
    });
  }
  const response = await stream.finalResponse();
  logRawOpenAiResponse("stream_final_response", response, {
    ...debug,
    event_count: eventIndex,
  });
  return response;
}

function buildScreenshotFunctionCallOutput(
  callId: string,
  text: string,
  screenshotBase64: string,
): any {
  return buildFunctionCallOutput(callId, [
    { type: "input_text", text },
    {
      type: "input_image",
      image_url: `data:image/png;base64,${screenshotBase64}`,
      detail: "original",
    },
  ]);
}

async function requestTalosDesktopActions(
  client: OpenAI,
  model: string,
  prompt: string,
  screenshotBase64: string,
  width: number,
  height: number,
  platform: AiAssistPlatform,
  context: GeneratedSecretContext,
): Promise<AiDesktopActionResponse> {
  let response: any = await streamRawOpenAiResponse(
    client,
    withOpenAIReasoning({
      model,
      tools: talosDesktopTools(),
      tool_choice: talosDesktopToolChoice(),
      input: [buildScreenshotMessage(buildPrompt(prompt, platform), screenshotBase64)],
    }),
    {
      operation: "desktop_action_initial",
      model,
      platform,
      prompt_chars: prompt.length,
      screenshot_width: width,
      screenshot_height: height,
    },
  );

  let screenshotSent = false;
  const generatedSecrets: GeneratedSecretSummary[] = [];
  for (;;) {
    response = await resolveGeneratedSecretToolCalls(
      client,
      model,
      response,
      talosDesktopTools(),
      context,
      generatedSecrets,
      {
        operation: "desktop_action",
        platform,
      },
    );
    const toolCall = extractTalosDesktopStepCall(response);
    if (!toolCall) {
      throw new Error("OpenAI did not return a Talos desktop step");
    }

    const actions = toolCall.args.actions ?? [];
    const screenshotRequested = desktopActionsRequestScreenshot(actions);
    const actionable = actionableDesktopActions(actions);
    if (actionable.length > 0) {
      const normalizedActions = actionable.map((action) =>
        normalizeTalosDesktopAction(action, width, height),
      );
      return {
        assistantMessage:
          toolCall.args.message ||
          extractAssistantMessage(response, normalizedActions),
        actions: normalizedActions,
        responseId: typeof response?.id === "string" ? response.id : null,
        generatedSecrets,
      };
    }

    if (!screenshotRequested) {
      throw new Error(
        "OpenAI returned a Talos desktop step without executable actions",
      );
    }
    if (screenshotSent) {
      throw new Error(
        "OpenAI requested an additional screenshot; multi-turn desktop observation is not enabled yet",
      );
    }

    const previousResponseId = response.id;
    response = await streamRawOpenAiResponse(
      client,
      withOpenAIReasoning({
        model,
        tools: talosDesktopTools(),
        tool_choice: talosDesktopToolChoice(),
        previous_response_id: previousResponseId,
        input: [
          buildScreenshotFunctionCallOutput(
            toolCall.callId,
            JSON.stringify({
              ok: true,
              result: "Screenshot observation supplied.",
              observation:
                "The attached image is the current remote desktop state for the requested screenshot.",
            }),
            screenshotBase64,
          ),
        ],
      }),
      {
        operation: "desktop_action_screenshot",
        model,
        platform,
        previous_response_id: previousResponseId,
        prompt_chars: prompt.length,
        screenshot_width: width,
        screenshot_height: height,
      },
    );
    screenshotSent = true;
  }
}

function buildTaskContinueInput(
  callId: string,
  screenshotBase64: string,
  lastStepResult?: string | null,
): any[] {
  const result =
    typeof lastStepResult === "string" ? lastStepResult.trim() : "";
  return [
    buildScreenshotFunctionCallOutput(
      callId,
      JSON.stringify({
        ok: true,
        result: result || "Viewer executed the previous action batch.",
        observation:
          "The attached image is the latest remote desktop state after the previous Talos desktop step. Base the next action only on this image.",
      }),
      screenshotBase64,
    ),
  ];
}

function buildTaskStepResponse(
  task: AiDesktopTaskState,
  response: any,
  status: AiDesktopTaskStatus,
  actions: AiDesktopAction[],
  assistantMessage: string,
  generatedSecrets: GeneratedSecretSummary[] = [],
): AiDesktopTaskStepResponse {
  const responseId =
    typeof response?.id === "string" ? response.id : task.responseId;
  task.responseId = responseId ?? null;
  task.status = status;
  task.updatedAt = Date.now();
  return {
    taskId: task.taskId,
    status,
    plan: task.plan,
    assistantMessage,
    actions,
    responseId: task.responseId,
    stepIndex: task.stepIndex,
    maxSteps: task.maxSteps,
    generatedSecrets,
  };
}

async function normalizeTaskResponse(
  client: OpenAI,
  task: AiDesktopTaskState,
  response: any,
  screenshotBase64: string,
  width: number,
  height: number,
): Promise<AiDesktopTaskStepResponse> {
  let screenshotSent = false;
  const maxActions = taskMaxActionsPerStep();
  const generatedSecrets: GeneratedSecretSummary[] = [...task.generatedSecrets];

  for (;;) {
    response = await resolveGeneratedSecretToolCalls(
      client,
      task.model,
      response,
      talosDesktopTools(),
      {
        jobId: task.jobId,
        organizationId: task.organizationId,
        userId: task.userId,
        agentId: task.agentId,
      },
      generatedSecrets,
      {
        operation: "desktop_task",
        task_id: task.taskId,
        session_id: task.sessionId,
        platform: task.platform,
      },
    );
    task.generatedSecrets = mergeGeneratedSecretSummaries(task.generatedSecrets, generatedSecrets);
    const outputText = extractOutputText(response);
    const parsedText = parseTaskText(outputText);
    const toolCall = extractTalosDesktopStepCall(response);
    const stepArgs = toolCall?.args ?? {};
    const plan = stepArgs.plan ?? parsedText.plan;
    if (plan) {
      task.plan = plan;
    }

    if (!toolCall) {
      task.pendingToolCallId = null;
      task.stepIndex += 1;
      const status = parsedText.status ?? (outputText ? "complete" : "failed");
      const assistantMessage =
        parsedText.message ||
        outputText ||
        (status === "complete"
          ? "The AI marked the task complete."
          : "The AI stopped without returning a Talos desktop step.");
      return buildTaskStepResponse(
        task,
        response,
        status,
        [],
        assistantMessage,
        generatedSecrets,
      );
    }

    const rawActions = stepArgs.actions ?? [];
    const screenshotRequested = desktopActionsRequestScreenshot(rawActions);
    const actionable = actionableDesktopActions(rawActions);
    if (actionable.length > 0) {
      if (actionable.length > maxActions) {
        throw new Error(
          `OpenAI returned ${actionable.length} desktop actions, exceeding max ${maxActions}`,
        );
      }
      const normalizedActions = actionable.map((action) =>
        normalizeTalosDesktopAction(action, width, height),
      );
      task.pendingToolCallId = toolCall.callId;
      task.stepIndex += 1;
      const assistantMessage =
        stepArgs.message ||
        parsedText.message ||
        extractAssistantMessage(response, normalizedActions);
      return buildTaskStepResponse(
        task,
        response,
        "running",
        normalizedActions,
        assistantMessage,
        generatedSecrets,
      );
    }

    if (!screenshotRequested) {
      task.pendingToolCallId = null;
      task.stepIndex += 1;
      const status = stepArgs.status ?? parsedText.status ?? "complete";
      const assistantMessage =
        stepArgs.message ||
        parsedText.message ||
        outputText ||
        (status === "complete"
          ? "The AI marked the task complete."
          : "The AI stopped without returning executable actions.");
      return buildTaskStepResponse(
        task,
        response,
        status,
        [],
        assistantMessage,
        generatedSecrets,
      );
    }
    if (screenshotSent) {
      throw new Error(
        "OpenAI requested repeated screenshots without returning actions",
      );
    }

    const previousResponseId = response.id;
    response = await streamRawOpenAiResponse(
      client,
      withOpenAIReasoning({
        model: task.model,
        tools: talosDesktopTools(),
        tool_choice: talosDesktopToolChoice(),
        previous_response_id: previousResponseId,
        input: [
          buildScreenshotFunctionCallOutput(
            toolCall.callId,
            JSON.stringify({
              ok: true,
              result: "Screenshot observation supplied.",
              observation:
                "The attached image is the current remote desktop state for the requested screenshot.",
            }),
            screenshotBase64,
          ),
        ],
      }),
      {
        operation: "desktop_task_screenshot",
        task_id: task.taskId,
        session_id: task.sessionId,
        model: task.model,
        platform: task.platform,
        previous_response_id: previousResponseId,
        step_index: task.stepIndex,
        screenshot_width: width,
        screenshot_height: height,
      },
    );
    screenshotSent = true;
  }
}

export async function startAiDesktopTask(
  request: AiDesktopTaskStartRequest,
): Promise<AiDesktopTaskStepResponse> {
  if (!aiAssistEnabled()) {
    throw new Error("RMM AI assist is disabled");
  }

  cleanupExpiredTasks();
  const model =
    (process.env.OPENAI_COMPUTER_USE_MODEL || DEFAULT_OPENAI_MODEL).trim() ||
    DEFAULT_OPENAI_MODEL;
  const platform = normalizeAssistPlatform(request.platform);
  const startedAt = Date.now();
  const rmmServerBase = await verifyRemoteDesktopSession(
    request.sessionId,
    request.sessionToken,
    request.rmmApiBase,
  );
  const task: AiDesktopTaskState = {
    taskId: randomUUID(),
    goal: request.goal,
    sessionId: request.sessionId,
    sessionToken: request.sessionToken,
    rmmApiBase: request.rmmApiBase,
    rmmServerBase,
    platform,
    deviceContext: request.deviceContext ?? null,
    jobId: request.jobId ?? null,
    organizationId: request.organizationId ?? null,
    userId: request.userId ?? null,
    conversationId: request.conversationId ?? null,
    agentId: request.agentId ?? null,
    generatedSecrets: mergeGeneratedSecretSummaries([], request.generatedSecrets),
    model,
    responseId: null,
    pendingToolCallId: null,
    plan: [`Goal: ${request.goal}`],
    status: "running",
    stepIndex: 0,
    maxSteps: taskMaxSteps(),
    createdAt: Date.now(),
    updatedAt: Date.now(),
  };
  aiDesktopTasks.set(task.taskId, task);

  log.info("desktop task started", {
    task_id: task.taskId,
    session_id: request.sessionId,
    rmm_api_base: rmmServerBase,
    goal_chars: request.goal.length,
    screenshot_width: request.width,
    screenshot_height: request.height,
    platform,
    model,
    max_steps: task.maxSteps,
  });

  try {
    const response: any = await streamRawOpenAiResponse(
      getOpenAiClient(),
      withOpenAIReasoning({
        model,
        tools: talosDesktopTools(),
        tool_choice: talosDesktopToolChoice(),
        input: [
          buildScreenshotMessage(
            buildTaskPrompt(
              request.goal,
              platform,
              request.deviceContext ?? null,
              task.generatedSecrets,
            ),
            request.screenshotBase64,
          ),
        ],
      }),
      {
        operation: "desktop_task_initial",
        task_id: task.taskId,
        session_id: request.sessionId,
        model,
        platform,
        step_index: task.stepIndex,
        screenshot_width: request.width,
        screenshot_height: request.height,
      },
    );
    const result = await normalizeTaskResponse(
      getOpenAiClient(),
      task,
      response,
      request.screenshotBase64,
      request.width,
      request.height,
    );
    log.info("desktop task step prepared", {
      task_id: task.taskId,
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      step_index: result.stepIndex,
      status: result.status,
      action_count: result.actions.length,
      action_types: result.actions.map((action) => action.type).join(","),
    });
    return result;
  } catch (error) {
    task.status = "failed";
    task.updatedAt = Date.now();
    log.error("desktop task start failed", {
      task_id: task.taskId,
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
}

export async function continueAiDesktopTask(
  request: AiDesktopTaskContinueRequest,
): Promise<AiDesktopTaskStepResponse> {
  if (!aiAssistEnabled()) {
    throw new Error("RMM AI assist is disabled");
  }

  cleanupExpiredTasks();
  const task = aiDesktopTasks.get(request.taskId);
  if (!task) {
    throw new Error("AI desktop task not found");
  }
  if (
    task.sessionId !== request.sessionId ||
    task.sessionToken !== request.sessionToken
  ) {
    throw new Error("AI desktop task session does not match");
  }
  if (task.status !== "running") {
    return buildTaskStepResponse(
      task,
      { id: task.responseId },
      task.status,
      [],
      "The AI task is no longer running.",
    );
  }
  if (!task.responseId || !task.pendingToolCallId) {
    throw new Error(
      "AI desktop task has no pending Talos desktop step to continue",
    );
  }
  if (task.stepIndex >= task.maxSteps) {
    task.status = "failed";
    task.pendingToolCallId = null;
    return buildTaskStepResponse(
      task,
      { id: task.responseId },
      "failed",
      [],
      `Stopped after reaching the maximum of ${task.maxSteps} AI steps.`,
    );
  }

  const startedAt = Date.now();
  await verifyRemoteDesktopSession(
    task.sessionId,
    task.sessionToken,
    request.rmmApiBase ?? task.rmmApiBase,
  );
  if (request.deviceContext && !task.deviceContext) {
    task.deviceContext = request.deviceContext;
  }
  task.jobId = task.jobId ?? request.jobId ?? null;
  task.organizationId = task.organizationId ?? request.organizationId ?? null;
  task.userId = task.userId ?? request.userId ?? null;
  task.conversationId = task.conversationId ?? request.conversationId ?? null;
  task.agentId = task.agentId ?? request.agentId ?? null;
  task.generatedSecrets = mergeGeneratedSecretSummaries(task.generatedSecrets, request.generatedSecrets);

  try {
    const response: any = await streamRawOpenAiResponse(
      getOpenAiClient(),
      withOpenAIReasoning({
        model: task.model,
        tools: talosDesktopTools(),
        tool_choice: talosDesktopToolChoice(),
        previous_response_id: task.responseId,
        input: buildTaskContinueInput(
          task.pendingToolCallId,
          request.screenshotBase64,
          request.lastStepResult,
        ),
      }),
      {
        operation: "desktop_task_continue",
        task_id: task.taskId,
        session_id: task.sessionId,
        model: task.model,
        platform: task.platform,
        previous_response_id: task.responseId,
        step_index: task.stepIndex,
        screenshot_width: request.width,
        screenshot_height: request.height,
        last_step_result_chars: request.lastStepResult?.length ?? 0,
      },
    );
    const result = await normalizeTaskResponse(
      getOpenAiClient(),
      task,
      response,
      request.screenshotBase64,
      request.width,
      request.height,
    );
    log.info("desktop task continued", {
      task_id: task.taskId,
      session_id: task.sessionId,
      duration_ms: Date.now() - startedAt,
      platform: task.platform,
      step_index: result.stepIndex,
      status: result.status,
      action_count: result.actions.length,
      action_types: result.actions.map((action) => action.type).join(","),
    });
    return result;
  } catch (error) {
    task.status = "failed";
    task.updatedAt = Date.now();
    log.error("desktop task continue failed", {
      task_id: task.taskId,
      session_id: task.sessionId,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
}

export async function runAiDesktopActionPoc(
  request: AiDesktopActionRequest,
): Promise<AiDesktopActionResponse> {
  if (!aiAssistEnabled()) {
    throw new Error("RMM AI assist is disabled");
  }

  const model =
    (process.env.OPENAI_COMPUTER_USE_MODEL || DEFAULT_OPENAI_MODEL).trim() ||
    DEFAULT_OPENAI_MODEL;
  const platform = normalizeAssistPlatform(request.platform);
  const startedAt = Date.now();
  const rmmServerBase = await verifyRemoteDesktopSession(
    request.sessionId,
    request.sessionToken,
    request.rmmApiBase,
  );
  log.info("desktop action request started", {
    session_id: request.sessionId,
    rmm_api_base: rmmServerBase,
    prompt_chars: request.prompt.length,
    screenshot_width: request.width,
    screenshot_height: request.height,
    platform,
    model,
  });

  try {
    const result = await requestTalosDesktopActions(
      getOpenAiClient(),
      model,
      request.prompt,
      request.screenshotBase64,
      request.width,
      request.height,
      platform,
      {
        jobId: request.jobId,
        organizationId: request.organizationId,
        userId: request.userId,
        agentId: request.agentId,
      },
    );
    log.info("desktop action request completed", {
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      action_count: result.actions.length,
      action_types: result.actions.map((action) => action.type).join(","),
    });
    return result;
  } catch (error) {
    log.error("desktop action request failed", {
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
}

export async function proposeAiShellCommand(
  request: AiShellAssistRequest,
): Promise<AiShellAssistResponse> {
  if (!aiAssistEnabled()) {
    throw new Error("RMM AI assist is disabled");
  }

  const prompt = request.prompt.trim();
  if (!prompt) {
    throw new Error("prompt is required");
  }

  const transcript = (request.transcript || "").slice(
    -MAX_SHELL_TRANSCRIPT_CHARS,
  );
  const history = normalizeShellAssistHistory(request.history);
  const model =
    (
      process.env.OPENAI_SHELL_ASSIST_MODEL ||
      process.env.OPENAI_COMPUTER_USE_MODEL ||
      DEFAULT_OPENAI_MODEL
    ).trim() || DEFAULT_OPENAI_MODEL;
  const startedAt = Date.now();
  const rmmServerBase = await verifyShellSession(
    request.sessionId,
    request.sessionToken,
    request.rmmApiBase,
  );
  log.info("shell assist proposal request started", {
    session_id: request.sessionId,
    rmm_api_base: rmmServerBase,
    platform: request.platform ?? null,
    prompt_chars: prompt.length,
    transcript_chars: transcript.length,
    history_turns: history.length,
    model,
  });

  try {
    let response: any = await streamRawOpenAiResponse(
      getOpenAiClient(),
      withOpenAIReasoning({
        model,
        tools: talosShellTools(),
        tool_choice: talosShellToolChoice(),
        input: buildShellAssistPrompt({ ...request, prompt }, transcript, history),
      }),
      {
        operation: "shell_assist",
        session_id: request.sessionId,
        model,
        platform: request.platform ?? null,
        prompt_chars: prompt.length,
        transcript_chars: transcript.length,
        history_turns: history.length,
      },
    );
    const generatedSecrets: GeneratedSecretSummary[] = mergeGeneratedSecretSummaries([], request.generatedSecrets);
    response = await resolveGeneratedSecretToolCalls(
      getOpenAiClient(),
      model,
      response,
      talosShellTools(),
      {
        jobId: request.jobId,
        organizationId: request.organizationId,
        userId: request.userId,
        agentId: request.agentId,
      },
      generatedSecrets,
      {
        operation: "shell_assist",
        session_id: request.sessionId,
        platform: request.platform ?? null,
      },
    );
    const proposal = extractTalosShellCommandProposal(response);
    proposal.generatedSecrets = generatedSecrets;
    log.info("shell assist proposal prepared", {
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      response_id: proposal.responseId,
      action: proposal.action,
      command_chars: proposal.command.length,
      generated_secret_count: generatedSecrets.length,
    });
    return proposal;
  } catch (error) {
    log.error("shell assist proposal request failed", {
      session_id: request.sessionId,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
    });
    throw error;
  }
}

export async function logAiShellCommandApproved(
  request: AiShellApprovalRequest,
): Promise<void> {
  const command = request.command.trim();
  if (!command) {
    throw new Error("command is required");
  }
  const rmmServerBase = await verifyShellSession(
    request.sessionId,
    request.sessionToken,
    request.rmmApiBase,
  );
  const commandHash = createHash("sha256").update(command).digest("hex");
  log.info("shell assist command approved", {
    session_id: request.sessionId,
    rmm_api_base: rmmServerBase,
    platform: request.platform ?? null,
    response_id: request.responseId ?? null,
    command_sha256: commandHash,
    command_chars: command.length,
  });
}
