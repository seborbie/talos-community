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
  import Dialog from '$lib/ui/Dialog.svelte';
  import { User, Mail, Lock, Save, Eye, EyeOff, AlertTriangle } from 'lucide-svelte';
  import { z } from 'zod';
  import { userApi } from '$lib/api';

  const profileSchema = z
    .object({
      email: z.string().email('Invalid email address'),
      currentPassword: z.string().min(1, 'Current password is required'),
      newPassword: z.string().min(8, 'New password must be at least 8 characters').optional(),
      confirmPassword: z.string().optional(),
    })
    .refine((data) => {
      if (data.newPassword && data.newPassword !== data.confirmPassword) {
        return false;
      }
      return true;
    }, {
      message: "New passwords don't match",
      path: ['confirmPassword'],
    });

  let isEditing = false;
  let showCurrentPassword = false;
  let showNewPassword = false;
  let showConfirmPassword = false;
  let isLoading = false;
  let userData: {
    user: { id: string; email: string; createdAt: string };
  } | null = null;
  let deleteConfirmStep = 0;
  let deleteConfirmText = '';
  let isDeletingAccount = false;
  let isDeleteDialogOpen = false;

  let form = {
    email: '',
    currentPassword: '',
    newPassword: '',
    confirmPassword: '',
  };
  let errors: Record<string, string> = {};

  const setErrors = (issues: z.ZodIssue[]) => {
    const next: Record<string, string> = {};
    issues.forEach((issue) => {
      const key = issue.path[0];
      if (typeof key === 'string') {
        next[key] = issue.message;
      }
    });
    errors = next;
  };

  const fetchUserData = async () => {
    try {
      const data = await userApi.getProfile();
      userData = data;
      form.email = data.user.email;
    } catch (error) {
      console.error('Error fetching user data:', error);
    }
  };

  const onSubmit = async () => {
    const parsed = profileSchema.safeParse(form);
    if (!parsed.success) {
      setErrors(parsed.error.issues);
      return;
    }
    try {
      isLoading = true;
      await userApi.updateProfile({
        email: parsed.data.email !== userData?.user.email ? parsed.data.email : undefined,
        currentPassword: parsed.data.currentPassword,
        newPassword: parsed.data.newPassword || undefined,
      });
      await fetchUserData();
      isEditing = false;
      form.currentPassword = '';
      form.newPassword = '';
      form.confirmPassword = '';
      alert('Profile updated successfully!');
    } catch (error: any) {
      console.error('Error updating profile:', error);
      alert(error.message || 'Failed to update profile');
    } finally {
      isLoading = false;
    }
  };

  const handleCancel = () => {
    isEditing = false;
    form.email = userData?.user.email || '';
    form.currentPassword = '';
    form.newPassword = '';
    form.confirmPassword = '';
    errors = {};
  };

  const closeDeleteDialog = () => {
    deleteConfirmStep = 0;
    deleteConfirmText = '';
    isDeleteDialogOpen = false;
  };

  const handleDeleteAccount = async () => {
    if (!isDeleteDialogOpen) {
      deleteConfirmStep = 1;
      isDeleteDialogOpen = true;
      return;
    }

    if (deleteConfirmStep === 1) {
      if (deleteConfirmText.toLowerCase() !== 'delete my account') {
        alert('Please type "delete my account" exactly to confirm');
        return;
      }
      deleteConfirmStep = 2;
      return;
    }

    if (deleteConfirmStep === 2) {
      try {
        isDeletingAccount = true;
        await userApi.deleteAccount();
        localStorage.removeItem('token');
        window.location.href = '/';
      } catch (error: any) {
        console.error('Error deleting account:', error);
        alert(error.message || 'Failed to delete account');
        closeDeleteDialog();
      } finally {
        isDeletingAccount = false;
      }
    }
  };

  onMount(fetchUserData);
</script>

