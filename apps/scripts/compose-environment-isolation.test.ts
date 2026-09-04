import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const composePath = resolve(import.meta.dir, '..', '..', 'infra', 'docker-compose.dev.yml');
const compose = readFileSync(composePath, 'utf8');

const rustEnvironmentAllowlists = {
  talos_server: [
    'API_BACKEND_URL',
    'RMM_BIND_ADDR',
    'RMM_CORS_ORIGINS',
    'RMM_MAX_EXECUTION_SECS',
    'RMM_MAX_OUTPUT_BYTES',
    'RMM_PING_INTERVAL_SECS',
    'RMM_RELAY_URL',
    'RMM_SERVER_API_KEY',
    'RMM_TELEMETRY_PRODUCER_URL',
    'RUST_LOG',
  ],
  talos_relay: [
    'RMM_RELAY_BIND_ADDR',
    'RMM_RELAY_CLEANUP_INTERVAL_SECS',
    'RMM_RELAY_PENDING_TTL_SECS',
    'RMM_RELAY_TLS_CERT_PATH',
    'RMM_RELAY_TLS_KEY_PATH',
    'RMM_RELAY_TLS_TERMINATED',
    'RUST_LOG',
  ],
  talos_telemetry_consumer: [
    'RMM_AZURITE_ACCOUNT_KEY',
    'RMM_AZURITE_ACCOUNT_NAME',
    'RMM_AZURITE_BLOB_ENDPOINT',
    'RMM_AZURITE_CONTAINER',
    'RMM_SERVER_API_KEY',
    'RMM_TELEMETRY_BASELINE_STABILITY_THRESHOLD',
    'RMM_TELEMETRY_COMPAT_SNAPSHOT_UPSERT_URL',
    'RMM_TELEMETRY_CONSUMER_GROUP',
    'RMM_TELEMETRY_CONSUMER_RESTART_BACKOFF_MS',
    'RMM_TELEMETRY_DECISION_EXECUTE_URL',
    'RMM_TELEMETRY_EVENTS_BATCH_URL',
    'RMM_TELEMETRY_EVENTS_TOPIC',
    'RMM_TELEMETRY_GRAPH_APPLY_URL',
    'RMM_TELEMETRY_KAFKA_BROKERS',
    'RMM_TELEMETRY_KAFKA_FETCH_MAX_BYTES',
    'RMM_TELEMETRY_KAFKA_FETCH_MAX_PARTITION_BYTES',
    'RMM_TELEMETRY_KAFKA_FETCH_MAX_WAIT_MS',
    'RMM_TELEMETRY_KAFKA_FETCH_MIN_BYTES',
    'RMM_TELEMETRY_KAFKA_REBALANCE_TIMEOUT_MS',
    'RMM_TELEMETRY_KAFKA_SESSION_TIMEOUT_MS',
    'RMM_TELEMETRY_MANIFEST_URL',
    'RMM_TELEMETRY_MAX_RETRIES',
    'RMM_TELEMETRY_OFFSET_COMMIT_RETENTION_MS',
    'RMM_TELEMETRY_PATCH_PROGRESS_PROJECT_URL',
    'RMM_TELEMETRY_PATCH_PROGRESS_TOPIC',
    'RMM_TELEMETRY_PROCESSED_CHECK_URL',
    'RMM_TELEMETRY_REMEDIATION_COMMANDS_TOPIC',
    'RMM_TELEMETRY_REMEDIATION_COMMAND_PROJECT_URL',
    'RMM_TELEMETRY_REMEDIATION_DLQ_TOPIC',
    'RMM_TELEMETRY_REMEDIATION_ENQUEUE_URL',
    'RMM_TELEMETRY_REMEDIATION_STATUS_PROJECT_URL',
    'RMM_TELEMETRY_REMEDIATION_STATUS_TOPIC',
    'RMM_TELEMETRY_RETRY_BASE_MS',
    'RMM_TELEMETRY_RULES_URL_BASE',
    'RMM_TELEMETRY_SERVICE_KEY',
    'RMM_TELEMETRY_SNAPSHOT_DLQ_TOPIC',
    'RMM_TELEMETRY_SNAPSHOT_TOPIC',
    'RUST_LOG',
    'SERVICE_KEY',
  ],
  talos_telemetry_producer: [
    'RMM_SERVER_API_KEY',
    'RMM_TELEMETRY_EVENTS_TOPIC',
    'RMM_TELEMETRY_KAFKA_BROKERS',
    'RMM_TELEMETRY_PATCH_PROGRESS_TOPIC',
    'RMM_TELEMETRY_PRODUCER_BIND_ADDR',
    'RMM_TELEMETRY_REMEDIATION_COMMANDS_TOPIC',
    'RMM_TELEMETRY_REMEDIATION_STATUS_TOPIC',
    'RMM_TELEMETRY_SNAPSHOT_TOPIC',
    'RUST_LOG',
  ],
  talos_ai_runner: [
    'RMM_AI_ASSIST_MAX_ACTIONS_PER_STEP',
    'RMM_AI_ASSIST_MAX_STEPS',
    'RMM_AI_ASSIST_UNCHANGED_FRAME_WAIT_SECS',
    'RMM_SERVER_API_KEY',
    'RMM_SERVER_HTTP_URL',
    'RUST_LOG',
    'TALOS_AI_RUNNER_APPROVAL_TIMEOUT_SECS',
    'TALOS_AI_RUNNER_BIND_ADDR',
    'TALOS_AI_RUNNER_CALLBACK_BASE_URL',
    'TALOS_AI_RUNNER_COMMAND_CHECKPOINT_MS',
    'TALOS_AI_RUNNER_COMMAND_MAX_WAIT_SECS',
    'TALOS_AI_RUNNER_ID',
    'TALOS_AI_RUNNER_JOB_TIMEOUT_SECS',
    'TALOS_AI_RUNNER_LEASE_HEARTBEAT_SECS',
    'TALOS_AI_RUNNER_MAX_CONCURRENT_JOBS',
    'TALOS_AI_RUNNER_RELAY_CA_PATH',
    'TALOS_AI_RUNNER_RELAY_VERIFY_HOSTNAME',
    'TALOS_AI_RUNNER_SCREENSHOT_READ_TIMEOUT_SECS',
    'TALOS_AI_RUNNER_SERVICE_KEY',
  ],
} as const;

