import { describe, expect, test } from "bun:test";
import {
  acquireAiRunnerJobLease,
  appendAiRunnerArtifactFromCallback,
  appendAiRunnerEventFromCallback,
  approveAiRunnerCommandApproval,
  buildAiRunnerDeviceContextFromDevice,
  buildAiRunnerJobDispatchBody,
  createAiRunnerCommandApprovalFromCallback,
  createAndDispatchAiRunnerJob,
  denyAiRunnerCommandApprovalAndUseDesktopControl,
  getAiRunnerReplayManifest,
  heartbeatAiRunnerJobLease,
  listAiRunnerCommandOutputDeltas,
  readAiRunnerShellTranscript,
  reconcileExpiredAiRunnerJobLeases,
  releaseAiRunnerJobLease,
  stopAiRunnerJob,
  updateAiRunnerCommandApprovalExecutionFromCallback,
  updateAiRunnerJobStatusFromCallback,
} from "../lib/commandCenterAiRunner";
import { env } from "../lib/env";

const baseDate = new Date("2026-06-11T16:30:00.000Z");

function createMockDb(options: { aiRunnerAutoApprove?: boolean; approvalGrant?: any } = {}) {
  let artifactCounter = 0;
  let messageCounter = 0;
  const job = {
    id: "job-live",
    organizationId: "org-a",
    userId: "user-a",
    conversationId: "conversation-a",
    agentId: "agent-a",
    goal: "Test runner job",
    jobType: "desktop_goal",
    status: "running",
    runnerId: "runner-a",
    dispatchRequest: null,
    leaseId: null as string | null,
    leaseOwnerRunnerId: null as string | null,
    leaseExpiresAt: null as Date | null,
    lastHeartbeatAt: null as Date | null,
    cancelRequestedAt: null as Date | null,
    resumeAttempt: 0,
    retryable: false,
    retryReason: null as string | null,
    approvalId: null,
    approvalChatSessionId: null,
    approvalRequestedAt: null,
    approvalRespondedAt: null,
    approvalExpiresAt: null,
    approvalWindowExpiresAt: null,
    resultMessageId: null,
    liveFrameMessageId: null as string | null,
    result: null,
    error: null,
    createdAt: baseDate,
    updatedAt: baseDate,
    startedAt: baseDate,
    finishedAt: null,
  };
  const conversation = {
    id: "conversation-a",
    organizationId: "org-a",
    userId: "user-a",
    title: "Desktop goal",
    createdAt: baseDate,
    updatedAt: baseDate,
  };
  const artifacts: any[] = [];
  const messages: any[] = [];
  const commandApprovals: any[] = [];
  const events: any[] = [];
  const approvalGrants: any[] = options.approvalGrant ? [options.approvalGrant] : [];

  const selectRecord = (record: any, select?: Record<string, any>) => {
    if (!select) return record;
    const selected: Record<string, unknown> = {};
    for (const key of Object.keys(select)) {
      if (select[key]) selected[key] = record[key];
    }
    return selected;
  };

  const db = {
    state: { job, artifacts, messages, commandApprovals, events, approvalGrants },
    commandCenterAiRunnerJob: {
      create: async ({ data }: any) => {
        Object.assign(job, {
          id: data.id ?? "job-created",
          organizationId: data.organizationId,
          userId: data.userId,
          conversationId: data.conversationId ?? null,
          agentId: data.agentId,
          goal: data.goal ?? null,
          jobType: data.jobType ?? "desktop_goal",
          status: data.status,
          runnerId: null,
          dispatchRequest: data.dispatchRequest ?? null,
          leaseId: null,
          leaseOwnerRunnerId: null,
          leaseExpiresAt: null,
          lastHeartbeatAt: null,
          cancelRequestedAt: null,
          resumeAttempt: 0,
          retryable: false,
          retryReason: null,
          approvalId: data.approvalId ?? null,
          approvalChatSessionId: null,
          approvalRequestedAt: data.approvalRequestedAt ?? null,
          approvalRespondedAt: data.approvalRespondedAt ?? null,
          approvalExpiresAt: data.approvalExpiresAt ?? null,
          approvalWindowExpiresAt: data.approvalWindowExpiresAt ?? null,
          resultMessageId: null,
          liveFrameMessageId: null,
          result: null,
          error: null,
          createdAt: baseDate,
          updatedAt: baseDate,
          startedAt: null,
          finishedAt: null,
        });
        return job;
      },
      findFirst: async ({ where }: any) => {
        const matches =
          (!where?.id || where.id === job.id) &&
          (!where?.organizationId || where.organizationId === job.organizationId) &&
          (!where?.userId || where.userId === job.userId) &&
          (!where?.conversationId || where.conversationId === job.conversationId) &&
          (!where?.status?.in || where.status.in.includes(job.status));
        return matches ? job : null;
      },
      findUnique: async ({ where }: any) => (where.id === job.id ? job : null),
      findMany: async ({ where }: any) => {
        const matchesStatus = !where?.status?.in || where.status.in.includes(job.status);
        const matchesLease =
          !where?.leaseId ||
          (where.leaseId.not === null ? job.leaseId !== null : where.leaseId === job.leaseId);
        const matchesExpiry =
          !where?.leaseExpiresAt?.lte ||
          (job.leaseExpiresAt instanceof Date && job.leaseExpiresAt.getTime() <= where.leaseExpiresAt.lte.getTime());
        return matchesStatus && matchesLease && matchesExpiry ? [job] : [];
      },
      update: async ({ where, data }: any) => {
        if (where.id !== job.id) throw new Error("job not found");
        Object.assign(job, data, { updatedAt: baseDate });
        return job;
      },
      updateMany: async ({ where, data }: any) => {
        if (where.id !== job.id) return { count: 0 };
        if ("liveFrameMessageId" in where && job.liveFrameMessageId !== where.liveFrameMessageId) {
          return { count: 0 };
        }
        if (where.leaseId !== undefined && job.leaseId !== where.leaseId) return { count: 0 };
        if (where.leaseOwnerRunnerId !== undefined && job.leaseOwnerRunnerId !== where.leaseOwnerRunnerId) return { count: 0 };
        if (where.status?.in && !where.status.in.includes(job.status)) return { count: 0 };
        if (where.leaseExpiresAt?.gt) {
          if (!(job.leaseExpiresAt instanceof Date) || job.leaseExpiresAt.getTime() <= where.leaseExpiresAt.gt.getTime()) {
            return { count: 0 };
          }
        }
        if (where.OR) {
          const matchesAny = where.OR.some((clause: any) => {
            if (clause.leaseId === null) return job.leaseId === null;
            if (clause.leaseExpiresAt === null) return job.leaseExpiresAt === null;
            if (clause.leaseExpiresAt?.lte) {
              return job.leaseExpiresAt instanceof Date && job.leaseExpiresAt.getTime() <= clause.leaseExpiresAt.lte.getTime();
            }
            return false;
          });
          if (!matchesAny) return { count: 0 };
        }
        if (where.leaseExpiresAt?.lte) {
          if (!(job.leaseExpiresAt instanceof Date) || job.leaseExpiresAt.getTime() > where.leaseExpiresAt.lte.getTime()) {
            return { count: 0 };
          }
        }
        Object.assign(job, data, { updatedAt: baseDate });
        return { count: 1 };
      },
    },
    commandCenterAiRunnerArtifact: {
      create: async ({ data }: any) => {
        artifactCounter += 1;
        const record = {
          id: `artifact-${artifactCounter}`,
          ...data,
          createdAt: baseDate,
        };
        artifacts.push(record);
        return record;
      },
      findFirst: async ({ where }: any) =>
        artifacts.find((artifact) => {
          if (where.id && artifact.id !== where.id) return false;
          if (where.jobId && artifact.jobId !== where.jobId) return false;
          if (where.name && artifact.name !== where.name) return false;
          return true;
        }) ?? null,
      findMany: async () => artifacts,
    },
    commandCenterAiRunnerEvent: {
      create: async ({ data }: any) => {
        if (events.some((event) => event.jobId === data.jobId && event.eventKey === data.eventKey)) {
          const error = new Error("unique constraint");
          (error as any).code = "P2002";
          throw error;
        }
        const record = {
          id: `event-${events.length + 1}`,
          ...data,
          createdAt: baseDate,
        };
        events.push(record);
        return record;
      },
      findFirst: async ({ where }: any) =>
        events.find((event) => {
          if (where.id && event.id !== where.id) return false;
          if (where.jobId && event.jobId !== where.jobId) return false;
          if (where.eventKey && event.eventKey !== where.eventKey) return false;
          return true;
        }) ?? null,
      findMany: async ({ where }: any = {}) =>
        events.filter((event) => {
          if (where?.id?.in && !where.id.in.includes(event.id)) return false;
          if (where?.jobId && event.jobId !== where.jobId) return false;
          if (where?.eventType && event.eventType !== where.eventType) return false;
          if (where?.commandApprovalId && event.commandApprovalId !== where.commandApprovalId) return false;
          if (where?.organizationId && event.organizationId !== where.organizationId) return false;
          if (where?.userId && event.userId !== where.userId) return false;
          if (where?.conversationId && event.conversationId !== where.conversationId) return false;
          if (where?.createdAt?.gt && event.createdAt <= where.createdAt.gt) return false;
          return true;
        }),
      update: async ({ where, data }: any) => {
        const record = events.find((event) => event.id === where.id);
        if (!record) throw new Error("event not found");
        Object.assign(record, data);
        return record;
      },
      deleteMany: async ({ where }: any) => {
        const ids = new Set(where?.id?.in ?? []);
        const before = events.length;
        for (let index = events.length - 1; index >= 0; index -= 1) {
          if (ids.has(events[index].id)) events.splice(index, 1);
        }
        return { count: before - events.length };
      },
    },
    commandCenterConversation: {
      findFirst: async ({ where, select }: any) => {
        const matches =
          conversation.id === where.id &&
          conversation.organizationId === where.organizationId &&
          conversation.userId === where.userId;
        return matches ? selectRecord(conversation, select) : null;
      },
      update: async ({ where, data }: any) => {
        if (where.id !== conversation.id) throw new Error("conversation not found");
        Object.assign(conversation, data);
        return conversation;
      },
    },
    commandCenterMessage: {
      create: async ({ data, select }: any) => {
        messageCounter += 1;
        const record = {
          id: `message-${messageCounter}`,
          ...data,
          createdAt: baseDate,
        };
        messages.push(record);
        return selectRecord(record, select);
      },
      updateMany: async ({ where, data }: any) => {
        const message = messages.find((candidate) => {
          if (candidate.id !== where.id) return false;
          return !where.conversationId || candidate.conversationId === where.conversationId;
        });
        if (!message) return { count: 0 };
        Object.assign(message, data);
        return { count: 1 };
      },
    },
    commandCenterAiRunnerCommandApproval: {
      create: async ({ data }: any) => {
        const record = {
          id: `approval-${commandApprovals.length + 1}`,
          ...data,
          createdAt: baseDate,
          updatedAt: baseDate,
          executedAt: null,
        };
        commandApprovals.push(record);
        return record;
      },
      findFirst: async ({ where }: any) =>
        commandApprovals.find((approval) => {
          if (where.id && approval.id !== where.id) return false;
          if (where.jobId && approval.jobId !== where.jobId) return false;
          if (where.turnIndex !== undefined && approval.turnIndex !== where.turnIndex) return false;
          if (where.organizationId && approval.organizationId !== where.organizationId) return false;
          if (where.userId && approval.userId !== where.userId) return false;
          return true;
        }) ?? null,
      findMany: async ({ where }: any = {}) =>
        commandApprovals.filter((approval) => {
          if (where?.jobId && approval.jobId !== where.jobId) return false;
          if (where?.organizationId && approval.organizationId !== where.organizationId) return false;
          if (where?.userId && approval.userId !== where.userId) return false;
          if (where?.status?.in && !where.status.in.includes(approval.status)) return false;
          return true;
        }),
      update: async ({ where, data }: any) => {
        const record = commandApprovals.find((approval) => approval.id === where.id);
        if (!record) throw new Error("approval not found");
        Object.assign(record, data, { updatedAt: baseDate });
        return record;
      },
    },
    commandCenterAiRunnerApprovalGrant: {
      findFirst: async ({ where }: any) =>
        approvalGrants.find((grant) => {
          if (where.organizationId && grant.organizationId !== where.organizationId) return false;
          if (where.userId && grant.userId !== where.userId) return false;
          if (where.agentId && grant.agentId !== where.agentId) return false;
          if (where.jobType && grant.jobType !== where.jobType) return false;
          if (where.expiresAt?.gt && !(grant.expiresAt > where.expiresAt.gt)) return false;
          return true;
        }) ?? null,
      create: async ({ data }: any) => {
        const grant = { id: `grant-${approvalGrants.length + 1}`, createdAt: baseDate, ...data };
        approvalGrants.push(grant);
        return grant;
      },
    },
    commandPolicy: {
      findMany: async () => [
        {
          id: BigInt(10),
          scopeType: "organization",
          policyType: "allow",
          allowedParameters: [],
          reason: null,
        },
      ],
    },
    organizationMember: {
      findFirst: async () => ({ role: "AGENT_ADMIN" }),
    },
    rmmDevice: {
      findFirst: async ({ where }: any = {}) => {
        if (where?.agentId && where.agentId !== "agent-a") return null;
        if (where?.organizationId && where.organizationId !== "org-a") return null;
        return {
          agentId: "agent-a",
          hostname: "win-ops-1",
          os: "Windows",
          ip: "10.0.0.10",
          version: "0.6.67",
          lastSeen: baseDate,
          aiRunnerAutoApprove: Boolean(options.aiRunnerAutoApprove),
          customerId: null,
          customer: null,
          site: null,
          telemetryState: null,
        };
      },
      findMany: async () => [
        {
          agentId: "agent-a",
          hostname: "win-ops-1",
          customer: { name: "Acme" },
        },
      ],
    },
  };

  return db;
}

