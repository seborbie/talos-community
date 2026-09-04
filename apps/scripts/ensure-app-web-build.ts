/**
 * Builds the API and frontend images only when their build inputs have changed.
 * Writes per-service hashes to .app-web-build-hashes.json and skips docker compose build
 * for services whose inputs are unchanged.
 */
import { $ } from 'bun';
import { createHash } from 'crypto';
import { readdirSync, statSync } from 'fs';
import { join, relative, resolve } from 'path';
import { dockerImageExists } from './docker-image-exists';
import { getLocalDockerEnv } from './docker-local-env';

const appsDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(appsDir, '..');
const composePath = resolve(repoRoot, 'infra', 'docker-compose.dev.yml');
const hashFilePath = resolve(appsDir, '.app-web-build-hashes.json');

const GENERATED_DIRS = new Set([
  'node_modules',
  'build',
  'dist',
  '.svelte-kit',
  '.vite',
  'coverage',
]);
const APP_WEB_SERVICES = ['api_backend', 'frontend'] as const;

const APP_WEB_IMAGE: Record<(typeof APP_WEB_SERVICES)[number], string> = {
  api_backend: 'talos/api-backend:dev',
  frontend: 'talos/frontend:dev',
};

type AppWebService = (typeof APP_WEB_SERVICES)[number];
type StoredHashes = Partial<Record<AppWebService, string>>;

function collectFiles(dir: string): string[] {
  const out: string[] = [];
  try {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      const st = statSync(full);
      if (st.isDirectory()) {
        if (!GENERATED_DIRS.has(name)) {
          out.push(...collectFiles(full));
        }
      } else {
        out.push(full);
      }
    }
  } catch {
    // ignore missing dirs
  }
  return out;
}

async function hashPaths(paths: string[], extraParts: string[] = []): Promise<string> {
  const parts = [...extraParts];
  for (const fullPath of paths.sort()) {
    const relPath = relative(repoRoot, fullPath);
    try {
      parts.push(relPath, await Bun.file(fullPath).text());
    } catch {
      parts.push(relPath, '');
    }
  }

  return createHash('sha256').update(parts.join('\0'), 'utf8').digest('hex');
}

async function computeApiBackendHash(): Promise<string> {
  const apiDir = join(appsDir, 'api_backend');
  const files = [
    join(appsDir, 'package.json'),
    join(appsDir, 'bun.lock'),
    join(appsDir, 'bunfig.toml'),
    join(appsDir, 'scripts', 'check-toolchain.ts'),
    join(apiDir, 'Dockerfile'),
    join(apiDir, 'package.json'),
    join(apiDir, 'server.ts'),
    ...collectFiles(join(apiDir, 'prisma')),
    ...collectFiles(join(apiDir, 'lib')),
    ...collectFiles(join(apiDir, 'routes')),
    ...collectFiles(join(apiDir, 'middleware')),
  ];
  return hashPaths(files);
}

export function frontendBuildArgumentHashParts(
  environment: Record<string, string | undefined> = process.env,
): string[] {
  const publicApiUrl = environment.PUBLIC_API_URL ?? 'http://localhost:3001';
  const publicRmmApiUrl = environment.PUBLIC_RMM_API_URL ?? 'http://localhost:3002';
  return [`PUBLIC_API_URL=${publicApiUrl}`, `PUBLIC_RMM_API_URL=${publicRmmApiUrl}`];
}

async function computeFrontendHash(): Promise<string> {
  const frontendDir = join(appsDir, 'frontend');
  const files = [
    join(appsDir, 'package.json'),
    join(appsDir, 'bun.lock'),
    join(appsDir, 'bunfig.toml'),
    join(appsDir, 'scripts', 'check-toolchain.ts'),
    join(frontendDir, 'Dockerfile'),
    ...collectFiles(frontendDir),
  ];
  return hashPaths(files, frontendBuildArgumentHashParts());
}

async function loadStoredHashes(): Promise<StoredHashes> {
  try {
    const raw = await Bun.file(hashFilePath).text();
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object') {
      return parsed as StoredHashes;
    }
  } catch {
    // no hash file or invalid json
  }

  return {};
}

async function storeHashes(hashes: StoredHashes): Promise<void> {
  await Bun.write(hashFilePath, `${JSON.stringify(hashes, null, 2)}\n`);
}

export async function ensureAppWebBuild(): Promise<AppWebService[]> {
  const nextHashes: Record<AppWebService, string> = {
    api_backend: await computeApiBackendHash(),
    frontend: await computeFrontendHash(),
  };
  const previousHashes = await loadStoredHashes();
  let changedServices = APP_WEB_SERVICES.filter(
    (service) => previousHashes[service] !== nextHashes[service],
  );

  if (changedServices.length === 0) {
    const missingImages: AppWebService[] = [];
    for (const service of APP_WEB_SERVICES) {
      if (!(await dockerImageExists(APP_WEB_IMAGE[service]))) {
        missingImages.push(service);
      }
    }
    if (missingImages.length === 0) {
      console.log('App web: image build skipped (no code changes).');
      return [];
    }
    console.log('App web: local image(s) missing, building %s...', missingImages.join(', '));
    changedServices = missingImages;
  }

  console.log('App web: building images for %s...', changedServices.join(', '));
  const dockerEnv = await getLocalDockerEnv();
  const proc = await $`docker compose -f ${composePath} build ${changedServices}`
    .env(dockerEnv)
    .cwd(repoRoot)
    .nothrow();
  if (proc.exitCode !== 0) {
    throw new Error(`docker compose build failed with exit code ${proc.exitCode ?? 1}`);
  }

  await storeHashes({ ...previousHashes, ...nextHashes });
  console.log('App web: build done.');
  return [...changedServices];
}

if (import.meta.main) {
  try {
    await ensureAppWebBuild();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