type ServiceName = keyof typeof rustEnvironmentAllowlists;

function composeServiceBlocks(source: string): Map<string, string> {
  const blocks = new Map<string, string>();
  const lines = source.split(/\r?\n/);
  let inServices = false;
  let currentName: string | undefined;
  let currentLines: string[] = [];

  const finishCurrent = () => {
    if (currentName) blocks.set(currentName, currentLines.join('\n'));
  };

  for (const line of lines) {
    if (line === 'services:') {
      inServices = true;
      continue;
    }
    if (!inServices) continue;

    const service = /^  ([a-zA-Z0-9_-]+):\s*$/.exec(line);
    if (service) {
      finishCurrent();
      currentName = service[1];
      currentLines = [];
      continue;
    }
    if (currentName) currentLines.push(line);
  }
  finishCurrent();
  return blocks;
}

function environmentKeys(block: string): string[] {
  const keys: string[] = [];
  let inEnvironment = false;
  for (const line of block.split(/\r?\n/)) {
    if (line === '    environment:') {
      inEnvironment = true;
      continue;
    }
    if (!inEnvironment) continue;
    if (/^    \S/.test(line)) break;
    const key = /^      ([A-Z][A-Z0-9_]*):/.exec(line)?.[1];
    if (key) keys.push(key);
  }
  return keys.sort();
}

describe('Compose service environment isolation', () => {
  const services = composeServiceBlocks(compose);

  test('allows broad env-file loading only for the API configuration owner', () => {
    const envFileOwners = [...services.entries()]
      .filter(([, block]) => /^    env_file:/m.test(block))
      .map(([name]) => name);

    expect(envFileOwners).toEqual(['api_backend']);
  });

  for (const [service, expectedKeys] of Object.entries(rustEnvironmentAllowlists) as [
    ServiceName,
    readonly string[],
  ][]) {
    test(`${service} has only its reviewed environment allowlist`, () => {
      const block = services.get(service);
      expect(block).toBeDefined();
      expect(block).not.toMatch(/^    env_file:/m);
      expect(environmentKeys(block ?? '')).toEqual([...expectedKeys].sort());
    });
  }

  test('does not expose the relay private-key directory to the AI runner', () => {
    const block = services.get('talos_ai_runner') ?? '';

    expect(block).toContain(
      'RMM_RELAY_TLS_CERT_HOST_PATH:-../apps/certs/local-dev-relay-fullchain.pem}}:/.certs/local-dev-relay-ca.pem:ro',
    );
    expect(block).not.toContain(':/.certs:ro');
  });

  test('mounts only the relay certificate and key files into the relay', () => {
    const block = services.get('talos_relay') ?? '';

    expect(block).toContain(
      'source: ${RMM_RELAY_TLS_CERT_HOST_PATH:-../apps/certs/local-dev-relay-fullchain.pem}',
    );
    expect(block).toContain(
      'source: ${RMM_RELAY_TLS_KEY_HOST_PATH:-../apps/certs/local-dev-relay-key.pem}',
    );
    expect(block).toContain('create_host_path: false');
    expect(block).not.toContain('RMM_RELAY_CERTS_HOST_PATH');
    expect(block).not.toContain(':/.certs:ro');
    expect(block).not.toContain('aliases:');
  });
});
