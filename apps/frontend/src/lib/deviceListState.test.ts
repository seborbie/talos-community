import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  decodeDeviceListState,
  encodeDeviceListState,
  loadDeviceListState,
  normalizeDeviceListState,
  saveDeviceListState
} from './deviceListState';

class MemoryStorage {
  private readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

test('normalizeDeviceListState clamps persisted state and drops unsupported fields', () => {
  const state = normalizeDeviceListState({
    page: '3',
    pageSize: '9999',
    sortBy: 'pendingUpdates',
    sortDirection: 'asc',
    filters: {
      q: 'server',
      status: 'offline',
      customerId: 'cust-1',
      siteId: 'none',
      pendingUpdates: 'true',
      rebootRequired: 'false',
      alertSeverity: 'error',
      lastSeenAgeMinutes: '1440',
      unsupported: 'ignored'
    }
  });

  assert.equal(state.page, 3);
  assert.equal(state.pageSize, 500);
  assert.equal(state.sortBy, 'pendingUpdates');
  assert.equal(state.sortDirection, 'asc');
  assert.equal(state.filters.q, 'server');
  assert.equal(state.filters.status, 'offline');
  assert.equal(state.filters.pendingUpdates, true);
  assert.equal(state.filters.rebootRequired, false);
  assert.equal(state.filters.alertSeverity, 'error');
  assert.equal(state.filters.lastSeenAgeMinutes, 1440);
});

test('device list state round-trips through local storage helpers', () => {
  const storage = new MemoryStorage();
  const state = normalizeDeviceListState({
    page: 2,
    pageSize: 25,
    sortBy: 'hostname',
    sortDirection: 'asc',
    filters: {
      status: 'online',
      tag: 'VIP',
      os: 'Windows'
    }
  });

  saveDeviceListState(storage, state);
  assert.deepEqual(loadDeviceListState(storage), state);
  assert.deepEqual(decodeDeviceListState(encodeDeviceListState(state)), state);
});

test('decodeDeviceListState ignores corrupt persisted JSON', () => {
  assert.equal(decodeDeviceListState('{not json'), null);
});
