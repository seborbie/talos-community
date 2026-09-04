<script lang="ts">
  import { cn } from '$lib/utils';
  export let className = '';
  $: classes = cn(
    'border-b border-white/6 transition-colors hover:bg-white/5 data-[state=selected]:bg-white/8',
    className,
    $$restProps.class
  );
</script>

<tr class={classes} {...$$restProps}>
  <slot></slot>
</tr>
