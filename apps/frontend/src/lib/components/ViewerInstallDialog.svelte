<script lang="ts">
  import Dialog from '$lib/ui/Dialog.svelte';
  import Button from '$lib/ui/Button.svelte';

  export let open = false;
  export let downloading = false;
  export let connectLabel = 'Viewer';
  export let installMessage =
    'Talos Viewer is required to open remote desktop, shell, file transfer, and registry sessions on Windows.';
  export let downloadSupported = true;

  export let onDownload: (() => void | Promise<void>) | null = null;
  export let onRetry: (() => void | Promise<void>) | null = null;
  export let onCancel: (() => void | Promise<void>) | null = null;

  const runAction = (action: (() => void | Promise<void>) | null) => {
    if (!action) {
      return;
    }
    void action();
  };
</script>

<Dialog bind:open className="max-w-xl">
  <div class="space-y-4">
    <div class="space-y-2">
      <p class="text-xs font-semibold uppercase tracking-[0.28em] text-sky-300/80">Talos Viewer Required</p>
      <h2 class="text-2xl font-semibold">Install Talos Viewer to continue</h2>
      <p class="text-sm leading-6 text-[rgba(210,232,255,0.82)]">
        {installMessage}
      </p>
    </div>

    <div class="rounded-xl border border-white/10 bg-white/5 px-4 py-3 text-sm text-[rgba(210,232,255,0.78)]">
      After installing, return to this page and retry <span class="font-semibold text-white">{connectLabel}</span>.
    </div>

    <div class="flex flex-wrap justify-end gap-2">
      <Button variant="ghost" on:click={() => runAction(onCancel)}>Cancel</Button>
      <Button variant="outline" on:click={() => runAction(onRetry)}>Retry</Button>
      {#if downloadSupported}
        <Button on:click={() => runAction(onDownload)} disabled={downloading}>
          {downloading ? 'Downloading...' : 'Download Viewer'}
        </Button>
      {/if}
    </div>
  </div>
</Dialog>
