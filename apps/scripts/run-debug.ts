import { resolve } from 'path';
import { spawn, type ChildProcess } from 'child_process';
import { createServer } from 'net';
import { chmod, writeFile } from 'node:fs/promises';
import {
  assertNativeDebugEnvironment,
  mergeDebugEnvironment,
  resolveNativeDebugCredentials,
  resolveNativeDebugCorsAllowedOrigins,
  resolveDebugJwtSecret,
  resolveDebugRelayUrl,
  serializeNativeDebugSecrets,
  withNativeDebugProcessEnvironment,
  withNativeRelayCertificates,
} from './debug-environment';
import { DEV_RELAY_URL, ensureRelayUrl } from './ensure-relay-url';
import { requireRelayCertificates, resolveRelayCertificateFiles } from './relay-certificates';
import { assertLocalDockerEngineResponsive } from './docker-local-env';

const appsDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(appsDir, '..');
const bunExe = process.execPath;
const cargoExe = process.env.CARGO || 'cargo';
const apiBackendDir = resolve(appsDir, 'api_backend');
const frontendDir = resolve(appsDir, 'frontend');

function parseEnvFile(contents: string): Record<string, string> {
  const parsed: Record<string, string> = {};

  for (const rawLine of contents.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#')) {
      continue;
    }

    const separatorIndex = line.indexOf('=');
    if (separatorIndex <= 0) {
      continue;
    }

    const key = line.slice(0, separatorIndex).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      continue;
    }

    let value = line.slice(separatorIndex + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }

    parsed[key] = value;
  }

  return parsed;
}

async function loadEnvFile(path: string): Promise<Record<string, string>> {
  const contents = await Bun.file(path)
    .text()
    .catch(() => '');
  return parseEnvFile(contents);
}

const appsEnv = await loadEnvFile(resolve(appsDir, '.env'));
const apiBackendEnv = await loadEnvFile(resolve(apiBackendDir, '.env'));
const configuredDebugEnv = mergeDebugEnvironment(appsEnv, apiBackendEnv, process.env);
const debugSecretsPath = resolve(appsDir, '.env.debug.local');
const storedDebugSecrets = await loadEnvFile(debugSecretsPath);
const debugCredentialResolution = resolveNativeDebugCredentials(
  configuredDebugEnv,
  storedDebugSecrets,
);
const nativeDebugEnv = debugCredentialResolution.environment;
const serializedDebugSecrets = serializeNativeDebugSecrets(
  debugCredentialResolution.persistedSecrets,
);
if (serializedDebugSecrets) {
  const existingDebugSecrets = await Bun.file(debugSecretsPath)
    .text()
    .catch(() => '');
  if (existingDebugSecrets !== serializedDebugSecrets) {
    await writeFile(debugSecretsPath, serializedDebugSecrets, { encoding: 'utf8', mode: 0o600 });
    await chmod(debugSecretsPath, 0o600);
  }
}
if (debugCredentialResolution.repairedVariables.length > 0) {
  console.log(
    `Using stable local debug credentials for: ${debugCredentialResolution.repairedVariables.join(', ')}. ` +
      'Credential values were not logged.',
  );
}
const relayCertificateFiles = resolveRelayCertificateFiles(nativeDebugEnv, repoRoot);

function ensureRustLogDirective(filter: string, directive: string): string {
  const target = directive.split('=')[0]?.trim();
  if (!target) {
    return filter;
  }
  const hasDirective = filter
    .split(',')
    .map((part) => part.trim())
    .some((part) => part === target || part.startsWith(`${target}=`));
  return hasDirective ? filter : `${filter},${directive}`;
}

function debugRustLog(): string {
  const base = process.env.RUST_LOG ?? appsEnv.RUST_LOG ?? apiBackendEnv.RUST_LOG ?? 'info';
  return [
    'samsa=warn',
    'talos_ai_runner=debug',
    'talos_relay=debug',
    'talos_telemetry_consumer=debug',
    'talos_telemetry_producer=debug',
  ].reduce(ensureRustLogDirective, base);
}

