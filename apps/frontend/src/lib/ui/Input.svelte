<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  export let type = 'text';
  export let value = '';
  $: classes = cn(
    'glass-input flex h-10 w-full rounded-md px-3 py-2 text-sm file:border-0 file:bg-transparent file:text-sm file:font-medium disabled:cursor-not-allowed disabled:opacity-40',
    className,
    $$restProps.class
  );
</script>

<input
  {type}
  class={classes}
  bind:value
  {...$$restProps}
/>
