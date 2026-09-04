import { prisma } from "../prisma";
import { createAiRunnerMcpServer } from "./servers/aiRunnerServer";
import { createDeviceMcpServer } from "./servers/deviceServer";
import { createSecretBrokerMcpServer } from "./servers/secretBrokerServer";
import type {
  CommandCenterMcpContext,
  CommandCenterMcpServer,
  CommandCenterMcpStatusEvent,
  CommandCenterMcpTool,
  CommandCenterMcpToolDefinition,
} from "./types";

const TOOL_STATUS_MESSAGES = [
  "Recalculating splines",
  "Cross-checking inventory signals",
  "Sorting endpoint breadcrumbs",
  "Aligning device context",
  "Tracing software records",
  "Comparing telemetry notes",
  "Reading device inventory",
  "Pulling the latest endpoint context",
  "Opening a secure desktop view",
  "Observing desktop state",
  "Executing desktop actions",
  "Checking relay state",
  "Packaging visual context",
];

function randomToolStatusMessage(): string {
  return TOOL_STATUS_MESSAGES[Math.floor(Math.random() * TOOL_STATUS_MESSAGES.length)];
}

function toJsonSafe(value: unknown): unknown {
  if (typeof value === "bigint") {
    const asNumber = Number(value);
    return Number.isSafeInteger(asNumber) ? asNumber : value.toString();
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (Array.isArray(value)) {
    return value.map(toJsonSafe);
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return Object.fromEntries(Object.entries(record).map(([key, item]) => [key, toJsonSafe(item)]));
  }
  return value;
}

function parseToolArguments(raw: unknown): Record<string, unknown> {
  if (!raw) return {};
  if (typeof raw === "object" && !Array.isArray(raw)) {
    return raw as Record<string, unknown>;
  }
  if (typeof raw !== "string") {
    throw new Error("Tool call arguments must be a JSON object");
  }
  const trimmed = raw.trim();
  if (!trimmed) return {};
  const parsed = JSON.parse(trimmed);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Tool call arguments must be a JSON object");
  }
  return parsed as Record<string, unknown>;
}

export class ApiLocalMcpClient {
  private readonly servers: CommandCenterMcpServer[];
  private readonly toolsByName = new Map<string, CommandCenterMcpTool>();

  constructor(servers: CommandCenterMcpServer[]) {
    this.servers = servers;
    for (const server of servers) {
      for (const tool of server.tools) {
        if (this.toolsByName.has(tool.definition.name)) {
          throw new Error(`Duplicate MCP tool registered: ${tool.definition.name}`);
        }
        this.toolsByName.set(tool.definition.name, tool);
      }
    }
  }

  listServers(): Array<{ name: string; version: string }> {
    return this.servers.map((server) => ({
      name: server.name,
      version: server.version,
    }));
  }

  listToolDefinitions(): CommandCenterMcpToolDefinition[] {
    return [...this.toolsByName.values()].map((tool) => tool.definition);
  }

  openAiTools(): any[] {
    return this.listToolDefinitions().map((tool) => ({
      type: "function",
      name: tool.name,
      description: tool.description,
      parameters: tool.inputSchema,
    }));
  }

  hasTool(name: string): boolean {
    return this.toolsByName.has(name);
  }

  async executeTool(
    name: string,
    args: unknown,
    context: CommandCenterMcpContext,
  ): Promise<unknown> {
    const tool = this.toolsByName.get(name);
    if (!tool) {
      throw new Error(`Unknown MCP tool: ${name}`);
    }
    await context.emitStatus?.({
      phase: "tool",
      message: randomToolStatusMessage(),
    });
    const parsedArgs = parseToolArguments(args);
    const result = await tool.handler(parsedArgs, context);
    return toJsonSafe(result);
  }
}

export const commandCenterMcpClient = new ApiLocalMcpClient([
  createDeviceMcpServer(),
  createAiRunnerMcpServer(),
  createSecretBrokerMcpServer(),
]);

export async function createCommandCenterMcpContext(
  userId: string,
  emitStatus?: (event: CommandCenterMcpStatusEvent) => void | Promise<void>,
  options: {
    conversationId?: string | null;
    abortSignal?: AbortSignal;
    addAttachment?: CommandCenterMcpContext["addAttachment"];
  } = {},
): Promise<CommandCenterMcpContext> {
  const membership = await prisma.organizationMember.findFirst({
    where: { userId },
    include: {
      organization: true,
      user: { select: { id: true, email: true } },
    },
  });
  if (!membership) {
    throw new Error("No organization is available for this user");
  }
  return {
    userId,
    userEmail: membership.user?.email ?? null,
    organizationId: membership.organizationId,
    organizationName: membership.organization?.name ?? null,
    role: membership.role,
    conversationId: options.conversationId ?? null,
    abortSignal: options.abortSignal,
    emitStatus,
    addAttachment: options.addAttachment,
  };
}

export function buildFunctionCallOutput(callId: string, output: unknown): any {
  return {
    type: "function_call_output",
    call_id: callId,
    output: typeof output === "string" ? output : JSON.stringify(toJsonSafe(output)),
  };
}

export function extractOpenAiFunctionCalls(response: any): Array<{
  callId: string;
  name: string;
  arguments: Record<string, unknown>;
}> {
  const output = Array.isArray(response?.output) ? response.output : [];
  const calls: Array<{ callId: string; name: string; arguments: Record<string, unknown> }> = [];
  for (const item of output) {
    if (
      item?.type === "function_call" &&
      typeof item.name === "string" &&
      typeof item.call_id === "string"
    ) {
      calls.push({
        callId: item.call_id,
        name: item.name,
        arguments: parseToolArguments(item.arguments),
      });
    }
  }
  return calls;
}