const debugEnv = {
  ...withNativeDebugProcessEnvironment(
    withNativeRelayCertificates(nativeDebugEnv, relayCertificateFiles),
  ),
  RUST_LOG: debugRustLog(),
  JWT_SECRET: resolveDebugJwtSecret(nativeDebugEnv),
  TOKEN_TTL: process.env.TOKEN_TTL ?? appsEnv.TOKEN_TTL ?? apiBackendEnv.TOKEN_TTL ?? '1h',
  MACHINE_TOKEN_TTL:
    process.env.MACHINE_TOKEN_TTL ??
    appsEnv.MACHINE_TOKEN_TTL ??
    apiBackendEnv.MACHINE_TOKEN_TTL ??
    '30d',
  DATABASE_URL: 'postgresql://talos:talos@localhost:3004/talos',
  RMM_DATABASE_URL: 'postgresql://talos:talos@localhost:3004/talos',
  CORS_ALLOWED_ORIGINS: resolveNativeDebugCorsAllowedOrigins(nativeDebugEnv),
  API_BACKEND_URL: 'http://127.0.0.1:3001',
  INTERNAL_API_URL: 'http://127.0.0.1:3001',
  RMM_SERVER_HTTP_URL: 'http://127.0.0.1:3002',
  TALOS_AI_RUNNER_URL: 'http://127.0.0.1:3010',
  TALOS_AI_RUNNER_BIND_ADDR: nativeDebugEnv.TALOS_AI_RUNNER_BIND_ADDR?.trim() || '127.0.0.1:3010',
  TALOS_AI_RUNNER_CALLBACK_BASE_URL: 'http://127.0.0.1:3001',
  TALOS_AI_RUNNER_JOB_TIMEOUT_SECS: '420',
  TALOS_AI_RUNNER_SCREENSHOT_READ_TIMEOUT_SECS: '60',
  TALOS_AI_RUNNER_APPROVAL_TIMEOUT_SECS: '300',
  RMM_BIND_ADDR: nativeDebugEnv.RMM_BIND_ADDR?.trim() || '127.0.0.1:3002',
  RMM_TELEMETRY_PRODUCER_URL: 'http://127.0.0.1:3003',
  RMM_RELAY_URL: resolveDebugRelayUrl(nativeDebugEnv, DEV_RELAY_URL),
  RMM_RELAY_BIND_ADDR: process.env.RMM_DEBUG_RELAY_BIND_ADDR?.trim() || '127.0.0.1:17443',
  RMM_TELEMETRY_KAFKA_BROKERS: '127.0.0.1:3005',
  RMM_TELEMETRY_PRODUCER_BIND_ADDR:
    nativeDebugEnv.RMM_TELEMETRY_PRODUCER_BIND_ADDR?.trim() || '127.0.0.1:3003',
  RMM_TELEMETRY_UPSERT_URL: 'http://127.0.0.1:3001/rmm/telemetry/snapshots/upsert',
  RMM_TELEMETRY_EVENTS_BATCH_URL: 'http://127.0.0.1:3001/rmm/telemetry/events/batch',
  RMM_TELEMETRY_MANIFEST_URL: 'http://127.0.0.1:3001/rmm/telemetry/manifest/snapshots',
  RMM_TELEMETRY_GRAPH_APPLY_URL: 'http://127.0.0.1:3001/rmm/telemetry/graph/apply-batch',
  RMM_TELEMETRY_DECISION_EXECUTE_URL:
    'http://127.0.0.1:3001/rmm/telemetry/internal/decisions/execute',
  RMM_TELEMETRY_REMEDIATION_JOBS_URL: 'http://127.0.0.1:3001/rmm/telemetry/remediation/jobs',
  RMM_TELEMETRY_REMEDIATION_COMMAND_PROJECT_URL:
    'http://127.0.0.1:3001/rmm/telemetry/remediation/commands/project',
  RMM_TELEMETRY_REMEDIATION_STATUS_PROJECT_URL:
    'http://127.0.0.1:3001/rmm/telemetry/remediation/commands/status',
  RMM_TELEMETRY_PATCH_PROGRESS_PROJECT_URL: 'http://127.0.0.1:3001/rmm/telemetry/patch/progress',
  RMM_TELEMETRY_REMEDIATION_COMMANDS_TOPIC: 'rmm_telemetry_remediation_commands',
  RMM_TELEMETRY_REMEDIATION_STATUS_TOPIC: 'rmm_telemetry_remediation_status',
  RMM_TELEMETRY_REMEDIATION_DLQ_TOPIC: 'rmm_telemetry_remediation_dlq',
  RMM_TELEMETRY_REMEDIATION_ENQUEUE_URL:
    'http://127.0.0.1:3002/api/rmm/internal/remediation/commands/enqueue',
  RMM_TELEMETRY_RULES_URL_BASE: 'http://127.0.0.1:3001/rmm/telemetry/rules',
  RMM_TELEMETRY_PROCESSED_CHECK_URL: 'http://127.0.0.1:3001/rmm/telemetry/messages/processed',
  RMM_TELEMETRY_COMPAT_SNAPSHOT_UPSERT_URL: 'http://127.0.0.1:3001/rmm/telemetry/snapshots/upsert',
  RMM_AZURITE_BLOB_ENDPOINT: 'http://127.0.0.1:3008/devstoreaccount1',
};

