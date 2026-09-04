<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('aero-label text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70', className, $$restProps.class);
</script>

<label class={classes} {...$$restProps}>
  <slot></slot>
</label>
