import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const compose = readFileSync(
  resolve(import.meta.dir, '..', '..', 'infra', 'docker-compose.dev.yml'),
  'utf8',
);

const expectedBindings = [
  '${TALOS_FRONTEND_HOST_BIND:-127.0.0.1}:3000:3000',
  '${TALOS_API_HOST_BIND:-127.0.0.1}:3001:3001',
  '${TALOS_RMM_SERVER_HOST_BIND:-127.0.0.1}:3002:17110',
  '${TALOS_TELEMETRY_PRODUCER_HOST_BIND:-127.0.0.1}:3003:17120',
  '${TALOS_POSTGRES_HOST_BIND:-127.0.0.1}:3004:5432',
  '${TALOS_REDPANDA_HOST_BIND:-127.0.0.1}:3005:3005',
  '${TALOS_REDPANDA_HOST_BIND:-127.0.0.1}:3006:3006',
  '${TALOS_REDPANDA_CONSOLE_HOST_BIND:-127.0.0.1}:3007:8080',
  '${TALOS_AZURITE_HOST_BIND:-127.0.0.1}:3008:10000',
  '${TALOS_REDPANDA_HOST_BIND:-127.0.0.1}:3009:9644',
  '${TALOS_AI_RUNNER_HOST_BIND:-127.0.0.1}:3010:3010',
  '${RMM_RELAY_HOST_BIND:-127.0.0.1}:${RMM_RELAY_HOST_PORT:-17443}:443',
] as const;

describe('Compose host exposure contract', () => {
  test('publishes every development and Community port on loopback by default', () => {
    for (const binding of expectedBindings) {
      expect(compose).toContain(`- "${binding}"`);
    }

    const portEntries: string[] = [];
    let inPorts = false;
    for (const line of compose.split(/\r?\n/)) {
      if (line === '    ports:') {
        inPorts = true;
        continue;
      }
      if (inPorts && /^    \S/.test(line)) inPorts = false;
      if (inPorts && /^      - /.test(line)) portEntries.push(line);
    }
    expect(portEntries).toHaveLength(expectedBindings.length);
    expect(portEntries.every((line) => line.includes('127.0.0.1'))).toBe(true);
  });

  test('keeps container listeners separate from host publication policy', () => {
    expect(compose).toContain('API_BIND_HOST: 0.0.0.0');
    expect(compose).not.toContain('API_TRUSTED_PROXIES:');
  });
});
