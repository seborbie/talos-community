import { Prisma, PrismaClient } from '@prisma/client';

export type ReportId =
  | 'fleet_health'
  | 'patch_compliance'
  | 'device_inventory'
  | 'software_inventory'
  | 'alert_history'
  | 'uptime_offline'
  | 'command_remediation_outcomes'
  | 'remote_support_activity';

export type ReportFormat = 'json' | 'csv' | 'pdf';
export type ReportFrequency = 'daily' | 'weekly' | 'monthly';

export interface ReportColumn {
  key: string;
  label: string;
}

export interface ReportDefinition {
  id: ReportId;
  name: string;
  description: string;
  category: 'health' | 'inventory' | 'operations';
  formats: ReportFormat[];
  columns: ReportColumn[];
}

export interface ReportFilters {
  organizationId: string;
  from: Date | null;
  to: Date | null;
  customerId: string | null;
  siteId: string | null;
  limit: number;
  offlineMinutes: number;
}

export type ReportFilterInput = Partial<{
  from: unknown;
  to: unknown;
  customerId: unknown;
  siteId: unknown;
  limit: unknown;
  offlineMinutes: unknown;
}>;

export type ReportRow = Record<string, unknown>;

export interface ReportRepository {
  getFleetHealth(filters: ReportFilters): Promise<ReportRow[]>;
  getPatchCompliance(filters: ReportFilters): Promise<ReportRow[]>;
  getDeviceInventory(filters: ReportFilters): Promise<ReportRow[]>;
  getSoftwareInventory(filters: ReportFilters): Promise<ReportRow[]>;
  getAlertHistory(filters: ReportFilters): Promise<ReportRow[]>;
  getUptimeOffline(filters: ReportFilters): Promise<ReportRow[]>;
  getCommandRemediationOutcomes(filters: ReportFilters): Promise<ReportRow[]>;
  getRemoteSupportActivity(filters: ReportFilters): Promise<ReportRow[]>;
}

type QueryablePrisma = Pick<PrismaClient, '$queryRaw'>;

export class ReportValidationError extends Error {
  statusCode = 400;
}

const commonDeviceColumns: ReportColumn[] = [
  { key: 'agentId', label: 'Agent ID' },
  { key: 'hostname', label: 'Hostname' },
  { key: 'customerName', label: 'Customer' },
  { key: 'siteName', label: 'Site' },
  { key: 'os', label: 'OS' },
  { key: 'lastSeen', label: 'Last Seen' }
];

