<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import Button from '$lib/ui/Button.svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import { toast } from '$lib/toast';
  import { authApi, authUtils, orgsApi } from '$lib/api';
  import { Server, Eye, EyeOff, Loader2 } from 'lucide-svelte';
  import { z } from 'zod';

  const loginSchema = z.object({
    email:    z.string().email('Invalid email address'),
    password: z.string().min(1, 'Password is required'),
  });

  let email        = '';
  let password     = '';
  let showPassword = false;
  let isLoading    = false;
  let errors: { email?: string; password?: string } = {};
  let loginError   = '';
  let registrationOpen = false;

  onMount(() => {
    void authApi.getRegistrationStatus()
      .then((status) => { registrationOpen = status.registrationOpen; })
      .catch(() => { registrationOpen = false; });
  });

  $: redirectTo = $page.url.searchParams.get('redirect') || '';

  const safeRedirect = (value: string): string | null => {
    if (!value.startsWith('/') || value.startsWith('//')) return null;
    return value;
  };

  const setErrors = (issues: z.ZodIssue[]) => {
    const next: { email?: string; password?: string } = {};
    issues.forEach((issue) => {
      const key = issue.path[0];
      if (key === 'email' || key === 'password') next[key] = issue.message;
    });
    errors = next;
  };

  const onSubmit = async () => {
    loginError = '';
    const parsed = loginSchema.safeParse({ email, password });
    if (!parsed.success) { setErrors(parsed.error.issues); return; }
    try {
      isLoading = true;
      const response = await authApi.login(parsed.data);
      authUtils.setToken(response.token);
      toast({ title: 'Welcome back!', description: "You've been successfully logged in." });
      const redirect = safeRedirect(redirectTo);
      if (redirect) {
        goto(redirect);
        return;
      }
      try {
        const current = await orgsApi.getCurrent();
        if ((current as any)?.needsOnboarding) goto('/dashboard/onboarding');
        else goto('/dashboard');
      } catch { goto('/dashboard'); }
    } catch (error: any) {
      console.error('Login error:', error);
      const status = error?.statusCode;
      if (status === 401 || status === 400) {
        loginError = 'Incorrect email or password. Please check your credentials and try again.';
      } else if (status === 429) {
        loginError = 'Too many failed attempts. Please wait a moment before trying again.';
      } else {
        loginError = error?.message || 'Something went wrong. Please try again.';
      }
    } finally { isLoading = false; }
  };
</script>

<div class="min-h-screen flex items-center justify-center p-4">
  <div class="w-full max-w-md">
    <!-- Wordmark -->
    <div class="text-center mb-8">
      <div class="flex items-center justify-center gap-3 mb-4">
        <div class="auth-logo-icon">
          <Server class="h-5 w-5 text-white" />
        </div>
        <span class="auth-wordmark">Talos</span>
      </div>
      <h1 class="text-2xl font-bold text-white/90">Welcome back</h1>
      <p class="auth-subtitle mt-2">Sign in to your account</p>
    </div>

    <Card className="p-0">
      <CardHeader>
        <CardTitle>Sign In</CardTitle>
        <CardDescription>Enter your credentials to access your dashboard</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="space-y-4" novalidate on:submit|preventDefault={onSubmit}>
          <div class="space-y-2">
            <Label for="email">Email</Label>
            <Input id="email" type="email" placeholder="you@example.com" bind:value={email} on:input={() => { loginError = ''; errors.email = undefined; }} />
            {#if errors.email}
              <p class="text-sm text-red-400">{errors.email}</p>
            {/if}
          </div>

          <div class="space-y-2">
            <Label for="password">Password</Label>
            <div class="relative">
              <Input id="password" type={showPassword ? 'text' : 'password'} placeholder="Your password" bind:value={password} on:input={() => { loginError = ''; errors.password = undefined; }} />
              <button
                type="button"
                class="absolute right-3 top-1/2 -translate-y-1/2 auth-eye-btn"
                on:click={() => (showPassword = !showPassword)}
                aria-label="Toggle password visibility"
              >
                {#if showPassword}<EyeOff class="h-4 w-4" />{:else}<Eye class="h-4 w-4" />{/if}
              </button>
            </div>
            {#if errors.password}
              <p class="text-sm text-red-400">{errors.password}</p>
            {/if}
          </div>

          {#if loginError}
            <div class="aero-alert-error" role="alert">{loginError}</div>
          {/if}

          <Button type="submit" className="w-full" disabled={isLoading}>
            {#if isLoading}<Loader2 class="mr-2 h-4 w-4 animate-spin" />Signing in...{:else}Sign In{/if}
          </Button>
        </form>

        {#if registrationOpen}
          <div class="mt-6 text-center">
            <p class="text-sm auth-subtitle">
              This deployment has not been initialized.
              <a href="/register" class="auth-link">Create the first administrator</a>
            </p>
          </div>
        {/if}
      </CardContent>
    </Card>
  </div>
</div>

<style>
  .auth-logo-icon {
    width: 40px; height: 40px; border-radius: 11px;
    background: linear-gradient(145deg, rgba(70, 150, 255, 0.85), rgba(20, 80, 210, 0.9));
    border: 1px solid rgba(120, 190, 255, 0.3);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.35), 0 0 18px rgba(50,130,255,0.4);
    display: flex; align-items: center; justify-content: center;
  }
  .auth-wordmark {
    font-size: 1.5rem; font-weight: 700; letter-spacing: -0.03em;
    background: linear-gradient(180deg, #ffffff 0%, rgba(160, 210, 255, 0.8) 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  .auth-subtitle { color: rgba(160, 205, 255, 0.55); }
  .auth-link {
    color: rgba(110, 180, 255, 0.9); font-weight: 500;
    text-decoration: none; transition: color 0.15s;
  }
  .auth-link:hover { color: rgba(150, 210, 255, 1); }
  .auth-eye-btn { color: rgba(160, 200, 255, 0.5); transition: color 0.15s; }
  .auth-eye-btn:hover { color: rgba(200, 230, 255, 0.85); }

  :global(html.light) .auth-wordmark {
    background: linear-gradient(180deg, #081428 0%, #1a4888 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  :global(html.light) h1 { color: #0a1628 !important; }
  :global(html.light) .auth-subtitle { color: rgba(12, 42, 108, 0.58); }
  :global(html.light) .auth-link { color: rgba(18, 68, 200, 0.88); }
  :global(html.light) .auth-link:hover { color: rgba(12, 48, 180, 1); }
  :global(html.light) .auth-eye-btn { color: rgba(10, 50, 140, 0.45); }
  :global(html.light) .auth-eye-btn:hover { color: rgba(10, 50, 140, 0.8); }
</style>
