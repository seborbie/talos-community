import OpenAI from "openai";
import { createLogger } from "./logger";
import { commandCenterMcpSystemNotice } from "./commandCenterMcp";
import {
  buildFunctionCallOutput,
  commandCenterMcpClient,
  createCommandCenterMcpContext,
  extractOpenAiFunctionCalls,
} from "./mcp/client";
import type { CommandCenterMcpStatusEvent } from "./mcp/types";
import type { CommandCenterMessageAttachment } from "./commandCenterAiRunner";

const log = createLogger("api_backend::command_center_ai");

const DEFAULT_OPENAI_MODEL = "gpt-5.5";
const COMMAND_CENTER_RESPONSE_TIMEOUT_MS = 90_000;
const MAX_MESSAGES = 32;
const MAX_MESSAGE_CHARS = 8_000;
const MAX_TRANSCRIPT_CHARS = 32_000;
const MAX_TOOL_ROUNDS = 10;

export type CommandCenterChatRole = "user" | "assistant";

export type CommandCenterChatMessage = {
  role: CommandCenterChatRole;
  content: string;
};

export type CommandCenterChatRequest = {
  messages: CommandCenterChatMessage[];
  userId?: string | null;
  conversationId?: string | null;
  onStatus?: (event: CommandCenterMcpStatusEvent) => void | Promise<void>;
};

export type CommandCenterChatStreamRequest = CommandCenterChatRequest & {
  onDelta?: (delta: string) => void | Promise<void>;
  abortSignal?: AbortSignal;
};

export type CommandCenterChatResponse = {
  content: string;
  model: string;
  responseId: string | null;
  attachments?: CommandCenterMessageAttachment[];
};

let cachedClient: OpenAI | null = null;

function commandCenterModel(): string {
  return (
    process.env.OPENAI_COMMAND_CENTER_MODEL ||
    process.env.OPENAI_COMPUTER_USE_MODEL ||
    DEFAULT_OPENAI_MODEL
  ).trim() || DEFAULT_OPENAI_MODEL;
}

function getOpenAiClient(): OpenAI {
  const apiKey = (process.env.OPENAI_API_KEY || "").trim();
  if (!apiKey) {
    throw new Error("OPENAI_API_KEY is not configured");
  }
  if (!cachedClient) {
    cachedClient = new OpenAI({
      apiKey,
      timeout: COMMAND_CENTER_RESPONSE_TIMEOUT_MS,
    });
  }
  return cachedClient;
}

function withNoReasoning<T extends object>(
  params: T,
): T & { reasoning: { effort: "none" } } {
  return {
    ...params,
    reasoning: { effort: "none" },
  };
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

function throwIfCommandCenterAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw new Error("Command Center request stopped by operator");
  }
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

async function streamOpenAiResponse(
  params: Record<string, unknown>,
  options: {
    signal?: AbortSignal;
    onDelta?: (delta: string) => void | Promise<void>;
    debug?: Record<string, unknown>;
  } = {},
): Promise<{ response: any; streamedText: string }> {
  const stream: any = getOpenAiClient().responses.stream(
    withNoReasoning(params),
    options.signal ? { signal: options.signal } : undefined,
  );
  let streamedText = "";
  let canStreamText = false;
  let sawFunctionCall = false;
  let eventIndex = 0;

  for await (const event of stream) {
    eventIndex += 1;
    logRawOpenAiEvent("stream_event", event, {
      ...options.debug,
      event_index: eventIndex,
      event_type: typeof event?.type === "string" ? event.type : null,
    });
    if (event?.type === "response.output_item.added") {
      const itemType = event?.item?.type;
      if (itemType === "message") {
        canStreamText = true;
      } else if (itemType === "function_call") {
        sawFunctionCall = true;
        canStreamText = false;
      }
    }
    if (
      event?.type === "response.output_text.delta" &&
      canStreamText &&
      !sawFunctionCall &&
      typeof event.delta === "string" &&
      event.delta
    ) {
      streamedText += event.delta;
      await options.onDelta?.(event.delta);
    }
  }

  const response = await stream.finalResponse();
  logRawOpenAiResponse("stream_final_response", response, {
    ...options.debug,
    event_count: eventIndex,
    streamed_text_chars: streamedText.length,
  });
  return { response, streamedText };
}

