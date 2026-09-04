<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('text-sm text-muted-foreground', className, $$restProps.class);
</script>

<p class={classes} {...$$restProps}>
  <slot></slot>
</p>
