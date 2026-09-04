<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('glass-card text-foreground', className, $$restProps.class);
</script>

<div class={classes} {...$$restProps}>
  <slot></slot>
</div>
