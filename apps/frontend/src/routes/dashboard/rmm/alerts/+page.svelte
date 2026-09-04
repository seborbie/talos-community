<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { rmmApi } from '$lib/api';
  import type { RmmAlertSeverity, RmmAlertStatus, RmmTelemetryAlert, RmmTelemetryAlertRule } from '$lib/types';
  import { CheckCircle2, Clock3, ExternalLink, ListFilter, RefreshCw, Search, ShieldOff } from 'lucide-svelte';

  type StatusFilter = RmmAlertStatus | 'all';
  type SeverityFilter = RmmAlertSeverity | 'all';

  const statusOptions: Array<{ value: StatusFilter; label: string }> = [
    { value: 'open', label: 'Open' },
    { value: 'acknowledged', label: 'Acknowledged' },
    { value: 'snoozed', label: 'Snoozed' },
    { value: 'resolved', label: 'Resolved' },
    { value: 'suppressed', label: 'Suppressed' },
    { value: 'all', label: 'All' }
  ];

  const severityOptions: Array<{ value: SeverityFilter; label: string }> = [
    { value: 'all', label: 'All severities' },
    { value: 'critical', label: 'Critical' },
    { value: 'high', label: 'High' },
    { value: 'medium', label: 'Medium' },
    { value: 'low', label: 'Low' },
    { value: 'info', label: 'Info' }
  ];

  let alerts: RmmTelemetryAlert[] = [];
  let rules: RmmTelemetryAlertRule[] = [];
  let loading = true;
  let actionId: string | null = null;
  let error: string | null = null;
  let query = '';
  let statusFilter: StatusFilter = 'open';
  let severityFilter: SeverityFilter = 'all';

  const dateLabel = (value: string | null): string => {
    if (!value) return '-';
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? '-' : new Date(parsed).toLocaleString();
  };

  const titleCase = (value: string | null | undefined): string =>
    (value || '-').replaceAll('_', ' ').replace(/\b\w/g, (letter) => letter.toUpperCase());

  const severityClass = (severity: string): string => {
    switch (severity) {
      case 'critical':
        return 'bg-rose-100 text-rose-800';
      case 'high':
        return 'bg-orange-100 text-orange-800';
      case 'medium':
        return 'bg-amber-100 text-amber-800';
      case 'low':
        return 'bg-blue-100 text-blue-800';
      default:
        return 'bg-slate-200 text-slate-700';
    }
  };

  const statusClass = (status: string): string => {
    switch (status) {
      case 'open':
        return 'bg-rose-100 text-rose-800';
      case 'acknowledged':
        return 'bg-sky-100 text-sky-800';
      case 'snoozed':
        return 'bg-violet-100 text-violet-800';
      case 'resolved':
        return 'bg-green-100 text-green-800';
      case 'suppressed':
        return 'bg-slate-200 text-slate-700';
      default:
        return 'bg-gray-100 text-gray-700';
    }
  };

  const loadAlerts = async () => {
    try {
      loading = true;
      error = null;
      const [alertResponse, ruleResponse] = await Promise.all([
        rmmApi.getAlerts({
          status: statusFilter,
          severity: severityFilter,
          q: query.trim() || undefined,
          limit: 300
        }),
        rmmApi.getAlertRules({ enabled: true })
      ]);
      alerts = alertResponse.items;
      rules = ruleResponse.items;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load alerts';
      alerts = [];
    } finally {
      loading = false;
    }
  };

  const replaceAlert = (updated: RmmTelemetryAlert) => {
    alerts = alerts.map((alert) => (alert.id === updated.id ? updated : alert));
  };

  const runAction = async (alert: RmmTelemetryAlert, action: 'ack' | 'snooze' | 'resolve') => {
    try {
      actionId = `${action}:${alert.id}`;
      const updated =
        action === 'ack'
          ? await rmmApi.acknowledgeAlert(alert.id)
          : action === 'snooze'
            ? await rmmApi.snoozeAlert(alert.id, 60)
            : await rmmApi.resolveAlert(alert.id);
      replaceAlert(updated);
    } catch (err) {
      error = err instanceof Error ? err.message : 'Alert action failed';
    } finally {
      actionId = null;
    }
  };

  const openDevice = (agentId: string) => {
    goto(`/dashboard/rmm/${encodeURIComponent(agentId)}`);
  };

  $: summary = {
    open: alerts.filter((alert) => alert.status === 'open').length,
    acknowledged: alerts.filter((alert) => alert.status === 'acknowledged').length,
    snoozed: alerts.filter((alert) => alert.status === 'snoozed').length,
    critical: alerts.filter((alert) => alert.severity === 'critical').length,
    high: alerts.filter((alert) => alert.severity === 'high').length
  };

  onMount(() => {
    loadAlerts();
  });
</script>

