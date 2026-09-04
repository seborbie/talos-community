<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('flex flex-col space-y-1.5 p-6', className, $$restProps.class);
</script>

<div class={classes} {...$$restProps}>
  <slot></slot>
</div>