function cleanMessage(message: CommandCenterChatMessage): CommandCenterChatMessage | null {
  const content = message.content.trim();
  if (!content) {
    return null;
  }
  return {
    role: message.role,
    content: content.length > MAX_MESSAGE_CHARS ? content.slice(0, MAX_MESSAGE_CHARS) : content,
  };
}

export function normalizeCommandCenterMessages(
  messages: CommandCenterChatMessage[],
): CommandCenterChatMessage[] {
  const cleaned = messages
    .filter((message) => message.role === "user" || message.role === "assistant")
    .map(cleanMessage)
    .filter((message): message is CommandCenterChatMessage => Boolean(message))
    .slice(-MAX_MESSAGES);

  let remaining = MAX_TRANSCRIPT_CHARS;
  const trimmed: CommandCenterChatMessage[] = [];
  for (let index = cleaned.length - 1; index >= 0; index -= 1) {
    const message = cleaned[index];
    if (remaining <= 0) {
      break;
    }
    const content = message.content.slice(-remaining);
    remaining -= content.length;
    trimmed.unshift({ ...message, content });
  }
  return trimmed;
}

export function buildCommandCenterPrompt(
  messages: CommandCenterChatMessage[],
  options: { organizationName?: string | null } = {},
): string {
  const transcript = messages
    .map((message) => {
      const speaker = message.role === "user" ? "Operator" : "Talos";
      return `${speaker}: ${message.content}`;
    })
    .join("\n\n");

  return [
    "You are Talos Command Center, the AI assistant inside a remote monitoring and management dashboard.",
    "Use a calm, concise operator-support voice. Help with devices, alerts, customers, patches, users, troubleshooting, and remediation planning.",
    options.organizationName
      ? `Current organization: ${options.organizationName}.`
      : "Current organization: the operator's active Talos organization.",
    commandCenterMcpSystemNotice(),
    "When the operator asks for device or customer information, use the available private Talos data tools before answering.",
    "If a device or customer search is ambiguous, ask one concise clarification question instead of guessing.",
    "For action workflows that need a generated secret, identify the target customer and device before calling create_generated_secret. Do not create a secret while the target device is unresolved.",
    "If the operator uses shorthand such as dc or domain controller for a specific customer, search/list that customer's devices; if exactly one device is returned, treat it as the intended target, otherwise ask for clarification.",
    "If the operator asks for a specific device snapshot detail, find the device first, list available snapshot paths if needed, then read the appropriate allowlisted path.",
    "If the operator asks you to perform a remediation or action goal on a device, use the private Talos runner tools with shell-first execution. Use desktop-only execution only when the operator specifically asks for visible GUI control. If the tool says it is awaiting endpoint approval, tell the operator Talos is waiting for endpoint approval and do not claim the device is connected. If the tool says the job is still running, say the runner has started and that command approvals or screenshots will appear in the chat as Talos works.",
    "If you call create_generated_secret before start_ai_runner_job, pass the returned secretHandle in start_ai_runner_job.generatedSecretHandles. Do not rely on only mentioning the shellReference or desktopReference in the goal text.",
    "For Windows shell password resets with generated secrets, the shellReference is already a PowerShell SecureString variable; tell the runner to pass it directly to SecureString-compatible cmdlets rather than wrapping it in ConvertTo-SecureString.",
    "When reporting snapshot-derived data, include the collection time if the tool result provides one.",
    "Do not claim you ran commands, changed settings, or contacted endpoints unless the operator explicitly provides that evidence in the conversation.",
    "Never mention MCP, tools, function calls, JSON-RPC, or internal implementation details in the user-facing answer.",
    "Prefer clear bullets for multi-step operational answers. Keep short answers short.",
    "",
    "--- conversation ---",
    transcript,
    "--- end conversation ---",
    "",
    "Return only the next Talos assistant message.",
  ].join("\n");
}

