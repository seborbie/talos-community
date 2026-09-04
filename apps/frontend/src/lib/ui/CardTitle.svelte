<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('text-2xl font-semibold leading-none tracking-tight', className, $$restProps.class);
</script>

<h3 class={classes} {...$$restProps}>
  <slot></slot>
</h3>
