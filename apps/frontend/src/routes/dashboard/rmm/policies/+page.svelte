<script lang="ts">
  import { onMount } from 'svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { RefreshCw } from 'lucide-svelte';
  import { policiesApi, customerApi } from '$lib/api';
  import type { CommandPolicy, Customer, OrgRole } from '$lib/types';

  let policies: CommandPolicy[] = [];
  let customers: Customer[] = [];
  let loading = true;
  let error: string | null = null;
  let selectedCustomerId = 'all';
  let selectedRole: OrgRole | 'all' = 'all';

  const fetchData = async () => {
    try {
      loading = true;
      error = null;
      const [policiesData, customersData] = await Promise.all([
        policiesApi.listPolicies(),
        customerApi.getCustomers()
      ]);
      policies = policiesData;
      customers = customersData;
    } catch (err) {
      console.error('Failed to fetch command policies:', err);
      error = err instanceof Error ? err.message : 'Failed to load command policies';
    } finally {
      loading = false;
    }
  };

  const matchesCustomer = (policy: CommandPolicy) => {
    if (selectedCustomerId === 'all') return true;
    return policy.customerId === selectedCustomerId;
  };

  const matchesRole = (policy: CommandPolicy) => {
    if (selectedRole === 'all') return true;
    return policy.roleScope === selectedRole;
  };

  const globalPolicies = () =>
    policies.filter((policy) => policy.scopeType === 'global');

  const organizationPolicies = () =>
    policies.filter((policy) => policy.scopeType === 'organization');

  const customerPolicies = () =>
    policies.filter(
      (policy) => policy.scopeType === 'customer' && matchesCustomer(policy)
    );

  const rolePolicies = () =>
    policies.filter((policy) => policy.scopeType === 'role' && matchesRole(policy));

  const scopeBadgeClass = (scope: CommandPolicy['scopeType']) => {
    switch (scope) {
      case 'global':
        return 'aero-badge-purple';
      case 'organization':
        return 'aero-badge-blue';
      case 'customer':
        return 'aero-badge-green';
      case 'role':
        return 'aero-badge-amber';
      default:
        return 'aero-badge-neutral';
    }
  };

  const policyBadgeClass = (policyType: CommandPolicy['policyType']) =>
    policyType === 'allow' ? 'aero-badge-green' : 'aero-badge-red';

  onMount(fetchData);
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">RMM Command Reference</h1>
      <p class="text-sm aero-muted mt-1">View global and scoped allow/deny policies.</p>
    </div>
    <Button variant="outline" on:click={fetchData} disabled={loading}>
      <RefreshCw class={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} />
      Refresh
    </Button>
  </div>

  {#if error}
    <div class="aero-alert-error">
      {error}
    </div>
  {/if}

  <Card>
    <CardHeader>
      <CardTitle>Filters</CardTitle>
      <CardDescription>Filter customer and role scoped policies.</CardDescription>
    </CardHeader>
    <CardContent className="grid grid-cols-1 gap-4 md:grid-cols-2">
      <div class="space-y-2">
        <Label for="customerFilter">Customer</Label>
        <select
          id="customerFilter"
          class="glass-input w-full"
          bind:value={selectedCustomerId}
        >
          <option value="all">All customers</option>
          {#each customers as customer}
            <option value={customer.id}>{customer.name}</option>
          {/each}
        </select>
      </div>
      <div class="space-y-2">
        <Label for="roleFilter">Role</Label>
        <select id="roleFilter" class="glass-input w-full" bind:value={selectedRole}>
          <option value="all">All roles</option>
          <option value="SUPER_ADMIN">Super Admin</option>
          <option value="AGENT_ADMIN">Agent Admin</option>
          <option value="VIEWER">Viewer</option>
        </select>
      </div>
    </CardContent>
  </Card>

  <Card>
    <CardHeader>
      <CardTitle>Global Policies</CardTitle>
      <CardDescription>System-wide allow/deny rules (read only).</CardDescription>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-20">
          <div class="animate-spin rounded-full h-6 w-6 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if globalPolicies().length === 0}
        <div class="text-sm aero-empty-state">No global policies found.</div>
      {:else}
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Command</TableHead>
              <TableHead>Policy</TableHead>
              <TableHead>Description</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each globalPolicies() as policy}
              <TableRow>
                <TableCell className="font-medium">
                  <code class="text-sm">{policy.commandName}</code>
                </TableCell>
                <TableCell>
                  <span class={policyBadgeClass(policy.policyType)}>
                    {policy.policyType}
                  </span>
                </TableCell>
                <TableCell>{policy.description || '—'}</TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
      {/if}
    </CardContent>
  </Card>

  <div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
    <Card>
      <CardHeader>
        <CardTitle>Organization Policies</CardTitle>
        <CardDescription>Rules applied at the organization scope.</CardDescription>
      </CardHeader>
      <CardContent>
        {#if organizationPolicies().length === 0}
          <div class="text-sm aero-empty-state">No organization policies found.</div>
        {:else}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Command</TableHead>
                <TableHead>Policy</TableHead>
                <TableHead>Description</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each organizationPolicies() as policy}
                <TableRow>
                  <TableCell className="font-medium">
                    <code class="text-sm">{policy.commandName}</code>
                  </TableCell>
                  <TableCell>
                    <span class={policyBadgeClass(policy.policyType)}>
                      {policy.policyType}
                    </span>
                  </TableCell>
                  <TableCell>{policy.description || '—'}</TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        {/if}
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>Customer Policies</CardTitle>
        <CardDescription>Rules applied to the selected customer.</CardDescription>
      </CardHeader>
      <CardContent>
        {#if customerPolicies().length === 0}
          <div class="text-sm aero-empty-state">No customer policies found.</div>
        {:else}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Command</TableHead>
                <TableHead>Policy</TableHead>
                <TableHead>Scope</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each customerPolicies() as policy}
                <TableRow>
                  <TableCell className="font-medium">
                    <code class="text-sm">{policy.commandName}</code>
                  </TableCell>
                  <TableCell>
                    <span class={policyBadgeClass(policy.policyType)}>
                      {policy.policyType}
                    </span>
                  </TableCell>
                  <TableCell>
                    <span class={scopeBadgeClass(policy.scopeType)}>
                      {policy.scopeType}
                    </span>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        {/if}
      </CardContent>
    </Card>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Role Policies</CardTitle>
      <CardDescription>Rules applied to the selected role.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if rolePolicies().length === 0}
        <div class="text-sm aero-empty-state">No role policies found.</div>
      {:else}
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Command</TableHead>
              <TableHead>Policy</TableHead>
              <TableHead>Role</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each rolePolicies() as policy}
              <TableRow>
                <TableCell className="font-medium">
                  <code class="text-sm">{policy.commandName}</code>
                </TableCell>
                <TableCell>
                  <span class={policyBadgeClass(policy.policyType)}>
                    {policy.policyType}
                  </span>
                </TableCell>
                <TableCell>
                  <span class={scopeBadgeClass(policy.scopeType)}>
                    {policy.roleScope ?? '—'}
                  </span>
                </TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
      {/if}
    </CardContent>
  </Card>
</div>