export const REPORT_DEFINITIONS: ReportDefinition[] = [
  {
    id: 'fleet_health',
    name: 'Fleet Health',
    description: 'Device online state, patch pressure, alert volume, and open remediation work.',
    category: 'health',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'onlineStatus', label: 'Online Status' },
      { key: 'pendingUpdates', label: 'Pending Updates' },
      { key: 'rebootRequired', label: 'Reboot Required' },
      { key: 'criticalEvents', label: 'Critical Events' },
      { key: 'openRemediations', label: 'Open Remediations' },
      { key: 'healthStatus', label: 'Health Status' }
    ]
  },
  {
    id: 'patch_compliance',
    name: 'Patch Compliance',
    description: 'Pending and installed update posture by device.',
    category: 'health',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'telemetryCollectedAt', label: 'Telemetry Collected' },
      { key: 'pendingUpdates', label: 'Pending Updates' },
      { key: 'installedUpdates', label: 'Installed Updates' },
      { key: 'rebootRequired', label: 'Reboot Required' },
      { key: 'complianceStatus', label: 'Compliance Status' }
    ]
  },
  {
    id: 'device_inventory',
    name: 'Device Inventory',
    description: 'Current RMM device inventory with customer and site scope.',
    category: 'inventory',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'ip', label: 'IP Address' },
      { key: 'version', label: 'Agent Version' },
      { key: 'telemetryCollectedAt', label: 'Telemetry Collected' },
      { key: 'cpuModel', label: 'CPU' },
      { key: 'memoryTotalGb', label: 'Memory GB' },
      { key: 'installedApps', label: 'Installed Apps' },
      { key: 'pendingUpdates', label: 'Pending Updates' }
    ]
  },
  {
    id: 'software_inventory',
    name: 'Software Inventory',
    description: 'Installed software discovered from current telemetry.',
    category: 'inventory',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'appName', label: 'Application' },
      { key: 'publisher', label: 'Publisher' },
      { key: 'version', label: 'Version' },
      { key: 'installDate', label: 'Install Date' },
      { key: 'source', label: 'Source' },
      { key: 'is64Bit', label: '64-bit' },
      { key: 'collectedAt', label: 'Collected' }
    ]
  },
  {
    id: 'alert_history',
    name: 'Alert History',
    description: 'Telemetry events and alert-like signals across the fleet.',
    category: 'operations',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'occurredAt', label: 'Occurred' },
      { key: 'eventType', label: 'Event Type' },
      { key: 'severity', label: 'Severity' },
      { key: 'source', label: 'Source' },
      { key: 'message', label: 'Message' }
    ]
  },
  {
    id: 'uptime_offline',
    name: 'Uptime and Offline',
    description: 'Last-seen recency and offline duration by device.',
    category: 'health',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      ...commonDeviceColumns,
      { key: 'onlineStatus', label: 'Online Status' },
      { key: 'offlineMinutes', label: 'Offline Minutes' },
      { key: 'offlineThresholdMinutes', label: 'Threshold Minutes' }
    ]
  },
  {
    id: 'command_remediation_outcomes',
    name: 'Command and Remediation Outcomes',
    description: 'Command execution and remediation job outcomes.',
    category: 'operations',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      { key: 'activityType', label: 'Activity Type' },
      { key: 'activityAt', label: 'Activity At' },
      { key: 'agentId', label: 'Agent ID' },
      { key: 'hostname', label: 'Hostname' },
      { key: 'customerName', label: 'Customer' },
      { key: 'siteName', label: 'Site' },
      { key: 'summary', label: 'Summary' },
      { key: 'status', label: 'Status' },
      { key: 'outcome', label: 'Outcome' }
    ]
  },
  {
    id: 'remote_support_activity',
    name: 'Remote Support Activity',
    description: 'Remote desktop, shell, file transfer, and support-like command activity.',
    category: 'operations',
    formats: ['json', 'csv', 'pdf'],
    columns: [
      { key: 'activityAt', label: 'Activity At' },
      { key: 'agentId', label: 'Agent ID' },
      { key: 'hostname', label: 'Hostname' },
      { key: 'customerName', label: 'Customer' },
      { key: 'siteName', label: 'Site' },
      { key: 'command', label: 'Command' },
      { key: 'wasAllowed', label: 'Allowed' },
      { key: 'outcome', label: 'Outcome' }
    ]
  }
];

export const REPORT_DEFINITION_BY_ID = new Map(REPORT_DEFINITIONS.map((definition) => [definition.id, definition]));

const REPORT_IDS = new Set(REPORT_DEFINITIONS.map((definition) => definition.id));
const DEFAULT_LIMIT = 500;
const MAX_LIMIT = 5000;
const DEFAULT_OFFLINE_MINUTES = 5;

const isNonEmptyString = (value: unknown): value is string =>
  typeof value === 'string' && value.trim().length > 0;

function firstValue(value: unknown): unknown {
  return Array.isArray(value) ? value[0] : value;
}

function parseDateFilter(value: unknown, field: string): Date | null {
  const raw = firstValue(value);
  if (!isNonEmptyString(raw)) return null;
  const parsed = new Date(raw);
  if (Number.isNaN(parsed.getTime())) {
    throw new ReportValidationError(`${field} must be a valid ISO date/time`);
  }
  return parsed;
}

function parsePositiveInt(value: unknown, fallback: number, min: number, max: number, field: string): number {
  const raw = firstValue(value);
  if (raw == null || raw === '') return fallback;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < min) {
    throw new ReportValidationError(`${field} must be a positive integer`);
  }
  return Math.min(parsed, max);
}

function parseNullableId(value: unknown): string | null {
  const raw = firstValue(value);
  if (!isNonEmptyString(raw)) return null;
  return raw.trim();
}

export function parseReportId(value: unknown): ReportId {
  if (!isNonEmptyString(value) || !REPORT_IDS.has(value.trim() as ReportId)) {
    throw new ReportValidationError('Unknown reportId');
  }
  return value.trim() as ReportId;
}

