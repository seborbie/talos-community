#!/usr/bin/env bun

import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

export type ProductionDatabaseMode = 'bundled' | 'external';

export const PRODUCTION_BASE_COMPOSE_PATH = 'infra/compose.community.yml';
export const PRODUCTION_POSTGRES_COMPOSE_PATH = 'infra/compose.community-postgres.yml';

export const PRODUCTION_PERSISTENT_SERVICES = [
  'api_backend',
  'frontend',
  'talos_relay',
  'talos_server',
] as const;

export const PRODUCTION_ONE_SHOT_SERVICES = ['database_preflight', 'database_migrate'] as const;

const PRODUCTION_BASE_SERVICES = [
  ...PRODUCTION_PERSISTENT_SERVICES,
  ...PRODUCTION_ONE_SHOT_SERVICES,
].sort();

const PRODUCTION_POSTGRES_OVERLAY_SERVICES = [
  'api_backend',
  'database_migrate',
  'database_preflight',
  'postgres',
].sort();

type ComposeService = {
  image?: unknown;
  build?: unknown;
  command?: unknown;
  environment?: unknown;
  networks?: unknown;
  ports?: unknown;
  expose?: unknown;
  depends_on?: unknown;
  restart?: unknown;
  volumes?: unknown;
};

type ComposeDocument = {
  services?: Record<string, ComposeService>;
  networks?: Record<string, Record<string, unknown> | null>;
  volumes?: Record<string, unknown>;
};

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === 'string');
}

function parseCompose(source: string, label: string, failures: string[]): ComposeDocument {
  try {
    const parsed = Bun.YAML.parse(source);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      failures.push(`${label} must contain one Compose mapping`);
      return {};
    }
    return parsed as ComposeDocument;
  } catch {
    failures.push(`${label} must be valid YAML`);
    return {};
  }
}

function exactKeys(
  actual: Record<string, unknown> | undefined,
  expected: readonly string[],
  label: string,
  failures: string[],
): void {
  const keys = Object.keys(actual ?? {}).sort();
  if (JSON.stringify(keys) !== JSON.stringify([...expected].sort())) {
    failures.push(`${label} must contain exactly: ${[...expected].sort().join(', ')}`);
  }
}

function requireCondition(
  service: ComposeService | undefined,
  dependency: string,
  condition: string,
  label: string,
  failures: string[],
): void {
  const dependencies = asRecord(service?.depends_on);
  const configured = asRecord(dependencies[dependency]).condition;
  if (configured !== condition) {
    failures.push(`${label} must depend on ${dependency} with condition ${condition}`);
  }
}

function environment(service: ComposeService | undefined): Record<string, unknown> {
  return asRecord(service?.environment);
}

function networks(service: ComposeService | undefined): string[] {
  if (Array.isArray(service?.networks)) return asStringArray(service.networks);
  return Object.keys(asRecord(service?.networks));
}

function requireOnlyEnvironmentOwners(
  services: Record<string, ComposeService>,
  key: string,
  expectedOwners: readonly string[],
  failures: string[],
): void {
  const owners = Object.entries(services)
    .filter(([, service]) => Object.hasOwn(environment(service), key))
    .map(([name]) => name)
    .sort();
  if (JSON.stringify(owners) !== JSON.stringify([...expectedOwners].sort())) {
    failures.push(`${key} must be provided only to: ${[...expectedOwners].sort().join(', ')}`);
  }
}

export function productionComposePaths(mode: ProductionDatabaseMode, repoRoot: string): string[] {
  const base = resolve(repoRoot, PRODUCTION_BASE_COMPOSE_PATH);
  return mode === 'bundled' ? [base, resolve(repoRoot, PRODUCTION_POSTGRES_COMPOSE_PATH)] : [base];
}

