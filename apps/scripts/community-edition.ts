#!/usr/bin/env bun

import { resolve } from 'node:path';
import {
  COMMUNITY_REQUIRED_ENVIRONMENT_VARIABLES,
  assertSecureEnvironment,
} from '../api_backend/lib/environmentPolicy';
import { getLocalDockerEnv } from './docker-local-env';
import { requireRelayCertificates } from './relay-certificates';

export const COMMUNITY_CORE_SERVICES = [
  'postgres',
  'api_backend',
  'frontend',
  'talos_relay',
  'talos_server',
] as const;

export const COMMUNITY_PROJECT_NAME = 'talos-community';

export const COMMUNITY_DATABASE_WAIT_TIMEOUT_SECONDS = 60;

export const COMMUNITY_CORE_WAIT_TIMEOUT_SECONDS = 120;

export const COMMUNITY_PRISMA_SCHEMA_PATH = './prisma/schema.prisma';

export const COMMUNITY_DISABLED_INTEGRATIONS = {
  TALOS_COMPOSE_TELEMETRY_PRODUCER_URL: '',
  TALOS_COMPOSE_AI_RUNNER_URL: '',
} as const;

export type CommunityCommand = 'up' | 'stop' | 'down' | 'config';

export type CommunityComposePlan = {
  phase: string;
  argv: string[];
  cwd: string;
  environment: Record<string, string>;
};

export type CommunityPlanExecutor = (plan: CommunityComposePlan) => Promise<number>;

const appsDir = resolve(import.meta.dir, '..');
const defaultRepoRoot = resolve(appsDir, '..');

export function parseCommunityCommand(args: readonly string[]): CommunityCommand {
  const [command, ...extra] = args;
  if (
    extra.length > 0 ||
    (command !== 'up' && command !== 'stop' && command !== 'down' && command !== 'config')
  ) {
    throw new Error('Usage: bun ./scripts/community-edition.ts <up|stop|down|config>');
  }
  return command;
}

export function communityEnvironment(
  baseEnvironment: Readonly<Record<string, string>>,
): Record<string, string> {
  return {
    ...baseEnvironment,
    ...COMMUNITY_DISABLED_INTEGRATIONS,
  };
}

export function assertCommunityEnvironment(
  environment: Readonly<Record<string, string | undefined>>,
): void {
  assertSecureEnvironment(
    environment,
    COMMUNITY_REQUIRED_ENVIRONMENT_VARIABLES,
    'Community Edition',
  );
}

export function createCommunityComposePlans(
  command: CommunityCommand,
  baseEnvironment: Readonly<Record<string, string>>,
  repoRoot: string = defaultRepoRoot,
): CommunityComposePlan[] {
  const cwd = resolve(repoRoot);
  const composePath = resolve(cwd, 'infra', 'docker-compose.dev.yml');
  const envFilePath = resolve(cwd, 'apps', '.env');
  const composeArgv = [
    'docker',
    'compose',
    '--project-name',
    COMMUNITY_PROJECT_NAME,
    ...(command === 'up' || command === 'config' ? ['--env-file', envFilePath] : []),
    '-f',
    composePath,
  ];
  const environment = communityEnvironment(baseEnvironment);

  if (command === 'up') {
    return [
      {
        phase: 'wait for PostgreSQL',
        argv: [
          ...composeArgv,
          'up',
          '--detach',
          '--wait',
          '--wait-timeout',
          String(COMMUNITY_DATABASE_WAIT_TIMEOUT_SECONDS),
          'postgres',
        ],
        cwd,
        environment,
      },
      {
        phase: 'apply database migrations',
        argv: [
          ...composeArgv,
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
        ],
        cwd,
        environment,
      },
      {
        phase: 'start core services',
        argv: [
          ...composeArgv,
          'up',
          '--detach',
          '--build',
          '--wait',
          '--wait-timeout',
          String(COMMUNITY_CORE_WAIT_TIMEOUT_SECONDS),
          ...COMMUNITY_CORE_SERVICES,
        ],
        cwd,
        environment,
      },
    ];
  }

  const argv = [...composeArgv];
  if (command === 'stop') {
    argv.push('stop', ...COMMUNITY_CORE_SERVICES);
  } else if (command === 'down') {
    argv.push('down');
  } else {
    argv.push('config', '--quiet');
  }

  return [{ phase: command, argv, cwd, environment }];
}

export async function executeCommunityComposePlans(
  plans: readonly CommunityComposePlan[],
  execute: CommunityPlanExecutor,
): Promise<number> {
  for (const plan of plans) {
    const exitCode = await execute(plan);
    if (exitCode !== 0) {
      return exitCode;
    }
  }
  return 0;
}

export async function runCommunityEdition(command: CommunityCommand): Promise<number> {
  const environment = await getLocalDockerEnv(
    command === 'up' || command === 'config' ? resolve(defaultRepoRoot, 'apps', '.env') : undefined,
  );
  if (command === 'up' || command === 'config') {
    assertCommunityEnvironment(environment);
  }
  if (command === 'up') {
    await requireRelayCertificates(environment, defaultRepoRoot);
  }
  const plans = createCommunityComposePlans(command, environment);
  if (command === 'up') {
    console.log(`Starting Talos Community Edition: ${COMMUNITY_CORE_SERVICES.join(', ')}`);
  }

  return await executeCommunityComposePlans(plans, async (plan) => {
    console.log(`Community Edition: ${plan.phase}...`);
    const child = Bun.spawn(plan.argv, {
      cwd: plan.cwd,
      env: plan.environment,
      stdin: 'inherit',
      stdout: 'inherit',
      stderr: 'inherit',
    });
    const exitCode = await child.exited;
    if (exitCode !== 0) {
      console.error(`Community Edition failed during: ${plan.phase}`);
    }
    return exitCode;
  });
}

if (import.meta.main) {
  try {
    const command = parseCommunityCommand(process.argv.slice(2));
    process.exit(await runCommunityEdition(command));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
