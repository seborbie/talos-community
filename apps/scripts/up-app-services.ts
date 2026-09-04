import { $ } from 'bun';
import { resolve } from 'path';
import { getLocalDockerEnv } from './docker-local-env';
import { ensureAppWebBuild } from './ensure-app-web-build';
import { ensureRelayUrl } from './ensure-relay-url';
import { ensureRustServersBuild } from './ensure-rust-servers-build';

const appsDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(appsDir, '..');
const composePath = resolve(repoRoot, 'infra', 'docker-compose.dev.yml');
const rustServices = [
  'talos_server',
  'talos_relay',
  'talos_telemetry_consumer',
  'talos_telemetry_producer',
  'talos_ai_runner',
] as const;

async function main() {
  await ensureRelayUrl();
  const rebuiltAppWebServices = await ensureAppWebBuild();
  const rebuiltRust = await ensureRustServersBuild();
  const changedServices = [...rebuiltAppWebServices, ...(rebuiltRust ? [...rustServices] : [])];

  if (changedServices.length > 0) {
    const dockerEnv = await getLocalDockerEnv();
    const recreate =
      await $`docker compose -f ${composePath} up --detach --no-build --no-deps --force-recreate ${changedServices}`
        .env(dockerEnv)
        .cwd(repoRoot)
        .nothrow();
    if (recreate.exitCode !== 0) {
      process.exit(recreate.exitCode ?? 1);
    }
  }

  console.log('App services: prepare complete.');
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
