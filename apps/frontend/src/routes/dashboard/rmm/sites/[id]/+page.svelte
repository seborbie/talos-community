<script lang="ts">
  import { page } from '$app/stores';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { siteApi, rmmApi } from '$lib/api';
  import type { Site, RmmDevice } from '$lib/types';

  type SiteDetails = Site & { deviceCount?: number };

  let site: SiteDetails | null = null;
  let devices: RmmDevice[] = [];
  let loading = true;
  let devicesLoading = true;
  let error: string | null = null;

  const fetchSite = async (id: string) => {
    try {
      loading = true;
      error = null;
      site = await siteApi.getSite(id);
    } catch (err) {
      console.error('Failed to fetch site:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch site';
    } finally {
      loading = false;
    }
  };

  const fetchDevices = async () => {
    try {
      devicesLoading = true;
      devices = await rmmApi.getDevices();
    } catch (err) {
      console.error('Failed to fetch devices:', err);
    } finally {
      devicesLoading = false;
    }
  };

  const formatDate = (value?: string | null) => {
    if (!value) return '—';
    return new Date(value).toLocaleString();
  };

  $: siteId = $page.params.id;
  $: devicesAtSite = siteId ? devices.filter((d) => d.siteId === siteId) : [];

  $: if (siteId) {
    void fetchSite(siteId);
    void fetchDevices();
  }
</script>

<div class="space-y-6">
  <div class="space-y-2">
    <a class="aero-link text-sm font-medium hover:underline" href="/dashboard/rmm/sites">
      ← Back to sites
    </a>
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">{site?.name ?? 'Site Details'}</h1>
      <p class="text-sm aero-muted mt-1">
        {#if site?.customerName}
          Site under <a class="aero-link hover:underline" href="/dashboard/rmm/customers/{site.customerId}">{site.customerName}</a>
          · <a class="aero-link hover:underline" href="/dashboard/rmm/installers?scopeType=site&customerId={site.customerId}&siteId={site.id}">Create installer</a>
        {:else}
          Site details and devices.
        {/if}
      </p>
    </div>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Site Overview</CardTitle>
      <CardDescription>View site metadata and devices assigned to this site.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-32">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if error}
        <div class="aero-alert-error">{error}</div>
      {:else if site}
        <dl class="grid gap-6 md:grid-cols-2">
          <div class="space-y-1">
            <dt class="aero-detail-label">Name</dt>
            <dd class="aero-detail-value-lg">{site.name}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Customer</dt>
            <dd class="aero-detail-value">
              <a class="device-link" href="/dashboard/rmm/customers/{site.customerId}">{site.customerName ?? '—'}</a>
            </dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Devices</dt>
            <dd class="aero-detail-value-lg">{site.deviceCount ?? 0}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Timezone</dt>
            <dd class="aero-detail-value">{site.timezone ?? '—'}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Site ID</dt>
            <dd class="aero-detail-value break-all">{site.id}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Created</dt>
            <dd class="aero-detail-value">{formatDate(site.createdAt)}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Last Updated</dt>
            <dd class="aero-detail-value">{formatDate(site.updatedAt)}</dd>
          </div>
        </dl>
      {:else}
        <div class="aero-empty-state">Site not found.</div>
      {/if}
    </CardContent>
  </Card>

  {#if site && !loading}
    <Card>
      <CardHeader>
        <CardTitle>Devices at this site</CardTitle>
        <CardDescription>Devices assigned to this site. Assign from the main Devices page.</CardDescription>
      </CardHeader>
      <CardContent>
        {#if devicesLoading}
          <div class="flex items-center justify-center h-24">
            <div class="animate-spin rounded-full h-6 w-6 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
          </div>
        {:else if devicesAtSite.length === 0}
          <div class="aero-empty-state">No devices assigned to this site.</div>
        {:else}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Hostname</TableHead>
                <TableHead>OS</TableHead>
                <TableHead>IP</TableHead>
                <TableHead>Last Seen</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each devicesAtSite as device}
                <TableRow>
                  <TableCell className="font-medium">
                    <a
                      class="device-link"
                      href="/dashboard/rmm/{device.agentId}"
                    >
                      {device.hostname}
                    </a>
                  </TableCell>
                  <TableCell>{device.os}</TableCell>
                  <TableCell>{device.ip}</TableCell>
                  <TableCell>{formatDate(device.lastSeen)}</TableCell>
                  <TableCell className="text-right">
                    <a
                      class="device-link"
                      href="/dashboard/rmm/{device.agentId}"
                    >
                      View
                    </a>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        {/if}
      </CardContent>
    </Card>
  {/if}
</div>
