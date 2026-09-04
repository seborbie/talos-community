<script lang="ts">
  import { goto } from '$app/navigation';
  import { orgsApi } from '$lib/api';
  import Card from '$lib/ui/Card.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Button from '$lib/ui/Button.svelte';

  let name = '';
  let memberEmail = '';
  let memberPassword = '';
  let memberRole: 'AGENT_ADMIN' | 'VIEWER' = 'AGENT_ADMIN';
  let isSubmitting = false;

  const handleSubmit = async () => {
    try {
      isSubmitting = true;
      const members = memberEmail
        ? [{ email: memberEmail, password: memberPassword || undefined, role: memberRole }]
        : [];
      await orgsApi.onboard({ name, members });
      goto('/dashboard');
    } catch (e: any) {
      alert(e.message || 'Failed to complete onboarding');
    } finally {
      isSubmitting = false;
    }
  };
</script>

<div class="max-w-2xl mx-auto">
  <Card>
    <CardHeader>
      <CardTitle>Set up your organization</CardTitle>
      <CardDescription>
        Enter your company details and add an admin or viewer.
      </CardDescription>
    </CardHeader>
    <CardContent>
      <form class="space-y-6" on:submit|preventDefault={handleSubmit}>
        <div class="space-y-2">
          <Label>Organization Name</Label>
          <Input placeholder="Acme Inc." bind:value={name} />
        </div>
        <div class="border border-white/10 rounded-md p-4 space-y-3" style="background: rgba(255,255,255,0.03)">
          <div class="font-medium">Optional: Add an additional member</div>
          <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div class="space-y-2">
              <Label>Email</Label>
              <Input placeholder="admin@acme.com" bind:value={memberEmail} />
            </div>
            <div class="space-y-2">
              <Label>Password (new users)</Label>
              <Input type="password" placeholder="Min 8 characters" bind:value={memberPassword} />
            </div>
          </div>
          <div class="space-y-2">
            <Label>Role</Label>
            <select class="glass-input flex h-10 w-full" bind:value={memberRole}>
              <option value="AGENT_ADMIN">Agent Admin</option>
              <option value="VIEWER">Viewer</option>
            </select>
          </div>
        </div>
        <div class="flex justify-end">
          <Button type="submit" disabled={isSubmitting}>
            {isSubmitting ? 'Setting up...' : 'Create Organization'}
          </Button>
        </div>
      </form>
    </CardContent>
  </Card>
</div>
