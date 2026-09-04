<script lang="ts">
  import { toasts, dismiss } from '$lib/toast';
</script>

<div class="fixed top-4 right-4 z-[100] flex w-full max-w-sm flex-col gap-3 px-4 sm:px-0">
  {#each $toasts as toast (toast.id)}
    <div
      class="toast-panel relative flex w-full items-start justify-between gap-4 overflow-hidden rounded-xl p-4"
      class:toast-destructive={toast.variant === 'destructive'}
    >
      <div class="grid gap-1 pr-5">
        {#if toast.title}
          <div class="text-sm font-semibold">{toast.title}</div>
        {/if}
        {#if toast.description}
          <div class="text-sm opacity-75">{toast.description}</div>
        {/if}
      </div>
      <button
        type="button"
        class="absolute right-2.5 top-2.5 rounded-full w-6 h-6 flex items-center justify-center opacity-50 hover:opacity-100 transition-opacity text-base leading-none"
        on:click={() => dismiss(toast.id)}
        aria-label="Dismiss"
      >×</button>
    </div>
  {/each}
</div>

<style>
  .toast-panel {
    background: rgba(10, 20, 55, 0.88);
    backdrop-filter: blur(24px) saturate(180%);
    -webkit-backdrop-filter: blur(24px) saturate(180%);
    border: 1px solid rgba(70, 140, 255, 0.2);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.07),
      0 12px 40px rgba(0, 0, 0, 0.55),
      0 4px 16px rgba(0, 0, 0, 0.35);
    color: rgba(220, 240, 255, 0.9);
    animation: toast-in 0.28s cubic-bezier(0.22, 1, 0.36, 1) forwards;
  }
  .toast-destructive {
    background: rgba(60, 10, 10, 0.88) !important;
    border-color: rgba(255, 80, 80, 0.28) !important;
    color: rgba(255, 200, 200, 0.92) !important;
  }
  @keyframes toast-in {
    from { opacity: 0; transform: translateX(24px) scale(0.97); }
    to   { opacity: 1; transform: translateX(0)    scale(1); }
  }

  /* Light mode */
  :global(html.light) .toast-panel {
    background: rgba(255, 255, 255, 0.85);
    border-color: rgba(100, 158, 220, 0.28);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.9),
      0 12px 40px rgba(0, 30, 100, 0.14);
    color: #0a1628;
  }
  :global(html.light) .toast-destructive {
    background: rgba(255, 245, 245, 0.9) !important;
    border-color: rgba(220, 50, 50, 0.3) !important;
    color: rgba(160, 20, 20, 0.92) !important;
  }
</style>
