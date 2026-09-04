<script lang="ts">
  import { onMount } from 'svelte';
  import { auditApi } from '$lib/api';
  import type { AuditEvent } from '$lib/types';
  import Button from '$lib/ui/Button.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { Download, RefreshCw, Search, ShieldCheck } from 'lucide-svelte';

  let events: AuditEvent[] = [];
  let nextCursor: string | null = null;
  let loading = false;
  let exporting = false;
  let error = '';

  let q = '';
  let result = 'all';
  let actionType = '';
  let agentId = '';
  let from = '';
  let to = '';

  const params = () => ({
    limit: 100,
    q,
    result,
    actionType,
    agentId,
    from: from ? new Date(from).toISOString() : undefined,
    to: to ? new Date(to).toISOString() : undefined
  });

  const loadEvents = async (cursor?: string | null) => {
    loading = true;
    error = '';
    try {
      const response = await auditApi.listEvents({
        ...params(),
        cursor: cursor ?? null
      });
      events = cursor ? [...events, ...response.items] : response.items;
      nextCursor = response.nextCursor;
    } catch (err: any) {
      error = err?.message || 'Failed to load audit events';
    } finally {
      loading = false;
    }
  };

  const applyFilters = () => loadEvents(null);

  const exportCsv = async () => {
    exporting = true;
    error = '';
    try {
      const blob = await auditApi.exportCsv(params());
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = 'talos-audit-events.csv';
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
    } catch (err: any) {
      error = err?.message || 'Failed to export audit events';
    } finally {
      exporting = false;
    }
  };

  const formatDate = (value: string) => new Date(value).toLocaleString();

  const formatAction = (value: string) =>
    value
      .split('.')
      .map((part) => part.replace(/_/g, ' '))
      .join(' / ');

  const resultClass = (value: string) => {
    if (value === 'success') return 'aero-badge-online';
    if (value === 'blocked') return 'aero-badge-amber';
    return 'aero-badge-red';
  };

  const actorLabel = (event: AuditEvent) => event.userEmail || event.userId || event.actorType;

  const targetLabel = (event: AuditEvent) =>
    event.targetName || event.targetId || event.agentId || event.targetType;

  onMount(() => {
    loadEvents(null);
  });
</script>

<div class="space-y-6">
  <div class="flex flex-col gap-3 md:flex-row md:items-end md:justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">Audit Log</h1>
      <p class="text-sm aero-muted mt-1">Operator actions, remote sessions, policy edits, and installer activity.</p>
    </div>
    <div class="flex gap-2">
      <Button variant="outline" on:click={() => loadEvents(null)} disabled={loading}>
        <RefreshCw class="h-4 w-4 mr-2" />
        Refresh
      </Button>
      <Button on:click={exportCsv} disabled={exporting}>
        <Download class="h-4 w-4 mr-2" />
        {exporting ? 'Exporting' : 'Export CSV'}
      </Button>
    </div>
  </div>

  <Card>
    <CardHeader>
      <CardTitle className="flex items-center gap-2">
        <ShieldCheck class="h-5 w-5" />
        Filters
      </CardTitle>
      <CardDescription>Search by action, actor, target, agent, session, or error text.</CardDescription>
    </CardHeader>
    <CardContent>
      <div class="grid gap-4 lg:grid-cols-[1.5fr_1fr_1fr_1fr]">
        <div class="space-y-2">
          <Label for="q">Search</Label>
          <div class="relative">
            <Search class="absolute left-3 top-2.5 h-4 w-4 aero-muted" />
            <Input id="q" className="pl-9" bind:value={q} placeholder="policy.update, agent id, user email" />
          </div>
        </div>
        <div class="space-y-2">
          <Label for="result">Result</Label>
          <select id="result" bind:value={result} class="glass-input w-full">
            <option value="all">All</option>
            <option value="success">Success</option>
            <option value="blocked">Blocked</option>
            <option value="failure">Failure</option>
          </select>
        </div>
        <div class="space-y-2">
          <Label for="actionType">Action</Label>
          <Input id="actionType" bind:value={actionType} placeholder="remote_desktop.start" />
        </div>
        <div class="space-y-2">
          <Label for="agentId">Agent</Label>
          <Input id="agentId" bind:value={agentId} placeholder="agent id" />
        </div>
      </div>
      <div class="grid gap-4 mt-4 md:grid-cols-[1fr_1fr_auto] md:items-end">
        <div class="space-y-2">
          <Label for="from">From</Label>
          <Input id="from" type="datetime-local" bind:value={from} />
        </div>
        <div class="space-y-2">
          <Label for="to">To</Label>
          <Input id="to" type="datetime-local" bind:value={to} />
        </div>
        <Button on:click={applyFilters} disabled={loading}>
          <Search class="h-4 w-4 mr-2" />
          {loading ? 'Searching' : 'Search'}
        </Button>
      </div>
    </CardContent>
  </Card>

  <Card>
    <CardHeader>
      <CardTitle>Events</CardTitle>
      <CardDescription>{events.length} loaded</CardDescription>
    </CardHeader>
    <CardContent>
      {#if error}
        <div class="aero-alert-error">{error}</div>
      {/if}

      <div class="aero-table-wrap">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Time</TableHead>
              <TableHead>Result</TableHead>
              <TableHead>Action</TableHead>
              <TableHead>Actor</TableHead>
              <TableHead>Target</TableHead>
              <TableHead>Agent</TableHead>
              <TableHead>Session</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#if events.length === 0 && !loading}
              <TableRow>
                <TableCell colspan={7} className="text-center aero-muted py-8">No audit events found.</TableCell>
              </TableRow>
            {:else}
              {#each events as event}
                <TableRow>
                  <TableCell className="whitespace-nowrap">{formatDate(event.occurredAt)}</TableCell>
                  <TableCell>
                    <span class={resultClass(event.result)}>{event.result}</span>
                  </TableCell>
                  <TableCell>
                    <div class="font-medium">{formatAction(event.actionType)}</div>
                    {#if event.errorMessage}
                      <div class="text-xs text-red-300 mt-1">{event.errorMessage}</div>
                    {/if}
                  </TableCell>
                  <TableCell className="max-w-[220px] truncate">{actorLabel(event)}</TableCell>
                  <TableCell className="max-w-[240px] truncate">{targetLabel(event)}</TableCell>
                  <TableCell className="max-w-[180px] truncate">{event.agentId ?? '-'}</TableCell>
                  <TableCell className="max-w-[180px] truncate">{event.sessionId ?? '-'}</TableCell>
                </TableRow>
              {/each}
            {/if}
          </TableBody>
        </Table>
      </div>

      {#if nextCursor}
        <div class="mt-4 flex justify-center">
          <Button variant="outline" on:click={() => loadEvents(nextCursor)} disabled={loading}>
            {loading ? 'Loading' : 'Load More'}
          </Button>
        </div>
      {/if}
    </CardContent>
  </Card>
</div>