export async function runCommandCenterChat(
  request: CommandCenterChatRequest,
): Promise<CommandCenterChatResponse> {
  const messages = normalizeCommandCenterMessages(request.messages);
  if (!messages.some((message) => message.role === "user")) {
    throw new Error("at least one user message is required");
  }
  if (!request.userId) {
    throw new Error("user context is required");
  }

  const model = commandCenterModel();
  const startedAt = Date.now();
  const attachments: CommandCenterMessageAttachment[] = [];
  const mcpContext = await createCommandCenterMcpContext(request.userId, request.onStatus, {
    conversationId: request.conversationId ?? null,
    addAttachment: (attachment) => {
      if (!attachments.some((existing) => existing.artifactId === attachment.artifactId)) {
        attachments.push(attachment);
      }
    },
  });
  log.info("command center chat request started", {
    user_id: request.userId ?? null,
    organization_id: mcpContext.organizationId,
    message_count: messages.length,
    model,
  });

  try {
    let response: any = await getOpenAiClient().responses.create(
      withNoReasoning({
        model,
        tools: commandCenterMcpClient.openAiTools(),
        input: buildCommandCenterPrompt(messages, {
          organizationName: mcpContext.organizationName,
        }),
      }),
    );
    logRawOpenAiResponse("chat_initial_response", response, {
      user_id: request.userId ?? null,
      organization_id: mcpContext.organizationId,
      model,
      tool_round: 0,
    });

    for (let round = 0; round < MAX_TOOL_ROUNDS; round += 1) {
      const toolCalls = extractOpenAiFunctionCalls(response);
      if (toolCalls.length === 0) {
        const content = extractOutputText(response);
        if (!content) {
          throw new Error("OpenAI did not return a Command Center response");
        }
        log.info("command center chat response prepared", {
          user_id: request.userId ?? null,
          organization_id: mcpContext.organizationId,
          duration_ms: Date.now() - startedAt,
          response_id: typeof response?.id === "string" ? response.id : null,
          content_chars: content.length,
          tool_rounds: round,
          model,
        });
        return {
          content,
          model,
          responseId: typeof response?.id === "string" ? response.id : null,
          attachments,
        };
      }

      if (typeof response?.id !== "string") {
        throw new Error("OpenAI tool response did not include a response id");
      }

      const toolOutputs = [];
      for (const toolCall of toolCalls) {
        try {
          const result = await commandCenterMcpClient.executeTool(
            toolCall.name,
            toolCall.arguments,
            mcpContext,
          );
          toolOutputs.push(
            buildFunctionCallOutput(toolCall.callId, {
              ok: true,
              result,
            }),
          );
        } catch (error) {
          toolOutputs.push(
            buildFunctionCallOutput(toolCall.callId, {
              ok: false,
              error: error instanceof Error ? error.message : String(error),
            }),
          );
        }
      }

      const previousResponseId = response.id;
      response = await getOpenAiClient().responses.create(
        withNoReasoning({
          model,
          tools: commandCenterMcpClient.openAiTools(),
          previous_response_id: previousResponseId,
          input: toolOutputs,
        }),
      );
      logRawOpenAiResponse("chat_tool_response", response, {
        user_id: request.userId ?? null,
        organization_id: mcpContext.organizationId,
        model,
        tool_round: round + 1,
        previous_response_id: previousResponseId,
        tool_output_count: toolOutputs.length,
      });
    }

    const content = extractOutputText(response);
    if (!content) {
      throw new Error("OpenAI did not return a Command Center response after tool use");
    }
    log.info("command center chat response prepared", {
      user_id: request.userId ?? null,
      organization_id: mcpContext.organizationId,
      duration_ms: Date.now() - startedAt,
      response_id: typeof response?.id === "string" ? response.id : null,
      content_chars: content.length,
      tool_rounds: MAX_TOOL_ROUNDS,
      model,
    });
    return {
      content,
      model,
      responseId: typeof response?.id === "string" ? response.id : null,
      attachments,
    };
  } catch (error) {
    log.error("command center chat request failed", {
      user_id: request.userId ?? null,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
      model,
    });
    throw error;
  }
}

