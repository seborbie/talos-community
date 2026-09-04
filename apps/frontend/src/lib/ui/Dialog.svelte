<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import { browser } from '$app/environment';
  import { cn } from '$lib/utils';

  export let open = false;
  export let className = '';

  const dispatch = createEventDispatcher();
  let previousOverflow: string | null = null;

  const close = () => {
    open = false;
    dispatch('close');
  };

  const handleKeydown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') close();
  };

  $: if (browser) {
    if (open) {
      if (previousOverflow === null) {
        previousOverflow = document.body.style.overflow;
        document.body.style.overflow = 'hidden';
      }
      window.addEventListener('keydown', handleKeydown);
    } else {
      window.removeEventListener('keydown', handleKeydown);
      if (previousOverflow !== null) {
        document.body.style.overflow = previousOverflow;
        previousOverflow = null;
      }
    }
  }

  onDestroy(() => {
    if (browser) {
      window.removeEventListener('keydown', handleKeydown);
      if (previousOverflow !== null) document.body.style.overflow = previousOverflow;
    }
  });

  $: contentClasses = cn(
    'fixed left-1/2 top-1/2 z-50 grid w-full max-w-lg -translate-x-1/2 -translate-y-1/2 gap-4 p-6 shadow-2xl duration-200 sm:rounded-xl',
    className
  );
</script>

{#if open}
  <div class="fixed inset-0 z-50">
    <!-- Blurred backdrop -->
    <button
      type="button"
      class="absolute inset-0 bg-black/60 backdrop-blur-sm"
      aria-label="Close dialog"
      on:click={close}
    ></button>

    <!-- Glass panel -->
    <div
      class={contentClasses}
      role="dialog"
      aria-modal="true"
      tabindex="0"
      on:click|stopPropagation
      on:keydown|stopPropagation
      style="
        background: rgba(8, 18, 52, 0.82);
        backdrop-filter: blur(32px) saturate(180%);
        -webkit-backdrop-filter: blur(32px) saturate(180%);
        border: 1px solid rgba(70, 140, 255, 0.18);
        box-shadow:
          inset 0 1px 0 rgba(255,255,255,0.08),
          0 32px 80px rgba(0,0,0,0.7),
          0 8px 32px rgba(0,0,0,0.45);
        color: rgba(220, 240, 255, 0.9);
      "
    >
      <button
        type="button"
        class="absolute right-4 top-4 rounded-full w-7 h-7 flex items-center justify-center opacity-60 hover:opacity-100 transition-opacity focus:outline-none"
        style="background: rgba(255,255,255,0.1); border: 1px solid rgba(255,255,255,0.14); color: rgba(200,225,255,0.8);"
        on:click={close}
        aria-label="Close"
      >
        ×
      </button>
      <slot></slot>
    </div>
  </div>
{/if}

<style>
  /* Light mode overrides */
  :global(html.light) div[role="dialog"] {
    background: rgba(255, 255, 255, 0.88) !important;
    backdrop-filter: blur(32px) saturate(200%) !important;
    -webkit-backdrop-filter: blur(32px) saturate(200%) !important;
    border-color: rgba(255, 255, 255, 0.7) !important;
    box-shadow:
      inset 0 1px 0 rgba(255,255,255,0.9),
      0 32px 80px rgba(0,30,100,0.18),
      0 8px 32px rgba(0,20,80,0.1) !important;
    color: #0a1628 !important;
  }
</style>
