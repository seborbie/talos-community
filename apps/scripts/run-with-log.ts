import { spawn, type ChildProcess } from 'child_process';
import { createWriteStream, rmSync, writeFileSync } from 'fs';
import { basename, resolve } from 'path';

const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'] as const;

function usage(): never {
  console.error('Usage: bun ./scripts/run-with-log.ts <log-file-name> -- <command> [args...]');
  process.exit(1);
}

function signalExitCode(signal: NodeJS.Signals): number {
  const signalNumbers: Partial<Record<NodeJS.Signals, number>> = {
    SIGHUP: 1,
    SIGINT: 2,
    SIGTERM: 15,
  };
  return 128 + (signalNumbers[signal] ?? 0);
}

const args = process.argv.slice(2);
if (args.length < 3 || args[1] !== '--') {
  usage();
}

const logFileName = args[0];
if (!logFileName || basename(logFileName) !== logFileName) {
  console.error('Log file name must be a plain file name in the current directory.');
  process.exit(1);
}

const command = args.slice(2);
if (command.length === 0) {
  usage();
}

const logPath = resolve(process.cwd(), logFileName);
rmSync(logPath, { force: true });
writeFileSync(logPath, '');

const logStream = createWriteStream(logPath, { flags: 'a' });
let child: ChildProcess | null = null;
let forcedExit = false;

function writeOutput(stream: NodeJS.WriteStream, chunk: string | Buffer) {
  stream.write(chunk);
  logStream.write(chunk);
}

function writeLine(message: string) {
  writeOutput(process.stdout, `${message}\n`);
}

function closeLog(): Promise<void> {
  return new Promise((resolveClose) => {
    logStream.end(resolveClose);
  });
}

function executableFor(name: string): string {
  return name === 'bun' ? process.execPath : name;
}

writeLine(`Logging output to ${logPath}`);

child = spawn(executableFor(command[0]), command.slice(1), {
  cwd: process.cwd(),
  env: process.env,
  stdio: ['inherit', 'pipe', 'pipe'],
});

child.stdout?.on('data', (chunk: Buffer) => writeOutput(process.stdout, chunk));
child.stderr?.on('data', (chunk: Buffer) => writeOutput(process.stderr, chunk));

child.once('error', async (error) => {
  writeOutput(process.stderr, `Failed to start ${command[0]}: ${error.message}\n`);
  await closeLog();
  process.exit(1);
});

for (const signal of signals) {
  process.once(signal, () => {
    if (forcedExit) {
      child?.kill('SIGKILL');
      process.exit(signalExitCode(signal));
    }
    forcedExit = true;
    child?.kill(signal);
  });
}

const exitCode = await new Promise<number>((resolveExit) => {
  child?.once('close', (code, signal) => {
    if (typeof code === 'number') {
      resolveExit(code);
      return;
    }
    resolveExit(signal ? signalExitCode(signal) : 0);
  });
});

await closeLog();
process.exit(exitCode);
