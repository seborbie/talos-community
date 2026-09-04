/**
 * Ensures the database is reachable and migrations are applied (idempotent).
 * Run from apps/ so DATABASE_URL is in .env. Retries a few times so Postgres
 * from infra has time to become ready after `infra:up`.
 */
import { access } from 'fs/promises';
import { resolve } from 'path';

const schemaPath = resolve(import.meta.dir, '..', 'api_backend', 'prisma', 'schema.prisma');
const appsDir = resolve(import.meta.dir, '..');
const bunExe = process.execPath;
const maxAttempts = 15;
const delayMs = 2000;

async function pathExists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function ensurePrismaDependency() {
  const prismaConfigModule = resolve(appsDir, 'node_modules', 'prisma', 'config.js');
  if (await pathExists(prismaConfigModule)) {
    return;
  }

  console.log('Installing apps dependencies for Prisma CLI...');
  const proc = Bun.spawn([bunExe, 'install', '--frozen-lockfile'], {
    cwd: appsDir,
    env: process.env,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  const exitCode = await proc.exited;
  if (exitCode !== 0) {
    process.exit(exitCode);
  }
}

async function attemptMigrate(): Promise<{ ok: boolean; stderr: string }> {
  const proc = Bun.spawn(
    [bunExe, 'x', '--bun', 'prisma', 'migrate', 'deploy', '--schema', schemaPath],
    {
      cwd: appsDir,
      env: process.env,
      stdout: 'pipe',
      stderr: 'pipe',
    },
  );
  const stderr = await new Response(proc.stderr).text();
  return { ok: (await proc.exited) === 0, stderr };
}

async function main() {
  // Load apps/.env so DATABASE_URL is set for the spawned prisma
  const envPath = resolve(import.meta.dir, '..', '.env');
  const env = await Bun.file(envPath)
    .text()
    .catch(() => '');
  for (const line of env.split('\n')) {
    const trimmed = line.replace(/\r$/, '').trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq > 0) {
      const key = trimmed.slice(0, eq).trim();
      let value = trimmed
        .slice(eq + 1)
        .trim()
        .replace(/\r$/, '');
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      )
        value = value.slice(1, -1);
      if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) process.env[key] = value;
    }
  }

  await ensurePrismaDependency();

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    const { ok, stderr } = await attemptMigrate();
    if (ok) {
      console.log('Database ready (migrations applied).');
      process.exit(0);
    }
    if (attempt < maxAttempts) {
      console.warn(
        `Database not ready (attempt ${attempt}/${maxAttempts}), retrying in ${delayMs / 1000}s...`,
      );
      if (stderr) console.warn(stderr.trim());
      await new Promise((r) => setTimeout(r, delayMs));
    } else {
      console.error('Prisma migrate deploy failed:');
      console.error(stderr || '(no stderr)');
    }
  }

  console.error(
    'Failed to apply migrations after %d attempts. Is Postgres running (e.g. bun run infra:up --cwd apps)?',
    maxAttempts,
  );
  process.exit(1);
}

main();
