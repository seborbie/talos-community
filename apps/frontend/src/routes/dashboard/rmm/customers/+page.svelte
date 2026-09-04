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
  import { customerApi } from '$lib/api';
  import type { Customer } from '$lib/types';

  type CustomerRow = Customer & { deviceCount?: number };

  let customers: CustomerRow[] = [];
  let loading = true;
  let error: string | null = null;
  let dialogOpen = false;
  let editingCustomer: CustomerRow | null = null;
  let formName = '';
  let formDescription = '';
  let saving = false;

  const fetchCustomers = async () => {
    try {
      loading = true;
      error = null;
      customers = await customerApi.getCustomers();
    } catch (err) {
      console.error('Failed to fetch customers:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch customers';
    } finally {
      loading = false;
    }
  };

  const openCreate = () => {
    editingCustomer = null;
    formName = '';
    formDescription = '';
    dialogOpen = true;
  };

  const openEdit = (customer: CustomerRow) => {
    editingCustomer = customer;
    formName = customer.name;
    formDescription = customer.description ?? '';
    dialogOpen = true;
  };

  const handleSave = async () => {
    if (!formName.trim()) {
      alert('Customer name is required');
      return;
    }

    try {
      saving = true;
      if (editingCustomer) {
        await customerApi.updateCustomer(editingCustomer.id, {
          name: formName.trim(),
          description: formDescription.trim() || null
        });
      } else {
        await customerApi.createCustomer({
          name: formName.trim(),
          description: formDescription.trim() || null
        });
      }
      dialogOpen = false;
      await fetchCustomers();
    } catch (err: any) {
      alert(err?.message || 'Failed to save customer');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async (customer: CustomerRow) => {
    if (customer.isUnassigned) return;
    if (!confirm(`Delete ${customer.name}? Devices will be moved to Unassigned.`)) return;
    try {
      await customerApi.deleteCustomer(customer.id);
      await fetchCustomers();
    } catch (err: any) {
      alert(err?.message || 'Failed to delete customer');
    }
  };

  onMount(fetchCustomers);
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">RMM Customers</h1>
      <p class="text-sm aero-muted mt-1">Organize devices by customer or site.</p>
    </div>
    <Button className="flex items-center gap-2" on:click={openCreate}>
      <Plus class="h-4 w-4" />
      New Customer
    </Button>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Customers</CardTitle>
      <CardDescription>Manage customer records used to group RMM devices.</CardDescription>
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
              <TableHead>Name</TableHead>
              <TableHead>Description</TableHead>
              <TableHead>Devices</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each customers as customer}
              <TableRow>
                <TableCell className="font-medium">
                  <a
                    class="device-link"
                    href={`/dashboard/rmm/customers/${customer.id}/sites`}
                  >
                    {customer.name}
                  </a>
                  {#if customer.isUnassigned}
                    <span class="aero-badge-amber ml-2">Default</span>
                  {/if}
                </TableCell>
                <TableCell>{customer.description ?? '—'}</TableCell>
                <TableCell>{customer.deviceCount ?? 0}</TableCell>
                <TableCell className="text-right">
                  <div class="flex justify-end gap-2">
                    <Button
                      variant="outline"
                      disabled={customer.isUnassigned}
                      on:click={() => openEdit(customer)}
                    >
                      <Pencil class="mr-2 h-4 w-4" />
                      Edit
                    </Button>
                    <Button
                      variant="destructive"
                      disabled={customer.isUnassigned}
                      on:click={() => handleDelete(customer)}
                    >
                      <Trash2 class="mr-2 h-4 w-4" />
                      Delete
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
        {#if customers.length === 0}
          <div class="text-center py-6 aero-empty-state">No customers created yet.</div>
        {/if}
      {/if}
    </CardContent>
  </Card>

  <Dialog bind:open={dialogOpen} on:close={() => (dialogOpen = false)}>
    <div class="space-y-4">
      <div>
        <h2 class="text-lg font-semibold">{editingCustomer ? 'Edit Customer' : 'Create Customer'}</h2>
        <p class="aero-dialog-subtitle">
          {editingCustomer ? 'Update customer details.' : 'Add a new customer for device assignment.'}
        </p>
      </div>
      <div class="space-y-2">
        <Label>Name</Label>
        <Input bind:value={formName} placeholder="Customer name" disabled={editingCustomer?.isUnassigned} />
      </div>
      <div class="space-y-2">
        <Label>Description</Label>
        <Input bind:value={formDescription} placeholder="Optional notes" disabled={editingCustomer?.isUnassigned} />
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="outline" on:click={() => (dialogOpen = false)}>Cancel</Button>
        <Button on:click={handleSave} disabled={saving || !!editingCustomer?.isUnassigned}>
          {saving ? 'Saving...' : editingCustomer ? 'Save Changes' : 'Create Customer'}
        </Button>
      </div>
    </div>
  </Dialog>
</div>
