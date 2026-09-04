#!/usr/bin/env bun
import { resolve } from 'path';
import { getLocalDockerEnv } from './docker-local-env';

function getScriptArgs() {
  const argv = process.argv.slice(1);
  const firstArg = argv[0];
  if (!firstArg) {
    return [];
  }

  const scriptPath = resolve(import.meta.path);
  const firstArgPath = resolve(firstArg);
  if (firstArgPath === scriptPath) {
    return argv.slice(1);
  }

  return argv;
}

const args = getScriptArgs();
if (args.length === 0 || args[0] === '-h' || args[0] === '--help') {
  console.error('Usage: bun ./scripts/run-docker.mts <docker args...>');
  process.exit(args.length === 0 ? 1 : 0);
}

const argv = args[0] === 'docker' ? args : ['docker', ...args];
const proc = Bun.spawn(argv, {
  env: await getLocalDockerEnv(),
  cwd: process.cwd(),
  stdin: 'inherit',
  stdout: 'inherit',
  stderr: 'inherit',
});

process.exit(await proc.exited);
