import { resolve } from 'path';
import {
  COMMUNITY_REQUIRED_ENVIRONMENT_VARIABLES,
  assertSecureEnvironment,
} from '../api_backend/lib/environmentPolicy';
import { getLocalDockerEnv } from './docker-local-env';
import { requireRelayCertificates } from './relay-certificates';

const appsDir = resolve(import.meta.dir, '..');
const bunExe = process.execPath;
const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'] as const;
const devScripts = ['telemetry:up', 'db:ensure', 'app-services:up'] as const;

type BunProcess = ReturnType<typeof Bun.spawn>;

let activeProcess: BunProcess | null = null;
let forcedExit = false;

function signalExitCode(signal: NodeJS.Signals): number {
  const signalNumbers: Partial<Record<NodeJS.Signals, number>> = {
    SIGHUP: 1,
    SIGINT: 2,
    SIGTERM: 15,
  };
  return 128 + (signalNumbers[signal] ?? 0);
}

for (const signal of signals) {
  process.once(signal, () => {
    if (forcedExit) {
      activeProcess?.kill('SIGKILL');
      process.exit(signalExitCode(signal));
    }
    forcedExit = true;
    activeProcess?.kill(signal);
  });
}

async function runBunScript(script: string, environment: Record<string, string>): Promise<number> {
  const proc = Bun.spawn([bunExe, 'run', script], {
    cwd: appsDir,
    env: environment,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  activeProcess = proc;
  try {
    return await proc.exited;
  } finally {
    activeProcess = null;
  }
}

const devEnvironment = await getLocalDockerEnv(resolve(appsDir, '.env'));
assertSecureEnvironment(
  devEnvironment,
  COMMUNITY_REQUIRED_ENVIRONMENT_VARIABLES,
  'Development stack',
);
await requireRelayCertificates(devEnvironment, resolve(appsDir, '..'));

for (const script of devScripts) {
  const exitCode = await runBunScript(script, devEnvironment);
  if (exitCode !== 0) {
    process.exit(exitCode);
  }
}
