<script lang="ts">
  import { onMount } from 'svelte';
  import Button from '$lib/ui/Button.svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { customerApi, reportApi, siteApi } from '$lib/api';
  import type {
    Customer,
    RmmReportDataResponse,
    RmmReportDefinition,
    RmmReportFilters,
    RmmReportFormat,
    RmmReportFrequency,
    RmmReportId,
    RmmReportRun,
    RmmReportSchedule,
    Site
  } from '$lib/types';
  import { CalendarClock, Download, FileText, Play, RefreshCw, Trash2 } from 'lucide-svelte';

  let definitions: RmmReportDefinition[] = [];
  let customers: Customer[] = [];
  let sites: Site[] = [];
  let runs: RmmReportRun[] = [];
  let schedules: RmmReportSchedule[] = [];

  let selectedReportId: RmmReportId = 'fleet_health';
  let reportData: RmmReportDataResponse | null = null;
  let loading = true;
  let generating = false;
  let exporting = false;
  let creatingRun = false;
  let creatingSchedule = false;
  let deletingScheduleId: string | null = null;
  let error: string | null = null;
  let actionMessage: string | null = null;

  let fromDate = '';
  let toDate = '';
  let customerId = '';
  let siteId = '';
  let limit = '250';
  let offlineMinutes = '5';

  let scheduleName = '';
  let scheduleFormat: RmmReportFormat = 'csv';
  let scheduleFrequency: RmmReportFrequency = 'weekly';
  let scheduleEmailTo = '';

  $: selectedDefinition = definitions.find((definition) => definition.id === selectedReportId) ?? definitions[0] ?? null;
  $: selectedReportData = reportData?.definition.id === selectedReportId ? reportData : null;
  $: visibleColumns = selectedReportData?.definition.columns ?? selectedDefinition?.columns ?? [];
  $: visibleRows = selectedReportData?.items ?? [];
  $: sitesForCustomer = customerId ? sites.filter((site) => site.customerId === customerId) : sites;
  $: if (siteId && !sitesForCustomer.some((site) => site.id === siteId)) {
    siteId = '';
  }

  const buildFilters = (): RmmReportFilters => ({
    from: fromDate || null,
    to: toDate || null,
    customerId: customerId || null,
    siteId: siteId || null,
    limit: limit || 250,
    offlineMinutes: offlineMinutes || 5
  });

  const formatDate = (value: string | null | undefined) => {
    if (!value) return '—';
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? '—' : new Date(parsed).toLocaleString();
  };

  const reportName = (reportId: RmmReportId | string) =>
    definitions.find((definition) => definition.id === reportId)?.name ?? reportId.replaceAll('_', ' ');

  const cellValue = (row: Record<string, unknown>, key: string) => {
    const value = row[key];
    if (value == null || value === '') return '—';
    if (typeof value === 'boolean') return value ? 'Yes' : 'No';
    if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T/.test(value)) return formatDate(value);
    if (typeof value === 'object') return JSON.stringify(value);
    return String(value);
  };

  const saveBlobFile = (filename: string, blob: Blob) => {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const refreshRunsAndSchedules = async () => {
    const [runRows, scheduleRows] = await Promise.all([
      reportApi.listRuns(),
      reportApi.listSchedules()
    ]);
    runs = runRows;
    schedules = scheduleRows;
  };

  const loadReport = async () => {
    try {
      generating = true;
      error = null;
      actionMessage = null;
      reportData = await reportApi.generateReport(selectedReportId, buildFilters());
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load report';
    } finally {
      generating = false;
    }
  };

  const loadAll = async () => {
    try {
      loading = true;
      error = null;
      const [definitionRows, customerRows, siteRows] = await Promise.all([
        reportApi.listDefinitions(),
        customerApi.getCustomers(),
        siteApi.getSites()
      ]);
      definitions = definitionRows;
      customers = customerRows;
      sites = siteRows;
      selectedReportId = definitions[0]?.id ?? 'fleet_health';
      await Promise.all([loadReport(), refreshRunsAndSchedules()]);
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load reports';
    } finally {
      loading = false;
    }
  };

  const exportCsv = async () => {
    try {
      exporting = true;
      error = null;
      const result = await reportApi.downloadCsv(selectedReportId, buildFilters());
      saveBlobFile(result.filename, result.blob);
      actionMessage = `Downloaded ${reportName(selectedReportId)} CSV`;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to export CSV';
    } finally {
      exporting = false;
    }
  };

  const createRun = async (format: RmmReportFormat) => {
    try {
      creatingRun = true;
      error = null;
      const created = await reportApi.createRun({
        reportId: selectedReportId,
        format,
        filters: buildFilters()
      });
      actionMessage = format === 'pdf'
        ? 'PDF report run stored with delivery stubbed'
        : `Stored ${created.run.rowCount} row report run`;
      await refreshRunsAndSchedules();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to create report run';
    } finally {
      creatingRun = false;
    }
  };

  const downloadRunCsv = async (run: RmmReportRun) => {
    try {
      exporting = true;
      const result = await reportApi.downloadRunCsv(run.id);
      saveBlobFile(result.filename, result.blob);
      actionMessage = `Downloaded ${reportName(run.reportId)} run CSV`;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to download report run';
    } finally {
      exporting = false;
    }
  };

  const createSchedule = async () => {
    try {
      creatingSchedule = true;
      error = null;
      const emailTo = scheduleEmailTo
        .split(',')
        .map((item) => item.trim())
        .filter(Boolean);
      const created = await reportApi.createSchedule({
        name: scheduleName.trim() || undefined,
        reportId: selectedReportId,
        format: scheduleFormat,
        frequency: scheduleFrequency,
        filters: buildFilters(),
        emailTo
      });
      scheduleName = '';
      actionMessage = `Scheduled ${created.name}`;
      await refreshRunsAndSchedules();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to create schedule';
    } finally {
      creatingSchedule = false;
    }
  };

  const deleteSchedule = async (schedule: RmmReportSchedule) => {
    if (!confirm(`Delete schedule "${schedule.name}"?`)) return;
    try {
      deletingScheduleId = schedule.id;
      await reportApi.deleteSchedule(schedule.id);
      actionMessage = `Deleted schedule "${schedule.name}"`;
      await refreshRunsAndSchedules();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to delete schedule';
    } finally {
      deletingScheduleId = null;
    }
  };

  onMount(() => {
    void loadAll();
  });
</script>

<div class="space-y-6">
  <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">Reports</h1>
      <p class="text-sm aero-muted mt-1">Fleet, inventory, patch, alert, support, and remediation reporting.</p>
    </div>
    <div class="flex flex-wrap gap-2">
      <Button variant="outline" on:click={loadAll} disabled={loading || generating}>
        <RefreshCw class="mr-2 h-4 w-4" />
        Refresh
      </Button>
      <Button on:click={exportCsv} disabled={exporting || generating || loading}>
        <Download class="mr-2 h-4 w-4" />
        {exporting ? 'Exporting...' : 'CSV'}
      </Button>
    </div>
  </div>

  {#if error}
    <div class="rounded border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">{error}</div>
  {/if}
  {#if actionMessage}
    <div class="rounded border border-green-200 bg-green-50 px-4 py-3 text-sm text-green-700">{actionMessage}</div>
  {/if}

  <div class="grid gap-6 xl:grid-cols-[minmax(0,1fr)_360px]">
    <div class="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>Report Builder</CardTitle>
          <CardDescription>Generate filtered report data and export CSV files.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            <div class="space-y-2">
              <Label for="report">Report</Label>
              <select id="report" bind:value={selectedReportId} class="input-like">
                {#each definitions as definition}
                  <option value={definition.id}>{definition.name}</option>
                {/each}
              </select>
            </div>
            <div class="space-y-2">
              <Label for="fromDate">From</Label>
              <Input id="fromDate" type="date" bind:value={fromDate} />
            </div>
            <div class="space-y-2">
              <Label for="toDate">To</Label>
              <Input id="toDate" type="date" bind:value={toDate} />
            </div>
            <div class="space-y-2">
              <Label for="customer">Customer</Label>
              <select id="customer" bind:value={customerId} class="input-like">
                <option value="">All customers</option>
                {#each customers as customer}
                  <option value={customer.id}>{customer.name}</option>
                {/each}
              </select>
            </div>
            <div class="space-y-2">
              <Label for="site">Site</Label>
              <select id="site" bind:value={siteId} class="input-like">
                <option value="">All sites</option>
                {#each sitesForCustomer as site}
                  <option value={site.id}>{site.name}</option>
                {/each}
              </select>
            </div>
            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-2">
                <Label for="limit">Rows</Label>
                <Input id="limit" type="number" min="1" max="5000" bind:value={limit} />
              </div>
              <div class="space-y-2">
                <Label for="offlineMinutes">Offline</Label>
                <Input id="offlineMinutes" type="number" min="1" bind:value={offlineMinutes} />
              </div>
            </div>
          </div>
          <div class="mt-4 flex flex-wrap gap-2">
            <Button on:click={loadReport} disabled={generating || loading}>
              <Play class="mr-2 h-4 w-4" />
              {generating ? 'Generating...' : 'Generate'}
            </Button>
            <Button variant="outline" on:click={() => createRun('json')} disabled={creatingRun || generating || loading}>
              <FileText class="mr-2 h-4 w-4" />
              Save Run
            </Button>
            <Button variant="outline" on:click={() => createRun('pdf')} disabled={creatingRun || generating || loading}>
              <FileText class="mr-2 h-4 w-4" />
              Save PDF Stub
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>{selectedDefinition?.name ?? 'Report'} Results</CardTitle>
          <CardDescription>{visibleRows.length} rows returned from current telemetry and device tables.</CardDescription>
        </CardHeader>
        <CardContent>
          {#if loading || generating}
            <div class="aero-empty-state py-8 text-center">Loading report data...</div>
          {:else if visibleRows.length === 0}
            <div class="aero-empty-state py-8 text-center">No report rows match the selected filters.</div>
          {:else}
            <div class="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    {#each visibleColumns as column}
                      <TableHead>{column.label}</TableHead>
                    {/each}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {#each visibleRows as row}
                    <TableRow>
                      {#each visibleColumns as column}
                        <TableCell>{cellValue(row, column.key)}</TableCell>
                      {/each}
                    </TableRow>
                  {/each}
                </TableBody>
              </Table>
            </div>
          {/if}
        </CardContent>
      </Card>
    </div>

    <div class="space-y-6">
      <Card>
        <CardHeader>
          <CardTitle>Schedule</CardTitle>
          <CardDescription>Store recurring CSV or PDF report configurations.</CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <div class="space-y-2">
            <Label for="scheduleName">Name</Label>
            <Input id="scheduleName" bind:value={scheduleName} placeholder={selectedDefinition?.name ?? 'Report'} />
          </div>
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-2">
              <Label for="scheduleFrequency">Frequency</Label>
              <select id="scheduleFrequency" bind:value={scheduleFrequency} class="input-like">
                <option value="daily">Daily</option>
                <option value="weekly">Weekly</option>
                <option value="monthly">Monthly</option>
              </select>
            </div>
            <div class="space-y-2">
              <Label for="scheduleFormat">Format</Label>
              <select id="scheduleFormat" bind:value={scheduleFormat} class="input-like">
                <option value="csv">CSV</option>
                <option value="pdf">PDF</option>
              </select>
            </div>
          </div>
          <div class="space-y-2">
            <Label for="scheduleEmailTo">Email Recipients</Label>
            <Input id="scheduleEmailTo" bind:value={scheduleEmailTo} placeholder="ops@example.com, client@example.com" />
          </div>
          <Button class="w-full" on:click={createSchedule} disabled={creatingSchedule || loading}>
            <CalendarClock class="mr-2 h-4 w-4" />
            {creatingSchedule ? 'Saving...' : 'Save Schedule'}
          </Button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Scheduled Reports</CardTitle>
          <CardDescription>{schedules.length} stored configurations.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="space-y-3">
            {#each schedules as schedule}
              <div class="rounded border border-white/10 p-3">
                <div class="flex items-start justify-between gap-3">
                  <div>
                    <div class="font-medium">{schedule.name}</div>
                    <div class="text-xs aero-muted">{reportName(schedule.reportId)} · {schedule.frequency} · {schedule.format.toUpperCase()}</div>
                    <div class="text-xs aero-muted">Next: {formatDate(schedule.nextRunAt)}</div>
                    <div class="text-xs aero-muted">Delivery: {schedule.emailDeliveryStatus}</div>
                  </div>
                  <button
                    type="button"
                    class="icon-btn"
                    aria-label="Delete schedule"
                    disabled={deletingScheduleId === schedule.id}
                    on:click={() => deleteSchedule(schedule)}
                  >
                    <Trash2 class="h-4 w-4" />
                  </button>
                </div>
              </div>
            {:else}
              <div class="aero-empty-state py-5 text-center">No scheduled reports.</div>
            {/each}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Recent Runs</CardTitle>
          <CardDescription>{runs.length} generated report runs.</CardDescription>
        </CardHeader>
        <CardContent>
          <div class="space-y-3">
            {#each runs.slice(0, 8) as run}
              <div class="rounded border border-white/10 p-3">
                <div class="flex items-center justify-between gap-3">
                  <div>
                    <div class="font-medium">{reportName(run.reportId)}</div>
                    <div class="text-xs aero-muted">{run.format.toUpperCase()} · {run.rowCount} rows · {run.deliveryStatus}</div>
                    <div class="text-xs aero-muted">{formatDate(run.createdAt)}</div>
                  </div>
                  <button type="button" class="text-xs aero-link hover:underline" on:click={() => downloadRunCsv(run)}>CSV</button>
                </div>
              </div>
            {:else}
              <div class="aero-empty-state py-5 text-center">No report runs yet.</div>
            {/each}
          </div>
        </CardContent>
      </Card>
    </div>
  </div>
</div>

<style>
  .input-like {
    width: 100%;
    min-height: 2.5rem;
    border-radius: 0.375rem;
    border: 1px solid rgba(148, 163, 184, 0.28);
    background: rgba(15, 23, 42, 0.34);
    padding: 0.5rem 0.75rem;
    font-size: 0.875rem;
    color: inherit;
    outline: none;
  }

  .input-like:focus {
    border-color: rgba(96, 165, 250, 0.65);
    box-shadow: 0 0 0 2px rgba(96, 165, 250, 0.18);
  }

  .icon-btn {
    display: inline-flex;
    height: 2rem;
    width: 2rem;
    align-items: center;
    justify-content: center;
    border-radius: 0.375rem;
    color: rgba(248, 113, 113, 0.9);
    transition: background 120ms ease;
  }

  .icon-btn:hover {
    background: rgba(248, 113, 113, 0.12);
  }

  :global(html.light) .input-like {
    background: rgba(255, 255, 255, 0.88);
    border-color: rgba(100, 116, 139, 0.22);
  }
</style>
