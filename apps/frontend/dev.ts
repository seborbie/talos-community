// Simple launcher to respect FRONTEND_PORT, then default 3000
import { spawn } from 'node:child_process';

const port: string = process.env.FRONTEND_PORT || '3000';
const args: string[] = ['x', '--bun', 'vite', 'dev', '--host', '--port', port, '--strictPort'];

const child = spawn(process.execPath, args, { stdio: ['inherit', 'pipe', 'pipe'], shell: false });

child.stdout?.on('data', (chunk: Buffer) => {
  const text: string = chunk.toString();
  process.stdout.write(text);
});

child.stderr?.on('data', (chunk: Buffer) => {
  const text: string = chunk.toString();
  process.stderr.write(text);
});

child.on('exit', (code: number | null) => {
  process.exit(code ?? 0);
});
