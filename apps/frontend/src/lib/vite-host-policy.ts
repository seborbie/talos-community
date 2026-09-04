import type { ServerOptions } from 'vite';

export const FRONTEND_DEV_ALLOWED_HOSTS = ['dev.talos.cloud'] as const;

export function createFrontendViteServerConfig(): ServerOptions {
  return {
    allowedHosts: [...FRONTEND_DEV_ALLOWED_HOSTS],
  };
}
