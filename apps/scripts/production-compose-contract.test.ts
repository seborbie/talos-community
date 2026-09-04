import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  PRODUCTION_BASE_COMPOSE_PATH,
  PRODUCTION_ONE_SHOT_SERVICES,
  PRODUCTION_PERSISTENT_SERVICES,
  PRODUCTION_POSTGRES_COMPOSE_PATH,
  checkProductionComposeContract,
  productionComposeContractFailures,
  productionComposePaths,
} from './production-compose-contract';

const repoRoot = resolve(import.meta.dir, '..', '..');
const baseSource = readFileSync(resolve(repoRoot, PRODUCTION_BASE_COMPOSE_PATH), 'utf8');
const postgresSource = readFileSync(resolve(repoRoot, PRODUCTION_POSTGRES_COMPOSE_PATH), 'utf8');

describe('Production Community Compose contract', () => {
  test('the tracked topology satisfies the production contract', async () => {
    expect((await checkProductionComposeContract(repoRoot)).failures).toEqual([]);
    expect(PRODUCTION_PERSISTENT_SERVICES).toEqual([
      'api_backend',
      'frontend',
      'talos_relay',
      'talos_server',
    ]);
    expect(PRODUCTION_ONE_SHOT_SERVICES).toEqual(['database_preflight', 'database_migrate']);
  });

  test('selects bundled or external PostgreSQL without editing YAML', () => {
    expect(productionComposePaths('bundled', repoRoot)).toEqual([
      resolve(repoRoot, PRODUCTION_BASE_COMPOSE_PATH),
      resolve(repoRoot, PRODUCTION_POSTGRES_COMPOSE_PATH),
    ]);
    expect(productionComposePaths('external', repoRoot)).toEqual([
      resolve(repoRoot, PRODUCTION_BASE_COMPOSE_PATH),
    ]);
  });

  test('detects accidental host publication', () => {
    const unsafeBase = baseSource.replace(
      '    expose:\n      - "3001"',
      '    ports:\n      - "0.0.0.0:3001:3001"\n    expose:\n      - "3001"',
    );

    expect(productionComposeContractFailures(unsafeBase, postgresSource)).toContain(
      'api_backend must not publish a host port',
    );
  });

  test('detects a migration ordering regression', () => {
    const unsafeBase = baseSource.replace(
      'condition: service_completed_successfully',
      'condition: service_started',
    );

    expect(productionComposeContractFailures(unsafeBase, postgresSource)).toContain(
      'database_migrate must depend on database_preflight with condition service_completed_successfully',
    );
  });

  test('detects secret expansion into the relay boundary', () => {
    const unsafeBase = baseSource.replace(
      '      RUST_LOG: "${TALOS_RUST_LOG:-info}"\n      RMM_RELAY_BIND_ADDR:',
      '      RUST_LOG: "${TALOS_RUST_LOG:-info}"\n      JWT_SECRET: "${TALOS_JWT_SECRET}"\n      RMM_RELAY_BIND_ADDR:',
    );

    expect(productionComposeContractFailures(unsafeBase, postgresSource)).toContain(
      'JWT_SECRET must be provided only to: api_backend',
    );
  });
});