const serviceCommands = [
  { name: 'api', argv: [bunExe, '--watch', 'server.ts'], cwd: apiBackendDir },
  { name: 'web', argv: [bunExe, 'run', 'web_dev'] },
  { name: 'talos_server', argv: [cargoExe, 'run', '-p', 'talos_server'] },
  { name: 'talos_relay', argv: [cargoExe, 'run', '-p', 'talos_relay'] },
  { name: 'telemetry_consumer', argv: [cargoExe, 'run', '-p', 'talos_telemetry_consumer'] },
  { name: 'telemetry_producer', argv: [cargoExe, 'run', '-p', 'talos_telemetry_producer'] },
  { name: 'talos_ai_runner', argv: [cargoExe, 'run', '-p', 'talos_ai_runner'] },
];

const requiredServicePorts = [
  { name: 'frontend', port: 3000 },
  { name: 'api_backend', port: 3001 },
  { name: 'talos_server', port: 3002 },
  { name: 'telemetry_producer', port: 3003 },
  { name: 'talos_ai_runner', port: 3010 },
  { name: 'talos_relay', port: 17443 },
];

type ManagedProcess = {
  name: string;
  child: ChildProcess;
  exited: Promise<number>;
  kill(signal?: NodeJS.Signals): void;
};

type ShutdownReason =
  | { kind: 'signal'; signal: NodeJS.Signals }
  | { kind: 'service-exit'; name: string; exitCode: number };

function signalExitCode(signal: NodeJS.Signals): number {
  const signalNumbers: Partial<Record<NodeJS.Signals, number>> = {
    SIGHUP: 1,
    SIGINT: 2,
    SIGQUIT: 3,
    SIGILL: 4,
    SIGTRAP: 5,
    SIGABRT: 6,
    SIGBUS: 7,
    SIGFPE: 8,
    SIGKILL: 9,
    SIGUSR1: 10,
    SIGSEGV: 11,
    SIGUSR2: 12,
    SIGPIPE: 13,
    SIGALRM: 14,
    SIGTERM: 15,
  };
  return 128 + (signalNumbers[signal] ?? 0);
}

function spawnManaged(name: string, argv: string[], cwd: string = appsDir): ManagedProcess {
  const child = spawn(argv[0], argv.slice(1), {
    cwd,
    env: debugEnv,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  });

  const exited = new Promise<number>((resolveExit) => {
    child.once('exit', (code, signal) => {
      if (typeof code === 'number') {
        resolveExit(code);
        return;
      }
      resolveExit(signal ? signalExitCode(signal) : 0);
    });
  });

  return {
    name,
    child,
    exited,
    kill(signal: NodeJS.Signals = 'SIGTERM') {
      if (!child.pid) {
        return;
      }
      if (process.platform === 'win32') {
        child.kill(signal);
        return;
      }
      process.kill(-child.pid, signal);
    },
  };
}

