import { describe, expect, test } from 'bun:test';
import {
  generateReportRows,
  normalizeReportFilters,
  REPORT_DEFINITION_BY_ID,
  ReportFilters,
  ReportRepository,
  ReportRow,
  rowsToCsv
} from './reports';

class RecordingRepository implements ReportRepository {
  public lastFilters: ReportFilters | null = null;

  private capture(filters: ReportFilters, rows: ReportRow[] = []) {
    this.lastFilters = filters;
    return Promise.resolve(rows);
  }

  getFleetHealth(filters: ReportFilters) {
    return this.capture(filters);
  }

  getPatchCompliance(filters: ReportFilters) {
    return this.capture(filters, [
      {
        agentId: 'agent-1',
        hostname: 'alpha',
        customerName: 'Acme',
        siteName: 'HQ',
        os: 'Windows',
        lastSeen: '2026-05-10T10:00:00.000Z',
        pendingUpdates: 2,
        installedUpdates: 41,
        rebootRequired: true,
        complianceStatus: 'reboot_required'
      }
    ]);
  }

  getDeviceInventory(filters: ReportFilters) {
    return this.capture(filters, [
      {
        agentId: 'agent-2',
        hostname: 'beta',
        customerName: 'Contoso',
        siteName: 'Warehouse',
        os: 'Windows 11',
        lastSeen: '2026-05-10T11:00:00.000Z'
      }
    ]);
  }

  getSoftwareInventory(filters: ReportFilters) {
    return this.capture(filters);
  }

  getAlertHistory(filters: ReportFilters) {
    return this.capture(filters);
  }

  getUptimeOffline(filters: ReportFilters) {
    return this.capture(filters);
  }

  getCommandRemediationOutcomes(filters: ReportFilters) {
    return this.capture(filters);
  }

  getRemoteSupportActivity(filters: ReportFilters) {
    return this.capture(filters);
  }
}

describe('reports', () => {
  test('normalizes report query filters and passes them to inventory generation', async () => {
    const filters = normalizeReportFilters('org-1', {
      from: '2026-05-01T00:00:00.000Z',
      to: '2026-05-10T23:59:59.000Z',
      customerId: 'cust-1',
      siteId: 'site-2',
      limit: '25',
      offlineMinutes: '30'
    });
    const repository = new RecordingRepository();

    const rows = await generateReportRows(repository, 'device_inventory', filters);

    expect(rows).toHaveLength(1);
    expect(repository.lastFilters?.organizationId).toBe('org-1');
    expect(repository.lastFilters?.customerId).toBe('cust-1');
    expect(repository.lastFilters?.siteId).toBe('site-2');
    expect(repository.lastFilters?.from?.toISOString()).toBe('2026-05-01T00:00:00.000Z');
    expect(repository.lastFilters?.to?.toISOString()).toBe('2026-05-10T23:59:59.000Z');
    expect(repository.lastFilters?.limit).toBe(25);
    expect(repository.lastFilters?.offlineMinutes).toBe(30);
  });

  test('rejects inverted date ranges before report execution', () => {
    expect(() =>
      normalizeReportFilters('org-1', {
        from: '2026-05-11T00:00:00.000Z',
        to: '2026-05-10T00:00:00.000Z'
      })
    ).toThrow('from must be before to');
  });

  test('exports patch compliance rows as CSV with stable headers and escaping', async () => {
    const filters = normalizeReportFilters('org-1', { customerId: 'cust-1' });
    const repository = new RecordingRepository();
    const rows = await generateReportRows(repository, 'patch_compliance', filters);
    const definition = REPORT_DEFINITION_BY_ID.get('patch_compliance')!;

    const csv = rowsToCsv(
      [
        {
          ...rows[0],
          hostname: 'alpha, primary',
          complianceStatus: 'needs "reboot"'
        }
      ],
      definition.columns
    );

    expect(csv.split('\r\n')[0]).toContain('Agent ID,Hostname,Customer,Site,OS');
    expect(csv).toContain('"alpha, primary"');
    expect(csv).toContain('"needs ""reboot"""');
    expect(csv).toContain('true');
  });
});