export function productionComposeContractFailures(
  baseSource: string,
  postgresSource: string,
): string[] {
  const failures: string[] = [];
  const base = parseCompose(baseSource, PRODUCTION_BASE_COMPOSE_PATH, failures);
  const postgresOverlay = parseCompose(postgresSource, PRODUCTION_POSTGRES_COMPOSE_PATH, failures);
  const baseServices = base.services ?? {};
  const overlayServices = postgresOverlay.services ?? {};

  exactKeys(baseServices, PRODUCTION_BASE_SERVICES, 'production base service set', failures);
  exactKeys(
    overlayServices,
    PRODUCTION_POSTGRES_OVERLAY_SERVICES,
    'PostgreSQL overlay service set',
    failures,
  );

  for (const [name, service] of [
    ...Object.entries(baseServices),
    ...Object.entries(overlayServices),
  ]) {
    if (Object.hasOwn(service, 'ports')) {
      failures.push(`${name} must not publish a host port`);
    }
    if (Object.hasOwn(service, 'build')) {
      failures.push(`${name} must use a released image rather than a source build`);
    }
  }

  const expectedImages: Record<string, string> = {
    database_preflight: '${TALOS_API_BACKEND_IMAGE:?',
    database_migrate: '${TALOS_API_BACKEND_IMAGE:?',
    api_backend: '${TALOS_API_BACKEND_IMAGE:?',
    frontend: '${TALOS_FRONTEND_IMAGE:?',
    talos_relay: '${TALOS_RELAY_IMAGE:?',
    talos_server: '${TALOS_SERVER_IMAGE:?',
  };
  for (const [name, prefix] of Object.entries(expectedImages)) {
    const image = baseServices[name]?.image;
    if (typeof image !== 'string' || !image.startsWith(prefix)) {
      failures.push(`${name} must require its released image reference`);
    }
  }

  const expectedExposedPorts: Record<string, string[]> = {
    api_backend: ['3001'],
    frontend: ['3000'],
    talos_relay: ['443'],
    talos_server: ['17110'],
  };
  for (const [name, expected] of Object.entries(expectedExposedPorts)) {
    if (JSON.stringify(asStringArray(baseServices[name]?.expose)) !== JSON.stringify(expected)) {
      failures.push(`${name} must expose only container port ${expected.join(', ')}`);
    }
  }
  for (const name of PRODUCTION_ONE_SHOT_SERVICES) {
    if (Object.hasOwn(baseServices[name] ?? {}, 'expose')) {
      failures.push(`${name} must not expose a port`);
    }
  }
  if (JSON.stringify(asStringArray(overlayServices.postgres?.expose)) !== '["5432"]') {
    failures.push('postgres must expose only its private container port 5432');
  }

  if (!base.networks?.talos_edge) failures.push('base must define the talos_edge network');
  if (base.networks?.talos_data?.internal !== true) {
    failures.push('base must define talos_data as an internal network');
  }
  for (const name of PRODUCTION_PERSISTENT_SERVICES) {
    if (!networks(baseServices[name]).includes('talos_edge')) {
      failures.push(`${name} must join talos_edge for the edge overlay`);
    }
  }
  if (JSON.stringify(networks(overlayServices.postgres)) !== '["talos_data"]') {
    failures.push('postgres must join only talos_data');
  }

  requireCondition(
    baseServices.database_migrate,
    'database_preflight',
    'service_completed_successfully',
    'database_migrate',
    failures,
  );
  requireCondition(
    baseServices.api_backend,
    'database_migrate',
    'service_completed_successfully',
    'api_backend',
    failures,
  );
  requireCondition(
    overlayServices.database_preflight,
    'postgres',
    'service_healthy',
    'database_preflight',
    failures,
  );
  if (baseServices.database_preflight?.restart !== 'on-failure:5') {
    failures.push('database_preflight must use a bounded on-failure:5 retry policy');
  }
  if (baseServices.database_migrate?.restart !== 'no') {
    failures.push('database_migrate must remain a one-shot job');
  }
  const migrationCommand = asStringArray(baseServices.database_migrate?.command).join(' ');
  if (!migrationCommand.includes('prisma migrate deploy')) {
    failures.push('database_migrate must run prisma migrate deploy');
  }

  if (environment(baseServices.database_preflight).TALOS_DATABASE_MODE !== 'external') {
    failures.push('base database preflight must select external mode');
  }
  if (environment(overlayServices.database_preflight).TALOS_DATABASE_MODE !== 'bundled') {
    failures.push('PostgreSQL overlay must select bundled mode');
  }
  if (environment(baseServices.api_backend).DATABASE_URL !== '${TALOS_DATABASE_URL-}') {
    failures.push('external mode must use only TALOS_DATABASE_URL');
  }

  requireOnlyEnvironmentOwners(
    baseServices,
    'DATABASE_URL',
    ['api_backend', 'database_migrate', 'database_preflight'],
    failures,
  );
  requireOnlyEnvironmentOwners(baseServices, 'JWT_SECRET', ['api_backend'], failures);
  requireOnlyEnvironmentOwners(baseServices, 'APP_ENCRYPTION_KEY', ['api_backend'], failures);
  requireOnlyEnvironmentOwners(
    baseServices,
    'RMM_SERVER_API_KEY',
    ['api_backend', 'talos_server'],
    failures,
  );

  for (const [name, service] of Object.entries(baseServices)) {
    if (Object.hasOwn(service, 'env_file')) {
      failures.push(`${name} must use an explicit production environment allowlist`);
    }
    if (Object.hasOwn(service, 'volumes')) {
      failures.push(`${name} must not receive a host filesystem mount in the base topology`);
    }
  }
  const relayEnvironment = environment(baseServices.talos_relay);
  if (
    relayEnvironment.RMM_RELAY_TLS_TERMINATED !== 'true' ||
    Object.keys(relayEnvironment).some((key) => /TLS_(?:CERT|KEY)_PATH/.test(key))
  ) {
    failures.push('talos_relay must rely on edge TLS termination without certificate mounts');
  }

  const postgresImage = overlayServices.postgres?.image;
  if (typeof postgresImage !== 'string' || !postgresImage.includes('postgres:16-alpine@sha256:')) {
    failures.push('bundled PostgreSQL must default to the reviewed PostgreSQL 16 digest');
  }
  const postgresPassword = environment(overlayServices.postgres).POSTGRES_PASSWORD;
  if (typeof postgresPassword !== 'string' || !postgresPassword.includes(':?')) {
    failures.push('bundled PostgreSQL must require an installation-specific password');
  }
  if (!Object.hasOwn(postgresOverlay.volumes ?? {}, 'talos_postgres_data')) {
    failures.push('bundled PostgreSQL must own the talos_postgres_data volume');
  }

  if (/replace[_-]with|your[_-](?:secret|password|key)/i.test(baseSource + postgresSource)) {
    failures.push('production Compose files must not contain example credential values');
  }

  return failures;
}

export async function checkProductionComposeContract(
  repoRoot = resolve(import.meta.dir, '..', '..'),
): Promise<{ failures: string[] }> {
  const [baseSource, postgresSource] = await Promise.all([
    readFile(resolve(repoRoot, PRODUCTION_BASE_COMPOSE_PATH), 'utf8'),
    readFile(resolve(repoRoot, PRODUCTION_POSTGRES_COMPOSE_PATH), 'utf8'),
  ]);
  return { failures: productionComposeContractFailures(baseSource, postgresSource) };
}

if (import.meta.main) {
  const { failures } = await checkProductionComposeContract();
  if (failures.length > 0) {
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log('Production Community Compose contract is valid.');
}
