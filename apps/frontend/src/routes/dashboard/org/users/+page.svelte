<script lang="ts">
  import { onMount } from 'svelte';
  import { orgsApi, policiesApi } from '$lib/api';
  import type { OrganizationMember, OrgRole, HaloConfig, CommandPolicy } from '$lib/types';
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
  import { Plus } from 'lucide-svelte';

  let members: OrganizationMember[] = [];
  let loading = true;
  let isAddOpen = false;
  let newEmail = '';
  let newPassword = '';
  let newRole: OrgRole = 'VIEWER';
  let saving = false;
  let currentRole: OrgRole | null = null;
  let haloConfig: HaloConfig = { baseUrl: '', clientId: '', clientSecret: '' };
  let haloSaving = false;
  let haloLoading = true;
  let rolePolicies: CommandPolicy[] = [];
  let rolePoliciesLoading = true;
  let rolePolicyCommand = '';
  let rolePolicyRole: OrgRole = 'VIEWER';
  let rolePolicyDescription = '';
  let rolePolicySaving = false;

  const canManageRolePolicies = () => currentRole === 'SUPER_ADMIN' || currentRole === 'AGENT_ADMIN';

  const fetchRolePolicies = async () => {
    try {
      rolePoliciesLoading = true;
      const policies = await policiesApi.listPolicies();
      rolePolicies = policies.filter((policy) => policy.scopeType === 'role');
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to load role policies');
    } finally {
      rolePoliciesLoading = false;
    }
  };

  const handleCreateRolePolicy = async () => {
    if (!canManageRolePolicies()) return;
    const commandName = rolePolicyCommand.trim();
    if (!commandName) {
      alert('Command name is required');
      return;
    }

    try {
      rolePolicySaving = true;
      await policiesApi.createPolicy({
        commandName,
        scopeType: 'role',
        roleScope: rolePolicyRole,
        policyType: 'allow',
        description: rolePolicyDescription.trim() || undefined
      });
      rolePolicyCommand = '';
      rolePolicyDescription = '';
      await fetchRolePolicies();
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to create role policy');
    } finally {
      rolePolicySaving = false;
    }
  };

  const fetchMembers = async () => {
    try {
      loading = true;
      const data = await orgsApi.listMembers();
      members = data;
    } finally {
      loading = false;
    }
  };

  const init = async () => {
    await Promise.all([fetchMembers(), fetchRolePolicies()]);
    try {
      const current = await orgsApi.getCurrent();
      if ('membership' in current && typeof current.membership?.role === 'string') {
        currentRole = current.membership.role;
      }
      try {
        haloLoading = true;
        const cfg = await orgsApi.getHaloConfig();
        haloConfig = cfg;
      } finally {
        haloLoading = false;
      }
    } catch {}
  };

  const handleAdd = async () => {
    try {
      saving = true;
      await orgsApi.addMember({ email: newEmail, password: newPassword || undefined, role: newRole });
      isAddOpen = false;
      newEmail = '';
      newPassword = '';
      newRole = 'VIEWER';
      await fetchMembers();
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to add member');
    } finally {
      saving = false;
    }
  };

  const handleRoleChange = async (memberId: string, role: OrgRole) => {
    try {
      await orgsApi.updateMemberRole(memberId, role);
      await fetchMembers();
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to update role');
    }
  };

  const handleRemove = async (memberId: string) => {
    if (!confirm('Remove this member?')) return;
    try {
      await orgsApi.removeMember(memberId);
      await fetchMembers();
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to remove member');
    }
  };

  const handleSaveHalo = async () => {
    try {
      haloSaving = true;
      await orgsApi.updateHaloConfig(haloConfig);
      alert('Halo configuration saved');
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to save configuration');
    } finally {
      haloSaving = false;
    }
  };

  const handleClearHalo = async () => {
    if (!confirm('Clear Halo configuration?')) return;
    try {
      await orgsApi.clearHaloConfig();
      haloConfig = { baseUrl: '', clientId: '', clientSecret: '' };
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Failed to clear configuration');
    }
  };

  onMount(init);
</script>

