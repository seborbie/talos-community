import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  COMMUNITY_CORE_SERVICES,
  COMMUNITY_CORE_WAIT_TIMEOUT_SECONDS,
  COMMUNITY_DATABASE_WAIT_TIMEOUT_SECONDS,
  COMMUNITY_DISABLED_INTEGRATIONS,
  COMMUNITY_PRISMA_SCHEMA_PATH,
  COMMUNITY_PROJECT_NAME,
  assertCommunityEnvironment,
  communityEnvironment,
  createCommunityComposePlans,
  executeCommunityComposePlans,
  parseCommunityCommand,
} from './community-edition';

describe('Community Edition launcher', () => {
  test('rejects public example credentials before any Compose phase', () => {
    const configured = {
      JWT_SECRET: 'generated-jwt-value',
      APP_ENCRYPTION_KEY: 'generated-encryption-value',
      TOKEN_TTL: '1h',
      MACHINE_TOKEN_TTL: '30d',
      RMM_SERVER_API_KEY: 'generated-rmm-key',
    };

    expect(() => assertCommunityEnvironment(configured)).not.toThrow();
    expect(() =>
      assertCommunityEnvironment({
        ...configured,
        SERVICE_KEY: 'replace_with_shared_service_key',
      }),
    ).toThrow('public example credentials are still configured: SERVICE_KEY');
    expect(() =>
      assertCommunityEnvironment({
        ...configured,
        RMM_SERVER_API_KEY: 'replace_with_shared_rmm_server_key',
      }),
    ).toThrow('RMM_SERVER_API_KEY');
    expect(() =>
      assertCommunityEnvironment({
        TOKEN_TTL: '1h',
        MACHINE_TOKEN_TTL: '30d',
      }),
    ).toThrow('APP_ENCRYPTION_KEY, JWT_SECRET, RMM_SERVER_API_KEY');
  });

  test('keeps host-native .env URLs out of container service routing', () => {
    const compose = readFileSync(
      resolve(import.meta.dir, '..', '..', 'infra', 'docker-compose.dev.yml'),
      'utf8',
    );

    expect(compose).toContain(
      'RMM_TELEMETRY_PRODUCER_URL: "${TALOS_COMPOSE_TELEMETRY_PRODUCER_URL-http://talos_telemetry_producer:17120}"',
    );
    expect(compose).toContain(
      'TALOS_AI_RUNNER_URL: "${TALOS_COMPOSE_AI_RUNNER_URL-http://talos_ai_runner:3010}"',
    );
    expect(compose).toContain('RMM_SERVER_HTTP_URL: http://talos_server:17110');
    expect(compose).toContain(
      'AZURE_STORAGE_CONNECTION_STRING: ${TALOS_COMPOSE_AZURE_STORAGE_CONNECTION_STRING:-',
    );
    expect(compose).toContain('BlobEndpoint=http://azurite:10000/devstoreaccount1;');
  });

  test('selects exactly the five core services from the shared Compose file', () => {
    const repoRoot = resolve('test-repository');
    const plans = createCommunityComposePlans('up', {}, repoRoot);
    const startPlan = plans[2];

    expect(COMMUNITY_CORE_SERVICES).toEqual([
      'postgres',
      'api_backend',
      'frontend',
      'talos_relay',
      'talos_server',
    ]);
    expect(startPlan?.argv).toEqual([
      'docker',
      'compose',
      '--project-name',
      COMMUNITY_PROJECT_NAME,
      '--env-file',
      resolve(repoRoot, 'apps', '.env'),
      '-f',
      resolve(repoRoot, 'infra', 'docker-compose.dev.yml'),
      'up',
      '--detach',
      '--build',
      '--wait',
      '--wait-timeout',
      String(COMMUNITY_CORE_WAIT_TIMEOUT_SECONDS),
      ...COMMUNITY_CORE_SERVICES,
    ]);
    expect(startPlan?.argv).not.toContain('redpanda-0');
    expect(startPlan?.argv).not.toContain('talos_telemetry_producer');
    expect(startPlan?.argv).not.toContain('talos_telemetry_consumer');
    expect(startPlan?.argv).not.toContain('talos_ai_runner');
  });

  test('defines readiness checks for every persistent core service', () => {
    const compose = readFileSync(
      resolve(import.meta.dir, '..', '..', 'infra', 'docker-compose.dev.yml'),
      'utf8',
    );

    for (const service of COMMUNITY_CORE_SERVICES) {
      const serviceHeader = `\n  ${service}:\n`;
      const serviceHeaderIndex = compose.indexOf(serviceHeader);
      expect(serviceHeaderIndex).toBeGreaterThan(-1);
      const serviceStart = serviceHeaderIndex + 1;
      const remainingCompose = compose.slice(serviceStart + serviceHeader.length - 1);
      const nextServiceMatch = /\n  [a-zA-Z0-9_-]+:\n/.exec(remainingCompose);
      const nextService =
        nextServiceMatch?.index === undefined
          ? -1
          : serviceStart + serviceHeader.length - 1 + nextServiceMatch.index;
      const definition = compose.slice(
        serviceStart,
        nextService === -1 ? compose.length : nextService,
      );
      expect(definition).toContain('healthcheck:');
    }
  });

  test('waits for PostgreSQL and applies migrations before starting API-dependent services', () => {
    const repoRoot = resolve('test-repository');
    const plans = createCommunityComposePlans('up', {}, repoRoot);
    const composePrefix = [
      'docker',
      'compose',
      '--project-name',
      COMMUNITY_PROJECT_NAME,
      '--env-file',
      resolve(repoRoot, 'apps', '.env'),
      '-f',
      resolve(repoRoot, 'infra', 'docker-compose.dev.yml'),
    ];

    expect(plans.map((plan) => plan.phase)).toEqual([
      'wait for PostgreSQL',
      'apply database migrations',
      'start core services',
    ]);
    expect(plans[0]?.argv).toEqual([
      ...composePrefix,
      'up',
      '--detach',
      '--wait',
      '--wait-timeout',
      String(COMMUNITY_DATABASE_WAIT_TIMEOUT_SECONDS),
      'postgres',
    ]);
    expect(plans[1]?.argv).toEqual([
      ...composePrefix,
      'run',
      '--rm',
      '--no-deps',
      '--no-TTY',
      '--build',
      'api_backend',
      'bun',
      'x',
      '--bun',
      'prisma',
      'migrate',
      'deploy',
      '--schema',
      COMMUNITY_PRISMA_SCHEMA_PATH,
    ]);
    expect(plans[0]?.argv).not.toContain('api_backend');
    expect(plans[1]?.argv).not.toContain('frontend');
    expect(plans[1]?.argv).not.toContain('talos_server');
  });

  test('explicitly disables every optional service integration', () => {
    const baseEnvironment = {
      KEEP_ME: 'present',
      TALOS_COMPOSE_TELEMETRY_PRODUCER_URL: 'http://custom-producer:17120',
      TALOS_COMPOSE_AI_RUNNER_URL: 'http://custom-runner:3010',
    };

    expect(communityEnvironment(baseEnvironment)).toEqual({
      KEEP_ME: 'present',
      ...COMMUNITY_DISABLED_INTEGRATIONS,
    });
    expect(baseEnvironment.TALOS_COMPOSE_TELEMETRY_PRODUCER_URL).toBe(
      'http://custom-producer:17120',
    );
  });

  test('passes apps/.env to startup and validation without coupling cleanup to the file', () => {
    const repoRoot = resolve('test-repository');

    for (const command of ['up', 'config'] as const) {
      for (const plan of createCommunityComposePlans(command, {}, repoRoot)) {
        const envFileIndex = plan.argv.indexOf('--env-file');
        expect(envFileIndex).toBeGreaterThan(-1);
        expect(plan.argv[envFileIndex + 1]).toBe(resolve(repoRoot, 'apps', '.env'));
      }
    }
    for (const command of ['stop', 'down'] as const) {
      const [plan] = createCommunityComposePlans(command, {}, repoRoot);
      expect(plan?.argv).not.toContain('--env-file');
    }
  });

  test('lifecycle commands stay in the dedicated project and config validates the shared file', () => {
    const repoRoot = resolve('test-repository');
    const [stop] = createCommunityComposePlans('stop', {}, repoRoot);
    const [down] = createCommunityComposePlans('down', {}, repoRoot);
    const [config] = createCommunityComposePlans('config', {}, repoRoot);

    expect(stop?.argv.slice(-COMMUNITY_CORE_SERVICES.length - 1)).toEqual([
      'stop',
      ...COMMUNITY_CORE_SERVICES,
    ]);
    expect(down?.argv.slice(-1)).toEqual(['down']);
    expect(config?.argv.slice(-2)).toEqual(['config', '--quiet']);
    expect(stop?.argv).toContain(COMMUNITY_PROJECT_NAME);
    expect(down?.argv).toContain(COMMUNITY_PROJECT_NAME);
    expect(config?.argv).toContain(COMMUNITY_PROJECT_NAME);
    expect(stop?.environment).toEqual(COMMUNITY_DISABLED_INTEGRATIONS);
    expect(down?.environment).toEqual(COMMUNITY_DISABLED_INTEGRATIONS);
    expect(config?.environment).toEqual(COMMUNITY_DISABLED_INTEGRATIONS);
  });

  test('does not start services when the migration phase fails', async () => {
    const plans = createCommunityComposePlans('up', {});
    const executed: string[] = [];

    const exitCode = await executeCommunityComposePlans(plans, async (plan) => {
      executed.push(plan.phase);
      return plan.phase === 'apply database migrations' ? 17 : 0;
    });

    expect(exitCode).toBe(17);
    expect(executed).toEqual(['wait for PostgreSQL', 'apply database migrations']);
  });

  test('starts core services after every prerequisite succeeds', async () => {
    const plans = createCommunityComposePlans('up', {});
    const executed: string[] = [];

    const exitCode = await executeCommunityComposePlans(plans, async (plan) => {
      executed.push(plan.phase);
      return 0;
    });

    expect(exitCode).toBe(0);
    expect(executed).toEqual(plans.map((plan) => plan.phase));
  });

  test('rejects ambiguous or unsupported commands before spawning Docker', () => {
    expect(parseCommunityCommand(['up'])).toBe('up');
    expect(parseCommunityCommand(['stop'])).toBe('stop');
    expect(parseCommunityCommand(['down'])).toBe('down');
    expect(parseCommunityCommand(['config'])).toBe('config');
    expect(() => parseCommunityCommand([])).toThrow('<up|stop|down|config>');
    expect(() => parseCommunityCommand(['up', '--profile', 'full'])).toThrow(
      '<up|stop|down|config>',
    );
  });
});
