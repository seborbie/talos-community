<script lang="ts">
  import { onMount } from 'svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Dialog from '$lib/ui/Dialog.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { Plus, Pencil, Trash2, RefreshCw } from 'lucide-svelte';
  import { policiesApi, orgsApi, customerApi } from '$lib/api';
  import type { CommandPolicy, CreatePolicyRequest, OrgRole, Customer } from '$lib/types';

  let policies: CommandPolicy[] = [];
  let customers: Customer[] = [];
  let loading = true;
  let error: string | null = null;
  let currentRole: OrgRole = 'VIEWER';

  let dialogOpen = false;
  let editingPolicy: CommandPolicy | null = null;
  let saving = false;
  let orgAllowSaving = false;

  let formData: CreatePolicyRequest = {
    commandName: '',
    scopeType: 'organization',
    policyType: 'allow'
  };
  let formDescription = '';
  let formReason = '';

  let orgAllowForm: CreatePolicyRequest = {
    commandName: '',
    scopeType: 'organization',
    policyType: 'allow'
  };
  let orgAllowDescription = '';

  const fetchData = async () => {
    try {
      loading = true;
      error = null;
      const [policiesData, orgData, customersData] = await Promise.all([
        policiesApi.listPolicies(),
        orgsApi.getCurrent(),
        customerApi.getCustomers()
      ]);
      policies = policiesData;
      customers = customersData;
      currentRole = (orgData as any).membership.role;
    } catch (err) {
      console.error('Failed to fetch policies:', err);
      error = err instanceof Error ? err.message : 'Failed to load policies';
    } finally {
      loading = false;
    }
  };

  const canEdit = () => currentRole === 'SUPER_ADMIN' || currentRole === 'AGENT_ADMIN';

  const openCreate = () => {
    editingPolicy = null;
    formData = {
      commandName: '',
      scopeType: 'organization',
      policyType: 'allow'
    };
    formDescription = '';
    formReason = '';
    dialogOpen = true;
  };

  const openEdit = (policy: CommandPolicy) => {
    editingPolicy = policy;
    formData = {
      commandName: policy.commandName,
      scopeType: policy.scopeType === 'global' ? 'organization' : policy.scopeType,
      policyType: policy.policyType,
      customerId: policy.customerId ?? undefined,
      roleScope: policy.roleScope ?? undefined
    };
    formDescription = policy.description ?? '';
    formReason = policy.reason ?? '';
    dialogOpen = true;
  };

  const handleSave = async () => {
    const commandName = formData.commandName.trim();
    if (!commandName) {
      alert('Command name is required');
      return;
    }
    if (formData.scopeType === 'customer' && !formData.customerId) {
      alert('Customer is required for customer scope');
      return;
    }
    if (formData.scopeType === 'role' && !formData.roleScope) {
      alert('Role is required for role scope');
      return;
    }

    try {
      saving = true;
      if (editingPolicy) {
        await policiesApi.updatePolicy(editingPolicy.id, {
          policyType: formData.policyType,
          description: formDescription.trim() || undefined,
          reason: formReason.trim() || undefined
        });
      } else {
        await policiesApi.createPolicy({
          ...formData,
          commandName,
          description: formDescription.trim() || undefined,
          reason: formReason.trim() || undefined
        });
      }
      dialogOpen = false;
      await fetchData();
    } catch (err: any) {
      alert(err?.message || 'Failed to save policy');
    } finally {
      saving = false;
    }
  };

  const handleDelete = async (policy: CommandPolicy) => {
    if (policy.scopeType === 'global') return;
    if (!confirm(`Delete policy for ${policy.commandName}?`)) return;
    try {
      await policiesApi.deletePolicy(policy.id);
      await fetchData();
    } catch (err: any) {
      alert(err?.message || 'Failed to delete policy');
    }
  };

  const handleOrgAllowSave = async () => {
    const commandName = orgAllowForm.commandName.trim();
    if (!commandName) {
      alert('Command name is required');
      return;
    }

    try {
      orgAllowSaving = true;
      await policiesApi.createPolicy({
        ...orgAllowForm,
        commandName,
        description: orgAllowDescription.trim() || undefined
      });
      orgAllowForm = {
        commandName: '',
        scopeType: 'organization',
        policyType: 'allow'
      };
      orgAllowDescription = '';
      await fetchData();
    } catch (err: any) {
      alert(err?.message || 'Failed to create organization allow-list policy');
    } finally {
      orgAllowSaving = false;
    }
  };

  const getScopeBadgeColor = (scope: string) => {
    switch (scope) {
      case 'global':
        return 'bg-purple-100 text-purple-800';
      case 'organization':
        return 'bg-blue-100 text-blue-800';
      case 'customer':
        return 'bg-green-100 text-green-800';
      case 'role':
        return 'bg-yellow-100 text-yellow-800';
      default:
        return 'bg-gray-100 text-gray-800';
    }
  };

  const getPolicyBadgeColor = (type: string) =>
    type === 'allow' ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-800';

  onMount(fetchData);
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">Command Policies</h1>
      <p class="text-sm aero-muted mt-1">Manage allowed and denied PowerShell commands.</p>
    </div>
    <div class="flex items-center gap-2">
      <Button variant="outline" on:click={fetchData} disabled={loading}>
        <RefreshCw class={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} />
        Refresh
      </Button>
      {#if canEdit()}
        <Button on:click={openCreate}>
          <Plus class="mr-2 h-4 w-4" />
          Add Policy
        </Button>
      {/if}
    </div>
  </div>

  {#if error}
    <div class="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-600">
      {error}
    </div>
  {/if}

  <Card>
    <CardHeader>
      <CardTitle>Active Policies</CardTitle>
      <CardDescription>
        Commands are evaluated in order: Role → Customer → Organization → Global. Deny rules
        take precedence.
      </CardDescription>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-32">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else if policies.length === 0}
        <div class="text-center text-gray-500 py-8">No policies found.</div>
      {:else}
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Command</TableHead>
              <TableHead>Scope</TableHead>
              <TableHead>Policy</TableHead>
              <TableHead>Description</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each policies as policy}
              <TableRow>
                <TableCell className="font-medium">
                  <code class="text-sm">{policy.commandName}</code>
                </TableCell>
                <TableCell>
                  <span class={`px-2 py-1 text-xs rounded ${getScopeBadgeColor(policy.scopeType)}`}>
                    {policy.scopeType}
                  </span>
                  {#if policy.roleScope}
                    <span class="ml-1 text-xs text-gray-500">({policy.roleScope})</span>
                  {/if}
                </TableCell>
                <TableCell>
                  <span class={`px-2 py-1 text-xs rounded ${getPolicyBadgeColor(policy.policyType)}`}>
                    {policy.policyType}
                  </span>
                </TableCell>
                <TableCell>
                  <span class="text-sm text-gray-600">{policy.description || '—'}</span>
                </TableCell>
                <TableCell className="text-right">
                  <div class="flex justify-end gap-2">
                    <Button
                      variant="outline"
                      disabled={policy.scopeType === 'global' || !canEdit()}
                      on:click={() => openEdit(policy)}
                    >
                      <Pencil class="mr-2 h-4 w-4" />
                      Edit
                    </Button>
                    <Button
                      variant="destructive"
                      disabled={policy.scopeType === 'global' || !canEdit()}
                      on:click={() => handleDelete(policy)}
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
      {/if}
    </CardContent>
  </Card>
  {#if canEdit()}
    <Card>
      <CardHeader>
        <CardTitle>Organization Allow List</CardTitle>
        <CardDescription>
          Add commands that should be allowed for your entire organization.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div class="space-y-4">
          <div class="space-y-2">
            <Label for="orgCommandName">Command Name</Label>
            <Input
              id="orgCommandName"
              placeholder="e.g., Get-EventLog"
              bind:value={orgAllowForm.commandName}
            />
          </div>
          <div class="space-y-2">
            <Label for="orgDescription">Description</Label>
            <Input
              id="orgDescription"
              bind:value={orgAllowDescription}
              placeholder="Optional description"
            />
          </div>
          <div class="flex justify-end">
            <Button on:click={handleOrgAllowSave} disabled={orgAllowSaving}>
              {orgAllowSaving ? 'Saving...' : 'Add Organization Allow'}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  {/if}
</div>

<Dialog bind:open={dialogOpen}>
  <div class="space-y-4">
    <h2 class="text-lg font-semibold">
      {editingPolicy ? 'Edit Command Policy' : 'Add Command Policy'}
    </h2>

    <div class="space-y-2">
      <Label for="commandName">Command Name</Label>
      <Input
        id="commandName"
        placeholder="e.g., Get-EventLog"
        bind:value={formData.commandName}
        disabled={!!editingPolicy}
      />
    </div>

    <div class="space-y-2">
      <Label for="scopeType">Scope</Label>
      <select
        id="scopeType"
        bind:value={formData.scopeType}
        disabled={!!editingPolicy}
        class="glass-input w-full"
      >
        <option value="organization">Organization</option>
        <option value="customer">Customer</option>
        <option value="role">Role</option>
      </select>
    </div>

    {#if formData.scopeType === 'customer'}
      <div class="space-y-2">
        <Label for="customerId">Customer</Label>
        <select
          id="customerId"
          bind:value={formData.customerId}
          disabled={!!editingPolicy}
          class="glass-input w-full"
        >
          <option value="">Select customer...</option>
          {#each customers as customer}
            <option value={customer.id}>{customer.name}</option>
          {/each}
        </select>
      </div>
    {/if}

    {#if formData.scopeType === 'role'}
      <div class="space-y-2">
        <Label for="roleScope">Role</Label>
        <select
          id="roleScope"
          bind:value={formData.roleScope}
          disabled={!!editingPolicy}
          class="glass-input w-full"
        >
          <option value="">Select role...</option>
          <option value="SUPER_ADMIN">Super Admin</option>
          <option value="AGENT_ADMIN">Agent Admin</option>
          <option value="VIEWER">Viewer</option>
        </select>
      </div>
    {/if}

    <div class="space-y-2">
      <Label for="policyType">Policy Type</Label>
      <select
        id="policyType"
        bind:value={formData.policyType}
        class="glass-input w-full"
      >
        <option value="allow">Allow</option>
        <option value="deny">Deny</option>
      </select>
    </div>

    <div class="space-y-2">
      <Label for="description">Description</Label>
      <Input id="description" bind:value={formDescription} placeholder="Optional description" />
    </div>

    {#if formData.policyType === 'deny'}
      <div class="space-y-2">
        <Label for="reason">Reason</Label>
        <Input id="reason" bind:value={formReason} placeholder="Why is this command denied?" />
      </div>
    {/if}

    <div class="flex justify-end gap-2 pt-4">
      <Button variant="outline" on:click={() => (dialogOpen = false)} disabled={saving}>
        Cancel
      </Button>
      <Button on:click={handleSave} disabled={saving}>
        {saving ? 'Saving...' : 'Save Policy'}
      </Button>
    </div>
  </div>
</Dialog>
