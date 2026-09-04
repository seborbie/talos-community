<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('flex items-center p-6 pt-0', className, $$restProps.class);
</script>

<div class={classes} {...$$restProps}>
  <slot></slot>
</div>
