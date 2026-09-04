import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  alertSeverityRank,
  buildDeviceListWhere,
  normalizeDeviceSavedViewState,
  parseBooleanFilter,
  parseDeviceListQuery,
  parseLastSeenAgeMinutes
} from './rmmDeviceList';

test('parseDeviceListQuery clamps pagination and normalizes filters', () => {
  const parsed = parseDeviceListQuery({
    page: '-10',
    pageSize: '9999',
    sortBy: 'customer',
    sortDirection: 'asc',
    status: 'offline',
    pendingUpdates: 'true',
    rebootRequired: '0',
    alertSeverity: 'warn',
    lastSeenAge: '7d',
    tagGroup: 'Production'
  });

  assert.equal(parsed.page, 1);
  assert.equal(parsed.pageSize, 500);
  assert.equal(parsed.sortBy, 'customer');
  assert.equal(parsed.sortDirection, 'asc');
  assert.equal(parsed.filters.status, 'offline');
  assert.equal(parsed.filters.pendingUpdates, true);
  assert.equal(parsed.filters.rebootRequired, false);
  assert.equal(parsed.filters.alertSeverity, 'warning');
  assert.equal(parsed.filters.lastSeenAgeMinutes, 10080);
  assert.equal(parsed.filters.tag, 'Production');
});

test('boolean and last-seen age filters accept operator-friendly values', () => {
  assert.equal(parseBooleanFilter('yes'), true);
  assert.equal(parseBooleanFilter('No'), false);
  assert.equal(parseBooleanFilter('all'), null);
  assert.equal(parseLastSeenAgeMinutes('15m'), 15);
  assert.equal(parseLastSeenAgeMinutes('2h'), 120);
  assert.equal(parseLastSeenAgeMinutes('3d'), 4320);
  assert.equal(parseLastSeenAgeMinutes('4w'), 40320);
});

test('buildDeviceListWhere includes org scoping and telemetry-backed filters', () => {
  const where = buildDeviceListWhere({
    organizationId: 'org-1',
    unassignedCustomerId: 'unassigned-org-1',
    now: new Date('2026-05-11T05:00:00Z'),
    filters: {
      status: 'online',
      pendingUpdates: true,
      rebootRequired: true,
      alertSeverity: 'error',
      lastSeenAgeMinutes: null,
      customerId: 'unassigned',
      siteId: 'none',
      os: 'Windows',
      version: '0.6',
      tag: 'VIP'
    }
  });

  const text = JSON.stringify(where);
  assert.match(text, /org-1/);
  assert.match(text, /unassigned-org-1/);
  assert.match(text, /pendingUpdatesCount/);
  assert.match(text, /rebootRequired/);
  assert.match(text, /telemetryEvents/);
  assert.match(text, /telemetryFactState/);
});

test('alert filters match warn aliases case-insensitively', () => {
  const where = buildDeviceListWhere({
    organizationId: 'org-1',
    unassignedCustomerId: 'unassigned-org-1',
    now: new Date('2026-05-11T05:00:00Z'),
    filters: {
      status: 'all',
      alertSeverity: 'warning',
      pendingUpdates: null,
      rebootRequired: null,
      lastSeenAgeMinutes: null
    }
  });

  const text = JSON.stringify(where);
  assert.match(text, /warn/);
  assert.match(text, /insensitive/);
  assert.equal(alertSeverityRank('WARN'), 2);
  assert.equal(alertSeverityRank('critical'), 4);
});

test('normalizeDeviceSavedViewState keeps only supported state fields', () => {
  const state = normalizeDeviceSavedViewState({
    pageSize: '25',
    sortBy: 'site',
    sortDirection: 'asc',
    filters: {
      q: 'server',
      customerId: 'cust-1',
      unsupported: 'ignored',
      alertSeverity: 'critical',
      pendingUpdates: '1'
    }
  });

  assert.deepEqual(state, {
    pageSize: 25,
    sortBy: 'site',
    sortDirection: 'asc',
    filters: {
      q: 'server',
      customerId: 'cust-1',
      siteId: undefined,
      status: 'all',
      os: undefined,
      version: undefined,
      tag: undefined,
      pendingUpdates: true,
      rebootRequired: null,
      alertSeverity: 'critical',
      lastSeenAgeMinutes: null
    }
  });
});
