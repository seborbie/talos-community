import { Prisma } from '@prisma/client';

export const DEVICE_LIST_DEFAULT_PAGE_SIZE = 50;
export const DEVICE_LIST_MAX_PAGE_SIZE = 500;
export const DEVICE_LIST_ONLINE_THRESHOLD_MS = 5 * 60 * 1000;
export const DEVICE_LIST_ALERT_WINDOW_DAYS = 7;

export const DEVICE_LIST_SORT_FIELDS = [
  'hostname',
  'customer',
  'site',
  'os',
  'version',
  'lastSeen',
  'status',
  'pendingUpdates',
  'rebootRequired',
  'alertSeverity'
] as const;

export type DeviceListSortBy = (typeof DEVICE_LIST_SORT_FIELDS)[number];
export type DeviceListSortDirection = 'asc' | 'desc';
export type DeviceListStatusFilter = 'all' | 'online' | 'offline';
export type DeviceListAlertSeverityFilter = 'info' | 'warning' | 'error' | 'critical';

export type DeviceListFilters = {
  q?: string;
  customerId?: string;
  siteId?: string;
  status: DeviceListStatusFilter;
  os?: string;
  version?: string;
  tag?: string;
  pendingUpdates?: boolean | null;
  rebootRequired?: boolean | null;
  alertSeverity?: DeviceListAlertSeverityFilter | null;
  lastSeenAgeMinutes?: number | null;
};

export type ParsedDeviceListQuery = {
  page: number;
  pageSize: number;
  sortBy: DeviceListSortBy;
  sortDirection: DeviceListSortDirection;
  filters: DeviceListFilters;
};

export type DeviceListWhereOptions = {
  organizationId: string;
  unassignedCustomerId: string;
  now?: Date;
  filters: DeviceListFilters;
};

export type DeviceSavedViewState = {
  filters: DeviceListFilters;
  sortBy: DeviceListSortBy;
  sortDirection: DeviceListSortDirection;
  pageSize: number;
};

const DEFAULT_FILTERS: DeviceListFilters = {
  status: 'all',
  pendingUpdates: null,
  rebootRequired: null,
  alertSeverity: null,
  lastSeenAgeMinutes: null
};

const ALERT_SEVERITY_ORDER: Record<DeviceListAlertSeverityFilter, string[]> = {
  info: ['info', 'warning', 'warn', 'error', 'critical'],
  warning: ['warning', 'warn', 'error', 'critical'],
  error: ['error', 'critical'],
  critical: ['critical']
};

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function firstQueryValue(value: unknown): string | null {
  const selected = Array.isArray(value) ? value[0] : value;
  if (selected === undefined || selected === null) return null;
  const text = String(selected).trim();
  return text || null;
}

function clampInteger(value: unknown, fallback: number, min: number, max: number): number {
  const text = firstQueryValue(value);
  if (!text) return fallback;
  const parsed = Number(text);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(Math.max(Math.trunc(parsed), min), max);
}

function parseSortBy(value: unknown): DeviceListSortBy {
  const text = firstQueryValue(value);
  if (text && DEVICE_LIST_SORT_FIELDS.includes(text as DeviceListSortBy)) {
    return text as DeviceListSortBy;
  }
  return 'lastSeen';
}

function parseSortDirection(value: unknown): DeviceListSortDirection {
  const text = firstQueryValue(value)?.toLowerCase();
  return text === 'asc' ? 'asc' : 'desc';
}

function parseStatus(value: unknown): DeviceListStatusFilter {
  const text = firstQueryValue(value)?.toLowerCase();
  if (text === 'online' || text === 'offline') return text;
  return 'all';
}

function parseAlertSeverity(value: unknown): DeviceListAlertSeverityFilter | null {
  const text = firstQueryValue(value)?.toLowerCase();
  if (text === 'warn') return 'warning';
  if (text === 'info' || text === 'warning' || text === 'error' || text === 'critical') return text;
  return null;
}

export function parseBooleanFilter(value: unknown): boolean | null {
  const text = firstQueryValue(value)?.toLowerCase();
  if (!text || text === 'all' || text === 'any') return null;
  if (['1', 'true', 'yes', 'y'].includes(text)) return true;
  if (['0', 'false', 'no', 'n'].includes(text)) return false;
  return null;
}