{#if !userData}
  <div class="flex items-center justify-center h-64">
    <div class="animate-spin rounded-full h-8 w-8 border-b-2" style="border-color: rgba(55,130,255,0.8)"></div>
  </div>
{:else}
  <div class="space-y-6">
    <!-- Header -->
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">Profile Settings</h1>
      <p class="mt-1 aero-muted">Manage your account information and preferences.</p>
    </div>

    <!-- Profile Information -->
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <User class="h-5 w-5" />
          Account Information
        </CardTitle>
        <CardDescription>
          Update your personal information and account settings.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-6">
        <form class="space-y-4" on:submit|preventDefault={onSubmit}>
          <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div class="space-y-2">
              <Label for="email">Email Address</Label>
              <div class="relative">
                <Mail class="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 opacity-40" />
                <Input
                  id="email"
                  type="email"
                  className="pl-10"
                  disabled={!isEditing}
                  bind:value={form.email}
                />
              </div>
              {#if errors.email}
                <p class="text-sm text-red-500">{errors.email}</p>
              {/if}
            </div>
            <div class="space-y-2">
              <Label for="memberSince">Member Since</Label>
              <Input
                id="memberSince"
                value={new Date(userData.user.createdAt).toLocaleDateString()}
                disabled
                className="opacity-60"
              />
            </div>
          </div>

          {#if isEditing}
            <div class="border-t border-white/10 pt-4">
              <h3 class="text-lg font-medium mb-4">Change Password</h3>
              <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                <div class="space-y-2">
                  <Label for="currentPassword">Current Password</Label>
                  <div class="relative">
                    <Lock class="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 opacity-40" />
                    <Input
                      id="currentPassword"
                      type={showCurrentPassword ? 'text' : 'password'}
                      className="pl-10 pr-10"
                      bind:value={form.currentPassword}
                    />
                    <button
                      type="button"
                      class="absolute right-3 top-1/2 transform -translate-y-1/2 opacity-40 hover:opacity-80 transition-opacity"
                      on:click={() => (showCurrentPassword = !showCurrentPassword)}
                    >
                      {#if showCurrentPassword}
                        <EyeOff class="h-4 w-4" />
                      {:else}
                        <Eye class="h-4 w-4" />
                      {/if}
                    </button>
                  </div>
                  {#if errors.currentPassword}
                    <p class="text-sm text-red-500">{errors.currentPassword}</p>
                  {/if}
                </div>
                <div class="space-y-2">
                  <Label for="newPassword">New Password (Optional)</Label>
                  <div class="relative">
                    <Lock class="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 opacity-40" />
                    <Input
                      id="newPassword"
                      type={showNewPassword ? 'text' : 'password'}
                      className="pl-10 pr-10"
                      bind:value={form.newPassword}
                    />
                    <button
                      type="button"
                      class="absolute right-3 top-1/2 transform -translate-y-1/2 opacity-40 hover:opacity-80 transition-opacity"
                      on:click={() => (showNewPassword = !showNewPassword)}
                    >
                      {#if showNewPassword}
                        <EyeOff class="h-4 w-4" />
                      {:else}
                        <Eye class="h-4 w-4" />
                      {/if}
                    </button>
                  </div>
                  {#if errors.newPassword}
                    <p class="text-sm text-red-500">{errors.newPassword}</p>
                  {/if}
                </div>
              </div>
              <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mt-4">
                <div class="space-y-2">
                  <Label for="confirmPassword">Confirm New Password</Label>
                  <div class="relative">
                    <Lock class="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 opacity-40" />
                    <Input
                      id="confirmPassword"
                      type={showConfirmPassword ? 'text' : 'password'}
                      className="pl-10 pr-10"
                      bind:value={form.confirmPassword}
                    />
                    <button
                      type="button"
                      class="absolute right-3 top-1/2 transform -translate-y-1/2 opacity-40 hover:opacity-80 transition-opacity"
                      on:click={() => (showConfirmPassword = !showConfirmPassword)}
                    >
                      {#if showConfirmPassword}
                        <EyeOff class="h-4 w-4" />
                      {:else}
                        <Eye class="h-4 w-4" />
                      {/if}
                    </button>
                  </div>
                  {#if errors.confirmPassword}
                    <p class="text-sm text-red-500">{errors.confirmPassword}</p>
                  {/if}
                </div>
              </div>
            </div>
          {/if}

          <div class="flex items-center justify-between pt-4">
            <div class="text-sm aero-muted">
              Account created on {new Date(userData.user.createdAt).toLocaleDateString()}
            </div>
            <div class="flex gap-2">
              {#if isEditing}
                <Button type="button" on:click={handleCancel} disabled={isLoading}>Cancel</Button>
                <Button type="submit" disabled={isLoading}>
                  {#if isLoading}
                    <Save class="mr-2 h-4 w-4 animate-spin" />
                    Saving...
                  {:else}
                    <Save class="mr-2 h-4 w-4" />
                    Save Changes
                  {/if}
                </Button>
              {:else}
                <Button type="button" on:click={() => (isEditing = true)}>Edit Profile</Button>
              {/if}
            </div>
          </div>
        </form>
      </CardContent>
    </Card>

    <!-- Danger Zone -->
    <Card className="border" style="border-color: rgba(255,80,80,0.25)">
      <CardHeader>
        <CardTitle className="text-rose">Danger Zone</CardTitle>
        <CardDescription>Permanent actions that cannot be undone</CardDescription>
      </CardHeader>
      <CardContent>
        <div class="flex items-center justify-between p-4 rounded-lg" style="background: rgba(255,60,60,0.08); border: 1px solid rgba(255,80,80,0.14);">
          <div>
            <h3 class="font-medium" style="color: rgba(255,160,160,0.9)">Delete Account</h3>
            <p class="text-sm" style="color: rgba(200,160,160,0.6)">
              Permanently delete your account and all associated data
            </p>
          </div>
          <Button variant="destructive" on:click={handleDeleteAccount} disabled={isDeletingAccount}>
            {isDeletingAccount ? 'Deleting...' : 'Delete Account'}
          </Button>
        </div>
      </CardContent>
    </Card>

    <!-- Delete Confirmation Dialog -->
    <Dialog bind:open={isDeleteDialogOpen} on:close={closeDeleteDialog}>
      <div class="space-y-4">
        <div class="flex items-center gap-2" style="color: rgba(255,140,140,0.9)">
          <AlertTriangle class="h-5 w-5" />
          <h2 class="text-lg font-semibold">{deleteConfirmStep === 1 ? 'Confirm Account Deletion' : 'Final Confirmation'}</h2>
        </div>
        <p class="text-sm" style="color: rgba(200,215,255,0.55)">
          {#if deleteConfirmStep === 1}
            This action cannot be undone. All your agents, calls, and data will be permanently deleted.
          {:else}
            Are you absolutely sure? This is your final chance to cancel.
          {/if}
        </p>
        {#if deleteConfirmStep === 1}
          <div class="space-y-2">
            <Label for="confirmText">Type "delete my account" to confirm:</Label>
            <Input id="confirmText" bind:value={deleteConfirmText} placeholder="delete my account" />
          </div>
        {/if}
        <div class="flex justify-end gap-2">
          <Button on:click={closeDeleteDialog} disabled={isDeletingAccount}>Cancel</Button>
          <Button
            variant="destructive"
            on:click={handleDeleteAccount}
            disabled={isDeletingAccount || (deleteConfirmStep === 1 && deleteConfirmText.toLowerCase() !== 'delete my account')}
          >
            {deleteConfirmStep === 1 ? 'Continue' : 'Delete Account'}
          </Button>
        </div>
      </div>
    </Dialog>
  </div>
{/if}
