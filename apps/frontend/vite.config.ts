import { sveltekit } from '@sveltejs/kit/vite';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { createFrontendViteServerConfig } from './src/lib/vite-host-policy.ts';

const appsEnvDir = fileURLToPath(new URL('..', import.meta.url));
export const frontendViteConfig = {
  envDir: appsEnvDir,
  plugins: [sveltekit()],
  // CSS @imports need a file path during SSR preprocessing, not an external JS package ID.
  ssr: { noExternal: ['tailwindcss'] },
  server: createFrontendViteServerConfig(),
};

export default defineConfig(frontendViteConfig);