export function parseLastSeenAgeMinutes(value: unknown): number | null {
  const text = firstQueryValue(value)?.toLowerCase();
  if (!text || text === 'all' || text === 'any') return null;

  const numeric = Number(text);
  if (Number.isFinite(numeric) && numeric > 0) {
    return Math.min(Math.trunc(numeric), 525600);
  }

  const match = /^(\d+)\s*([mhdw])$/.exec(text);
  if (!match) return null;

  const amount = Number(match[1]);
  if (!Number.isFinite(amount) || amount <= 0) return null;

  const multiplier =
    match[2] === 'm' ? 1 :
      match[2] === 'h' ? 60 :
        match[2] === 'd' ? 1440 :
          10080;

  return Math.min(amount * multiplier, 525600);
}

function readTextFilter(query: Record<string, unknown>, ...keys: string[]): string | undefined {
  for (const key of keys) {
    const value = firstQueryValue(query[key]);
    if (value) return value.slice(0, 200);
  }
  return undefined;
}

export function parseDeviceListQuery(queryInput: Record<string, unknown>): ParsedDeviceListQuery {
  const query = asRecord(queryInput) ?? {};
  const page = clampInteger(query.page, 1, 1, 100000);
  const pageSize = clampInteger(
    query.pageSize ?? query.limit,
    DEVICE_LIST_DEFAULT_PAGE_SIZE,
    1,
    DEVICE_LIST_MAX_PAGE_SIZE
  );
  const sortBy = parseSortBy(query.sortBy);
  const sortDirection = parseSortDirection(query.sortDirection ?? query.sortDir);
  const filters: DeviceListFilters = {
    ...DEFAULT_FILTERS,
    q: readTextFilter(query, 'q', 'search'),
    customerId: readTextFilter(query, 'customerId', 'customer'),
    siteId: readTextFilter(query, 'siteId', 'site'),
    status: parseStatus(query.status),
    os: readTextFilter(query, 'os'),
    version: readTextFilter(query, 'version', 'agentVersion'),
    tag: readTextFilter(query, 'tag', 'tags', 'group', 'tagGroup'),
    pendingUpdates: parseBooleanFilter(query.pendingUpdates),
    rebootRequired: parseBooleanFilter(query.rebootRequired),
    alertSeverity: parseAlertSeverity(query.alertSeverity),
    lastSeenAgeMinutes: parseLastSeenAgeMinutes(query.lastSeenAgeMinutes ?? query.lastSeenAge)
  };

  return { page, pageSize, sortBy, sortDirection, filters };
}

const containsInsensitive = (value: string): Prisma.StringFilter => ({
  contains: value,
  mode: 'insensitive'
});

function noPendingUpdatesWhere(): Prisma.RmmDeviceWhereInput {
  return {
    OR: [
      { telemetryState: { is: null } },
      { telemetryState: { is: { pendingUpdatesCount: null } } },
      { telemetryState: { is: { pendingUpdatesCount: { lte: 0 } } } }
    ]
  };
}

function noRebootRequiredWhere(): Prisma.RmmDeviceWhereInput {
  return {
    OR: [
      { telemetryState: { is: null } },
      { telemetryState: { is: { rebootRequired: null } } },
      { telemetryState: { is: { rebootRequired: false } } }
    ]
  };
}

function alertSeverityWhere(severities: string[]): Prisma.RmmTelemetryDeviceEventWhereInput[] {
  return severities.map((severity) => ({
    severity: { equals: severity, mode: 'insensitive' }
  }));
}

export function alertSeverityRank(value: unknown): number {
  const severity = typeof value === 'string' ? value.trim().toLowerCase() : '';
  switch (severity) {
    case 'critical':
      return 4;
    case 'error':
      return 3;
    case 'warning':
    case 'warn':
      return 2;
    case 'info':
      return 1;
    default:
      return 0;
  }
}

