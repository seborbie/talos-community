import { mkdir } from 'fs/promises';
import { join } from 'path';

const dockerConfigDir = join(process.env.USERPROFILE ?? process.env.HOME ?? '.', '.docker-empty');
const dockerConfigPath = join(dockerConfigDir, 'config.json');
const windowsDockerDesktopHost = 'npipe:////./pipe/dockerDesktopLinuxEngine';

type DockerProbeProcess = {
  exited: Promise<number>;
  kill(): void;
};

export type DockerProbeSpawner = (argv: string[]) => DockerProbeProcess;

export const DOCKER_ENGINE_PROBE_TIMEOUT_MS = 10_000;

async function ensureDockerConfig() {
  await mkdir(dockerConfigDir, { recursive: true });
  const config = {
    auths: {
      'https://index.docker.io/v1/': {},
    },
    credsStore: '',
  };
  await Bun.write(dockerConfigPath, `${JSON.stringify(config)}\n`);
}

export function parseEnvironmentFile(contents: string): Record<string, string> {
  const parsed: Record<string, string> = {};
  for (const rawLine of contents.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('export ')) line = line.slice('export '.length).trimStart();

    const separatorIndex = line.indexOf('=');
    if (separatorIndex <= 0) continue;
    const key = line.slice(0, separatorIndex).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;

    let value = line.slice(separatorIndex + 1).trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    } else {
      value = value.replace(/\s+#.*$/, '').trimEnd();
    }
    parsed[key] = value;
  }
  return parsed;
}

export function getLocalDockerHost(
  env: Record<string, string | undefined> = process.env,
  platform: string = process.platform,
): string | undefined {
  if (env.DOCKER_HOST) {
    return env.DOCKER_HOST;
  }

  if (platform === 'win32') {
    return windowsDockerDesktopHost;
  }

  return undefined;
}

export async function assertLocalDockerEngineResponsive(
  timeoutMs: number = DOCKER_ENGINE_PROBE_TIMEOUT_MS,
  spawnProcess?: DockerProbeSpawner,
): Promise<void> {
  let proc: DockerProbeProcess;
  try {
    proc = spawnProcess
      ? spawnProcess(['docker', 'info', '--format', '{{.ServerVersion}}'])
      : Bun.spawn(['docker', 'info', '--format', '{{.ServerVersion}}'], {
          env: await getLocalDockerEnv(),
          stdin: 'ignore',
          stdout: 'ignore',
          stderr: 'ignore',
        });
  } catch {
    throw new Error(
      'Docker CLI could not be started. Install or start Docker Desktop before running bun run debug.',
    );
  }

  let timeout: ReturnType<typeof setTimeout> | undefined;
  const outcome = await Promise.race([
    proc.exited.then((exitCode) => ({ kind: 'exit' as const, exitCode })),
    new Promise<{ kind: 'timeout' }>((resolveTimeout) => {
      timeout = setTimeout(() => resolveTimeout({ kind: 'timeout' }), timeoutMs);
    }),
  ]);
  if (timeout) clearTimeout(timeout);

  if (outcome.kind === 'timeout') {
    try {
      proc.kill();
    } catch {
      // The Docker CLI may have exited while the timeout was being handled.
    }
    throw new Error(
      `Docker engine did not respond within ${Math.ceil(timeoutMs / 1_000)} seconds. ` +
        'Restart Docker Desktop, wait until its engine is ready, then rerun bun run debug.',
    );
  }

  if (outcome.exitCode !== 0) {
    throw new Error(
      `Docker engine is unavailable (docker info exited with code ${outcome.exitCode}). ` +
        'Start or restart Docker Desktop, wait until its engine is ready, then rerun bun run debug.',
    );
  }
}

export async function getLocalDockerEnv(envFilePath?: string): Promise<Record<string, string>> {
  let fileEnvironment: Record<string, string> = {};
  if (envFilePath) {
    const envFile = Bun.file(envFilePath);
    if (!(await envFile.exists())) {
      throw new Error(
        `Required environment file is missing: ${envFilePath}\nCopy apps/.env.example to apps/.env and replace the required placeholders.`,
      );
    }
    fileEnvironment = parseEnvironmentFile(await envFile.text());
  }
  const env = {
    ...fileEnvironment,
    ...process.env,
  } as Record<string, string>;

  if (process.env.DOCKER_CONFIG) {
    env.DOCKER_CONFIG = process.env.DOCKER_CONFIG;
  } else if (process.platform === 'win32') {
    await ensureDockerConfig();
    env.DOCKER_CONFIG = dockerConfigDir;
  }

  const dockerHost = getLocalDockerHost();
  if (dockerHost) {
    env.DOCKER_HOST = dockerHost;
  }

  return env;
}
