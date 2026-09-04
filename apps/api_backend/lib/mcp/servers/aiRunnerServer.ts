import { Prisma } from "@prisma/client";
import {
  createAndDispatchAiRunnerJob,
  getAiRunnerJob,
  stopAiRunnerJob,
  waitForAiRunnerJob,
} from "../../commandCenterAiRunner";
import { prisma } from "../../prisma";
import type {
  CommandCenterMcpContext,
  CommandCenterMcpServer,
  CommandCenterMcpTool,
} from "../types";

const SERVER_NAME = "talos-ai-runner";
const SERVER_VERSION = "0.1.0";
const DEFAULT_WAIT_TIMEOUT_MS = 45_000;
const MAX_WAIT_TIMEOUT_MS = 90_000;

function readString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function readLimit(value: unknown, fallback: number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(MAX_WAIT_TIMEOUT_MS, Math.max(1_000, Math.trunc(parsed)));
}

function readSecretHandles(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const handles: string[] = [];
  for (const item of value) {
    const handle = typeof item === "string" ? item.trim() : "";
    if (!/^sec_[a-z0-9]{16}$/.test(handle)) {
      throw new Error("generatedSecretHandles must contain valid generated secret handles");
    }
    handles.push(handle);
  }
  return [...new Set(handles)];
}

function containsInsensitive(value: string) {
  return { contains: value, mode: "insensitive" as Prisma.QueryMode };
}

function normalizeSearch(value: string): string {
  return value.toLowerCase().replace(/\s+/g, " ").trim();
}

function isDeviceRoleShorthand(value: string): boolean {
  const normalized = normalizeSearch(value);
  return [
    "dc",
    "domain controller",
    "domain-controller",
    "the dc",
  ].includes(normalized);
}

async function resolveAgentId(
  context: CommandCenterMcpContext,
  args: Record<string, unknown>,
): Promise<
  | { ok: true; agentId: string; device: { agentId: string; hostname: string; customerName: string | null } }
  | { ok: false; message: string; candidates?: Array<{ agentId: string; hostname: string; customerName: string | null }> }
> {
  const explicitAgentId = readString(args.agentId);
  if (explicitAgentId) {
    const device = await prisma.rmmDevice.findFirst({
      where: {
        organizationId: context.organizationId,
        agentId: explicitAgentId,
      },
      include: { customer: { select: { name: true } } },
    });
    if (!device) {
      return { ok: false, message: "No device was found for that agent ID." };
    }
    return {
      ok: true,
      agentId: device.agentId,
      device: {
        agentId: device.agentId,
        hostname: device.hostname,
        customerName: device.customer?.name ?? null,
      },
    };
  }

  const query = readString(args.deviceQuery || args.hostname || args.device);
  if (!query) {
    return {
      ok: false,
      message: "A device name or agent ID is required before opening a secure desktop view.",
    };
  }
  const customerName = readString(args.customerName || args.customer);
  const devices = await prisma.rmmDevice.findMany({
    where: {
      organizationId: context.organizationId,
      OR: [
        { agentId: containsInsensitive(query) },
        { hostname: containsInsensitive(query) },
        { ip: containsInsensitive(query) },
      ],
      ...(customerName
        ? {
            customer: {
              name: containsInsensitive(customerName),
            },
          }
        : {}),
    },
    include: { customer: { select: { name: true } } },
    orderBy: { lastSeen: "desc" },
    take: 6,
  });

  const candidates = devices.map((device) => ({
    agentId: device.agentId,
    hostname: device.hostname,
    customerName: device.customer?.name ?? null,
  }));

  if (devices.length === 0) {
    if (customerName && isDeviceRoleShorthand(query)) {
      const fallbackDevices = await prisma.rmmDevice.findMany({
        where: {
          organizationId: context.organizationId,
          customer: {
            name: containsInsensitive(customerName),
          },
        },
        include: { customer: { select: { name: true } } },
        orderBy: { lastSeen: "desc" },
        take: 6,
      });
      const fallbackCandidates = fallbackDevices.map((device) => ({
        agentId: device.agentId,
        hostname: device.hostname,
        customerName: device.customer?.name ?? null,
      }));
      if (fallbackDevices.length === 1) {
        return {
          ok: true,
          agentId: fallbackDevices[0].agentId,
          device: fallbackCandidates[0],
        };
      }
      if (fallbackDevices.length > 1) {
        return {
          ok: false,
          message: "Multiple customer devices could match that shorthand. Ask which device to use.",
          candidates: fallbackCandidates,
        };
      }
    }
    return {
      ok: false,
      message: customerName
        ? "No matching device was found for that customer."
        : "No matching device was found. Ask for the customer or a more specific device name.",
    };
  }
  if (devices.length > 1) {
    return {
      ok: false,
      message: "Multiple matching devices were found. Ask which device to use.",
      candidates,
    };
  }

  return {
    ok: true,
    agentId: devices[0].agentId,
    device: candidates[0],
  };
}

