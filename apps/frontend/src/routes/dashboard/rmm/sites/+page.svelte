<script lang="ts">
  import { onMount } from 'svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import Dialog from '$lib/ui/Dialog.svelte';
  import { Plus, Pencil, Trash2 } from 'lucide-svelte';
  import { customerApi, siteApi } from '$lib/api';
  import type { Customer, Site } from '$lib/types';

  type SiteRow = Site & { deviceCount?: number };

  let sites: SiteRow[] = [];
  let customers: (Customer & { deviceCount?: number })[] = [];
  let loading = true;
  let customersLoading = true;
  let error: string | null = null;
  let dialogOpen = false;
  let editingSite: SiteRow | null = null;
  let formCustomerId = '';
  let formName = '';
  let formTimezone = '';
  let saving = false;

  const fetchSites = async () => {
    try {
      loading = true;
      error = null;
      sites = await siteApi.getSites();
    } catch (err) {
      console.error('Failed to fetch sites:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch sites';
    } finally {
      loading = false;
    }
  };

  const fetchCustomers = async () => {
    try {
      customersLoading = true;
      customers = await customerApi.getCustomers();
    } catch (err) {
      console.error('Failed to fetch customers:', err);
    } finally {
      customersLoading = false;
    }
  };

  const openCreate = () => {
    editingSite = null;
    formCustomerId = '';
    formName = '';
    formTimezone = '';
    dialogOpen = true;
  };

  const openEdit = (site: SiteRow) => {
    editingSite = site;
    formCustomerId = site.customerId;
    formName = site.name;
    formTimezone = site.timezone ?? '';
    dialogOpen = true;
  };

  const handleSave = async () => {
    if (!formName.trim()) {
      alert('Site name is required');
      return;
    }
    if (!editingSite && !formCustomerId) {
      alert('Please select a customer');
      return;
    }

    try {
      saving = true;
      if (editingSite) {
        await siteApi.updateSite(editingSite.id, {
          name: formName.trim(),
          timezone: formTimezone.trim() || null
        });
      } else {
        await siteApi.createSite({
          customerId: formCustomerId,
          name: formName.trim(),
          timezone: formTimezone.trim() || null
        });
      }
      dialogOpen = false;
      await fetchSites();
    } catch (err: any) {
      alert(err?.message || 'Failed to save site');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async (site: SiteRow) => {
    if (!confirm(`Delete "${site.name}"? Devices at this site will have their site assignment cleared.`)) return;
    try {
      await siteApi.deleteSite(site.id);
      await fetchSites();
    } catch (err: any) {
      alert(err?.message || 'Failed to delete site');
    }
  };

  onMount(() => {
    fetchSites();
    fetchCustomers();
  });
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">RMM Sites</h1>
      <p class="text-sm aero-muted mt-1">Sites belong to customers. Assign devices to sites for finer grouping.</p>
    </div>
    <Button className="flex items-center gap-2" on:click={openCreate}>
      <Plus class="h-4 w-4" />
      New Site
    </Button>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Sites</CardTitle>
      <CardDescription>Manage sites under each customer. Devices can be assigned to a site.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-32">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if error}
        <div class="aero-alert-error">{error}</div>
      {:else}
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Site</TableHead>
              <TableHead>Customer</TableHead>
              <TableHead>Timezone</TableHead>
              <TableHead>Devices</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each sites as site}
              <TableRow>
                <TableCell className="font-medium">
                  <a
                    class="device-link"
                    href={`/dashboard/rmm/sites/${site.id}`}
                  >
                    {site.name}
                  </a>
                </TableCell>
                <TableCell>
                  <a
                    class="device-link"
                    href={`/dashboard/rmm/customers/${site.customerId}/sites`}
                  >
                    {site.customerName ?? '—'}
                  </a>
                </TableCell>
                <TableCell>{site.timezone ?? '—'}</TableCell>
                <TableCell>{site.deviceCount ?? 0}</TableCell>
                <TableCell className="text-right">
                  <div class="flex justify-end gap-2">
                    <Button variant="outline" on:click={() => openEdit(site)}>
                      <Pencil class="mr-2 h-4 w-4" />
                      Edit
                    </Button>
                    <Button variant="destructive" on:click={() => handleDelete(site)}>
                      <Trash2 class="mr-2 h-4 w-4" />
                      Delete
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
        {#if sites.length === 0}
          <div class="aero-empty-state">No sites yet. Create a site under a customer.</div>
        {/if}
      {/if}
    </CardContent>
  </Card>

  <Dialog bind:open={dialogOpen} on:close={() => (dialogOpen = false)}>
    <div class="space-y-4">
      <div>
        <h2 class="text-lg font-semibold">{editingSite ? 'Edit Site' : 'Create Site'}</h2>
        <p class="text-sm aero-dialog-subtitle">
          {editingSite ? 'Update site details.' : 'Add a new site under a customer.'}
        </p>
      </div>
      {#if !editingSite}
        <div class="space-y-2">
          <Label>Customer</Label>
          <select
            bind:value={formCustomerId}
            class="glass-input flex h-10 w-full"
            disabled={customersLoading}
          >
            <option value="">Select customer</option>
            {#each customers.filter((c) => !c.isUnassigned) as customer}
              <option value={customer.id}>{customer.name}</option>
            {/each}
          </select>
        </div>
      {/if}
      <div class="space-y-2">
        <Label>Site name</Label>
        <Input bind:value={formName} placeholder="e.g. Headquarters" />
      </div>
      <div class="space-y-2">
        <Label>Timezone (optional)</Label>
        <Input bind:value={formTimezone} placeholder="e.g. America/New_York" />
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="outline" on:click={() => (dialogOpen = false)}>Cancel</Button>
        <Button on:click={handleSave} disabled={saving}>
          {saving ? 'Saving...' : editingSite ? 'Save Changes' : 'Create Site'}
        </Button>
      </div>
    </div>
  </Dialog>
</div>