describe("Command Center AI runner device context", () => {
  test("builds a compact core and security context from telemetry", () => {
    const context = buildAiRunnerDeviceContextFromDevice(
      {
        agentId: "agent-win",
        hostname: "win-ops-1",
        os: "Windows",
        ip: "10.0.0.5",
        version: "0.6.66",
        lastSeen: new Date("2026-06-15T09:55:00.000Z"),
        customer: { name: "Acme" },
        site: { name: "London" },
        telemetryState: {
          collectedAt: new Date("2026-06-15T10:00:00.000Z"),
          hostname: "win-ops-1",
          osName: "Windows 11 Pro",
          osVersion: "23H2",
          agentVersion: "0.6.67",
          cpuModel: "Intel Core i7",
          cpuPhysicalCores: 8,
          cpuLogicalCores: 16,
          memoryTotalBytes: BigInt(17179869184),
          pendingUpdatesCount: 3,
          rebootRequired: true,
          inventoryData: {
            collection: {
              operating_system: {
                system: {
                  architecture: "x64",
                  timezone: "Europe/London",
                  locale: "en-GB",
                  domain: "ACME.LOCAL",
                  os: {
                    serial_number: "do-not-send",
                  },
                },
              },
              hardware: {
                secure_boot: true,
                tpm: { present: true, enabled: true },
                network_adapters: [{ mac_address: "00:11:22:33:44:55" }],
              },
              security: {
                firewall: { enabled: { domain: true, private: true, public: false } },
                antivirus: { windows_defender: { enabled: true } },
                bitlocker: { enabled: false },
                local_users: [{ username: "secret-user" }],
              },
              software: {
                installed_apps: [{ app_name: "Private App" }],
              },
            },
          },
        },
      },
      new Date("2026-06-15T10:10:00.000Z"),
    );

    expect(context).toMatchObject({
      agentId: "agent-win",
      hostname: "win-ops-1",
      customerName: "Acme",
      siteName: "London",
      snapshot: {
        collectedAt: "2026-06-15T10:00:00.000Z",
        ageSeconds: 600,
      },
      platform: {
        family: "windows",
        osName: "Windows 11 Pro",
        osVersion: "23H2",
        architecture: "x64",
        timezone: "Europe/London",
        locale: "en-GB",
        domain: "ACME.LOCAL",
      },
      agent: {
        version: "0.6.67",
        lastSeen: "2026-06-15T09:55:00.000Z",
      },
      hardware: {
        cpuModel: "Intel Core i7",
        physicalCores: 8,
        logicalCores: 16,
        memoryTotalBytes: 17179869184,
      },
      state: {
        pendingUpdatesCount: 3,
        rebootRequired: true,
      },
      network: {
        primaryIp: "10.0.0.5",
      },
      shell: {
        runAs: "system",
        account: "NT AUTHORITY\\SYSTEM",
        elevated: true,
        description: "AI shell commands run as the local Windows SYSTEM account, not the signed-in user.",
      },
      security: {
        firewallEnabled: true,
        secureBoot: true,
        tpmPresent: true,
        tpmEnabled: true,
        antivirusEnabled: true,
        bitlockerEnabled: false,
      },
    });
    const serialized = JSON.stringify(context);
    expect(serialized).not.toContain("do-not-send");
    expect(serialized).not.toContain("00:11:22:33:44:55");
    expect(serialized).not.toContain("secret-user");
    expect(serialized).not.toContain("Private App");
  });

  test("falls back to device identity when telemetry is absent", () => {
    const context = buildAiRunnerDeviceContextFromDevice(
      {
        agentId: "agent-linux",
        hostname: "linux-1",
        os: "Ubuntu Linux",
        ip: "10.0.0.9",
        version: "0.6.66",
        lastSeen: new Date("2026-06-15T09:55:00.000Z"),
        customer: null,
        site: null,
        telemetryState: null,
      },
      new Date("2026-06-15T10:10:00.000Z"),
    );

    expect(context).toMatchObject({
      agentId: "agent-linux",
      hostname: "linux-1",
      snapshot: { collectedAt: null, ageSeconds: null },
      platform: { family: "linux", osName: "Ubuntu Linux" },
      agent: { version: "0.6.66" },
      network: { primaryIp: "10.0.0.9" },
      shell: {
        runAs: "configured_user",
        account: null,
        elevated: false,
        description: "AI shell commands run as a configured Linux shell user, not root unless explicitly configured.",
      },
      security: {
        firewallEnabled: null,
        secureBoot: null,
        tpmPresent: null,
        tpmEnabled: null,
        antivirusEnabled: null,
        bitlockerEnabled: null,
      },
    });
  });

  test("prefers meaningful inventory values over placeholders", () => {
    const context = buildAiRunnerDeviceContextFromDevice({
      agentId: "agent-linux",
      hostname: "unknown",
      os: "unknown",
      ip: "0.0.0.0",
      version: "0.6.66",
      lastSeen: baseDate,
      customer: null,
      site: null,
      telemetryState: {
        collectedAt: baseDate,
        inventoryData: {
          collection: {
            operating_system: {
              system: {
                hostname: "linux-real",
                name: "Ubuntu 24.04 LTS",
                version: "24.04",
                architecture: "arm64",
              },
            },
            network: {
              adapters: [
                {
                  ips: [
                    { address: "127.0.0.1" },
                    { address: "192.168.1.50" },
                  ],
                },
              ],
            },
          },
        },
      },
    });

    expect(context.hostname).toBe("linux-real");
    expect(context.platform).toMatchObject({
      family: "linux",
      osName: "Ubuntu 24.04 LTS",
      osVersion: "24.04",
      architecture: "arm64",
    });
    expect(context.network.primaryIp).toBe("192.168.1.50");
    expect(context.shell).toMatchObject({
      runAs: "configured_user",
      account: null,
      elevated: false,
    });
  });

  test("describes macOS shell assist as root LaunchDaemon context", () => {
    const context = buildAiRunnerDeviceContextFromDevice({
      agentId: "agent-mac",
      hostname: "mac-1",
      os: "macOS",
      ip: "10.0.0.12",
      version: "0.6.66",
      lastSeen: baseDate,
      customer: null,
      site: null,
      telemetryState: null,
    });

    expect(context.shell).toEqual({
      runAs: "root",
      account: "root",
      elevated: true,
      description: "AI shell commands run as root from the Talos LaunchDaemon context, not the console user.",
    });
  });

  test("includes device context in the runner dispatch body without persistence fields", () => {
    const deviceContext = buildAiRunnerDeviceContextFromDevice({
      agentId: "agent-a",
      hostname: "host-a",
      os: "Windows",
      ip: "10.0.0.10",
      version: "0.6.66",
      lastSeen: baseDate,
      customer: null,
      site: null,
      telemetryState: null,
    });

    const body = buildAiRunnerJobDispatchBody(
      { organizationId: "org-a", userId: "user-a" },
      { id: "job-a", conversationId: "conversation-a", agentId: "agent-a" },
      {
        goal: "check updates",
        jobType: "shell_goal",
        deviceContext,
        generatedSecrets: [
          {
            secretHandle: "sec_a1b2c3d4e5f6g7h8",
            shellReference: "$__talos_secret_f6g7h8",
            desktopReference: null,
            secureNoteUrl: "/SN/a1b2c3d4",
            expiresAt: "2026-06-18T10:00:00.000Z",
            purpose: "Temporary password",
          },
        ],
        approvalMode: "already_granted",
        approval: null,
        callbackBaseUrl: "https://api.example.test",
      },
    );

    expect(body.deviceContext).toEqual(deviceContext);
    expect(body).toMatchObject({
      jobId: "job-a",
      organizationId: "org-a",
      userId: "user-a",
      conversationId: "conversation-a",
      agentId: "agent-a",
      jobType: "shell_goal",
      callbackBaseUrl: "https://api.example.test",
      generatedSecrets: [
        {
          secretHandle: "sec_a1b2c3d4e5f6g7h8",
          shellReference: "$__talos_secret_f6g7h8",
          desktopReference: null,
          secureNoteUrl: "/SN/a1b2c3d4",
          expiresAt: "2026-06-18T10:00:00.000Z",
          purpose: "Temporary password",
        },
      ],
    });
    expect(body).not.toHaveProperty("deviceContextJsonb");
  });
});

