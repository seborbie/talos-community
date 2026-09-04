<script lang="ts">
  import { page } from '$app/stores';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import { customerApi } from '$lib/api';
  import type { Customer } from '$lib/types';

  type CustomerDetails = Customer & { deviceCount?: number };

  let customer: CustomerDetails | null = null;
  let loading = true;
  let error: string | null = null;

  const fetchCustomer = async (id: string) => {
    try {
      loading = true;
      error = null;
      customer = await customerApi.getCustomer(id);
    } catch (err) {
      console.error('Failed to fetch customer:', err);
      error = err instanceof Error ? err.message : 'Failed to fetch customer';
    } finally {
      loading = false;
    }
  };

  const formatDate = (value?: string | null) => {
    if (!value) return '—';
    return new Date(value).toLocaleString();
  };

  $: if ($page.params.id) {
    void fetchCustomer($page.params.id);
  }
</script>

<div class="space-y-6">
  <div class="space-y-2">
    <a class="aero-link text-sm font-medium hover:underline" href="/dashboard/rmm/customers">
      ← Back to customers
    </a>
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <div>
        <h1 class="text-3xl font-bold aero-gradient-text">{customer?.name ?? 'Customer Details'}</h1>
        <p class="text-sm aero-muted mt-1">Detailed information about this customer.</p>
      </div>
      {#if customer}
        <div class="flex items-center gap-4">
          {#if !customer.isUnassigned}
            <a
              class="aero-link text-sm font-medium hover:underline"
              href="/dashboard/rmm/customers/{customer.id}/sites"
            >
              Manage sites →
            </a>
          {/if}
          <a
            class="aero-link text-sm font-medium hover:underline"
            href="/dashboard/rmm/installers?scopeType=customer&customerId={customer.id}"
          >
            Create installer →
          </a>
        </div>
      {/if}
    </div>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Customer Overview</CardTitle>
      <CardDescription>View customer metadata, device counts, and status.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-32">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if error}
        <div class="aero-alert-error">{error}</div>
      {:else if customer}
        <dl class="grid gap-6 md:grid-cols-2">
          <div class="space-y-1">
            <dt class="aero-detail-label">Name</dt>
            <dd class="aero-detail-value-lg flex items-center gap-2">
              {customer.name}
              {#if customer.isUnassigned}
                <span class="aero-badge-amber">Default</span>
              {/if}
            </dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Devices</dt>
            <dd class="aero-detail-value-lg">{customer.deviceCount ?? 0}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Description</dt>
            <dd class="aero-detail-value">{customer.description ?? '—'}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Customer ID</dt>
            <dd class="aero-detail-value break-all">{customer.id}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Created</dt>
            <dd class="aero-detail-value">{formatDate(customer.createdAt)}</dd>
          </div>
          <div class="space-y-1">
            <dt class="aero-detail-label">Last Updated</dt>
            <dd class="aero-detail-value">{formatDate(customer.updatedAt)}</dd>
          </div>
        </dl>
      {:else}
        <div class="text-sm aero-empty-state">Customer not found.</div>
      {/if}
    </CardContent>
  </Card>
</div>
