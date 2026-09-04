import { resolve } from 'path';
import { getLocalDockerEnv } from './docker-local-env';

const appsDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(appsDir, '..');
const composePath = resolve(repoRoot, 'infra', 'docker-compose.dev.yml');
const gracefulTimeoutMs = 5000;

const devPorts = [
  { name: 'frontend', port: 3000 },
  { name: 'api_backend', port: 3001 },
  { name: 'talos_server', port: 3002 },
  { name: 'talos_telemetry_producer', port: 3003 },
  { name: 'talos_ai_runner', port: 3010 },
  { name: 'postgres', port: 3004 },
  { name: 'redpanda_kafka', port: 3005 },
  { name: 'redpanda_schema_registry', port: 3006 },
  { name: 'redpanda_console', port: 3007 },
  { name: 'azurite', port: 3008 },
  { name: 'redpanda_admin', port: 3009 },
  { name: 'talos_relay', port: 17443 },
] as const;

const parentProcessPatterns = [
  'scripts/run-debug.ts',
  'scripts\\run-debug.ts',
  'frontend/dev.ts',
  'frontend\\dev.ts',
] as const;

type CommandResult = {
  exitCode: number;
  stdout: string;
  stderr: string;
};

function parseArgs() {
  const args = new Set(process.argv.slice(2));
  return {
    dryRun: args.has('--dry-run'),
    skipDocker: args.has('--skip-docker'),
  };
}

async function runCommand(
  argv: string[],
  options: { cwd?: string; env?: Record<string, string>; quiet?: boolean; dryRun?: boolean } = {},
): Promise<CommandResult> {
  if (options.dryRun) {
    console.log(`[dry-run] ${argv.join(' ')}`);
    return { exitCode: 0, stdout: '', stderr: '' };
  }

  let proc: ReturnType<typeof Bun.spawn>;
  try {
    proc = Bun.spawn(argv, {
      cwd: options.cwd,
      env: options.env,
      stdout: 'pipe',
      stderr: 'pipe',
    });
  } catch (error) {
    return {
      exitCode: 127,
      stdout: '',
      stderr: error instanceof Error ? error.message : String(error),
    };
  }

  const readPipedOutput = (output: number | ReadableStream<Uint8Array> | undefined) => {
    if (output === undefined || typeof output === 'number') return Promise.resolve('');
    return new Response(output).text();
  };
  const [stdout, stderr, exitCode] = await Promise.all([
    readPipedOutput(proc.stdout),
    readPipedOutput(proc.stderr),
    proc.exited,
  ]);

  if (!options.quiet) {
    if (stdout.trim()) {
      process.stdout.write(stdout);
    }
    if (stderr.trim()) {
      process.stderr.write(stderr);
    }
  }

  return { exitCode, stdout, stderr };
}

function uniquePids(pids: number[]) {
  return [...new Set(pids)].filter(
    (pid) => Number.isInteger(pid) && pid > 0 && pid !== process.pid && pid !== process.ppid,
  );
}

async function getUnixPortPids(port: number): Promise<number[]> {
  const lsof = await runCommand(['lsof', '-nP', `-tiTCP:${port}`, '-sTCP:LISTEN'], { quiet: true });
  if (lsof.exitCode === 0) {
    return uniquePids(
      lsof.stdout
        .split(/\s+/)
        .map((value) => Number(value))
        .filter(Boolean),
    );
  }

  const ss = await runCommand(['ss', '-ltnp', `sport = :${port}`], { quiet: true });
  if (ss.exitCode !== 0) {
    return [];
  }

  const pids = [...ss.stdout.matchAll(/pid=(\d+)/g)].map((match) => Number(match[1]));
  return uniquePids(pids);
}

async function getWindowsPortPids(port: number): Promise<number[]> {
  const script = [
    "$ErrorActionPreference = 'SilentlyContinue'",
    `Get-NetTCPConnection -LocalPort ${port} -State Listen | Select-Object -ExpandProperty OwningProcess -Unique`,
  ].join('; ');
  const result = await runCommand(['powershell', '-NoProfile', '-Command', script], {
    quiet: true,
  });
  if (result.exitCode !== 0) {
    return [];
  }

  return uniquePids(
    result.stdout
      .split(/\s+/)
      .map((value) => Number(value))
      .filter(Boolean),
  );
}

async function getPortPids(port: number): Promise<number[]> {
  if (process.platform === 'win32') {
    return getWindowsPortPids(port);
  }

  return getUnixPortPids(port);
}

