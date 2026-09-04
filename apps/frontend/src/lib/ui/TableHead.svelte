<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn(
    'h-12 px-4 text-left align-middle font-medium [&:has([role=checkbox])]:pr-0',
    className,
    $$restProps.class
  );
</script>

<th class="{classes} aero-th" {...$$restProps}>
  <slot></slot>
</th>
