import {
  commandCenterMcpClient,
  type ApiLocalMcpClient,
} from "./mcp/client";
import type { CommandCenterMcpContext } from "./mcp/types";

const MCP_PROTOCOL_VERSION = "2024-11-05";
const SERVER_NAME = "talos-command-center";
const SERVER_VERSION = "0.2.0";

type JsonRpcRequest = {
  jsonrpc?: string;
  id?: string | number | null;
  method?: string;
  params?: unknown;
};

type JsonRpcResponse = {
  jsonrpc: "2.0";
  id: string | number | null;
  result?: unknown;
  error?: {
    code: number;
    message: string;
  };
};

export function commandCenterMcpSystemNotice(): string {
  const tools = commandCenterMcpClient
    .listToolDefinitions()
    .map((tool) => tool.name)
    .join(", ");
  return [
    "Private Talos data tools are available for organization-scoped customer and device questions.",
    `Available capability groups: ${tools}.`,
    "Use them when the operator asks for device inventory, customer/device lookup, telemetry snapshot details, secure desktop view, device connection, or current-screen capture.",
    "Do not mention MCP, tool calls, function calls, or internal implementation details in the final user-facing answer.",
  ].join(" ");
}

function response(id: string | number | null | undefined, result: unknown): JsonRpcResponse {
  return {
    jsonrpc: "2.0",
    id: id ?? null,
    result,
  };
}

function errorResponse(
  id: string | number | null | undefined,
  code: number,
  message: string,
): JsonRpcResponse {
  return {
    jsonrpc: "2.0",
    id: id ?? null,
    error: { code, message },
  };
}

function mcpToolDefinitions(client: ApiLocalMcpClient) {
  return client.listToolDefinitions().map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: tool.inputSchema,
  }));
}

export async function handleCommandCenterMcpRequest(
  body: unknown,
  context: CommandCenterMcpContext,
  client: ApiLocalMcpClient = commandCenterMcpClient,
): Promise<JsonRpcResponse | null> {
  const request = body as JsonRpcRequest;
  if (!request || request.jsonrpc !== "2.0" || typeof request.method !== "string") {
    return errorResponse(null, -32600, "Invalid JSON-RPC request");
  }

  if (request.id === undefined || request.id === null) {
    return null;
  }

  switch (request.method) {
    case "initialize":
      return response(request.id, {
        protocolVersion: MCP_PROTOCOL_VERSION,
        capabilities: {
          tools: {},
        },
        serverInfo: {
          name: SERVER_NAME,
          version: SERVER_VERSION,
          servers: client.listServers(),
        },
      });
    case "tools/list":
      return response(request.id, {
        tools: mcpToolDefinitions(client),
      });
    case "tools/call": {
      const params = request.params as { name?: unknown; arguments?: unknown } | null;
      const name = typeof params?.name === "string" ? params.name : "";
      if (!name) {
        return errorResponse(request.id, -32602, "Tool name is required");
      }
      try {
        const result = await client.executeTool(name, params?.arguments ?? {}, context);
        return response(request.id, {
          content: [
            {
              type: "text",
              text: JSON.stringify(result),
            },
          ],
        });
      } catch (error) {
        return errorResponse(
          request.id,
          -32602,
          error instanceof Error ? error.message : String(error),
        );
      }
    }
    default:
      return errorResponse(request.id, -32601, "Method not found");
  }
}