async function getParentDevPids(): Promise<number[]> {
  if (process.platform === 'win32') {
    const script = [
      "$ErrorActionPreference = 'SilentlyContinue'",
      'Get-CimInstance Win32_Process |',
      `Where-Object { ${parentProcessPatterns.map((pattern) => `$_.CommandLine -like '*${pattern.replace(/\\/g, '\\\\')}*'`).join(' -or ')} } |`,
      'Select-Object -ExpandProperty ProcessId -Unique',
    ].join(' ');
    const result = await runCommand(['powershell', '-NoProfile', '-Command', script], {
      quiet: true,
    });
    if (result.exitCode !== 0) {
      return [];
    }
    return uniquePids(result.stdout.split(/\s+/).map((value) => Number(value)));
  }

  const result = await runCommand(['ps', '-axo', 'pid=,command='], { quiet: true });
  if (result.exitCode !== 0) {
    return [];
  }

  const pids = [];
  for (const line of result.stdout.split(/\r?\n/)) {
    if (!parentProcessPatterns.some((pattern) => line.includes(pattern))) {
      continue;
    }

    const pid = Number(line.trim().match(/^(\d+)/)?.[1]);
    if (pid) {
      pids.push(pid);
    }
  }

  return uniquePids(pids);
}

async function collectDevPids() {
  const byPid = new Map<number, string[]>();

  for (const service of devPorts) {
    for (const pid of await getPortPids(service.port)) {
      const labels = byPid.get(pid) ?? [];
      labels.push(`${service.name}:${service.port}`);
      byPid.set(pid, labels);
    }
  }

  for (const pid of await getParentDevPids()) {
    const labels = byPid.get(pid) ?? [];
    labels.push('dev parent');
    byPid.set(pid, labels);
  }

  return byPid;
}

async function isPidAlive(pid: number) {
  if (process.platform === 'win32') {
    const result = await runCommand(
      ['powershell', '-NoProfile', '-Command', `Get-Process -Id ${pid}`],
      { quiet: true },
    );
    return result.exitCode === 0;
  }

  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function waitForExit(pids: number[], timeoutMs: number) {
  const deadline = Date.now() + timeoutMs;
  let remaining = pids;

  while (remaining.length > 0 && Date.now() < deadline) {
    await Bun.sleep(250);
    const alive = [];
    for (const pid of remaining) {
      if (await isPidAlive(pid)) {
        alive.push(pid);
      }
    }
    remaining = alive;
  }

  return remaining;
}

async function signalPid(pid: number, signal: 'SIGTERM' | 'SIGKILL', dryRun: boolean) {
  if (dryRun) {
    console.log(`[dry-run] ${signal} ${pid}`);
    return;
  }

  if (process.platform === 'win32') {
    const args =
      signal === 'SIGKILL' ? ['/PID', String(pid), '/T', '/F'] : ['/PID', String(pid), '/T'];
    await runCommand(['taskkill', ...args], { quiet: true });
    return;
  }

  try {
    process.kill(pid, signal);
  } catch {
    // Process may already have exited.
  }
}

async function stopDockerStack(dryRun: boolean) {
  console.log('Stopping Docker dev stack...');
  const result = await runCommand(
    ['docker', 'compose', '-f', composePath, 'down', '--remove-orphans'],
    { cwd: repoRoot, env: await getLocalDockerEnv(), dryRun },
  );

  if (result.exitCode !== 0) {
    console.error(
      `Docker compose down failed with exit code ${result.exitCode}. Continuing with local process cleanup.`,
    );
  }
}

async function stopLocalProcesses(dryRun: boolean) {
  const devPids = await collectDevPids();
  const pids = [...devPids.keys()];

  if (pids.length === 0) {
    console.log('No local dev/debug listeners found.');
    return;
  }

  for (const [pid, labels] of devPids.entries()) {
    console.log(`Stopping pid ${pid} (${labels.join(', ')})...`);
    await signalPid(pid, 'SIGTERM', dryRun);
  }

  if (dryRun) {
    return;
  }

  const remaining = await waitForExit(pids, gracefulTimeoutMs);
  if (remaining.length === 0) {
    console.log('Local dev/debug processes stopped.');
    return;
  }

  for (const pid of remaining) {
    console.log(`Force stopping pid ${pid}...`);
    await signalPid(pid, 'SIGKILL', false);
  }
}

async function main() {
  const { dryRun, skipDocker } = parseArgs();

  if (!skipDocker) {
    await stopDockerStack(dryRun);
  }

  await stopLocalProcesses(dryRun);
  console.log('Dev stack cleanup complete.');
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