export function buildDeviceListWhere(options: DeviceListWhereOptions): Prisma.RmmDeviceWhereInput {
  const now = options.now ?? new Date();
  const filters = options.filters;
  const and: Prisma.RmmDeviceWhereInput[] = [{ organizationId: options.organizationId }];

  if (filters.q) {
    const q = containsInsensitive(filters.q);
    and.push({
      OR: [
        { agentId: q },
        { hostname: q },
        { os: q },
        { ip: q },
        { version: q },
        { customer: { is: { name: q } } },
        { site: { is: { name: q } } }
      ]
    });
  }

  if (filters.customerId) {
    if (filters.customerId === 'unassigned') {
      and.push({
        OR: [
          { customerId: null },
          { customerId: options.unassignedCustomerId }
        ]
      });
    } else if (filters.customerId !== 'all') {
      and.push({ customerId: filters.customerId });
    }
  }

  if (filters.siteId) {
    if (filters.siteId === 'none') {
      and.push({ siteId: null });
    } else if (filters.siteId !== 'all') {
      and.push({ siteId: filters.siteId });
    }
  }

  if (filters.status !== 'all') {
    const threshold = new Date(now.getTime() - DEVICE_LIST_ONLINE_THRESHOLD_MS);
    and.push({
      lastSeen: filters.status === 'online' ? { gte: threshold } : { lt: threshold }
    });
  }

  if (filters.os) {
    const os = containsInsensitive(filters.os);
    and.push({
      OR: [
        { os },
        { telemetryState: { is: { osName: os } } },
        { telemetryState: { is: { osVersion: os } } }
      ]
    });
  }

  if (filters.version) {
    const version = containsInsensitive(filters.version);
    and.push({
      OR: [
        { version },
        { telemetryState: { is: { agentVersion: version } } }
      ]
    });
  }

  if (filters.tag) {
    and.push({
      telemetryFactState: {
        some: {
          organizationId: options.organizationId,
          factValueText: containsInsensitive(filters.tag),
          OR: [
            { factKey: containsInsensitive('tag') },
            { factKey: containsInsensitive('group') }
          ]
        }
      }
    });
  }

  if (filters.pendingUpdates === true) {
    and.push({ telemetryState: { is: { pendingUpdatesCount: { gt: 0 } } } });
  } else if (filters.pendingUpdates === false) {
    and.push(noPendingUpdatesWhere());
  }

  if (filters.rebootRequired === true) {
    and.push({ telemetryState: { is: { rebootRequired: true } } });
  } else if (filters.rebootRequired === false) {
    and.push(noRebootRequiredWhere());
  }

  if (filters.alertSeverity) {
    const since = new Date(now.getTime() - DEVICE_LIST_ALERT_WINDOW_DAYS * 24 * 60 * 60 * 1000);
    and.push({
      telemetryEvents: {
        some: {
          organizationId: options.organizationId,
          occurredAt: { gte: since },
          OR: alertSeverityWhere(ALERT_SEVERITY_ORDER[filters.alertSeverity])
        }
      }
    });
  }

  if (filters.lastSeenAgeMinutes) {
    const before = new Date(now.getTime() - filters.lastSeenAgeMinutes * 60 * 1000);
    and.push({ lastSeen: { lte: before } });
  }

  return and.length === 1 ? and[0]! : { AND: and };
}

export function buildDeviceListOrderBy(
  sortBy: DeviceListSortBy,
  sortDirection: DeviceListSortDirection
): Prisma.RmmDeviceOrderByWithRelationInput[] {
  const direction = sortDirection;
  switch (sortBy) {
    case 'hostname':
      return [{ hostname: direction }, { agentId: 'asc' }];
    case 'customer':
      return [{ customer: { name: direction } }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'site':
      return [{ site: { name: direction } }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'os':
      return [{ os: direction }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'version':
      return [{ version: direction }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'pendingUpdates':
      return [{ telemetryState: { pendingUpdatesCount: direction } }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'rebootRequired':
      return [{ telemetryState: { rebootRequired: direction } }, { hostname: 'asc' }, { agentId: 'asc' }];
    case 'status':
    case 'lastSeen':
    case 'alertSeverity':
    default:
      return [{ lastSeen: direction }, { agentId: 'asc' }];
  }
}

export function normalizeDeviceSavedViewState(input: unknown): DeviceSavedViewState {
  const record = asRecord(input) ?? {};
  const filterRecord = asRecord(record.filters) ?? record;
  const parsed = parseDeviceListQuery({
    ...filterRecord,
    pageSize: record.pageSize ?? record.limit,
    sortBy: record.sortBy,
    sortDirection: record.sortDirection ?? record.sortDir
  });

  return {
    filters: parsed.filters,
    sortBy: parsed.sortBy,
    sortDirection: parsed.sortDirection,
    pageSize: parsed.pageSize
  };
}

export function cleanSavedViewName(value: unknown): string {
  const name = firstQueryValue(value);
  return name ? name.slice(0, 80) : '';
}