export function parseReportFormat(value: unknown, fallback: ReportFormat = 'json'): ReportFormat {
  const raw = firstValue(value);
  if (raw == null || raw === '') return fallback;
  if (raw === 'json' || raw === 'csv' || raw === 'pdf') return raw;
  throw new ReportValidationError('format must be json, csv, or pdf');
}

export function parseReportFrequency(value: unknown): ReportFrequency {
  if (value === 'daily' || value === 'weekly' || value === 'monthly') return value;
  throw new ReportValidationError('frequency must be daily, weekly, or monthly');
}

export function normalizeReportFilters(organizationId: string, input: ReportFilterInput = {}): ReportFilters {
  const from = parseDateFilter(input.from, 'from');
  const to = parseDateFilter(input.to, 'to');
  if (from && to && from.getTime() > to.getTime()) {
    throw new ReportValidationError('from must be before to');
  }

  return {
    organizationId,
    from,
    to,
    customerId: parseNullableId(input.customerId),
    siteId: parseNullableId(input.siteId),
    limit: parsePositiveInt(input.limit, DEFAULT_LIMIT, 1, MAX_LIMIT, 'limit'),
    offlineMinutes: parsePositiveInt(
      input.offlineMinutes,
      DEFAULT_OFFLINE_MINUTES,
      1,
      43200,
      'offlineMinutes'
    )
  };
}

export function reportFiltersToJson(filters: ReportFilters) {
  return {
    from: filters.from?.toISOString() ?? null,
    to: filters.to?.toISOString() ?? null,
    customerId: filters.customerId,
    siteId: filters.siteId,
    limit: filters.limit,
    offlineMinutes: filters.offlineMinutes
  };
}

export function nextReportRunAt(frequency: ReportFrequency, from = new Date()): Date {
  const next = new Date(from.getTime());
  if (frequency === 'daily') {
    next.setUTCDate(next.getUTCDate() + 1);
  } else if (frequency === 'weekly') {
    next.setUTCDate(next.getUTCDate() + 7);
  } else {
    next.setUTCMonth(next.getUTCMonth() + 1);
  }
  next.setUTCHours(8, 0, 0, 0);
  return next;
}

function serializeDbValue(value: unknown): unknown {
  if (value instanceof Date) return value.toISOString();
  if (typeof value === 'bigint') {
    const asNumber = Number(value);
    return Number.isSafeInteger(asNumber) ? asNumber : value.toString();
  }
  if (Array.isArray(value)) return value.map(serializeDbValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, serializeDbValue(item)]));
  }
  return value;
}

function mapDbRows<T extends ReportRow>(rows: T[]): ReportRow[] {
  return rows.map((row) => serializeDbValue(row) as ReportRow);
}

function buildDeviceScopeConditions(filters: ReportFilters, dateExpression?: Prisma.Sql): Prisma.Sql[] {
  const conditions: Prisma.Sql[] = [Prisma.sql`d.organization_id = ${filters.organizationId}`];
  if (filters.customerId) conditions.push(Prisma.sql`d.customer_id = ${filters.customerId}`);
  if (filters.siteId) conditions.push(Prisma.sql`d.site_id = ${filters.siteId}`);
  if (dateExpression && filters.from) conditions.push(Prisma.sql`${dateExpression} >= ${filters.from}`);
  if (dateExpression && filters.to) conditions.push(Prisma.sql`${dateExpression} <= ${filters.to}`);
  return conditions;
}

function whereSql(conditions: Prisma.Sql[]): Prisma.Sql {
  return Prisma.sql`WHERE ${Prisma.join(conditions, ' AND ')}`;
}

function onlineStatusSql(filters: ReportFilters): Prisma.Sql {
  return Prisma.sql`
    CASE
      WHEN d.last_seen >= NOW() - (${filters.offlineMinutes} * INTERVAL '1 minute') THEN 'online'
      ELSE 'offline'
    END
  `;
}

export class PrismaReportRepository implements ReportRepository {
  constructor(private readonly prisma: QueryablePrisma) {}

