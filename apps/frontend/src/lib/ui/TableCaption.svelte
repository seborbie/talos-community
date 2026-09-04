<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('mt-4 text-sm text-muted-foreground', className, $$restProps.class);
</script>

<caption class={classes} {...$$restProps}>
  <slot></slot>
</caption>
