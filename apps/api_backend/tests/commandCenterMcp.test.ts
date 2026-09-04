import { describe, expect, test } from "bun:test";
import { ApiLocalMcpClient } from "../lib/mcp/client";
import { createDeviceMcpServer } from "../lib/mcp/servers/deviceServer";
import type { CommandCenterMcpContext, CommandCenterMcpServer } from "../lib/mcp/types";

const now = new Date("2026-06-10T08:30:00.000Z");

function context(overrides: Partial<CommandCenterMcpContext> = {}): CommandCenterMcpContext {
  return {
    userId: "user-a",
    userEmail: "user-a@example.test",
    organizationId: "org-a",
    organizationName: "Org A",
    role: "SUPER_ADMIN",
    ...overrides,
  };
}

function findTool(server: CommandCenterMcpServer, name: string) {
  const tool = server.tools.find((candidate) => candidate.definition.name === name);
  if (!tool) throw new Error(`Missing test tool ${name}`);
  return tool;
}

function createMockDb(options: {
  installedApps?: any[];
  fallbackInventory?: any;
} = {}) {
  const customers = [
    {
      id: "customer-a",
      organizationId: "org-a",
      name: "Customer A",
      description: null,
      isUnassigned: false,
      _count: { devices: 1, sites: 1 },
    },
    {
      id: "customer-b",
      organizationId: "org-b",
      name: "Customer B",
      description: null,
      isUnassigned: false,
      _count: { devices: 1, sites: 1 },
    },
  ];
  const devices = [
    {
      agentId: "agent-a",
      organizationId: "org-a",
      hostname: "workstation-a",
      os: "Windows",
      ip: "10.0.0.10",
      version: "1.0.0",
      lastSeen: now,
      websocketStatus: "online",
      customerId: "customer-a",
      siteId: null,
      customer: customers[0],
      site: null,
      telemetryState: {
        collectedAt: now,
        hostname: "workstation-a",
        osName: "Windows 11",
        osVersion: "23H2",
        agentVersion: "1.0.0",
        installedAppsCount: 2,
        pendingUpdatesCount: 0,
        rebootRequired: false,
        inventoryData: options.fallbackInventory ?? null,
        bootSessionId: "boot-a",
        cpuModel: "Test CPU",
        cpuPhysicalCores: 4,
        cpuLogicalCores: 8,
        cpuBaseMhz: 2800,
        memoryTotalBytes: 16_000_000_000n,
      },
    },
  ];
  const installedApps = options.installedApps ?? [];
  const emptyTable = {
    count: async () => 0,
    findMany: async () => [],
  };

  return {
    customer: {
      findMany: async ({ where, take }: any) =>
        customers
          .filter((customer) => customer.organizationId === where.organizationId)
          .slice(0, take),
      findFirst: async ({ where }: any) =>
        customers.find(
          (customer) =>
            customer.id === where.id && customer.organizationId === where.organizationId,
        ) ?? null,
    },
    rmmDevice: {
      findMany: async ({ where, take }: any) =>
        devices
          .filter(
            (device) =>
              device.organizationId === where.organizationId &&
              (!where.customerId || device.customerId === where.customerId),
          )
          .slice(0, take),
      findFirst: async ({ where }: any) =>
        devices.find(
          (device) =>
            device.agentId === where.agentId && device.organizationId === where.organizationId,
        ) ?? null,
    },
    rmmTelemetryDeviceInstalledApp: {
      count: async () => installedApps.length,
      findMany: async ({ take }: any) => installedApps.slice(0, take),
    },
    rmmTelemetryDeviceService: emptyTable,
    rmmTelemetryDeviceStartupItem: emptyTable,
    rmmTelemetryDeviceWindowsFeature: emptyTable,
    rmmTelemetryDevicePendingUpdate: emptyTable,
    rmmTelemetryDeviceInstalledUpdate: emptyTable,
    rmmTelemetryDeviceEvent: emptyTable,
  } as any;
}

describe("Command Center MCP client", () => {
  test("registers tools and emits a user-safe tool status before execution", async () => {
    const server: CommandCenterMcpServer = {
      name: "test-server",
      version: "1.0.0",
      tools: [
        {
          definition: {
            name: "echo",
            description: "Echo input",
            inputSchema: { type: "object", properties: {}, additionalProperties: false },
          },
          handler: async (args) => ({ args }),
        },
      ],
    };
    const client = new ApiLocalMcpClient([server]);
    const statuses: string[] = [];
    const result = await client.executeTool(
      "echo",
      { value: "ok" },
      context({
        emitStatus: (event) => {
          statuses.push(event.message);
        },
      }),
    );

    expect(client.openAiTools()[0].name).toBe("echo");
    expect(result).toEqual({ args: { value: "ok" } });
    expect(statuses).toHaveLength(1);
    expect(statuses[0]).not.toContain("MCP");
  });
});

describe("Device MCP server", () => {
  test("searches customers only inside the active organization", async () => {
    const server = createDeviceMcpServer(createMockDb());
    const result: any = await findTool(server, "search_customers").handler(
      { query: "Customer", limit: 10 },
      context(),
    );

    expect(result.items).toHaveLength(1);
    expect(result.items[0].id).toBe("customer-a");
  });

  test("reads installed applications from normalized telemetry first", async () => {
    const server = createDeviceMcpServer(
      createMockDb({
        installedApps: [
          {
            appName: "Slack",
            publisher: "Slack Technologies",
            version: "4.0",
            installDate: null,
            sizeBytes: 123n,
            source: "registry",
            location: null,
            is64Bit: true,
          },
        ],
      }),
    );
    const result: any = await findTool(server, "get_device_snapshot_path").handler(
      { agentId: "agent-a", path: "applications", limit: 10 },
      context(),
    );

    expect(result.source).toBe("normalized_telemetry");
    expect(result.items[0]).toMatchObject({
      appName: "Slack",
      publisher: "Slack Technologies",
      version: "4.0",
      sizeBytes: 123,
    });
  });

  test("falls back to inventory snapshot application paths", async () => {
    const server = createDeviceMcpServer(
      createMockDb({
        fallbackInventory: {
          software: {
            installed_programs: [
              {
                name: "Zoom",
                publisher: "Zoom Video Communications",
                version: "6.0",
              },
            ],
          },
        },
      }),
    );
    const result: any = await findTool(server, "get_device_snapshot_path").handler(
      { agentId: "agent-a", path: "applications", limit: 10 },
      context(),
    );

    expect(result.source).toBe("inventory_snapshot");
    expect(result.items[0]).toMatchObject({
      appName: "Zoom",
      publisher: "Zoom Video Communications",
      version: "6.0",
    });
  });

  test("rejects unsupported snapshot paths", async () => {
    const server = createDeviceMcpServer(createMockDb());

    await expect(
      findTool(server, "get_device_snapshot_path").handler(
        { agentId: "agent-a", path: "raw_json" },
        context(),
      ),
    ).rejects.toThrow("Unsupported device snapshot path");
  });

  test("does not return devices outside the active organization", async () => {
    const server = createDeviceMcpServer(createMockDb());

    await expect(
      findTool(server, "get_device_snapshot_path").handler(
        { agentId: "agent-a", path: "summary" },
        context({ organizationId: "org-b" }),
      ),
    ).rejects.toThrow("Device not found");
  });
});
