<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { cn } from '$lib/utils';

  const dispatch = createEventDispatcher();

  export let variant: 'default' | 'destructive' | 'outline' | 'secondary' | 'ghost' | 'link' = 'default';
  export let size: 'default' | 'sm' | 'lg' | 'icon' = 'default';
  export let type: 'button' | 'submit' | 'reset' = 'button';
  export let disabled = false;
  export let href: string | null = null;
  export let className = '';

  const base =
    'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-semibold transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-40';

  const variants: Record<typeof variant, string> = {
    default:     'aero-btn aero-btn-primary',
    destructive: 'aero-btn aero-btn-destructive',
    outline:     'aero-btn aero-btn-secondary',
    secondary:   'aero-btn aero-btn-secondary',
    ghost:       'aero-btn aero-btn-ghost',
    link:        'text-sky-300 underline-offset-4 hover:underline hover:text-sky-200 transition-colors',
  };

  const sizes: Record<typeof size, string> = {
    default: 'h-10 px-4 py-2',
    sm:      'h-9 px-3 text-xs',
    lg:      'h-11 px-8',
    icon:    'h-10 w-10 p-0',
  };

  $: classes = cn(base, variants[variant], sizes[size], className, $$restProps.class);
</script>

{#if href}
  <a href={href} class={classes} {...$$restProps}>
    <slot></slot>
  </a>
{:else}
  <button
    type={type}
    class={classes}
    {disabled}
    on:click={(event) => dispatch('click', event)}
    {...$$restProps}
  >
    <slot></slot>
  </button>
{/if}
