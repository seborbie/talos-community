import { Prisma } from "@prisma/client";
import { prisma } from "../../prisma";
import type {
  CommandCenterMcpContext,
  CommandCenterMcpServer,
  CommandCenterMcpTool,
} from "../types";

type DeviceMcpDatabase = typeof prisma;

type SnapshotPath =
  | "summary"
  | "applications"
  | "services"
  | "startup_items"
  | "windows_features"
  | "pending_updates"
  | "installed_updates"
  | "events";

const SERVER_NAME = "talos-device-inventory";
const SERVER_VERSION = "0.1.0";
const DEFAULT_LIMIT = 50;
const MAX_LIMIT = 100;
const SEARCH_LIMIT = 10;

const SNAPSHOT_PATHS: SnapshotPath[] = [
  "summary",
  "applications",
  "services",
  "startup_items",
  "windows_features",
  "pending_updates",
  "installed_updates",
  "events",
];

const SNAPSHOT_PATH_DESCRIPTIONS: Record<SnapshotPath, string> = {
  summary: "Device identity, customer, site, OS, last seen, and latest inventory state.",
  applications: "Installed applications/software inventory.",
  services: "Operating system services.",
  startup_items: "Startup items and launch entries.",
  windows_features: "Windows optional features and install state.",
  pending_updates: "Pending OS/software updates.",
  installed_updates: "Installed update history.",
  events: "Recent telemetry events.",
};

function readString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function clampLimit(value: unknown, fallback = DEFAULT_LIMIT): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(MAX_LIMIT, Math.max(1, Math.trunc(parsed)));
}

function containsInsensitive(value: string) {
  return { contains: value, mode: "insensitive" as Prisma.QueryMode };
}

function isoDate(value: unknown): string | null {
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (typeof value === "string" && value.trim()) {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? null : date.toISOString();
  }
  return null;
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
    if (!record) return undefined;
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
    if (typeof value === "number" && Number.isFinite(value)) return value;
    if (typeof value === "bigint") {
      return Number.isSafeInteger(Number(value)) ? Number(value) : null;
    }
    if (typeof value === "string" && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
  }
  return null;
}