export async function runCommandCenterChatStream(
  request: CommandCenterChatStreamRequest,
): Promise<CommandCenterChatResponse> {
  const messages = normalizeCommandCenterMessages(request.messages);
  if (!messages.some((message) => message.role === "user")) {
    throw new Error("at least one user message is required");
  }
  if (!request.userId) {
    throw new Error("user context is required");
  }

  const model = commandCenterModel();
  const startedAt = Date.now();
  const attachments: CommandCenterMessageAttachment[] = [];
  const mcpContext = await createCommandCenterMcpContext(request.userId, request.onStatus, {
    conversationId: request.conversationId ?? null,
    abortSignal: request.abortSignal,
    addAttachment: (attachment) => {
      if (!attachments.some((existing) => existing.artifactId === attachment.artifactId)) {
        attachments.push(attachment);
      }
    },
  });
  log.info("command center streaming chat request started", {
    user_id: request.userId ?? null,
    organization_id: mcpContext.organizationId,
    message_count: messages.length,
    model,
  });

  try {
    throwIfCommandCenterAborted(request.abortSignal);
    const firstTurn = await streamOpenAiResponse(
      {
        model,
        tools: commandCenterMcpClient.openAiTools(),
        input: buildCommandCenterPrompt(messages, {
          organizationName: mcpContext.organizationName,
        }),
      },
      {
        signal: request.abortSignal,
        onDelta: request.onDelta,
        debug: {
          user_id: request.userId ?? null,
          organization_id: mcpContext.organizationId,
          model,
          tool_round: 0,
        },
      },
    );
    let response: any = firstTurn.response;
    let streamedContent = firstTurn.streamedText;
    throwIfCommandCenterAborted(request.abortSignal);

    for (let round = 0; round < MAX_TOOL_ROUNDS; round += 1) {
      throwIfCommandCenterAborted(request.abortSignal);
      const toolCalls = extractOpenAiFunctionCalls(response);
      if (toolCalls.length === 0) {
        const content = extractOutputText(response) || streamedContent;
        if (!content) {
          throw new Error("OpenAI did not return a Command Center response");
        }
        log.info("command center streaming chat response prepared", {
          user_id: request.userId ?? null,
          organization_id: mcpContext.organizationId,
          duration_ms: Date.now() - startedAt,
          response_id: typeof response?.id === "string" ? response.id : null,
          content_chars: content.length,
          tool_rounds: round,
          model,
        });
        return {
          content,
          model,
          responseId: typeof response?.id === "string" ? response.id : null,
          attachments,
        };
      }

      if (typeof response?.id !== "string") {
        throw new Error("OpenAI tool response did not include a response id");
      }

      const toolOutputs = [];
      for (const toolCall of toolCalls) {
        throwIfCommandCenterAborted(request.abortSignal);
        try {
          const result = await commandCenterMcpClient.executeTool(
            toolCall.name,
            toolCall.arguments,
            mcpContext,
          );
          throwIfCommandCenterAborted(request.abortSignal);
          toolOutputs.push(
            buildFunctionCallOutput(toolCall.callId, {
              ok: true,
              result,
            }),
          );
        } catch (error) {
          if (request.abortSignal?.aborted) {
            throw error;
          }
          toolOutputs.push(
            buildFunctionCallOutput(toolCall.callId, {
              ok: false,
              error: error instanceof Error ? error.message : String(error),
            }),
          );
        }
      }

      const previousResponseId = response.id;
      throwIfCommandCenterAborted(request.abortSignal);
      const nextTurn = await streamOpenAiResponse(
        {
          model,
          tools: commandCenterMcpClient.openAiTools(),
          previous_response_id: previousResponseId,
          input: toolOutputs,
        },
        {
          signal: request.abortSignal,
          onDelta: request.onDelta,
          debug: {
            user_id: request.userId ?? null,
            organization_id: mcpContext.organizationId,
            model,
            tool_round: round + 1,
            previous_response_id: previousResponseId,
            tool_output_count: toolOutputs.length,
          },
        },
      );
      response = nextTurn.response;
      streamedContent += nextTurn.streamedText;
      throwIfCommandCenterAborted(request.abortSignal);
    }

    const content = extractOutputText(response) || streamedContent;
    if (!content) {
      throw new Error("OpenAI did not return a Command Center response after tool use");
    }
    log.info("command center streaming chat response prepared", {
      user_id: request.userId ?? null,
      organization_id: mcpContext.organizationId,
      duration_ms: Date.now() - startedAt,
      response_id: typeof response?.id === "string" ? response.id : null,
      content_chars: content.length,
      tool_rounds: MAX_TOOL_ROUNDS,
      model,
    });
    return {
      content,
      model,
      responseId: typeof response?.id === "string" ? response.id : null,
      attachments,
    };
  } catch (error) {
    log.error("command center streaming chat request failed", {
      user_id: request.userId ?? null,
      duration_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
      model,
    });
    throw error;
  }
}
