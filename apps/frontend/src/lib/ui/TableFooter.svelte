<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('border-t bg-muted/50 font-medium [&>tr]:last:border-b-0', className, $$restProps.class);
</script>

<tfoot class={classes} {...$$restProps}>
  <slot></slot>
</tfoot>
