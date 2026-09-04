import { Router } from "express";
import {
  runCommandCenterChat,
  runCommandCenterChatStream,
  type CommandCenterChatMessage,
} from "../lib/commandCenterAi";
import {
  appendCommandCenterMessage,
  compactCommandCenterConversationTitle,
  createCommandCenterConversation,
  deleteCommandCenterConversation,
  getCommandCenterConversation,
  listCommandCenterConversations,
  listCommandCenterMessages,
  type CommandCenterConversationContext,
} from "../lib/commandCenterConversations";
import {
  acquireAiRunnerJobLease,
  appendAiRunnerArtifactFromCallback,
  appendAiRunnerEventFromCallback,
  approveAiRunnerCommandApproval,
  createAiRunnerCommandApprovalFromCallback,
  denyAiRunnerCommandApprovalAndUseDesktopControl,
  denyAiRunnerCommandApproval,
  getAiRunnerConversationStreamSnapshot,
  getAiRunnerCommandApprovalFromCallback,
  getAiRunnerJobDetail,
  getAiRunnerReplayManifest,
  heartbeatAiRunnerJobLease,
  listAiRunnerCommandOutputDeltas,
  listAiRunnerJobs,
  readAiRunnerArtifactContent,
  readAiRunnerShellTranscript,
  releaseAiRunnerJobLease,
  stopAiRunnerJob,
  stopAiRunnerJobsForConversation,
  updateAiRunnerCommandApprovalExecutionFromCallback,
  updateAiRunnerJobStatusFromCallback,
} from "../lib/commandCenterAiRunner";
import { handleCommandCenterMcpRequest } from "../lib/commandCenterMcp";
import { env } from "../lib/env";
import { createCommandCenterMcpContext } from "../lib/mcp/client";
import { requireAuth, type AuthedRequest } from "../middleware/auth";

export const commandCenterRouter = Router();

function readMessages(value: unknown): CommandCenterChatMessage[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((item) => {
      const role = item?.role === "assistant" ? "assistant" : item?.role === "user" ? "user" : null;
      const content = typeof item?.content === "string" ? item.content : "";
      return role ? { role, content } : null;
    })
    .filter((item): item is CommandCenterChatMessage => Boolean(item));
}

function readConversationId(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function latestUserMessage(messages: CommandCenterChatMessage[]): CommandCenterChatMessage | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index].role === "user") {
      return messages[index];
    }
  }
  return null;
}

async function resolveCommandCenterConversationContext(
  userId: string,
): Promise<CommandCenterConversationContext> {
  const context = await createCommandCenterMcpContext(userId);
  return {
    organizationId: context.organizationId,
    userId,
  };
}

async function resolveChatConversation(
  context: CommandCenterConversationContext,
  conversationId: string | null,
  firstMessageContent: string,
) {
  if (conversationId) {
    return getCommandCenterConversation(context, conversationId);
  }
  return createCommandCenterConversation(context, {
    title: compactCommandCenterConversationTitle(firstMessageContent),
  });
}

function sendCommandCenterError(res: any, error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("at least one user message")) {
    return res.status(400).json({ error: message });
  }
  if (message.includes("user context")) {
    return res.status(401).json({ error: message });
  }
  if (message.includes("No organization")) {
    return res.status(404).json({ error: message, needsOnboarding: true });
  }
  if (message.includes("OPENAI_API_KEY") || message.includes("not configured")) {
    return res.status(503).json({ error: message });
  }
  if (message.includes("did not return")) {
    return res.status(502).json({ error: message });
  }
  if (message.includes("lease mismatch") || message.includes("lease_lost")) {
    return res.status(409).json({ error: message });
  }
  return res.status(500).json({ error: message || "Command Center chat request failed" });
}

function writeSseEvent(res: any, event: string, data: unknown) {
  res.write(`event: ${event}\n`);
  res.write(`data: ${JSON.stringify(data)}\n\n`);
}