function booleanValue(...values: unknown[]): boolean | null {
  for (const value of values) {
    if (typeof value === "boolean") return value;
    if (typeof value === "string") {
      const normalized = value.trim().toLowerCase();
      if (["true", "yes", "1", "enabled", "running"].includes(normalized)) return true;
      if (["false", "no", "0", "disabled", "stopped"].includes(normalized)) return false;
    }
  }
  return null;
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

function matchesQuery(item: unknown, query: string): boolean {
  const normalized = normalizeSearch(query);
  if (!normalized) return true;
  return normalizeSearch(JSON.stringify(item)).includes(normalized);
}

function pageItems(items: unknown[], limit: number, query = "") {
  const filtered = query ? items.filter((item) => matchesQuery(item, query)) : items;
  const sliced = filtered.slice(0, limit);
  return {
    items: sliced,
    total: filtered.length,
    truncated: filtered.length > sliced.length,
  };
}

function mapCustomer(customer: any) {
  return {
    id: customer.id,
    name: customer.name,
    description: customer.description ?? null,
    isUnassigned: Boolean(customer.isUnassigned),
    deviceCount: customer._count?.devices ?? undefined,
    siteCount: customer._count?.sites ?? undefined,
  };
}

function mapDeviceSummary(device: any) {
  const state = device.telemetryState ?? null;
  return {
    agentId: device.agentId,
    hostname: state?.hostname || device.hostname,
    customerId: device.customerId ?? null,
    customerName: device.customer?.name ?? null,
    siteId: device.siteId ?? null,
    siteName: device.site?.name ?? null,
    os: state?.osName || device.os,
    osVersion: state?.osVersion ?? null,
    ip: device.ip,
    version: state?.agentVersion || device.version || null,
    lastSeen: isoDate(device.lastSeen),
    websocketStatus: device.websocketStatus ?? "unknown",
    collectedAt: isoDate(state?.collectedAt),
    installedAppsCount: state?.installedAppsCount ?? null,
    pendingUpdatesCount: state?.pendingUpdatesCount ?? null,
    rebootRequired: state?.rebootRequired ?? null,
  };
}

async function getDeviceOrThrow(
  db: DeviceMcpDatabase,
  context: CommandCenterMcpContext,
  agentId: string,
) {
  const device = await db.rmmDevice.findFirst({
    where: {
      agentId,
      organizationId: context.organizationId,
    },
    include: {
      customer: true,
      site: true,
      telemetryState: true,
    },
  });
  if (!device) {
    throw new Error("Device not found in the current organization");
  }
  return device;
}

function mapInstalledApp(row: any) {
  return {
    appName: row.appName ?? row.app_name ?? row.name ?? row.displayName ?? row.display_name ?? null,
    publisher: row.publisher ?? row.vendor ?? null,
    version: row.version ?? row.displayVersion ?? null,
    installDate: row.installDate ?? row.install_date ?? null,
    sizeBytes: numberValue(row.sizeBytes, row.size_bytes, row.size),
    source: row.source ?? row.packageManager ?? row.package_manager ?? null,
    location: row.location ?? null,
    is64Bit: booleanValue(row.is64Bit, row.is_64_bit),
  };
}

function parseInstalledApps(inventoryData: unknown): unknown[] {
  const collection = asRecord(inventoryData);
  return arrayAtAnyPath(collection, [
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
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapInstalledApp(row) : null;
    })
    .filter((item) => item && (item as any).appName);
}

function mapService(row: any) {
  return {
    serviceName: row.serviceName ?? row.service_name ?? row.name ?? null,
    displayName: row.displayName ?? row.display_name ?? row.name ?? null,
    status: row.status ?? row.state ?? row.activeState ?? row.active_state ?? "unknown",
    startType: row.startType ?? row.start_type ?? row.unit_file_state ?? null,
    account: row.account ?? row.user ?? null,
    processId: numberValue(row.processId, row.process_id),
    isCritical: booleanValue(row.isCritical, row.is_critical),
    description: row.description ?? null,
    path: row.path ?? row.binaryPath ?? row.binary_path ?? null,
  };
}

function parseServices(inventoryData: unknown): unknown[] {
  return arrayAtAnyPath(asRecord(inventoryData), [
    ["operating_system", "services", "services"],
    ["operatingSystem", "services", "services"],
    ["services", "services"],
    ["services"],
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapService(row) : null;
    })
    .filter((item) => item && (item as any).serviceName);
}

function mapStartupItem(row: any) {
  return {
    itemName: row.itemName ?? row.item_name ?? row.name ?? null,
    command: row.command ?? row.path ?? row.target ?? null,
    location: row.location ?? row.source ?? null,
    userName: row.userName ?? row.user_name ?? row.user ?? null,
    isEnabled: booleanValue(row.isEnabled, row.is_enabled, row.enabled),
  };
}

function parseStartupItems(inventoryData: unknown): unknown[] {
  return arrayAtAnyPath(asRecord(inventoryData), [
    ["software", "startup_items"],
    ["software", "startupItems"],
    ["operating_system", "startup_items"],
    ["operatingSystem", "startupItems"],
    ["operating_system", "startup", "items"],
    ["startup_items"],
    ["startupItems"],
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapStartupItem(row) : null;
    })
    .filter((item) => item && (item as any).itemName);
}

function mapWindowsFeature(row: any) {
  return {
    featureName: row.featureName ?? row.feature_name ?? row.name ?? null,
    displayName: row.displayName ?? row.display_name ?? row.name ?? null,
    installState: row.installState ?? row.install_state ?? row.state ?? null,
    enabled: booleanValue(row.enabled, row.isEnabled, row.is_enabled),
  };
}

function parseWindowsFeatures(inventoryData: unknown): unknown[] {
  return arrayAtAnyPath(asRecord(inventoryData), [
    ["software", "features"],
    ["software", "windows_features"],
    ["software", "windowsFeatures"],
    ["operating_system", "windows_features"],
    ["operatingSystem", "windowsFeatures"],
    ["features"],
    ["windows_features"],
    ["windowsFeatures"],
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapWindowsFeature(row) : null;
    })
    .filter((item) => item && (item as any).featureName);
}

function mapPendingUpdate(row: any) {
  return {
    title: row.title ?? row.name ?? row.kb ?? null,
    description: row.description ?? null,
    kbArticle: row.kbArticle ?? row.kb_article ?? row.kb ?? null,
    isMandatory: booleanValue(row.isMandatory, row.is_mandatory),
    sizeBytes: numberValue(row.sizeBytes, row.size_bytes, row.size),
    requiresReboot: booleanValue(row.requiresReboot, row.requires_reboot),
  };
}

function parsePendingUpdates(inventoryData: unknown): unknown[] {
  return arrayAtAnyPath(asRecord(inventoryData), [
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
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapPendingUpdate(row) : null;
    })
    .filter((item) => item && (item as any).title);
}

function mapInstalledUpdate(row: any) {
  return {
    installedAt: isoDate(row.installedAt ?? row.installed_at ?? row.date),
    title: row.title ?? row.name ?? row.package ?? null,
    kbArticle: row.kbArticle ?? row.kb_article ?? row.kb ?? null,
    operation: row.operation ?? null,
    result: row.result ?? null,
    hresult: row.hresult ?? null,
  };
}

function parseInstalledUpdates(inventoryData: unknown): unknown[] {
  return arrayAtAnyPath(asRecord(inventoryData), [
    ["operating_system", "updates", "update_history"],
    ["operatingSystem", "updates", "updateHistory"],
    ["updates", "update_history"],
    ["updates", "updateHistory"],
    ["update_history"],
    ["updateHistory"],
  ])
    .map((item) => {
      const row = asRecord(item);
      return row ? mapInstalledUpdate(row) : null;
    })
    .filter((item) => item && (item as any).title);
}

async function searchCustomers(
  db: DeviceMcpDatabase,
  args: Record<string, unknown>,
  context: CommandCenterMcpContext,
) {
  const query = readString(args.query);
  const limit = clampLimit(args.limit, SEARCH_LIMIT);
  const where: Prisma.CustomerWhereInput = {
    organizationId: context.organizationId,
    ...(query
      ? {
          OR: [
            { name: containsInsensitive(query) },
            { description: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const rows = await db.customer.findMany({
    where,
    orderBy: [{ isUnassigned: "asc" }, { name: "asc" }],
    take: limit + 1,
    include: {
      _count: {
        select: {
          devices: true,
          sites: true,
        },
      },
    },
  });
  return {
    items: rows.slice(0, limit).map(mapCustomer),
    total: rows.length,
    truncated: rows.length > limit,
  };
}

async function searchDevices(
  db: DeviceMcpDatabase,
  args: Record<string, unknown>,
  context: CommandCenterMcpContext,
) {
  const query = readString(args.query);
  const customerId = readString(args.customerId);
  const limit = clampLimit(args.limit, SEARCH_LIMIT);
  const where: Prisma.RmmDeviceWhereInput = {
    organizationId: context.organizationId,
    ...(customerId ? { customerId } : {}),
    ...(query
      ? {
          OR: [
            { agentId: containsInsensitive(query) },
            { hostname: containsInsensitive(query) },
            { os: containsInsensitive(query) },
            { ip: containsInsensitive(query) },
            { customer: { name: containsInsensitive(query) } },
            { site: { name: containsInsensitive(query) } },
          ],
        }
      : {}),
  };
  const rows = await db.rmmDevice.findMany({
    where,
    orderBy: [{ lastSeen: "desc" }, { hostname: "asc" }],
    take: limit + 1,
    include: {
      customer: true,
      site: true,
      telemetryState: true,
    },
  });
  if (rows.length === 0 && customerId && isDeviceRoleShorthand(query)) {
    const fallbackRows = await db.rmmDevice.findMany({
      where: {
        organizationId: context.organizationId,
        customerId,
      },
      orderBy: [{ lastSeen: "desc" }, { hostname: "asc" }],
      take: limit + 1,
      include: {
        customer: true,
        site: true,
        telemetryState: true,
      },
    });
    return {
      items: fallbackRows.slice(0, limit).map(mapDeviceSummary),
      total: fallbackRows.length,
      truncated: fallbackRows.length > limit,
      interpretedQuery: query,
      message:
        fallbackRows.length === 1
          ? "No literal device match for the shorthand, but this customer has one device candidate."
          : "No literal device match for the shorthand; returned customer devices for clarification.",
    };
  }
  return {
    items: rows.slice(0, limit).map(mapDeviceSummary),
    total: rows.length,
    truncated: rows.length > limit,
  };
}

async function listDevicesByCustomer(
  db: DeviceMcpDatabase,
  args: Record<string, unknown>,
  context: CommandCenterMcpContext,
) {
  const customerId = readString(args.customerId);
  if (!customerId) {
    throw new Error("customerId is required");
  }
  const customer = await db.customer.findFirst({
    where: {
      id: customerId,
      organizationId: context.organizationId,
    },
  });
  if (!customer) {
    throw new Error("Customer not found in the current organization");
  }
  const result = await searchDevices(
    db,
    {
      query: readString(args.query),
      customerId,
      limit: clampLimit(args.limit, DEFAULT_LIMIT),
    },
    context,
  );
  return {
    customer: mapCustomer(customer),
    ...result,
  };
}

async function listDeviceSnapshotPaths(
  db: DeviceMcpDatabase,
  args: Record<string, unknown>,
  context: CommandCenterMcpContext,
) {
  const agentId = readString(args.agentId);
  if (!agentId) {
    throw new Error("agentId is required");
  }
  const device = await getDeviceOrThrow(db, context, agentId);
  const where = { organizationId: context.organizationId, agentId };
  const [
    applications,
    services,
    startupItems,
    windowsFeatures,
    pendingUpdates,
    installedUpdates,
    events,
  ] = await Promise.all([
    db.rmmTelemetryDeviceInstalledApp.count({ where }),
    db.rmmTelemetryDeviceService.count({ where }),
    db.rmmTelemetryDeviceStartupItem.count({ where }),
    db.rmmTelemetryDeviceWindowsFeature.count({ where }),
    db.rmmTelemetryDevicePendingUpdate.count({ where }),
    db.rmmTelemetryDeviceInstalledUpdate.count({ where }),
    db.rmmTelemetryDeviceEvent.count({ where }),
  ]);
  const counts: Partial<Record<SnapshotPath, number>> = {
    summary: 1,
    applications,
    services,
    startup_items: startupItems,
    windows_features: windowsFeatures,
    pending_updates: pendingUpdates,
    installed_updates: installedUpdates,
    events,
  };
  return {
    device: mapDeviceSummary(device),
    paths: SNAPSHOT_PATHS.map((path) => ({
      path,
      description: SNAPSHOT_PATH_DESCRIPTIONS[path],
      count: counts[path] ?? 0,
      available:
        path === "summary" ||
        Boolean((counts[path] ?? 0) > 0 || device.telemetryState?.inventoryData),
    })),
    collectedAt: isoDate(device.telemetryState?.collectedAt),
  };
}

async function readApplications(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceInstalledAppWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { appName: containsInsensitive(query) },
            { publisher: containsInsensitive(query) },
            { version: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceInstalledApp.count({ where }),
    db.rmmTelemetryDeviceInstalledApp.findMany({
      where,
      orderBy: [{ appName: "asc" }, { version: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapInstalledApp),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parseInstalledApps(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readServices(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceServiceWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { serviceName: containsInsensitive(query) },
            { displayName: containsInsensitive(query) },
            { status: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceService.count({ where }),
    db.rmmTelemetryDeviceService.findMany({
      where,
      orderBy: [{ serviceName: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapService),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parseServices(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readStartupItems(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceStartupItemWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { itemName: containsInsensitive(query) },
            { command: containsInsensitive(query) },
            { location: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceStartupItem.count({ where }),
    db.rmmTelemetryDeviceStartupItem.findMany({
      where,
      orderBy: [{ itemName: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapStartupItem),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parseStartupItems(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readWindowsFeatures(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceWindowsFeatureWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { featureName: containsInsensitive(query) },
            { displayName: containsInsensitive(query) },
            { installState: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceWindowsFeature.count({ where }),
    db.rmmTelemetryDeviceWindowsFeature.findMany({
      where,
      orderBy: [{ featureName: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapWindowsFeature),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parseWindowsFeatures(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readPendingUpdates(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDevicePendingUpdateWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { title: containsInsensitive(query) },
            { description: containsInsensitive(query) },
            { kbArticle: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDevicePendingUpdate.count({ where }),
    db.rmmTelemetryDevicePendingUpdate.findMany({
      where,
      orderBy: [{ title: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapPendingUpdate),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parsePendingUpdates(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readInstalledUpdates(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceInstalledUpdateWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { title: containsInsensitive(query) },
            { kbArticle: containsInsensitive(query) },
            { result: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceInstalledUpdate.count({ where }),
    db.rmmTelemetryDeviceInstalledUpdate.findMany({
      where,
      orderBy: [{ installedAt: "desc" }, { title: "asc" }],
      take: limit + 1,
    }),
  ]);
  if (rows.length > 0 || !device.telemetryState?.inventoryData) {
    return {
      items: rows.slice(0, limit).map(mapInstalledUpdate),
      total,
      truncated: total > limit,
      source: "normalized_telemetry",
    };
  }
  return {
    ...pageItems(parseInstalledUpdates(device.telemetryState.inventoryData), limit, query),
    source: "inventory_snapshot",
  };
}

async function readEvents(
  db: DeviceMcpDatabase,
  device: any,
  context: CommandCenterMcpContext,
  limit: number,
  query: string,
) {
  const where: Prisma.RmmTelemetryDeviceEventWhereInput = {
    organizationId: context.organizationId,
    agentId: device.agentId,
    ...(query
      ? {
          OR: [
            { eventType: containsInsensitive(query) },
            { severity: containsInsensitive(query) },
            { source: containsInsensitive(query) },
            { serviceName: containsInsensitive(query) },
            { processName: containsInsensitive(query) },
            { code: containsInsensitive(query) },
            { message: containsInsensitive(query) },
          ],
        }
      : {}),
  };
  const [total, rows] = await Promise.all([
    db.rmmTelemetryDeviceEvent.count({ where }),
    db.rmmTelemetryDeviceEvent.findMany({
      where,
      orderBy: [{ occurredAt: "desc" }],
      take: limit + 1,
    }),
  ]);
  return {
    items: rows.slice(0, limit).map((row) => ({
      eventId: row.eventId,
      occurredAt: isoDate(row.occurredAt),
      receivedAt: isoDate(row.receivedAt),
      eventType: row.eventType,
      severity: row.severity,
      source: row.source,
      serviceName: row.serviceName,
      processName: row.processName,
      code: row.code,
      message: row.message,
      attributes: row.attributesJsonb,
    })),
    total,
    truncated: total > limit,
    source: "normalized_telemetry",
  };
}

async function getDeviceSnapshotPath(
  db: DeviceMcpDatabase,
  args: Record<string, unknown>,
  context: CommandCenterMcpContext,
) {
  const agentId = readString(args.agentId);
  const path = readString(args.path) as SnapshotPath;
  const query = readString(args.query);
  const limit = clampLimit(args.limit, DEFAULT_LIMIT);
  if (!agentId) {
    throw new Error("agentId is required");
  }
  if (!SNAPSHOT_PATHS.includes(path)) {
    throw new Error(`Unsupported device snapshot path: ${path || "empty"}`);
  }

  const device = await getDeviceOrThrow(db, context, agentId);
  const base = {
    device: mapDeviceSummary(device),
    path,
    collectedAt: isoDate(device.telemetryState?.collectedAt),
  };

  switch (path) {
    case "summary":
      return {
        ...base,
        items: [
          {
            ...mapDeviceSummary(device),
            inventorySummary: device.telemetryState
              ? {
                  bootSessionId: device.telemetryState.bootSessionId,
                  cpuModel: device.telemetryState.cpuModel,
                  cpuPhysicalCores: device.telemetryState.cpuPhysicalCores,
                  cpuLogicalCores: device.telemetryState.cpuLogicalCores,
                  cpuBaseMhz: device.telemetryState.cpuBaseMhz,
                  memoryTotalBytes: numberValue(device.telemetryState.memoryTotalBytes),
                }
              : null,
          },
        ],
        total: 1,
        truncated: false,
        source: "device_summary",
      };
    case "applications":
      return { ...base, ...(await readApplications(db, device, context, limit, query)) };
    case "services":
      return { ...base, ...(await readServices(db, device, context, limit, query)) };
    case "startup_items":
      return { ...base, ...(await readStartupItems(db, device, context, limit, query)) };
    case "windows_features":
      return { ...base, ...(await readWindowsFeatures(db, device, context, limit, query)) };
    case "pending_updates":
      return { ...base, ...(await readPendingUpdates(db, device, context, limit, query)) };
    case "installed_updates":
      return { ...base, ...(await readInstalledUpdates(db, device, context, limit, query)) };
    case "events":
      return { ...base, ...(await readEvents(db, device, context, limit, query)) };
    default:
      throw new Error(`Unsupported device snapshot path: ${path}`);
  }
}

function objectSchema(properties: Record<string, unknown>, required: string[] = []) {
  return {
    type: "object",
    properties,
    required,
    additionalProperties: false,
  };
}

function createTool(
  definition: CommandCenterMcpTool["definition"],
  handler: CommandCenterMcpTool["handler"],
): CommandCenterMcpTool {
  return { definition, handler };
}

export function createDeviceMcpServer(db: DeviceMcpDatabase = prisma): CommandCenterMcpServer {
  return {
    name: SERVER_NAME,
    version: SERVER_VERSION,
    tools: [
      createTool(
        {
          name: "search_customers",
          description:
            "Search customers in the current Talos organization by name or description.",
          inputSchema: objectSchema({
            query: { type: "string", description: "Customer search text. Empty lists top customers." },
            limit: { type: "number", description: "Maximum customers to return, up to 100." },
          }),
        },
        (args, context) => searchCustomers(db, args, context),
      ),
      createTool(
        {
          name: "search_devices",
          description:
            "Search devices in the current Talos organization by hostname, agent id, IP, OS, customer, or site.",
          inputSchema: objectSchema({
            query: { type: "string", description: "Device search text." },
            customerId: { type: "string", description: "Optional customer id to narrow the search." },
            limit: { type: "number", description: "Maximum devices to return, up to 100." },
          }),
        },
        (args, context) => searchDevices(db, args, context),
      ),
      createTool(
        {
          name: "list_devices_by_customer",
          description:
            "List devices for a specific customer in the current Talos organization.",
          inputSchema: objectSchema(
            {
              customerId: { type: "string", description: "Customer id returned by search_customers." },
              query: { type: "string", description: "Optional device search text." },
              limit: { type: "number", description: "Maximum devices to return, up to 100." },
            },
            ["customerId"],
          ),
        },
        (args, context) => listDevicesByCustomer(db, args, context),
      ),
      createTool(
        {
          name: "list_device_snapshot_paths",
          description:
            "List the allowlisted telemetry paths available for a device snapshot.",
          inputSchema: objectSchema(
            {
              agentId: { type: "string", description: "Device agent id returned by device search." },
            },
            ["agentId"],
          ),
        },
        (args, context) => listDeviceSnapshotPaths(db, args, context),
      ),
      createTool(
        {
          name: "get_device_snapshot_path",
          description:
            "Read a bounded, allowlisted section of a device telemetry snapshot or normalized telemetry table.",
          inputSchema: objectSchema(
            {
              agentId: { type: "string", description: "Device agent id returned by device search." },
              path: {
                type: "string",
                enum: SNAPSHOT_PATHS,
                description:
                  "Allowlisted path to read: summary, applications, services, startup_items, windows_features, pending_updates, installed_updates, or events.",
              },
              query: { type: "string", description: "Optional filter inside the selected path." },
              limit: { type: "number", description: "Maximum rows to return, up to 100." },
            },
            ["agentId", "path"],
          ),
        },
        (args, context) => getDeviceSnapshotPath(db, args, context),
      ),
    ],
  };
}

export const deviceSnapshotPaths = SNAPSHOT_PATHS;
