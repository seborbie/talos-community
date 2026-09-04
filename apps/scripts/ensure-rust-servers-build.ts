/**
 * Builds the Rust server images only when their source code (or build config) has changed.
 * Writes a hash of relevant files to .rust-servers-build-hash; skips docker compose build
 * when the hash matches. Run from apps/ with compose -f ../infra/docker-compose.dev.yml.
 */
import { $ } from 'bun';
import { createHash } from 'crypto';
import { readdirSync, statSync } from 'fs';
import { join, resolve } from 'path';
import { dockerImageExists } from './docker-image-exists';
import { getLocalDockerEnv } from './docker-local-env';

const appsDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(appsDir, '..');
const hashFilePath = resolve(appsDir, '.rust-servers-build-hash');

const CRATES = [
  'talos_server',
  'talos_relay',
  'talos_telemetry_consumer',
  'talos_telemetry_producer',
  'talos_ai_runner',
  'talos_protocol',
] as const;

const RUST_SERVER_IMAGES = [
  'talos/talos-server:dev',
  'talos/talos-relay:dev',
  'talos/talos-telemetry-consumer:dev',
  'talos/talos-telemetry-producer:dev',
  'talos/talos-ai-runner:dev',
] as const;

async function anyRustServerImageMissing(): Promise<boolean> {
  for (const ref of RUST_SERVER_IMAGES) {
    if (!(await dockerImageExists(ref))) {
      return true;
    }
  }
  return false;
}

function collectFiles(dir: string): string[] {
  const out: string[] = [];
  try {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      const st = statSync(full);
      if (st.isDirectory()) {
        if (name !== 'target' && name !== 'node_modules' && !name.startsWith('.')) {
          out.push(...collectFiles(full));
        }
      } else if (name.endsWith('.rs') || name === 'Cargo.toml' || name === 'Cargo.lock') {
        out.push(full);
      }
    }
  } catch {
    // ignore missing dirs
  }
  return out;
}

async function computeHash(): Promise<string> {
  const parts: string[] = [];

  const rootCargo = join(appsDir, 'Cargo.toml');
  const rootLock = join(appsDir, 'Cargo.lock');
  for (const p of [rootCargo, rootLock]) {
    try {
      parts.push(p, await Bun.file(p).text());
    } catch {
      parts.push(p, '');
    }
  }

  const dockerfile = join(repoRoot, 'infra', 'Dockerfile.rust-servers');
  try {
    parts.push(dockerfile, await Bun.file(dockerfile).text());
  } catch {
    parts.push(dockerfile, '');
  }

  for (const crate of CRATES) {
    const crateDir = join(appsDir, crate);
    const files = collectFiles(crateDir).sort();
    for (const f of files) {
      try {
        parts.push(f, await Bun.file(f).text());
      } catch {
        parts.push(f, '');
      }
    }
  }

  const blob = parts.join('\0');
  return createHash('sha256').update(blob, 'utf8').digest('hex');
}

export async function ensureRustServersBuild(): Promise<boolean> {
  const profile = process.env.PROFILE ?? 'debug';
  const newHash = await computeHash();
  let needBuild = true;
  try {
    const stored = await Bun.file(hashFilePath).text();
    if (stored.trim() === `${profile}:${newHash}`) {
      needBuild = false;
    }
  } catch {
    // no hash file or unreadable
  }

  if (!needBuild) {
    if (!(await anyRustServerImageMissing())) {
      console.log('Rust servers: image build skipped (no code changes).');
      return false;
    }
    console.log('Rust servers: local image(s) missing, building...');
  }

  console.log('Rust servers: building images (profile=%s)...', profile);
  const composePath = resolve(repoRoot, 'infra', 'docker-compose.dev.yml');
  const dockerEnv = await getLocalDockerEnv();
  const proc =
    await $`docker compose -f ${composePath} build --build-arg PROFILE=${profile} talos_server talos_relay talos_telemetry_consumer talos_telemetry_producer talos_ai_runner`
      .env(dockerEnv)
      .cwd(repoRoot)
      .nothrow();
  if (proc.exitCode !== 0) {
    throw new Error(`docker compose build failed with exit code ${proc.exitCode ?? 1}`);
  }

  await Bun.write(hashFilePath, `${profile}:${newHash}`);
  console.log('Rust servers: build done.');
  return true;
}

if (import.meta.main) {
  try {
    await ensureRustServersBuild();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