  async getFleetHealth(filters: ReportFilters): Promise<ReportRow[]> {
    const where = whereSql(buildDeviceScopeConditions(filters, Prisma.sql`d.last_seen`));
    const onlineStatus = onlineStatusSql(filters);
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          ${onlineStatus} AS "onlineStatus",
          COALESCE(ds.pending_updates_count, pending.pending_count, 0)::int AS "pendingUpdates",
          COALESCE(ds.reboot_required, false) AS "rebootRequired",
          COALESCE(events.critical_events, 0)::int AS "criticalEvents",
          COALESCE(rem.open_remediations, 0)::int AS "openRemediations",
          CASE
            WHEN d.last_seen < NOW() - (${filters.offlineMinutes} * INTERVAL '1 minute') THEN 'offline'
            WHEN COALESCE(events.critical_events, 0) > 0 THEN 'critical'
            WHEN COALESCE(ds.pending_updates_count, pending.pending_count, 0) > 0 OR COALESCE(ds.reboot_required, false) THEN 'attention'
            ELSE 'healthy'
          END AS "healthStatus"
        FROM public.rmm_devices d
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        LEFT JOIN rmm_telemetry.device_state ds ON ds.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS pending_count
          FROM rmm_telemetry.device_pending_update
          WHERE organization_id = ${filters.organizationId}
          GROUP BY agent_id
        ) pending ON pending.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS critical_events
          FROM rmm_telemetry.device_event
          WHERE organization_id = ${filters.organizationId}
            AND severity IN ('critical', 'error', 'warning')
            AND occurred_at >= NOW() - INTERVAL '7 days'
          GROUP BY agent_id
        ) events ON events.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS open_remediations
          FROM rmm_telemetry.remediation_job
          WHERE organization_id = ${filters.organizationId}
            AND status IN ('queued', 'running', 'pending')
          GROUP BY agent_id
        ) rem ON rem.agent_id = d.agent_id
        ${where}
        ORDER BY d.last_seen DESC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getPatchCompliance(filters: ReportFilters): Promise<ReportRow[]> {
    const where = whereSql(buildDeviceScopeConditions(filters, Prisma.sql`COALESCE(ds.collected_at, d.last_seen)`));
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          ds.collected_at AS "telemetryCollectedAt",
          COALESCE(ds.pending_updates_count, pending.pending_count, 0)::int AS "pendingUpdates",
          COALESCE(installed.installed_count, 0)::int AS "installedUpdates",
          COALESCE(ds.reboot_required, false) AS "rebootRequired",
          CASE
            WHEN COALESCE(ds.pending_updates_count, pending.pending_count, 0) = 0
              AND COALESCE(ds.reboot_required, false) = false THEN 'compliant'
            WHEN COALESCE(ds.reboot_required, false) = true THEN 'reboot_required'
            ELSE 'pending_updates'
          END AS "complianceStatus"
        FROM public.rmm_devices d
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        LEFT JOIN rmm_telemetry.device_state ds ON ds.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS pending_count
          FROM rmm_telemetry.device_pending_update
          WHERE organization_id = ${filters.organizationId}
          GROUP BY agent_id
        ) pending ON pending.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS installed_count
          FROM rmm_telemetry.device_installed_update
          WHERE organization_id = ${filters.organizationId}
          GROUP BY agent_id
        ) installed ON installed.agent_id = d.agent_id
        ${where}
        ORDER BY "pendingUpdates" DESC, d.hostname ASC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getDeviceInventory(filters: ReportFilters): Promise<ReportRow[]> {
    const where = whereSql(buildDeviceScopeConditions(filters, Prisma.sql`d.last_seen`));
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.ip,
          d.version,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          ds.collected_at AS "telemetryCollectedAt",
          ds.cpu_model AS "cpuModel",
          CASE
            WHEN ds.memory_total_bytes IS NULL THEN NULL
            ELSE ROUND((ds.memory_total_bytes::numeric / 1073741824), 2)::float
          END AS "memoryTotalGb",
          COALESCE(ds.installed_apps_count, app_counts.app_count, 0)::int AS "installedApps",
          COALESCE(ds.pending_updates_count, pending.pending_count, 0)::int AS "pendingUpdates"
        FROM public.rmm_devices d
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        LEFT JOIN rmm_telemetry.device_state ds ON ds.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS app_count
          FROM rmm_telemetry.device_installed_app
          WHERE organization_id = ${filters.organizationId}
          GROUP BY agent_id
        ) app_counts ON app_counts.agent_id = d.agent_id
        LEFT JOIN (
          SELECT agent_id, COUNT(*) AS pending_count
          FROM rmm_telemetry.device_pending_update
          WHERE organization_id = ${filters.organizationId}
          GROUP BY agent_id
        ) pending ON pending.agent_id = d.agent_id
        ${where}
        ORDER BY d.hostname ASC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getSoftwareInventory(filters: ReportFilters): Promise<ReportRow[]> {
    const conditions = [
      Prisma.sql`app.organization_id = ${filters.organizationId}`,
      ...buildDeviceScopeConditions(filters, Prisma.sql`app.collected_at`)
    ];
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          app.app_name AS "appName",
          app.publisher,
          app.version,
          app.install_date AS "installDate",
          app.source,
          app.is_64_bit AS "is64Bit",
          app.collected_at AS "collectedAt"
        FROM rmm_telemetry.device_installed_app app
        JOIN public.rmm_devices d
          ON d.agent_id = app.agent_id
         AND d.organization_id = app.organization_id
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        ${whereSql(conditions)}
        ORDER BY app.app_name ASC, d.hostname ASC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getAlertHistory(filters: ReportFilters): Promise<ReportRow[]> {
    const conditions = [
      Prisma.sql`event.organization_id = ${filters.organizationId}`,
      ...buildDeviceScopeConditions(filters, Prisma.sql`event.occurred_at`)
    ];
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          event.occurred_at AS "occurredAt",
          event.event_type AS "eventType",
          event.severity,
          event.source,
          event.message
        FROM rmm_telemetry.device_event event
        JOIN public.rmm_devices d
          ON d.agent_id = event.agent_id
         AND d.organization_id = event.organization_id
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        ${whereSql(conditions)}
        ORDER BY event.occurred_at DESC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getUptimeOffline(filters: ReportFilters): Promise<ReportRow[]> {
    const where = whereSql(buildDeviceScopeConditions(filters, Prisma.sql`d.last_seen`));
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          d.agent_id AS "agentId",
          d.hostname,
          d.os,
          d.last_seen AS "lastSeen",
          c.name AS "customerName",
          s.name AS "siteName",
          ${onlineStatusSql(filters)} AS "onlineStatus",
          GREATEST(FLOOR(EXTRACT(EPOCH FROM (NOW() - d.last_seen)) / 60), 0)::int AS "offlineMinutes",
          ${filters.offlineMinutes}::int AS "offlineThresholdMinutes"
        FROM public.rmm_devices d
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        ${where}
        ORDER BY d.last_seen ASC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getCommandRemediationOutcomes(filters: ReportFilters): Promise<ReportRow[]> {
    const commandConditions = [
      Prisma.sql`cmd.organization_id = ${filters.organizationId}`,
      ...buildDeviceScopeConditions(filters, Prisma.sql`cmd.created_at`)
    ];
    const remediationConditions = [
      Prisma.sql`job.organization_id = ${filters.organizationId}`,
      ...buildDeviceScopeConditions(filters, Prisma.sql`job.requested_at`)
    ];
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT *
        FROM (
          SELECT
            'command' AS "activityType",
            cmd.created_at AS "activityAt",
            d.agent_id AS "agentId",
            d.hostname,
            c.name AS "customerName",
            s.name AS "siteName",
            cmd.command AS "summary",
            CASE WHEN cmd.was_allowed THEN 'allowed' ELSE 'blocked' END AS "status",
            CASE
              WHEN cmd.was_allowed = false THEN COALESCE(cmd.denial_reason, 'blocked')
              WHEN cmd.exit_code IS NULL THEN 'submitted'
              ELSE 'exit ' || cmd.exit_code::text
            END AS "outcome"
          FROM public.command_execution_log cmd
          JOIN public.rmm_devices d ON d.agent_id = cmd.agent_id AND d.organization_id = cmd.organization_id
          LEFT JOIN public.customers c ON c.id = d.customer_id
          LEFT JOIN public.rmm_sites s ON s.id = d.site_id
          ${whereSql(commandConditions)}
          UNION ALL
          SELECT
            'remediation' AS "activityType",
            job.requested_at AS "activityAt",
            d.agent_id AS "agentId",
            d.hostname,
            c.name AS "customerName",
            s.name AS "siteName",
            job.intent_id AS "summary",
            job.status,
            COALESCE(job.metadata_jsonb->>'outcome', job.metadata_jsonb->>'message', job.status) AS "outcome"
          FROM rmm_telemetry.remediation_job job
          JOIN public.rmm_devices d ON d.agent_id = job.agent_id AND d.organization_id = job.organization_id
          LEFT JOIN public.customers c ON c.id = d.customer_id
          LEFT JOIN public.rmm_sites s ON s.id = d.site_id
          ${whereSql(remediationConditions)}
        ) activity
        ORDER BY "activityAt" DESC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }

  async getRemoteSupportActivity(filters: ReportFilters): Promise<ReportRow[]> {
    const conditions = [
      Prisma.sql`cmd.organization_id = ${filters.organizationId}`,
      Prisma.sql`(
        cmd.command ILIKE '%remote%'
        OR cmd.command ILIKE '%viewer%'
        OR cmd.command ILIKE '%desktop%'
        OR cmd.command ILIKE '%connect%'
        OR cmd.command ILIKE '%shell%'
        OR cmd.command ILIKE '%file transfer%'
        OR cmd.command ILIKE '%registry%'
      )`,
      ...buildDeviceScopeConditions(filters, Prisma.sql`cmd.created_at`)
    ];
    const rows = await this.prisma.$queryRaw<ReportRow[]>(
      Prisma.sql`
        SELECT
          cmd.created_at AS "activityAt",
          d.agent_id AS "agentId",
          d.hostname,
          c.name AS "customerName",
          s.name AS "siteName",
          cmd.command,
          cmd.was_allowed AS "wasAllowed",
          CASE
            WHEN cmd.was_allowed = false THEN COALESCE(cmd.denial_reason, 'blocked')
            WHEN cmd.exit_code IS NULL THEN 'submitted'
            ELSE 'exit ' || cmd.exit_code::text
          END AS "outcome"
        FROM public.command_execution_log cmd
        JOIN public.rmm_devices d ON d.agent_id = cmd.agent_id AND d.organization_id = cmd.organization_id
        LEFT JOIN public.customers c ON c.id = d.customer_id
        LEFT JOIN public.rmm_sites s ON s.id = d.site_id
        ${whereSql(conditions)}
        ORDER BY cmd.created_at DESC
        LIMIT ${filters.limit}
      `
    );
    return mapDbRows(rows);
  }
}

export async function generateReportRows(
  repository: ReportRepository,
  reportId: ReportId,
  filters: ReportFilters
): Promise<ReportRow[]> {
  switch (reportId) {
    case 'fleet_health':
      return repository.getFleetHealth(filters);
    case 'patch_compliance':
      return repository.getPatchCompliance(filters);
    case 'device_inventory':
      return repository.getDeviceInventory(filters);
    case 'software_inventory':
      return repository.getSoftwareInventory(filters);
    case 'alert_history':
      return repository.getAlertHistory(filters);
    case 'uptime_offline':
      return repository.getUptimeOffline(filters);
    case 'command_remediation_outcomes':
      return repository.getCommandRemediationOutcomes(filters);
    case 'remote_support_activity':
      return repository.getRemoteSupportActivity(filters);
    default:
      throw new ReportValidationError('Unknown reportId');
  }
}

function csvValue(value: unknown): string {
  if (value == null) return '';
  const normalized = value instanceof Date
    ? value.toISOString()
    : typeof value === 'object'
      ? JSON.stringify(value)
      : String(value);
  return /[",\r\n]/.test(normalized) ? `"${normalized.replace(/"/g, '""')}"` : normalized;
}

export function rowsToCsv(rows: ReportRow[], columns: ReportColumn[]): string {
  const header = columns.map((column) => csvValue(column.label)).join(',');
  const body = rows.map((row) => columns.map((column) => csvValue(row[column.key])).join(','));
  return [header, ...body].join('\r\n');
}

export function reportFilename(reportId: ReportId, extension: 'csv' | 'json' | 'pdf', now = new Date()): string {
  const date = now.toISOString().slice(0, 10);
  return `talos-${reportId.replace(/_/g, '-')}-${date}.${extension}`;
}
