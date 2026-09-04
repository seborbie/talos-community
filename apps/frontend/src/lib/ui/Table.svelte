<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('w-full caption-bottom text-sm', className, $$restProps.class);
</script>

<div class="relative w-full overflow-x-auto overflow-y-visible">
  <table class={classes} {...$$restProps}>
    <slot></slot>
  </table>
</div>
