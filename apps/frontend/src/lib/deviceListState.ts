import type {
  RmmDeviceListAlertSeverity,
  RmmDeviceListFilters,
  RmmDeviceListQuery,
  RmmDeviceListSortBy,
  RmmDeviceListSortDirection,
  RmmDeviceListStatusFilter
} from './types';

export const DEVICE_LIST_STATE_STORAGE_KEY = 'talos:rmm:device-list-state:v1';

export const DEFAULT_DEVICE_LIST_FILTERS: RmmDeviceListFilters = {
  status: 'all',
  pendingUpdates: null,
  rebootRequired: null,
  alertSeverity: null,
  lastSeenAgeMinutes: null
};

export const DEFAULT_DEVICE_LIST_STATE: RmmDeviceListQuery = {
  page: 1,
  pageSize: 50,
  sortBy: 'lastSeen',
  sortDirection: 'desc',
  filters: DEFAULT_DEVICE_LIST_FILTERS
};

const SORT_FIELDS = new Set<RmmDeviceListSortBy>([
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
]);

const STATUS_FIELDS = new Set<RmmDeviceListStatusFilter>(['all', 'online', 'offline']);
const ALERT_SEVERITIES = new Set<RmmDeviceListAlertSeverity>(['info', 'warning', 'error', 'critical']);

type StorageLike = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

const asRecord = (value: unknown): Record<string, unknown> | null =>
  value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;

const cleanString = (value: unknown, maxLength = 200): string | undefined => {
  if (value === undefined || value === null) return undefined;
  const text = String(value).trim();
  return text ? text.slice(0, maxLength) : undefined;
};

const cleanBoolean = (value: unknown): boolean | null => {
  if (typeof value === 'boolean') return value;
  if (value === undefined || value === null || value === '') return null;
  const text = String(value).trim().toLowerCase();
  if (['true', '1', 'yes'].includes(text)) return true;
  if (['false', '0', 'no'].includes(text)) return false;
  return null;
};

const cleanPositiveNumber = (value: unknown, fallback: number, min: number, max: number): number => {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(Math.max(Math.trunc(parsed), min), max);
};

export function normalizeDeviceListFilters(value: unknown): RmmDeviceListFilters {
  const record = asRecord(value) ?? {};
  const statusRaw = cleanString(record.status)?.toLowerCase() as RmmDeviceListStatusFilter | undefined;
  const alertRaw = cleanString(record.alertSeverity)?.toLowerCase();
  const lastSeenAge = record.lastSeenAgeMinutes === null || record.lastSeenAgeMinutes === undefined
    ? null
    : cleanPositiveNumber(record.lastSeenAgeMinutes, 0, 1, 525600);

  return {
    q: cleanString(record.q),
    customerId: cleanString(record.customerId),
    siteId: cleanString(record.siteId),
    status: statusRaw && STATUS_FIELDS.has(statusRaw) ? statusRaw : 'all',
    os: cleanString(record.os),
    version: cleanString(record.version),
    tag: cleanString(record.tag),
    pendingUpdates: cleanBoolean(record.pendingUpdates),
    rebootRequired: cleanBoolean(record.rebootRequired),
    alertSeverity: alertRaw && ALERT_SEVERITIES.has(alertRaw as RmmDeviceListAlertSeverity)
      ? alertRaw as RmmDeviceListAlertSeverity
      : null,
    lastSeenAgeMinutes: lastSeenAge && lastSeenAge > 0 ? lastSeenAge : null
  };
}

export function normalizeDeviceListState(value: unknown): RmmDeviceListQuery {
  const record = asRecord(value) ?? {};
  const sortByRaw = cleanString(record.sortBy) as RmmDeviceListSortBy | undefined;
  const sortDirectionRaw = cleanString(record.sortDirection)?.toLowerCase() as RmmDeviceListSortDirection | undefined;

  return {
    page: cleanPositiveNumber(record.page, DEFAULT_DEVICE_LIST_STATE.page, 1, 100000),
    pageSize: cleanPositiveNumber(record.pageSize, DEFAULT_DEVICE_LIST_STATE.pageSize, 1, 500),
    sortBy: sortByRaw && SORT_FIELDS.has(sortByRaw) ? sortByRaw : DEFAULT_DEVICE_LIST_STATE.sortBy,
    sortDirection: sortDirectionRaw === 'asc' ? 'asc' : 'desc',
    filters: normalizeDeviceListFilters(record.filters)
  };
}

export function encodeDeviceListState(state: RmmDeviceListQuery): string {
  return JSON.stringify(normalizeDeviceListState(state));
}

export function decodeDeviceListState(raw: string | null): RmmDeviceListQuery | null {
  if (!raw) return null;
  try {
    return normalizeDeviceListState(JSON.parse(raw));
  } catch {
    return null;
  }
}

export function loadDeviceListState(storage: StorageLike): RmmDeviceListQuery | null {
  return decodeDeviceListState(storage.getItem(DEVICE_LIST_STATE_STORAGE_KEY));
}

export function saveDeviceListState(storage: StorageLike, state: RmmDeviceListQuery): void {
  storage.setItem(DEVICE_LIST_STATE_STORAGE_KEY, encodeDeviceListState(state));
}

export function clearDeviceListState(storage: StorageLike): void {
  storage.removeItem(DEVICE_LIST_STATE_STORAGE_KEY);
}