<div class="space-y-6">
  <div>
    <h1 class="text-3xl font-bold" >Organization Config</h1>
    <p class="text-white/55 mt-1">Manage integrations and users for your organization.</p>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Ticketing System Integration</CardTitle>
      <CardDescription>Configure Halo PSA integration. Only Super Admins can edit.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if haloLoading}
        <div class="flex items-center justify-center h-20">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else}
        <div class="grid gap-4 md:grid-cols-3">
          <div class="space-y-2 md:col-span-3">
            <Label>Halo Base URL</Label>
            <Input bind:value={haloConfig.baseUrl} placeholder="https://your.halopsa.com" disabled={currentRole !== 'SUPER_ADMIN'} />
          </div>
          <div class="space-y-2">
            <Label>Client ID</Label>
            <Input bind:value={haloConfig.clientId} placeholder="GUID" disabled={currentRole !== 'SUPER_ADMIN'} />
          </div>
          <div class="space-y-2 md:col-span-2">
            <Label>Client Secret</Label>
            <Input type="password" bind:value={haloConfig.clientSecret} placeholder="********" disabled={currentRole !== 'SUPER_ADMIN'} />
          </div>
          <div class="flex gap-2 md:col-span-3 justify-end">
            <Button variant="outline" on:click={handleClearHalo} disabled={currentRole !== 'SUPER_ADMIN'}>Clear</Button>
            <Button on:click={handleSaveHalo} disabled={currentRole !== 'SUPER_ADMIN' || haloSaving}>{haloSaving ? 'Saving...' : 'Save'}</Button>
          </div>
        </div>
      {/if}
    </CardContent>
  </Card>

  <Card>
    <CardHeader>
      <CardTitle>Roles & Permissions</CardTitle>
      <CardDescription>A quick guide to choose the right access level.</CardDescription>
    </CardHeader>
    <CardContent>
      <div class="grid gap-4 sm:grid-cols-3">
        <div class="rounded-md border p-4">
          <div class="font-semibold mb-1">Super Admin</div>
          <p class="text-sm text-gray-600">Full control. Can add/remove members, change roles, manage agents, and delete the organization.</p>
        </div>
        <div class="rounded-md border p-4">
          <div class="font-semibold mb-1">Agent Admin</div>
          <p class="text-sm text-gray-600">Operational lead. Can create and configure agents and view analytics, but cannot manage organization members or delete the org.</p>
        </div>
        <div class="rounded-md border p-4">
          <div class="font-semibold mb-1">Viewer</div>
          <p class="text-sm text-gray-600">Read-only access to dashboards and analytics. No configuration or user management.</p>
        </div>
      </div>
    </CardContent>
  </Card>

  <Card>
    <CardHeader>
      <CardTitle>Role Command Allow List</CardTitle>
      <CardDescription>Create allow-list policies scoped to a role.</CardDescription>
    </CardHeader>
    <CardContent>
      <div class="space-y-4">
        <div class="grid gap-4 md:grid-cols-3">
          <div class="space-y-2 md:col-span-2">
            <Label>Command Name</Label>
            <Input bind:value={rolePolicyCommand} placeholder="Get-Process" disabled={!canManageRolePolicies()} />
          </div>
          <div class="space-y-2">
            <Label>Role Scope</Label>
            <select class="glass-input flex h-10 w-full" bind:value={rolePolicyRole} disabled={!canManageRolePolicies()}>
              <option value="VIEWER">Viewer</option>
              <option value="AGENT_ADMIN">Agent Admin</option>
              <option value="SUPER_ADMIN">Super Admin</option>
            </select>
          </div>
          <div class="space-y-2 md:col-span-3">
            <Label>Description (optional)</Label>
            <Input bind:value={rolePolicyDescription} placeholder="Allowed for diagnostic workflows" disabled={!canManageRolePolicies()} />
          </div>
          <div class="flex justify-end md:col-span-3">
            <Button on:click={handleCreateRolePolicy} disabled={!canManageRolePolicies() || rolePolicySaving}>
              {rolePolicySaving ? 'Saving...' : 'Create Allow Policy'}
            </Button>
          </div>
        </div>
        {#if !canManageRolePolicies()}
          <p class="text-sm text-amber-600">Only Super Admins or Agent Admins can add role policies.</p>
        {/if}
        <div class="space-y-2">
          <div class="flex items-center justify-between">
            <h3 class="text-sm font-semibold text-gray-700">Existing Role Policies</h3>
            <Button variant="outline" on:click={fetchRolePolicies} disabled={rolePoliciesLoading}>
              {rolePoliciesLoading ? 'Refreshing...' : 'Refresh'}
            </Button>
          </div>
          {#if rolePoliciesLoading}
            <div class="flex items-center justify-center h-20">
              <div class="animate-spin rounded-full h-6 w-6 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
            </div>
          {:else if rolePolicies.length === 0}
            <p class="text-sm text-gray-500">No role policies yet.</p>
          {:else}
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Command</TableHead>
                  <TableHead>Role</TableHead>
                  <TableHead>Policy</TableHead>
                  <TableHead>Description</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {#each rolePolicies as policy}
                  <TableRow>
                    <TableCell className="font-medium">
                      <code class="text-sm">{policy.commandName}</code>
                    </TableCell>
                    <TableCell>{policy.roleScope ?? 'Unknown'}</TableCell>
                    <TableCell>{policy.policyType}</TableCell>
                    <TableCell>{policy.description ?? '—'}</TableCell>
                  </TableRow>
                {/each}
              </TableBody>
            </Table>
          {/if}
        </div>
      </div>
    </CardContent>
  </Card>

  <Card>
    <CardHeader>
      <div>
        <CardTitle>Members</CardTitle>
        <CardDescription>Roles: SUPER_ADMIN, AGENT_ADMIN, VIEWER</CardDescription>
      </div>
      <Button className="mt-3 w-fit bg-brand text-white hover:opacity-90" disabled={currentRole !== 'SUPER_ADMIN'} on:click={() => (isAddOpen = true)}>
        <Plus class="mr-2 h-4 w-4" /> Add Member
      </Button>
    </CardHeader>
    <CardContent>
      {#if loading}
        <div class="flex items-center justify-center h-32">
          <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
        </div>
      {:else}
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Email</TableHead>
              <TableHead>Role</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {#each members as member}
              <TableRow>
                <TableCell>{member.email}</TableCell>
                <TableCell>
                  <select class="glass-input flex h-9" bind:value={member.role} on:change={(e) => handleRoleChange(member.id, (e.currentTarget as HTMLSelectElement).value as OrgRole)}>
                    <option value="VIEWER">Viewer</option>
                    <option value="AGENT_ADMIN">Agent Admin</option>
                    <option value="SUPER_ADMIN">Super Admin</option>
                  </select>
                </TableCell>
                <TableCell className="text-right">
                  <Button variant="destructive" on:click={() => handleRemove(member.id)}>Remove</Button>
                </TableCell>
              </TableRow>
            {/each}
          </TableBody>
        </Table>
      {/if}
    </CardContent>
  </Card>

  <Dialog bind:open={isAddOpen} on:close={() => (isAddOpen = false)}>
    <div class="space-y-4">
      <div>
        <h2 class="text-lg font-semibold">Add Organization Member</h2>
        <p class="text-sm text-gray-500">
          Create or invite a member to your org.
          {#if currentRole !== 'SUPER_ADMIN'}
            <span class="block mt-1 text-amber-600">Only Super Admins can add members.</span>
          {/if}
        </p>
      </div>
      <div class="space-y-2">
        <Label>Email</Label>
        <Input bind:value={newEmail} placeholder="user@example.com" />
      </div>
      <div class="space-y-2">
        <Label>Password (new users)</Label>
        <Input bind:value={newPassword} placeholder="Min 8 characters" type="password" />
      </div>
      <div class="space-y-2">
        <Label>Role</Label>
        <select class="glass-input flex h-10 w-full" bind:value={newRole}>
          <option value="VIEWER">Viewer</option>
          <option value="AGENT_ADMIN">Agent Admin</option>
          <option value="SUPER_ADMIN">Super Admin</option>
        </select>
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="outline" on:click={() => (isAddOpen = false)}>Cancel</Button>
        <Button on:click={handleAdd} disabled={saving || currentRole !== 'SUPER_ADMIN'}>
          {saving ? 'Adding...' : 'Add Member'}
        </Button>
      </div>
    </div>
  </Dialog>
</div>
