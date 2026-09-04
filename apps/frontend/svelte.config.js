import adapter from '@sveltejs/adapter-node';
import { fileURLToPath } from 'node:url';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

const appsEnvDir = fileURLToPath(new URL('..', import.meta.url));

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess({ script: true }),
  kit: {
    adapter: adapter(),
    env: {
      dir: appsEnvDir,
    },
  },
};

export default config;