const startAiRunnerJob: CommandCenterMcpTool = {
  definition: {
    name: "start_ai_runner_job",
    description:
      "Start a bounded Talos AI runner job for an organization-scoped device. Use shell_first for remediation/action goals and desktop_only only when the operator specifically needs visible GUI control.",
    inputSchema: {
      type: "object",
      additionalProperties: false,
      properties: {
        agentId: { type: "string", description: "Exact Talos agent ID when known." },
        deviceQuery: { type: "string", description: "Hostname, IP, or partial device identifier." },
        hostname: { type: "string", description: "Device hostname if provided by the operator." },
        customerName: { type: "string", description: "Customer/company name to disambiguate the device." },
        goal: { type: "string", description: "Brief operator goal for the runner job." },
        waitTimeoutMs: {
          type: "number",
          description: "Optional completion wait timeout in milliseconds.",
        },
        executionMode: {
          type: "string",
          enum: ["shell_first", "desktop_only"],
          description: "Use shell_first for normal action/remediation goals; use desktop_only for visible GUI-only goals.",
        },
        generatedSecretHandles: {
          type: "array",
          items: { type: "string" },
          description:
            "Secret handles returned by create_generated_secret that this runner job is allowed to use. Pass handles here; do not only mention shellReference values in the goal.",
        },
      },
    },
  },
  handler: async (args, context) => {
    const resolved = await resolveAgentId(context, args);
    if (!resolved.ok) {
      return {
        ok: false,
        needsClarification: true,
        message: resolved.message,
        candidates: resolved.candidates ?? [],
      };
    }

    await context.emitStatus?.({
      phase: "tool",
      message: "Starting desktop goal runner",
    });
    const job = await createAndDispatchAiRunnerJob(
      { organizationId: context.organizationId, userId: context.userId },
      {
        agentId: resolved.agentId,
        conversationId: context.conversationId ?? null,
        goal: readString(args.goal) || "Perform the requested desktop goal",
        requesterLabel: context.userEmail || "A Talos operator",
        requesterEmail: context.userEmail,
        organizationName: context.organizationName,
        jobType: readString(args.executionMode) === "desktop_only" ? "desktop_goal" : "shell_goal",
        generatedSecretHandles: readSecretHandles(args.generatedSecretHandles),
      },
    );

    if (job.status === "approval_pending") {
      await context.emitStatus?.({
        phase: "tool",
        message: "Waiting for endpoint approval",
      });
      return {
        ok: false,
        awaitingApproval: true,
        jobId: job.id,
        job,
        device: resolved.device,
        approvalExpiresAt: job.approvalExpiresAt,
        message: "Waiting for endpoint approval.",
      };
    }

    await context.emitStatus?.({
      phase: "tool",
      message: "Observing the desktop",
    });
	    const result = await waitForAiRunnerJob(
	      { organizationId: context.organizationId, userId: context.userId },
	      job.id,
	      readLimit(args.waitTimeoutMs, DEFAULT_WAIT_TIMEOUT_MS),
	      context.abortSignal,
	    );

    for (const attachment of result.attachments) {
      if (attachment.presentation === "live_frame") {
        continue;
      }
      context.addAttachment?.(attachment);
    }

    return {
      ok: result.job.status === "succeeded",
      job: result.job,
      device: resolved.device,
      timedOut: result.timedOut,
      artifacts: result.artifacts.map((artifact) => ({
        id: artifact.id,
        artifactType: artifact.artifactType,
        name: artifact.name,
        mimeType: artifact.mimeType,
        metadata: artifact.metadata,
      })),
      attachments: result.attachments,
      message:
        result.job.status === "succeeded"
          ? "Desktop goal completed."
          : result.job.status === "approval_denied"
            ? "The endpoint user denied the screen access request."
          : result.job.status === "approval_expired"
            ? "The endpoint approval request expired."
          : result.timedOut
            ? "The desktop goal is running."
            : result.job.error || "The runner job did not complete successfully.",
    };
  },
};

const getAiRunnerJobTool: CommandCenterMcpTool = {
  definition: {
    name: "get_ai_runner_job",
    description: "Get status and artifact references for an AI runner job owned by the current user.",
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["jobId"],
      properties: {
        jobId: { type: "string" },
      },
    },
  },
  handler: async (args, context) => {
    const jobId = readString(args.jobId);
    if (!jobId) {
      throw new Error("jobId is required");
    }
    const job = await getAiRunnerJob(
      { organizationId: context.organizationId, userId: context.userId },
      jobId,
    );
    if (!job) {
      return { ok: false, message: "AI runner job not found" };
    }
	    const result = await waitForAiRunnerJob(
	      { organizationId: context.organizationId, userId: context.userId },
	      jobId,
	      1_000,
	      context.abortSignal,
	    );
    for (const attachment of result.attachments) {
      context.addAttachment?.(attachment);
    }
    return { ok: true, job: result.job, timedOut: result.timedOut, attachments: result.attachments };
  },
};

const stopAiRunnerJobTool: CommandCenterMcpTool = {
  definition: {
    name: "stop_ai_runner_job",
    description: "Stop an AI runner job owned by the current user.",
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["jobId"],
      properties: {
        jobId: { type: "string" },
      },
    },
  },
  handler: async (args, context) => {
    const jobId = readString(args.jobId);
    if (!jobId) {
      throw new Error("jobId is required");
    }
    const job = await stopAiRunnerJob(
      { organizationId: context.organizationId, userId: context.userId },
      jobId,
    );
    if (!job) {
      return { ok: false, message: "AI runner job not found" };
    }
    return { ok: true, job };
  },
};

export function createAiRunnerMcpServer(): CommandCenterMcpServer {
  return {
    name: SERVER_NAME,
    version: SERVER_VERSION,
    tools: [startAiRunnerJob, getAiRunnerJobTool, stopAiRunnerJobTool],
  };
}