<div class="space-y-6">
  <div class="flex flex-col gap-4 xl:flex-row xl:items-end xl:justify-between">
    <div>
      <h1 class="text-2xl font-semibold text-white/95">Alerts</h1>
      <p class="mt-1 text-sm text-white/55">Lifecycle queue for telemetry events, baseline facts, and routed decisions.</p>
    </div>
    <div class="grid gap-2 sm:grid-cols-[minmax(220px,1fr)_160px_170px_auto] xl:min-w-[760px]">
      <div class="relative">
        <Search class="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-white/35" />
        <Input bind:value={query} placeholder="Search alerts or devices" className="pl-9" />
      </div>
      <select class="glass-input h-10 rounded-md px-3 text-sm" bind:value={statusFilter} aria-label="Status filter" on:change={loadAlerts}>
        {#each statusOptions as option}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
      <select class="glass-input h-10 rounded-md px-3 text-sm" bind:value={severityFilter} aria-label="Severity filter" on:change={loadAlerts}>
        {#each severityOptions as option}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
      <Button variant="secondary" on:click={loadAlerts} disabled={loading}>
        <RefreshCw class="h-4 w-4 {loading ? 'animate-spin' : ''}" />
        Refresh
      </Button>
    </div>
  </div>

  {#if error}
    <div class="aero-alert-error" role="alert">{error}</div>
  {/if}

  <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-5">
    <Card>
      <CardHeader>
        <CardDescription>Open</CardDescription>
        <CardTitle>{summary.open}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Acknowledged</CardDescription>
        <CardTitle>{summary.acknowledged}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Snoozed</CardDescription>
        <CardTitle>{summary.snoozed}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Critical / High</CardDescription>
        <CardTitle>{summary.critical} / {summary.high}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Active Rules</CardDescription>
        <CardTitle>{rules.length}</CardTitle>
      </CardHeader>
    </Card>
  </div>

  <Card>
    <CardHeader>
      <div class="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
        <div>
          <CardTitle>Alert Queue</CardTitle>
          <CardDescription>Deduplicated telemetry conditions with operator ownership and recurrence tracking.</CardDescription>
        </div>
        <div class="inline-flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-2 text-xs text-white/55">
          <ListFilter class="h-3.5 w-3.5" />
          {titleCase(statusFilter)} / {titleCase(severityFilter)}
        </div>
      </div>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="aero-empty-state">Loading alerts...</div>
      {:else if alerts.length === 0}
        <div class="aero-empty-state">No alerts match the current filters.</div>
      {:else}
        <div class="aero-table-wrap overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Severity</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Alert</TableHead>
                <TableHead>Device</TableHead>
                <TableHead>Source</TableHead>
                <TableHead>Last Seen</TableHead>
                <TableHead>Owner</TableHead>
                <TableHead class="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each alerts as alert (alert.id)}
                <TableRow>
                  <TableCell>
                    <span class="rounded-full px-2 py-1 text-xs font-semibold {severityClass(alert.severity)}">{titleCase(alert.severity)}</span>
                  </TableCell>
                  <TableCell>
                    <span class="rounded-full px-2 py-1 text-xs font-semibold {statusClass(alert.status)}">{titleCase(alert.status)}</span>
                  </TableCell>
                  <TableCell class="min-w-[260px]">
                    <div class="font-medium text-white/90">{alert.title}</div>
                    <div class="mt-1 text-xs text-white/45">{alert.summary || `${alert.occurrenceCount} occurrence${alert.occurrenceCount === 1 ? '' : 's'}`}</div>
                  </TableCell>
                  <TableCell>
                    <div class="font-medium text-white/85">{alert.hostname || alert.agentId}</div>
                    <div class="mt-1 text-xs text-white/45">{[alert.customerName, alert.siteName].filter(Boolean).join(' / ') || 'Unassigned'}</div>
                  </TableCell>
                  <TableCell>
                    <div class="font-medium text-white/80">{titleCase(alert.sourceDomain)}</div>
                    <div class="mt-1 text-xs text-white/45">{alert.sourceKey}</div>
                  </TableCell>
                  <TableCell>{dateLabel(alert.lastSeenAt)}</TableCell>
                  <TableCell>{alert.ownerEmail || '-'}</TableCell>
                  <TableCell>
                    <div class="flex justify-end gap-2">
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label="Acknowledge alert"
                        title="Acknowledge"
                        disabled={alert.status === 'acknowledged' || alert.status === 'resolved' || actionId === `ack:${alert.id}`}
                        on:click={() => runAction(alert, 'ack')}
                      >
                        <CheckCircle2 class="h-4 w-4" />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label="Snooze alert"
                        title="Snooze"
                        disabled={alert.status === 'resolved' || alert.status === 'suppressed' || actionId === `snooze:${alert.id}`}
                        on:click={() => runAction(alert, 'snooze')}
                      >
                        <Clock3 class="h-4 w-4" />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label="Resolve alert"
                        title="Resolve"
                        disabled={alert.status === 'resolved' || actionId === `resolve:${alert.id}`}
                        on:click={() => runAction(alert, 'resolve')}
                      >
                        <ShieldOff class="h-4 w-4" />
                      </Button>
                      <Button size="icon" variant="ghost" aria-label="Open device" title="Open device" on:click={() => openDevice(alert.agentId)}>
                        <ExternalLink class="h-4 w-4" />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        </div>
      {/if}
    </CardContent>
  </Card>
</div>
