import { describe, expect, test } from 'bun:test';
import { FRONTEND_DEV_ALLOWED_HOSTS, createFrontendViteServerConfig } from './vite-host-policy';

describe('Vite development host policy', () => {
  test('allows only the reviewed development proxy hostname', () => {
    expect(FRONTEND_DEV_ALLOWED_HOSTS).toEqual(['dev.talos.cloud']);
    expect(createFrontendViteServerConfig().allowedHosts).toEqual(['dev.talos.cloud']);
    expect(createFrontendViteServerConfig().allowedHosts).not.toBe(true);
  });
});