describe("Command Center AI runner endpoint approval dispatch", () => {
  test("device auto-approval dispatches with endpoint approval already granted while preserving command approvals", async () => {
    const previousRunnerUrl = env.aiRunnerUrl;
    const previousRunnerKey = env.aiRunnerServiceKey;
    const previousCallbackBaseUrl = env.aiRunnerCallbackBaseUrl;
    const previousFetch = globalThis.fetch;
    const dispatches: Array<{ url: string; body: any; serviceKey: string | null }> = [];
    const db = createMockDb({ aiRunnerAutoApprove: true });

    (env as any).aiRunnerUrl = "https://runner.example.test";
    (env as any).aiRunnerServiceKey = "runner-key";
    (env as any).aiRunnerCallbackBaseUrl = "https://api.example.test";
    (globalThis as any).fetch = async (url: string, init?: RequestInit) => {
      dispatches.push({
        url,
        body: JSON.parse(String(init?.body ?? "{}")),
        serviceKey: init?.headers instanceof Headers
          ? init.headers.get("x-service-key")
          : ((init?.headers as Record<string, string> | undefined)?.["x-service-key"] ?? null),
      });
      return new Response(JSON.stringify({ runnerId: "runner-auto" }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    };

    try {
      const job = await createAndDispatchAiRunnerJob(
        { organizationId: "org-a", userId: "user-a" },
        {
          agentId: "agent-a",
          conversationId: "conversation-a",
          jobType: "shell_goal",
          goal: "Install net-tools",
          requesterLabel: "Talos Admin",
          requesterEmail: "admin@example.test",
          organizationName: "Contoso Ltd.",
        },
        db as any,
      );

      expect(job.status).toBe("running");
      expect(job.approvalId).toBeNull();
      expect(db.state.job.approvalRequestedAt).toBeNull();
      expect(db.state.job.approvalRespondedAt).toBeInstanceOf(Date);
      expect(db.state.approvalGrants).toHaveLength(0);
      expect(dispatches).toHaveLength(1);
      expect(dispatches[0]).toMatchObject({
        url: "https://runner.example.test/internal/jobs",
        serviceKey: "runner-key",
      });
      expect(dispatches[0].body).toMatchObject({
        organizationId: "org-a",
        userId: "user-a",
        agentId: "agent-a",
        jobType: "shell_goal",
        approvalMode: "already_granted",
        approval: null,
      });

      const commandApproval = await createAiRunnerCommandApprovalFromCallback(
        db.state.job.id,
        {
          turnIndex: 0,
          command: "apt-get install -y net-tools",
          explanation: "Install the package that provides ifconfig.",
          risk: "Package installation changes the endpoint.",
          notes: [],
          message: "Review this command.",
        },
        db as any,
      );
      expect(commandApproval?.status).toBe("pending");
      expect(db.state.messages[0].content).toContain("Command approval requested.");
    } finally {
      (env as any).aiRunnerUrl = previousRunnerUrl;
      (env as any).aiRunnerServiceKey = previousRunnerKey;
      (env as any).aiRunnerCallbackBaseUrl = previousCallbackBaseUrl;
      (globalThis as any).fetch = previousFetch;
    }
  });
});

describe("Command Center AI runner live frames", () => {
  test("creates one live-frame message and updates it for later screenshots", async () => {
    const db = createMockDb();

    await appendAiRunnerArtifactFromCallback(
      "job-live",
      {
        artifactType: "runner-screenshot",
        name: "desktop-goal-frame-1.png",
        mimeType: "image/png",
        contentBase64: "abc",
        appendToChat: true,
        chatPresentation: "live_frame",
        messageContent: "Observed the current desktop.",
        metadata: {
          width: 1024,
          height: 768,
          frameSeq: 1,
          cursor: { visible: false, width: 1024, height: 768 },
        },
      },
      db as any,
    );

    expect(db.state.artifacts).toHaveLength(1);
    expect(db.state.messages).toHaveLength(1);
    expect(db.state.job.liveFrameMessageId).toBe("message-1");
    expect(db.state.job.resultMessageId).toBeNull();
    expect(db.state.messages[0].metadata.attachments[0]).toMatchObject({
      artifactId: "artifact-1",
      presentation: "live_frame",
      frameSeq: 1,
      cursor: { visible: false, width: 1024, height: 768 },
    });

    await appendAiRunnerArtifactFromCallback(
      "job-live",
      {
        artifactType: "runner-screenshot",
        name: "desktop-goal-frame-2.png",
        mimeType: "image/png",
        contentBase64: "def",
        appendToChat: true,
        chatPresentation: "live_frame",
        messageContent: "Observed the updated desktop.",
        metadata: {
          width: 1024,
          height: 768,
          frameSeq: 2,
          cursor: { visible: true, x: 120, y: 80, width: 1024, height: 768 },
        },
      },
      db as any,
    );

    expect(db.state.artifacts).toHaveLength(2);
    expect(db.state.messages).toHaveLength(1);
    expect(db.state.messages[0].content).toBe("Observed the updated desktop.");
    expect(db.state.messages[0].metadata.attachments[0]).toMatchObject({
      artifactId: "artifact-2",
      presentation: "live_frame",
      frameSeq: 2,
      cursor: { visible: true, x: 120, y: 80, width: 1024, height: 768 },
    });
  });
});

describe("Command Center AI runner evidence artifacts", () => {
  test("assembles shell transcript chunks in sequence order", async () => {
    const db = createMockDb();
    db.state.artifacts.push(
      {
        id: "artifact-part-2",
        jobId: "job-live",
        organizationId: "org-a",
        userId: "user-a",
        artifactType: "runner-shell-transcript",
        name: "shell-transcript-job-live-part-2-of-2.txt",
        mimeType: "text/plain; charset=utf-8",
        contentBase64: Buffer.from("second").toString("base64"),
        metadata: { sequence: 2, totalChunks: 2 },
        createdAt: new Date("2026-06-11T16:30:02.000Z"),
      },
      {
        id: "artifact-part-1",
        jobId: "job-live",
        organizationId: "org-a",
        userId: "user-a",
        artifactType: "runner-shell-transcript",
        name: "shell-transcript-job-live-part-1-of-2.txt",
        mimeType: "text/plain; charset=utf-8",
        contentBase64: Buffer.from("first ").toString("base64"),
        metadata: { sequence: 1, totalChunks: 2 },
        createdAt: new Date("2026-06-11T16:30:01.000Z"),
      },
    );

    const transcript = await readAiRunnerShellTranscript(
      { organizationId: "org-a", userId: "user-a" },
      "job-live",
      db as any,
    );

    expect(transcript?.name).toBe("shell-transcript-job-live.txt");
    expect(transcript?.buffer.toString("utf8")).toBe("first second");
  });

  test("does not expose incomplete shell transcript chunk sets", async () => {
    const db = createMockDb();
    db.state.artifacts.push({
      id: "artifact-part-1",
      jobId: "job-live",
      organizationId: "org-a",
      userId: "user-a",
      artifactType: "runner-shell-transcript",
      name: "shell-transcript-job-live-part-1-of-2.txt",
      mimeType: "text/plain; charset=utf-8",
      contentBase64: Buffer.from("first ").toString("base64"),
      metadata: { sequence: 1, totalChunks: 2 },
      createdAt: baseDate,
    });

    const transcript = await readAiRunnerShellTranscript(
      { organizationId: "org-a", userId: "user-a" },
      "job-live",
      db as any,
    );

    expect(transcript).toBeNull();

    await updateAiRunnerJobStatusFromCallback(
      "job-live",
      {
        status: "failed",
        runnerId: "runner-a",
        error: "Runner failed with an incomplete transcript.",
      },
      db as any,
    );

    expect(db.state.messages).toHaveLength(0);
    expect(db.state.job.resultMessageId).toBeNull();
  });

  test("final result message explains approval unavailable without evidence", async () => {
    const db = createMockDb();

    const updated = await updateAiRunnerJobStatusFromCallback(
      "job-live",
      {
        status: "failed",
        runnerId: "runner-a",
        result: {
          phase: "approval_unavailable",
          reason: "no_interactive_user",
          message:
            "Endpoint approval could not be requested because no user is currently logged in on this device. Ask someone to sign in, then retry.",
        },
        error:
          "Endpoint approval could not be requested because no user is currently logged in on this device. Ask someone to sign in, then retry.",
      },
      db as any,
    );

    expect(updated?.status).toBe("failed");
    expect(db.state.job.resultMessageId).toBe("message-1");
    expect(db.state.messages).toHaveLength(1);
    expect(db.state.messages[0].content).toContain("no user is currently logged in");
    expect(db.state.messages[0].content).not.toContain("read chat approval relay payload");
    expect(db.state.messages[0].metadata.aiRunnerJob).toMatchObject({
      jobId: "job-live",
      status: "failed",
      shellTranscriptAvailable: false,
      desktopReplayAvailable: false,
      replayFrameCount: 0,
    });
  });

  test("builds replay manifest from live desktop frames and event display text", async () => {
    const db = createMockDb();

    await appendAiRunnerArtifactFromCallback(
      "job-live",
      {
        artifactType: "runner-screenshot",
        name: "desktop-goal-frame-2.png",
        mimeType: "image/png",
        contentBase64: "def",
        appendToChat: true,
        chatPresentation: "live_frame",
        messageContent: "Clicked the lower left corner.",
        metadata: {
          source: "live_vp8_relay_stream",
          width: 1024,
          height: 768,
          frameSeq: 2,
          stepIndex: 1,
          taskId: "task-a",
          cursor: { visible: true, x: 120, y: 80, width: 1024, height: 768 },
        },
      },
      db as any,
    );
    await appendAiRunnerArtifactFromCallback(
      "job-live",
      {
        artifactType: "runner-screenshot",
        name: "desktop-goal-frame-1.png",
        mimeType: "image/png",
        contentBase64: "abc",
        appendToChat: true,
        chatPresentation: "live_frame",
        messageContent: "Inspecting the desktop.",
        metadata: {
          source: "live_vp8_relay_stream",
          width: 1024,
          height: 768,
          frameSeq: 1,
          displayText: "Inspecting the desktop.",
          cursor: { visible: false, width: 1024, height: 768 },
        },
      },
      db as any,
    );

    const manifest = await getAiRunnerReplayManifest(
      { organizationId: "org-a", userId: "user-a" },
      "job-live",
      db as any,
    );

    expect(manifest?.deviceLabel).toBe("win-ops-1 (Acme)");
    expect(manifest?.defaultDelayMs).toBe(1000);
    expect(manifest?.frames.map((frame) => frame.frameSeq)).toEqual([1, 2]);
    expect(manifest?.frames[0].displayText).toBe("Inspecting the desktop.");
    expect(manifest?.frames[1]).toMatchObject({
      artifactId: "artifact-1",
      displayText: "Clicked the lower left corner.",
      taskId: "task-a",
      stepIndex: 1,
      cursor: { visible: true, x: 120, y: 80, width: 1024, height: 768 },
    });
  });

  test("final result message advertises evidence on non-success terminal jobs", async () => {
    const db = createMockDb();
    db.state.artifacts.push(
      {
        id: "artifact-transcript",
        jobId: "job-live",
        organizationId: "org-a",
        userId: "user-a",
        artifactType: "runner-shell-transcript",
        name: "shell-transcript-job-live.txt",
        mimeType: "text/plain; charset=utf-8",
        contentBase64: Buffer.from("transcript").toString("base64"),
        metadata: { sequence: 1, totalChunks: 1 },
        createdAt: baseDate,
      },
      {
        id: "artifact-frame",
        jobId: "job-live",
        organizationId: "org-a",
        userId: "user-a",
        artifactType: "runner-screenshot",
        name: "desktop-goal-frame-1.png",
        mimeType: "image/png",
        contentBase64: "abc",
        metadata: {
          source: "live_vp8_relay_stream",
          width: 1024,
          height: 768,
          frameSeq: 1,
          displayText: "Inspecting the desktop.",
        },
        createdAt: baseDate,
      },
    );

    const updated = await updateAiRunnerJobStatusFromCallback(
      "job-live",
      {
        status: "failed",
        runnerId: "runner-a",
        error: "Runner failed after collecting evidence.",
      },
      db as any,
    );

    expect(updated?.status).toBe("failed");
    expect(db.state.job.resultMessageId).toBe("message-1");
    expect(db.state.messages).toHaveLength(1);
    expect(db.state.messages[0].content).toContain("Runner failed after collecting evidence.");
    expect(db.state.messages[0].metadata.aiRunnerJob).toMatchObject({
      jobId: "job-live",
      status: "failed",
      shellTranscriptAvailable: true,
      desktopReplayAvailable: true,
      replayFrameCount: 1,
    });
  });
});

describe("Command Center AI runner command approvals", () => {
  test("allows arbitrary command text when risk and reasoning are supplied", async () => {
    const db = createMockDb();

    const created = await createAiRunnerCommandApprovalFromCallback(
      "job-live",
      {
        turnIndex: 0,
        command:
          "$ErrorActionPreference='Stop'; Import-Module ActiveDirectory; Get-ADUser -Filter \"Name -eq 'Leeroy Jenkins'\" | Select-Object Name,SamAccountName",
        explanation: "Identify the AD account before attempting a password reset.",
        risk: "Read-only AD query; no changes are made.",
        notes: ["If this is a local account, query local users next."],
        message: "Review this command.",
        modelResponseId: "resp-arbitrary",
      },
      db as any,
    );

    expect(created?.status).toBe("pending");
    expect(created?.policyAllowed).toBeNull();
    expect(created?.policyReason).toBeNull();
    expect(db.state.messages[0].content).toContain("Command approval requested.");
    expect(db.state.messages[0].content).toContain("Import-Module ActiveDirectory; Get-ADUser");

    const approved = await approveAiRunnerCommandApproval(
      { organizationId: "org-a", userId: "user-a" },
      "approval-1",
      db as any,
    );
    expect(approved?.status).toBe("approved");
    expect(approved?.policyAllowed).toBeNull();
    expect(approved?.policyReason).toBeNull();
  });

  test("creates, approves, and updates a command approval message", async () => {
    const db = createMockDb();

    const created = await createAiRunnerCommandApprovalFromCallback(
      "job-live",
      {
        turnIndex: 0,
        command: "Get-Service W32Time",
        explanation: "Checks the Windows Time service state.",
        risk: "Read-only service inspection.",
        notes: ["No changes are made."],
        message: "Review this command.",
        modelResponseId: "resp-a",
      },
      db as any,
    );

    expect(created?.status).toBe("pending");
    expect(db.state.commandApprovals).toHaveLength(1);
    expect(db.state.messages).toHaveLength(1);
    expect(db.state.messages[0].metadata.commandApproval).toMatchObject({
      id: "approval-1",
      status: "pending",
      command: "Get-Service W32Time",
    });

    const approved = await approveAiRunnerCommandApproval(
      { organizationId: "org-a", userId: "user-a" },
      "approval-1",
      db as any,
    );
    expect(approved?.status).toBe("approved");

    const executed = await updateAiRunnerCommandApprovalExecutionFromCallback(
      "job-live",
      "approval-1",
      {
        status: "executed",
        output: "Status   Name\nRunning  W32Time",
      },
      db as any,
    );
    expect(executed?.status).toBe("executed");
    expect(db.state.messages[0].metadata.commandApproval).toMatchObject({
      status: "executed",
      output: "Status   Name\nRunning  W32Time",
    });
    expect(db.state.messages[0].content).toContain("Command completed.");
    expect(db.state.messages[0].content).not.toContain("non-zero exit code");
  });

  test("transfers a shell command approval into desktop control on the same job", async () => {
    const db = createMockDb();
    db.state.job.jobType = "shell_goal";

    await createAiRunnerCommandApprovalFromCallback(
      "job-live",
      {
        turnIndex: 0,
        command: "Install-Module Example",
        explanation: "Install the requested module from the shell.",
        risk: "Package installation changes the endpoint.",
        notes: [],
        message: "Review this command.",
      },
      db as any,
    );

    const transferred = await denyAiRunnerCommandApprovalAndUseDesktopControl(
      { organizationId: "org-a", userId: "user-a" },
      "approval-1",
      db as any,
    );

    expect(transferred?.approval.status).toBe("desktop_control_requested");
    expect(transferred?.job?.id).toBe("job-live");
    expect(transferred?.job?.jobType).toBe("desktop_goal");
    expect(db.state.job.status).toBe("running");
    expect(db.state.commandApprovals[0].status).toBe("desktop_control_requested");
    expect(db.state.messages[0].content).toContain("Desktop control requested.");
    expect(db.state.events.some((event) => event.eventType === "desktop_control_requested")).toBe(true);
  });

  test("records command output delta events for stream snapshots", async () => {
    const db = createMockDb();

    await createAiRunnerCommandApprovalFromCallback(
      "job-live",
      {
        turnIndex: 0,
        command: "brew install example",
        explanation: "Install the requested package.",
        risk: "Package installation changes the endpoint.",
        notes: [],
        message: "Review this command.",
      },
      db as any,
    );

    const laterEvent = await appendAiRunnerEventFromCallback(
      "job-live",
      {
        eventKey: "command_output:approval-1:0000000001",
        eventType: "command_output_delta",
        runnerId: "runner-a",
        commandApprovalId: "approval-1",
        turnIndex: 0,
        payload: {
          jobId: "job-live",
          approvalId: "approval-1",
          turnIndex: 0,
          sequence: 1,
          text: "Installing...\n",
          outputOffset: 15,
          terminal: false,
        },
      },
      db as any,
    );
    const firstEvent = await appendAiRunnerEventFromCallback(
      "job-live",
      {
        eventKey: "command_output:approval-1:0000000000",
        eventType: "command_output_delta",
        runnerId: "runner-a",
        commandApprovalId: "approval-1",
        turnIndex: 0,
        payload: {
          jobId: "job-live",
          approvalId: "approval-1",
          turnIndex: 0,
          sequence: 0,
          text: "Downloading...\n",
          outputOffset: 0,
          terminal: false,
        },
      },
      db as any,
    );

    expect(laterEvent?.eventType).toBe("command_output_delta");
    expect(firstEvent?.eventType).toBe("command_output_delta");
    const deltas = await listAiRunnerCommandOutputDeltas(
      { organizationId: "org-a", userId: "user-a" },
      "conversation-a",
      {},
      db as any,
    );
    expect(deltas).toEqual([
      {
        eventId: "event-3",
        jobId: "job-live",
        approvalId: "approval-1",
        turnIndex: 0,
        sequence: 0,
        text: "Downloading...\n",
        outputOffset: 0,
        terminal: false,
        createdAt: baseDate.toISOString(),
      },
      {
        eventId: "event-2",
        jobId: "job-live",
        approvalId: "approval-1",
        turnIndex: 0,
        sequence: 1,
        text: "Installing...\n",
        outputOffset: 15,
        terminal: false,
        createdAt: baseDate.toISOString(),
      },
    ]);
  });
});

describe("Command Center AI runner stop handling", () => {
  test("records cancellation, moves active jobs to stopping, and settles pending command approvals", async () => {
    const db = createMockDb();
    const previousRunnerUrl = env.aiRunnerUrl;
    const previousRunnerKey = env.aiRunnerServiceKey;
    const previousFetch = globalThis.fetch;
    const stopCalls: Array<{ url: string; method?: string; serviceKey?: string | null }> = [];
    (env as any).aiRunnerUrl = "https://runner.example.test";
    (env as any).aiRunnerServiceKey = "runner-key";
    (globalThis as any).fetch = async (url: RequestInfo | URL, init?: RequestInit) => {
      stopCalls.push({
        url: String(url),
        method: init?.method,
        serviceKey: init?.headers instanceof Headers ? init.headers.get("x-service-key") : (init?.headers as any)?.["x-service-key"],
      });
      return new Response("", { status: 202 });
    };

    try {
      await createAiRunnerCommandApprovalFromCallback(
        "job-live",
        {
          turnIndex: 0,
          command: "Restart-Service W32Time",
          explanation: "Restart the Windows Time service.",
          risk: "Service restart.",
          notes: [],
          message: "Review this command.",
        },
        db as any,
      );

      const stopped = await stopAiRunnerJob(
        { organizationId: "org-a", userId: "user-a" },
        "job-live",
        db as any,
      );

      expect(stopped?.status).toBe("stopping");
      expect(db.state.job.status).toBe("stopping");
      expect(db.state.job.cancelRequestedAt).toBeInstanceOf(Date);
      expect(db.state.commandApprovals[0].status).toBe("denied");
      expect(db.state.messages[0].metadata.commandApproval.status).toBe("denied");
      expect(db.state.events.some((event) => event.eventType === "stop_requested")).toBe(true);
      expect(stopCalls).toEqual([
        {
          url: "https://runner.example.test/internal/jobs/job-live/stop",
          method: "POST",
          serviceKey: "runner-key",
        },
      ]);
    } finally {
      (globalThis as any).fetch = previousFetch;
      (env as any).aiRunnerUrl = previousRunnerUrl;
      (env as any).aiRunnerServiceKey = previousRunnerKey;
    }
  });

  test("does not stop jobs outside the requesting organization or user scope", async () => {
    const db = createMockDb();

    const wrongOrg = await stopAiRunnerJob(
      { organizationId: "org-b", userId: "user-a" },
      "job-live",
      db as any,
    );
    const wrongUser = await stopAiRunnerJob(
      { organizationId: "org-a", userId: "user-b" },
      "job-live",
      db as any,
    );

    expect(wrongOrg).toBeNull();
    expect(wrongUser).toBeNull();
    expect(db.state.job.status).toBe("running");
    expect(db.state.job.cancelRequestedAt).toBeNull();
    expect(db.state.events.some((event) => event.eventType === "stop_requested")).toBe(false);
  });

  test("leaves terminal jobs unchanged and does not send a runner stop request", async () => {
    const db = createMockDb();
    const previousFetch = globalThis.fetch;
    let fetchCalled = false;
    db.state.job.status = "succeeded";
    db.state.job.finishedAt = new Date("2026-06-15T12:00:00.000Z");
    (globalThis as any).fetch = async () => {
      fetchCalled = true;
      return new Response("", { status: 202 });
    };

    try {
      const stopped = await stopAiRunnerJob(
        { organizationId: "org-a", userId: "user-a" },
        "job-live",
        db as any,
      );

      expect(stopped?.status).toBe("succeeded");
      expect(db.state.job.status).toBe("succeeded");
      expect(db.state.job.cancelRequestedAt).toBeNull();
      expect(fetchCalled).toBe(false);
      expect(db.state.events.some((event) => event.eventType === "stop_requested")).toBe(false);
    } finally {
      (globalThis as any).fetch = previousFetch;
    }
  });

	  test("late success callbacks after cancellation leave the job stopped", async () => {
	    const db = createMockDb();
	    db.state.job.status = "stopping";
	    db.state.job.cancelRequestedAt = new Date("2026-06-15T12:00:00.000Z");

    const updated = await updateAiRunnerJobStatusFromCallback(
      "job-live",
      {
        status: "succeeded",
        eventKey: "status:succeeded:after-stop",
        result: { summary: "Completed after stop" },
      },
      db as any,
    );

    expect(updated?.status).toBe("stopped");
    expect(db.state.job.status).toBe("stopped");
	    expect(db.state.job.result).toBeNull();
	    expect(db.state.events.some((event) => event.eventKey === "status:succeeded:after-stop")).toBe(true);
	  });

	  test("late failure callbacks after cancellation leave the job stopped", async () => {
	    const db = createMockDb();
	    db.state.job.status = "stopping";
	    db.state.job.cancelRequestedAt = new Date("2026-06-15T12:00:00.000Z");

	    const updated = await updateAiRunnerJobStatusFromCallback(
	      "job-live",
	      {
	        status: "failed",
	        eventKey: "status:failed:after-stop",
	        error: "shell interrupted",
	      },
	      db as any,
	    );

	    expect(updated?.status).toBe("stopped");
	    expect(db.state.job.status).toBe("stopped");
	    expect(db.state.job.error).toBe("shell interrupted");
	    expect(db.state.events.some((event) => event.eventKey === "status:failed:after-stop")).toBe(true);
	  });
	});

describe("Command Center AI runner leases and idempotent callbacks", () => {
  test("acquires one active lease and rejects a competing runner", async () => {
    const db = createMockDb();

    const first = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);
    expect(first?.accepted).toBe(true);
    expect(first?.leaseId).toBeTruthy();

    const second = await acquireAiRunnerJobLease("job-live", "runner-b", 45_000, db as any);
    expect(second?.accepted).toBe(false);
    expect(second?.reason).toBe("lease_active");
  });

  test("heartbeat extends only the matching active lease", async () => {
    const db = createMockDb();
    const lease = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);

    const heartbeat = await heartbeatAiRunnerJobLease("job-live", lease?.leaseId, "runner-a", 45_000, db as any);
    expect(heartbeat?.accepted).toBe(true);

    const stale = await heartbeatAiRunnerJobLease("job-live", "wrong-lease", "runner-a", 45_000, db as any);
    expect(stale?.accepted).toBe(false);
    expect(stale?.reason).toBe("lease_lost");
  });

  test("heartbeat does not revive an expired lease", async () => {
    const db = createMockDb();
    const lease = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);
    db.state.job.leaseExpiresAt = new Date(Date.now() - 1_000);

    const heartbeat = await heartbeatAiRunnerJobLease("job-live", lease?.leaseId, "runner-a", 45_000, db as any);

    expect(heartbeat?.accepted).toBe(false);
    expect(heartbeat?.reason).toBe("lease_lost");
  });

  test("release clears only the matching lease", async () => {
    const db = createMockDb();
    const lease = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);

    const wrongRelease = await releaseAiRunnerJobLease("job-live", "wrong-lease", "runner-a", db as any);
    expect(wrongRelease?.accepted).toBe(false);
    expect(wrongRelease?.reason).toBe("lease_lost");
    expect(db.state.job.leaseId).toBe(lease?.leaseId);

    const release = await releaseAiRunnerJobLease("job-live", lease?.leaseId, "runner-a", db as any);
    expect(release?.accepted).toBe(true);
    expect(db.state.job.leaseId).toBeNull();
    expect(db.state.job.leaseOwnerRunnerId).toBeNull();
    expect(db.state.job.leaseExpiresAt).toBeNull();
  });

  test("duplicate artifact event key returns the existing artifact", async () => {
    const db = createMockDb();
    const lease = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);
    const input = {
      runnerId: "runner-a",
      leaseId: lease?.leaseId,
      eventKey: "artifact:runner-screenshot:1",
      artifactType: "runner-screenshot",
      name: "desktop-goal-frame-1.png",
      mimeType: "image/png",
      contentBase64: "abc",
      metadata: { frameSeq: 1 },
    };

    const first = await appendAiRunnerArtifactFromCallback("job-live", input, db as any);
    const duplicate = await appendAiRunnerArtifactFromCallback("job-live", input, db as any);

    expect(first?.id).toBe("artifact-1");
    expect(duplicate?.id).toBe("artifact-1");
    expect(db.state.artifacts).toHaveLength(1);
  });

  test("stale callbacks from a replaced lease do not mutate the job", async () => {
    const db = createMockDb();
    const first = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);
    db.state.job.leaseId = "replacement-lease";
    db.state.job.leaseOwnerRunnerId = "runner-b";
    db.state.job.leaseExpiresAt = new Date(Date.now() + 45_000);

    await expect(
      appendAiRunnerArtifactFromCallback(
        "job-live",
        {
          runnerId: "runner-a",
          leaseId: first?.leaseId,
          artifactType: "runner-screenshot",
          name: "stale.png",
          mimeType: "image/png",
          contentBase64: "abc",
        },
        db as any,
      ),
    ).rejects.toThrow("lease mismatch");
    expect(db.state.artifacts).toHaveLength(0);
  });

  test("callbacks from expired leases are rejected before reconciliation", async () => {
    const db = createMockDb();
    const lease = await acquireAiRunnerJobLease("job-live", "runner-a", 45_000, db as any);
    db.state.job.leaseExpiresAt = new Date(Date.now() - 1_000);

    await expect(
      appendAiRunnerArtifactFromCallback(
        "job-live",
        {
          runnerId: "runner-a",
          leaseId: lease?.leaseId,
          artifactType: "runner-screenshot",
          name: "expired.png",
          mimeType: "image/png",
          contentBase64: "abc",
        },
        db as any,
      ),
    ).rejects.toThrow("lease expired");
    expect(db.state.artifacts).toHaveLength(0);
  });

  test("expired lease reconciliation marks active jobs failed and retryable", async () => {
    const db = createMockDb();
    db.state.job.leaseId = "expired-lease";
    db.state.job.leaseOwnerRunnerId = "runner-a";
    db.state.job.leaseExpiresAt = new Date("2026-06-15T10:00:00.000Z");
    db.state.job.lastHeartbeatAt = new Date("2026-06-15T09:59:30.000Z");

    const count = await reconcileExpiredAiRunnerJobLeases(
      new Date("2026-06-15T10:01:00.000Z"),
      db as any,
    );

    expect(count).toBe(1);
    expect(db.state.job.status).toBe("failed");
    expect(db.state.job.retryable).toBe(true);
    expect(db.state.job.retryReason).toBe("lease_expired");
    expect(db.state.events.some((event) => event.eventType === "lease_expired")).toBe(true);
  });
});
