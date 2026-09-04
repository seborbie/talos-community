import { resolve } from 'node:path';

const workspaceDir = resolve(import.meta.dir, '..');

const MACOS_SWIFT_RUNNER = 'env DYLD_LIBRARY_PATH=/usr/lib/swift';

export function rustTestEnvironment(
  platform: NodeJS.Platform = process.platform,
  architecture: string = process.arch,
  source: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
  const environment = { ...source };

  if (platform !== 'darwin') return environment;

  const target =
    architecture === 'arm64'
      ? 'AARCH64_APPLE_DARWIN'
      : architecture === 'x64'
        ? 'X86_64_APPLE_DARWIN'
        : undefined;

  if (!target) return environment;

  const runnerVariable = `CARGO_TARGET_${target}_RUNNER`;
  environment[runnerVariable] ??= MACOS_SWIFT_RUNNER;
  return environment;
}

async function runCargo(args: string[], environment: NodeJS.ProcessEnv): Promise<void> {
  const child = Bun.spawn(['cargo', ...args], {
    cwd: workspaceDir,
    env: environment,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
  const exitCode = await child.exited;
  if (exitCode !== 0) process.exit(exitCode);
}

if (import.meta.main) {
  const environment = rustTestEnvironment();
  await runCargo(['test', '--workspace', '--all-targets', '--locked'], environment);
  await runCargo(['test', '--workspace', '--doc', '--locked'], environment);
}