function stopProcesses(processes: ManagedProcess[], signal: NodeJS.Signals = 'SIGTERM') {
  for (const proc of processes) {
    try {
      proc.kill(signal);
    } catch {
      // Process may already have exited.
    }
  }
}

async function canListen(port: number): Promise<boolean> {
  return new Promise((resolveCanListen) => {
    const server = createServer();
    server.once('error', () => resolveCanListen(false));
    server.once('listening', () => {
      server.close(() => resolveCanListen(true));
    });
    server.listen(port, '0.0.0.0');
  });
}

async function assertRequiredPortsAvailable() {
  const occupied = [];
  for (const service of requiredServicePorts) {
    if (!(await canListen(service.port))) {
      occupied.push(service);
    }
  }

  if (occupied.length === 0) {
    return;
  }

  const details = occupied
    .map((service) => `${service.name} needs port ${service.port}`)
    .join(', ');
  throw new Error(
    `Cannot start debug services because required ports are already in use: ${details}`,
  );
}

async function waitForProcesses(processes: ManagedProcess[], timeoutMs: number): Promise<boolean> {
  const processExit = Promise.all(processes.map((proc) => proc.exited)).then(() => true);
  const timeout = new Promise<boolean>((resolveTimeout) => {
    setTimeout(() => resolveTimeout(false), timeoutMs).unref();
  });
  return Promise.race([processExit, timeout]);
}

