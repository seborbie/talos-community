<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { CheckCircle2, Clipboard, Eye, Loader2, LockKeyhole, ShieldAlert } from 'lucide-svelte';
  import { authUtils, secureNotesApi } from '$lib/api';
  import type { SecureNoteCheckResponse, SecureNoteRevealResponse, SecureNoteStatus } from '$lib/types';
  import { toast } from '$lib/toast';
  import Button from '$lib/ui/Button.svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';

  type PageState = 'loading' | 'available' | 'revealing' | 'revealed' | 'unavailable' | 'error';

  $: code = ($page.params.code || '').trim().toLowerCase();
  let state: PageState = 'loading';
  let status: SecureNoteStatus | null = null;
  let check: SecureNoteCheckResponse | null = null;
  let reveal: SecureNoteRevealResponse | null = null;
  let message = '';
  let copied = false;

  const statusMessage = (value: SecureNoteStatus | null): string => {
    switch (value) {
      case 'expired':
        return 'This secure note has expired.';
      case 'viewed':
        return 'This secure note has already been viewed.';
      case 'unauthorized':
        return 'This secure note is not assigned to your account.';
      case 'invalid':
        return 'This secure note link is invalid.';
      case 'not_found':
        return 'This secure note could not be found.';
      default:
        return 'This secure note is unavailable.';
    }
  };

  const loadNote = async () => {
    if (!/^[a-z0-9]{8}$/.test(code)) {
      status = 'invalid';
      state = 'unavailable';
      message = statusMessage(status);
      return;
    }
    if (!authUtils.isAuthenticated()) {
      goto(`/login?redirect=${encodeURIComponent(`/SN/${code}`)}`);
      return;
    }
    state = 'loading';
    try {
      const result = await secureNotesApi.check(code);
      check = result;
      status = result.status;
      if (result.status === 'available') {
        state = 'available';
        message = '';
      } else {
        state = 'unavailable';
        message = statusMessage(result.status);
      }
    } catch (error: any) {
      state = 'error';
      message = error?.message || 'Unable to load this secure note.';
    }
  };

  const revealNote = async () => {
    if (state !== 'available') return;
    copied = false;
    state = 'revealing';
    try {
      const result = await secureNotesApi.reveal(code);
      reveal = result;
      status = result.status;
      if (result.status === 'revealed' && result.content) {
        state = 'revealed';
        message = '';
      } else {
        state = 'unavailable';
        message = statusMessage(result.status);
      }
    } catch (error: any) {
      state = 'error';
      message = error?.message || 'Unable to reveal this secure note.';
    }
  };

  const copySecret = async () => {
    if (!reveal?.content) return;
    try {
      await navigator.clipboard.writeText(reveal.content);
      copied = true;
      toast({ title: 'Copied', description: 'Secure note content copied to clipboard.' });
    } catch {
      copied = false;
      toast({ title: 'Copy failed', description: 'Clipboard access is unavailable in this browser.' });
    }
  };

  onMount(loadNote);
</script>

<svelte:head>
  <title>Secure Note | Talos</title>
</svelte:head>

<div class="min-h-screen flex items-center justify-center p-4">
  <div class="w-full max-w-xl">
    <div class="mb-6 flex items-center justify-center gap-3">
      <div class="secure-note-icon">
        <LockKeyhole class="h-5 w-5" />
      </div>
      <div>
        <h1 class="text-2xl font-semibold text-white/90">Secure Note</h1>
        <p class="text-sm text-white/50">SN/{code}</p>
      </div>
    </div>

    <Card>
      <CardHeader>
        <CardTitle>One-time Secret</CardTitle>
        <CardDescription>
          Reveal only when you are ready to view it. After reveal, this note cannot be opened again.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        {#if state === 'loading'}
          <div class="state-row">
            <Loader2 class="h-5 w-5 animate-spin" />
            <span>Checking secure note...</span>
          </div>
        {:else if state === 'available'}
          <div class="warning-box">
            <ShieldAlert class="h-5 w-5 shrink-0" />
            <div>
              <p class="font-medium text-white/90">This note will be destroyed after reveal.</p>
              {#if check?.expiresAt}
                <p class="mt-1 text-sm text-white/55">Expires {new Date(check.expiresAt).toLocaleString()}</p>
              {/if}
            </div>
          </div>
          <Button className="w-full" on:click={revealNote}>
            <Eye class="h-4 w-4" />
            Reveal secure note
          </Button>
        {:else if state === 'revealing'}
          <div class="state-row">
            <Loader2 class="h-5 w-5 animate-spin" />
            <span>Revealing and destroying secure note...</span>
          </div>
        {:else if state === 'revealed'}
          <div class="success-box">
            <CheckCircle2 class="h-5 w-5 shrink-0" />
            <div>
              <p class="font-medium text-white/90">Secure note revealed.</p>
              {#if reveal?.destroyedAt}
                <p class="mt-1 text-sm text-white/55">Destroyed {new Date(reveal.destroyedAt).toLocaleString()}</p>
              {/if}
            </div>
          </div>
          <pre class="secret-value">{reveal?.content}</pre>
          <Button variant="secondary" className="w-full" on:click={copySecret}>
            <Clipboard class="h-4 w-4" />
            {copied ? 'Copied' : 'Copy'}
          </Button>
        {:else}
          <div class="error-box">
            <ShieldAlert class="h-5 w-5 shrink-0" />
            <div>
              <p class="font-medium text-white/90">{message}</p>
              <p class="mt-1 text-sm text-white/55">Secure note content was not revealed.</p>
            </div>
          </div>
          <Button variant="secondary" className="w-full" on:click={loadNote}>Check again</Button>
        {/if}
      </CardContent>
    </Card>
  </div>
</div>

<style>
  .secure-note-icon {
    width: 42px;
    height: 42px;
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: rgba(235, 248, 255, 0.96);
    background: linear-gradient(145deg, rgba(14, 116, 144, 0.9), rgba(37, 99, 235, 0.85));
    border: 1px solid rgba(148, 221, 255, 0.28);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.26), 0 14px 34px rgba(12, 74, 110, 0.24);
  }

  .state-row,
  .warning-box,
  .success-box,
  .error-box {
    display: flex;
    gap: 0.75rem;
    align-items: flex-start;
    border-radius: 8px;
    padding: 1rem;
    border: 1px solid rgba(255, 255, 255, 0.12);
  }

  .state-row {
    align-items: center;
    color: rgba(226, 242, 255, 0.74);
    background: rgba(255, 255, 255, 0.04);
  }

  .warning-box {
    color: rgba(253, 230, 138, 0.95);
    background: rgba(161, 98, 7, 0.16);
  }

  .success-box {
    color: rgba(167, 243, 208, 0.95);
    background: rgba(5, 150, 105, 0.14);
  }

  .error-box {
    color: rgba(254, 202, 202, 0.95);
    background: rgba(153, 27, 27, 0.15);
  }

  .secret-value {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border-radius: 8px;
    padding: 1rem;
    color: rgba(255, 255, 255, 0.92);
    background: rgba(2, 6, 23, 0.72);
    border: 1px solid rgba(148, 163, 184, 0.24);
    font-size: 1rem;
    line-height: 1.6;
  }
</style>
