<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('p-4 align-middle text-sm [&:has([role=checkbox])]:pr-0', className, $$restProps.class);
</script>

<td class="{classes} aero-td" {...$$restProps}>
  <slot></slot>
</td>
