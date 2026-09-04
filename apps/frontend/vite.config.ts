import { sveltekit } from '@sveltejs/kit/vite';
import { fileURLToPath } from 'node:url';
import { createLogger, defineConfig } from 'vite';
import { createFrontendBuildWarningGate } from './src/lib/build-warning-policy';
import { createFrontendViteServerConfig } from './src/lib/vite-host-policy';

const appsEnvDir = fileURLToPath(new URL('..', import.meta.url));
const logger = createLogger();
const defaultWarn = logger.warn.bind(logger);
const handleBuildWarning = createFrontendBuildWarningGate();

logger.warn = (message, options) => {
  handleBuildWarning(message, (forwardedMessage) => defaultWarn(forwardedMessage, options));
};

export const frontendViteConfig = {
  customLogger: logger,
  envDir: appsEnvDir,
  plugins: [sveltekit()],
  server: createFrontendViteServerConfig(),
};

export default defineConfig(frontendViteConfig);
