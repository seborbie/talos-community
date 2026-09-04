<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('[&_tr:last-child]:border-0', className, $$restProps.class);
</script>

<tbody class={classes} {...$$restProps}>
  <slot></slot>
</tbody>