async function runInfraDown() {
  console.log('Stopping Docker debug infrastructure...');
  const proc = Bun.spawn([bunExe, 'run', 'infra:down'], {
    cwd: appsDir,
    env: debugEnv,
    stdin: 'ignore',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  const exitCode = await proc.exited;
  if (exitCode !== 0) {
    console.warn(`Docker infrastructure cleanup exited with code ${exitCode}.`);
  }
}

type KillableProcess = {
  exited: Promise<number>;
  kill(signal?: NodeJS.Signals): void;
};

async function runWorkspaceInstall(
  setActiveProcess: (proc: KillableProcess | null) => void,
): Promise<number> {
  const proc = Bun.spawn([bunExe, 'install', '--frozen-lockfile'], {
    cwd: appsDir,
    env: debugEnv,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  setActiveProcess(proc);
  try {
    return await proc.exited;
  } finally {
    setActiveProcess(null);
  }
}

async function runPrismaGenerate(
  setActiveProcess: (proc: KillableProcess | null) => void,
): Promise<number> {
  const proc = Bun.spawn(
    [bunExe, 'x', '--bun', 'prisma', 'generate', '--schema=api_backend/prisma/schema.prisma'],
    {
      cwd: appsDir,
      env: debugEnv,
      stdin: 'inherit',
      stdout: 'inherit',
      stderr: 'inherit',
    },
  );
  setActiveProcess(proc);
  try {
    return await proc.exited;
  } finally {
    setActiveProcess(null);
  }
}

async function main() {
  const processes: ManagedProcess[] = [];
  let activeSetupProcess: KillableProcess | null = null;
  let shutdownPromise: Promise<number> | null = null;
  const setActiveSetupProcess = (proc: KillableProcess | null) => {
    activeSetupProcess = proc;
  };
  const shutdown = async (reason: ShutdownReason): Promise<number> => {
    if (shutdownPromise) {
      stopProcesses(processes, 'SIGKILL');
      return shutdownPromise;
    }

    shutdownPromise = (async () => {
      const exitCode = reason.kind === 'signal' ? signalExitCode(reason.signal) : reason.exitCode;
      if (reason.kind === 'signal') {
        console.log(`Received ${reason.signal}; stopping debug services.`);
      } else {
        console.log(`${reason.name} exited with code ${reason.exitCode}; stopping debug services.`);
      }

      try {
        activeSetupProcess?.kill('SIGTERM');
      } catch {
        // Setup process may already have exited.
      }

      stopProcesses(processes, 'SIGTERM');
      const stopped = await waitForProcesses(processes, 5_000);
      if (!stopped) {
        console.warn('Some debug services did not stop after SIGTERM; forcing shutdown.');
        stopProcesses(processes, 'SIGKILL');
        await waitForProcesses(processes, 2_000);
      }

      await runInfraDown();
      return exitCode;
    })();

    return shutdownPromise;
  };

  let sigintCount = 0;
  process.on('SIGINT', () => {
    sigintCount += 1;
    if (sigintCount > 1) {
      stopProcesses(processes, 'SIGKILL');
      return;
    }
    void shutdown({ kind: 'signal', signal: 'SIGINT' }).then((exitCode) => process.exit(exitCode));
  });
  process.once('SIGTERM', () => {
    void shutdown({ kind: 'signal', signal: 'SIGTERM' }).then((exitCode) => process.exit(exitCode));
  });
  process.once('SIGHUP', () => {
    void shutdown({ kind: 'signal', signal: 'SIGHUP' }).then((exitCode) => process.exit(exitCode));
  });

  assertNativeDebugEnvironment(debugEnv);
  await ensureRelayUrl();
  await requireRelayCertificates(nativeDebugEnv, repoRoot);
  await assertRequiredPortsAvailable();
  await assertLocalDockerEngineResponsive();

  const infra = Bun.spawn([bunExe, 'run', 'infra:up'], {
    cwd: appsDir,
    env: debugEnv,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  setActiveSetupProcess(infra);
  const infraExitCode = await infra.exited;
  setActiveSetupProcess(null);
  if (shutdownPromise) {
    process.exit(await shutdownPromise);
  }
  if (infraExitCode !== 0) {
    process.exit(infraExitCode);
  }

  const db = Bun.spawn([bunExe, './scripts/ensure-db.ts'], {
    cwd: appsDir,
    env: debugEnv,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  setActiveSetupProcess(db);
  const dbExitCode = await db.exited;
  setActiveSetupProcess(null);
  if (shutdownPromise) {
    process.exit(await shutdownPromise);
  }
  if (dbExitCode !== 0) {
    process.exit(dbExitCode);
  }

  console.log('Ensuring Bun workspace dependencies match this platform...');
  const installExitCode = await runWorkspaceInstall(setActiveSetupProcess);
  if (shutdownPromise) {
    process.exit(await shutdownPromise);
  }
  if (installExitCode !== 0) {
    process.exit(installExitCode);
  }

  console.log('Generating API Prisma client...');
  const prismaGenerateExitCode = await runPrismaGenerate(setActiveSetupProcess);
  if (shutdownPromise) {
    process.exit(await shutdownPromise);
  }
  if (prismaGenerateExitCode !== 0) {
    process.exit(prismaGenerateExitCode);
  }

  console.log('Starting local debug services on Docker-exposed ports:');
  console.log('  api_backend: http://localhost:3001');
  console.log('  frontend:    http://localhost:3000');
  console.log('  talos_server:  http://localhost:3002');
  console.log('  talos_ai_runner: http://localhost:3010');
  console.log('  talos_relay:   tcp://localhost:17443 (TLS)');
  console.log('  telemetry:   http://localhost:3003');

  processes.push(
    ...serviceCommands.map(({ name, argv, cwd }) => {
      console.log(`  starting ${name}`);
      return spawnManaged(name, argv, cwd);
    }),
  );

  const firstExit = await Promise.race(
    processes.map((proc) => proc.exited.then((exitCode) => ({ exitCode, name: proc.name }))),
  );
  process.exit(
    await shutdown({ kind: 'service-exit', name: firstExit.name, exitCode: firstExit.exitCode }),
  );
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