function delay(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function requireCommandCenterServiceKey(req: any, res: any): boolean {
  const expected = (env.aiRunnerServiceKey || env.serviceKey || "").trim();
  if (!expected) {
    res.status(503).json({ error: "SERVICE_KEY is not configured" });
    return false;
  }
  const presented = String(req.headers["x-service-key"] || "").trim();
  if (!presented || presented !== expected) {
    res.status(401).json({ error: "unauthorized" });
    return false;
  }
  return true;
}

function assistantMetadata(result: { attachments?: unknown[] }) {
  return Array.isArray(result.attachments) && result.attachments.length > 0
    ? { attachments: result.attachments }
    : undefined;
}

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/lease",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const lease = await acquireAiRunnerJobLease(req.params.jobId, req.body?.runnerId);
      if (!lease) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.status(lease.accepted ? 200 : 409).json({ lease });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/lease/:leaseId/heartbeat",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const lease = await heartbeatAiRunnerJobLease(req.params.jobId, req.params.leaseId, req.body?.runnerId);
      if (!lease) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.status(lease.accepted ? 200 : 409).json({ lease });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/lease/:leaseId/release",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const lease = await releaseAiRunnerJobLease(req.params.jobId, req.params.leaseId, req.body?.runnerId);
      if (!lease) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.json({ lease });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/events",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const event = await appendAiRunnerEventFromCallback(req.params.jobId, req.body || {});
      if (!event) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.status(201).json({ event });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/status",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const job = await updateAiRunnerJobStatusFromCallback(req.params.jobId, req.body || {});
      if (!job) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.json({ job });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/artifacts",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const artifact = await appendAiRunnerArtifactFromCallback(req.params.jobId, req.body || {});
      if (!artifact) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.status(201).json({ artifact });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/command-approvals",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const approval = await createAiRunnerCommandApprovalFromCallback(req.params.jobId, req.body || {});
      if (!approval) {
        return res.status(404).json({ error: "AI runner job not found" });
      }
      return res.status(201).json({ approval });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.get(
  "/internal/ai-runner/jobs/:jobId/command-approvals/:approvalId",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const approval = await getAiRunnerCommandApprovalFromCallback(req.params.jobId, req.params.approvalId);
      if (!approval) {
        return res.status(404).json({ error: "AI runner command approval not found" });
      }
      return res.json({ approval });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/internal/ai-runner/jobs/:jobId/command-approvals/:approvalId/result",
  async (req, res) => {
    if (!requireCommandCenterServiceKey(req, res)) return;
    try {
      const approval = await updateAiRunnerCommandApprovalExecutionFromCallback(
        req.params.jobId,
        req.params.approvalId,
        req.body || {},
      );
      if (!approval) {
        return res.status(404).json({ error: "AI runner command approval not found" });
      }
      return res.json({ approval });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.get("/artifacts/:artifactId/content", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const artifact = await readAiRunnerArtifactContent(context, req.params.artifactId);
    if (!artifact) {
      return res.status(404).json({ error: "Artifact not found" });
    }
    res.setHeader("Content-Type", artifact.mimeType);
    res.setHeader("Content-Disposition", `inline; filename="${artifact.name.replace(/"/g, "")}"`);
    return res.send(artifact.buffer);
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/ai-runner/command-approvals/:approvalId/approve", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const approval = await approveAiRunnerCommandApproval(context, req.params.approvalId);
    if (!approval) {
      return res.status(404).json({ error: "AI runner command approval not found" });
    }
    return res.json({ approval });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/ai-runner/command-approvals/:approvalId/deny", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const approval = await denyAiRunnerCommandApproval(context, req.params.approvalId);
    if (!approval) {
      return res.status(404).json({ error: "AI runner command approval not found" });
    }
    return res.json({ approval });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/ai-runner/command-approvals/:approvalId/deny-and-use-desktop-control", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const result = await denyAiRunnerCommandApprovalAndUseDesktopControl(context, req.params.approvalId);
    if (!result) {
      return res.status(404).json({ error: "AI runner command approval not found" });
    }
    return res.status(result.job ? 202 : 200).json(result);
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get("/ai-runner/jobs", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const conversationId = readConversationId(req.query.conversationId);
    const active = String(req.query.active || "").toLowerCase() === "true";
    const items = await listAiRunnerJobs(context, { conversationId, active });
    return res.json({ items });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/ai-runner/jobs/:jobId/stop", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const job = await stopAiRunnerJob(context, req.params.jobId);
    if (!job) {
      return res.status(404).json({ error: "AI runner job not found" });
    }
    const detail = await getAiRunnerJobDetail(context, job.id);
    return res.status(202).json({ job: detail ?? job });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get("/ai-runner/jobs/:jobId/shell-transcript", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const transcript = await readAiRunnerShellTranscript(context, req.params.jobId);
    if (!transcript) {
      return res.status(404).json({ error: "AI runner shell transcript not found" });
    }
    res.setHeader("Content-Type", transcript.mimeType);
    res.setHeader("Content-Disposition", `attachment; filename="${transcript.name.replace(/"/g, "")}"`);
    return res.send(transcript.buffer);
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get("/ai-runner/jobs/:jobId/replay", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const replay = await getAiRunnerReplayManifest(context, req.params.jobId);
    if (!replay) {
      return res.status(404).json({ error: "AI runner replay not found" });
    }
    return res.json({ replay });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get("/ai-runner/jobs/:jobId", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const job = await getAiRunnerJobDetail(context, req.params.jobId);
    if (!job) {
      return res.status(404).json({ error: "AI runner job not found" });
    }
    return res.json({ job });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get(
  "/conversations/:conversationId/ai-runner/stream",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (req.jwt?.type !== "user") {
      return res.status(403).json({ error: "Machine tokens are not allowed" });
    }

    let context: CommandCenterConversationContext;
    let conversationId = req.params.conversationId;
    try {
      context = await resolveCommandCenterConversationContext(req.jwt.sub);
      const snapshot = await getAiRunnerConversationStreamSnapshot(context, conversationId);
      if (!snapshot) {
        return res.status(404).json({ error: "Conversation not found" });
      }

      let closed = false;
      res.on("close", () => {
        closed = true;
      });
      res.status(200);
      res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
      res.setHeader("Cache-Control", "no-cache, no-transform");
      res.setHeader("Connection", "keep-alive");
      res.flushHeaders?.();
      writeSseEvent(res, "snapshot", snapshot);

      let lastOutputAt =
        snapshot.output.length > 0
          ? new Date(snapshot.output[snapshot.output.length - 1].createdAt)
          : new Date(0);
      const seenOutputEventIds = new Set(snapshot.output.map((chunk) => chunk.eventId));
      let lastJobSignature = JSON.stringify(
        snapshot.jobs.map((job) => ({
          id: job.id,
          status: job.status,
          updatedAt: job.updatedAt,
          approval: job.pendingCommandApproval
            ? {
                id: job.pendingCommandApproval.id,
                status: job.pendingCommandApproval.status,
                updatedAt: job.pendingCommandApproval.updatedAt,
              }
            : null,
          latestApproval: job.latestCommandApproval
            ? {
                id: job.latestCommandApproval.id,
                status: job.latestCommandApproval.status,
                updatedAt: job.latestCommandApproval.updatedAt,
              }
            : null,
        })),
      );
      let heartbeatTicks = 0;
      while (!closed) {
        await delay(1_000);
        if (closed) break;
        try {
          const output = await listAiRunnerCommandOutputDeltas(context, conversationId, {
            after: new Date(Math.max(0, lastOutputAt.getTime() - 1_000)),
            take: 500,
          });
          for (const chunk of output) {
            if (seenOutputEventIds.has(chunk.eventId)) {
              continue;
            }
            seenOutputEventIds.add(chunk.eventId);
            if (seenOutputEventIds.size > 2_000) {
              const oldest = seenOutputEventIds.values().next().value;
              if (oldest) seenOutputEventIds.delete(oldest);
            }
            writeSseEvent(res, "command_output_delta", chunk);
            const createdAt = new Date(chunk.createdAt);
            if (createdAt.getTime() > lastOutputAt.getTime()) {
              lastOutputAt = createdAt;
            }
          }

          const jobs = await listAiRunnerJobs(context, { conversationId });
          const signature = JSON.stringify(
            jobs.map((job) => ({
              id: job.id,
              status: job.status,
              updatedAt: job.updatedAt,
              approval: job.pendingCommandApproval
                ? {
                    id: job.pendingCommandApproval.id,
                    status: job.pendingCommandApproval.status,
                    updatedAt: job.pendingCommandApproval.updatedAt,
                  }
                : null,
              latestApproval: job.latestCommandApproval
                ? {
                    id: job.latestCommandApproval.id,
                    status: job.latestCommandApproval.status,
                    updatedAt: job.latestCommandApproval.updatedAt,
                  }
                : null,
            })),
          );
          if (signature !== lastJobSignature) {
            lastJobSignature = signature;
            writeSseEvent(res, "jobs", { jobs });
          }

          heartbeatTicks += 1;
          if (heartbeatTicks >= 15) {
            heartbeatTicks = 0;
            writeSseEvent(res, "heartbeat", { at: new Date().toISOString() });
          }
        } catch (error) {
          writeSseEvent(res, "error", {
            error: error instanceof Error ? error.message : "AI runner stream failed",
          });
          break;
        }
      }
      if (!closed) {
        res.end();
      }
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post(
  "/conversations/:conversationId/ai-runner/stop",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (req.jwt?.type !== "user") {
      return res.status(403).json({ error: "Machine tokens are not allowed" });
    }

    try {
      const context = await resolveCommandCenterConversationContext(req.jwt.sub);
      const conversation = await getCommandCenterConversation(context, req.params.conversationId);
      if (!conversation) {
        return res.status(404).json({ error: "Conversation not found" });
      }
      const items = await stopAiRunnerJobsForConversation(context, conversation.id);
      return res.status(202).json({ items });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.get("/conversations", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const items = await listCommandCenterConversations(context);
    return res.json({ items });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/conversations", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await resolveCommandCenterConversationContext(req.jwt.sub);
    const conversation = await createCommandCenterConversation(context, {
      title: typeof req.body?.title === "string" ? req.body.title : null,
    });
    return res.status(201).json({ conversation });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.get(
  "/conversations/:conversationId/messages",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (req.jwt?.type !== "user") {
      return res.status(403).json({ error: "Machine tokens are not allowed" });
    }

    try {
      const context = await resolveCommandCenterConversationContext(req.jwt.sub);
      const items = await listCommandCenterMessages(context, req.params.conversationId);
      if (!items) {
        return res.status(404).json({ error: "Conversation not found" });
      }
      return res.json({ items });
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.delete(
  "/conversations/:conversationId",
  requireAuth,
  async (req: AuthedRequest, res) => {
    if (req.jwt?.type !== "user") {
      return res.status(403).json({ error: "Machine tokens are not allowed" });
    }

    try {
      const context = await resolveCommandCenterConversationContext(req.jwt.sub);
      const deleted = await deleteCommandCenterConversation(context, req.params.conversationId);
      if (!deleted) {
        return res.status(404).json({ error: "Conversation not found" });
      }
      return res.status(204).send();
    } catch (error) {
      return sendCommandCenterError(res, error);
    }
  },
);

commandCenterRouter.post("/chat", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  const messages = readMessages(req.body?.messages);
  if (messages.length === 0) {
    return res.status(400).json({ error: "messages are required" });
  }
  const userMessage = latestUserMessage(messages);
  if (!userMessage) {
    return res.status(400).json({ error: "at least one user message is required" });
  }

  try {
    const conversationContext = await resolveCommandCenterConversationContext(req.jwt.sub);
    const conversation = await resolveChatConversation(
      conversationContext,
      readConversationId(req.body?.conversationId),
      userMessage.content,
    );
    if (!conversation) {
      return res.status(404).json({ error: "Conversation not found" });
    }
    const persistedUserMessage = await appendCommandCenterMessage(conversationContext, conversation.id, {
      role: "user",
      content: userMessage.content,
    });
    if (!persistedUserMessage) {
      return res.status(404).json({ error: "Conversation not found" });
    }

    const result = await runCommandCenterChat({
      messages,
      userId: req.jwt.sub,
      conversationId: conversation.id,
    });
    await appendCommandCenterMessage(conversationContext, conversation.id, {
      role: "assistant",
      content: result.content,
      model: result.model,
      responseId: result.responseId,
      metadata: assistantMetadata(result),
    });
    return res.json({ ...result, conversationId: conversation.id });
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});

commandCenterRouter.post("/chat/stream", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  const messages = readMessages(req.body?.messages);
  if (messages.length === 0) {
    return res.status(400).json({ error: "messages are required" });
  }
  const userMessage = latestUserMessage(messages);
  if (!userMessage) {
    return res.status(400).json({ error: "at least one user message is required" });
  }

  let conversationContext: CommandCenterConversationContext;
  let conversationId: string;

  try {
    conversationContext = await resolveCommandCenterConversationContext(req.jwt.sub);
    const conversation = await resolveChatConversation(
      conversationContext,
      readConversationId(req.body?.conversationId),
      userMessage.content,
    );
    if (!conversation) {
      return res.status(404).json({ error: "Conversation not found" });
    }
    conversationId = conversation.id;
    const persistedUserMessage = await appendCommandCenterMessage(conversationContext, conversationId, {
      role: "user",
      content: userMessage.content,
    });
    if (!persistedUserMessage) {
      return res.status(404).json({ error: "Conversation not found" });
    }
  } catch (error) {
    return sendCommandCenterError(res, error);
  }

  let closed = false;
  const abortController = new AbortController();
  res.on("close", () => {
    closed = true;
    abortController.abort();
  });

  res.status(200);
  res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
  res.setHeader("Cache-Control", "no-cache, no-transform");
  res.setHeader("Connection", "keep-alive");
  res.flushHeaders?.();
  writeSseEvent(res, "conversation", { conversationId });

  try {
    const result = await runCommandCenterChatStream({
      messages,
      userId: req.jwt.sub,
      conversationId,
      abortSignal: abortController.signal,
      onStatus: (event) => {
        if (!closed) {
          writeSseEvent(res, "status", event);
        }
      },
      onDelta: (delta) => {
        if (!closed) {
          writeSseEvent(res, "delta", { delta });
        }
      },
    });
    await appendCommandCenterMessage(conversationContext, conversationId, {
      role: "assistant",
      content: result.content,
      model: result.model,
      responseId: result.responseId,
      metadata: assistantMetadata(result),
    });
    if (!closed) {
      writeSseEvent(res, "final", { ...result, conversationId });
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!closed) {
      writeSseEvent(res, "error", {
        error: message || "Command Center chat request failed",
      });
    }
  } finally {
    if (!closed) {
      res.end();
    }
  }
});

commandCenterRouter.post("/mcp", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }

  try {
    const context = await createCommandCenterMcpContext(req.jwt.sub);
    const result = await handleCommandCenterMcpRequest(req.body, context);
    if (!result) {
      return res.status(204).send();
    }
    return res.json(result);
  } catch (error) {
    return sendCommandCenterError(res, error);
  }
});
