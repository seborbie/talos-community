<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn('[&_tr]:border-b [&_tr]:border-white/8', className, $$restProps.class);
</script>

<thead class={classes} {...$$restProps}>
  <slot></slot>
</thead>
