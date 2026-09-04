import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/vite-plugin-svelte').SvelteConfig} */
export default {
  preprocess: vitePreprocess({ script: true }),
  compilerOptions: {
    // App.svelte / RemoteRegistry.svelte still use legacy `$:` reactivity.
    runes: false,
  },
};
