import { resolve } from 'node:path';

export type ProductionDatabaseMode = 'bundled' | 'external';

export type DatabasePreflightPlan = {
  argv: string[];
  cwd: string;
  stdin: string;
};

const ACCEPTED_EXTERNAL_SSL_MODES = new Set(['require', 'verify-ca', 'verify-full']);
const MIN_CONNECT_TIMEOUT_SECONDS = 1;
const MAX_CONNECT_TIMEOUT_SECONDS = 30;

function configurationError(message: string): Error {
  return new Error(`Production database configuration is invalid: ${message}`);
}

function parseMode(value: string | undefined): ProductionDatabaseMode {
  if (value === 'bundled' || value === 'external') return value;
  throw configurationError('TALOS_DATABASE_MODE must be bundled or external');
}

/**
 * Validate the one database URL shared by preflight, migrations, and the API. Error messages never
 * include the supplied URL because it can contain a password or client-certificate details.
 */
export function validateProductionDatabaseUrl(
  value: string | undefined,
  modeValue: string | undefined,
): URL {
  const mode = parseMode(modeValue);
  const normalized = value?.trim();
  if (!normalized) {
    throw configurationError('DATABASE_URL is required');
  }

  let parsed: URL;
  try {
    parsed = new URL(normalized);
  } catch {
    throw configurationError('DATABASE_URL must be a valid PostgreSQL URL');
  }

  if (parsed.protocol !== 'postgresql:' && parsed.protocol !== 'postgres:') {
    throw configurationError('DATABASE_URL must use the postgresql scheme');
  }
  if (!parsed.username || !parsed.hostname || parsed.pathname.length <= 1) {
    throw configurationError('DATABASE_URL must identify a user, host, and database');
  }

  const timeoutText = parsed.searchParams.get('connect_timeout');
  if (!timeoutText || !/^\d+$/.test(timeoutText)) {
    throw configurationError('DATABASE_URL must set an integer connect_timeout');
  }
  const timeoutSeconds = Number(timeoutText);
  if (
    timeoutSeconds < MIN_CONNECT_TIMEOUT_SECONDS ||
    timeoutSeconds > MAX_CONNECT_TIMEOUT_SECONDS
  ) {
    throw configurationError(
      `DATABASE_URL connect_timeout must be between ${MIN_CONNECT_TIMEOUT_SECONDS} and ${MAX_CONNECT_TIMEOUT_SECONDS} seconds`,
    );
  }

  const sslMode = parsed.searchParams.get('sslmode');
  if (mode === 'external' && (!sslMode || !ACCEPTED_EXTERNAL_SSL_MODES.has(sslMode))) {
    throw configurationError(
      'external DATABASE_URL must set sslmode=require, verify-ca, or verify-full',
    );
  }
  if (mode === 'bundled' && parsed.hostname !== 'postgres') {
    throw configurationError('bundled DATABASE_URL must use the private postgres service');
  }

  return parsed;
}

export function createDatabasePreflightPlan(
  environment: Readonly<Record<string, string | undefined>>,
  apiRoot = resolve(__dirname, '..'),
): DatabasePreflightPlan {
  validateProductionDatabaseUrl(environment.DATABASE_URL, environment.TALOS_DATABASE_MODE);
  return {
    argv: [
      'bun',
      'x',
      '--bun',
      'prisma',
      'db',
      'execute',
      '--stdin',
      '--schema',
      './prisma/schema.prisma',
    ],
    cwd: resolve(apiRoot),
    stdin: 'SELECT 1;\n',
  };
}

export async function runDatabasePreflight(
  environment: Readonly<Record<string, string | undefined>> = process.env,
): Promise<number> {
  const plan = createDatabasePreflightPlan(environment);
  const child = Bun.spawn(plan.argv, {
    cwd: plan.cwd,
    env: environment,
    stdin: 'pipe',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  child.stdin.write(plan.stdin);
  child.stdin.end();
  return await child.exited;
}

if (require.main === module) {
  void runDatabasePreflight()
    .then((exitCode) => {
      process.exitCode = exitCode;
    })
    .catch((error) => {
      console.error(
        error instanceof Error ? error.message : 'Production database preflight failed',
      );
      process.exitCode = 64;
    });
}
