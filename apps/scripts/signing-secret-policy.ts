import { extname, resolve } from 'node:path';

const FORBIDDEN_SIGNING_KEY_EXTENSIONS = new Set(['.key', '.p12', '.pfx']);
const PRIVATE_KEY_PEM_HEADER = /^-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----\s*$/m;

export type TrackedFile = {
  path: string;
  bytes: Uint8Array;
};

export function trackedSigningSecretFailures(files: TrackedFile[]): string[] {
  const failures: string[] = [];
  const decoder = new TextDecoder('utf-8', { fatal: false });

  for (const file of files) {
    const extension = extname(file.path).toLowerCase();
    if (FORBIDDEN_SIGNING_KEY_EXTENSIONS.has(extension)) {
      failures.push(`tracked signing-key container is forbidden: ${file.path}`);
      continue;
    }

    const contents = decoder.decode(file.bytes);
    if (PRIVATE_KEY_PEM_HEADER.test(contents)) {
      failures.push(`tracked PEM private key is forbidden: ${file.path}`);
    }
  }

  return failures;
}

async function trackedPaths(repoRoot: string): Promise<string[]> {
  const process = Bun.spawn(['git', '-C', repoRoot, 'ls-files', '-z'], {
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(process.stdout).arrayBuffer(),
    new Response(process.stderr).text(),
    process.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`git ls-files failed (${exitCode}): ${stderr.trim()}`);
  }

  return new TextDecoder()
    .decode(stdout)
    .split('\0')
    .filter((path) => path.length > 0);
}

export async function checkTrackedSigningSecrets(
  repoRoot = resolve(import.meta.dir, '../..'),
): Promise<{ failures: string[]; scannedFiles: number }> {
  const paths = await trackedPaths(repoRoot);
  const files: TrackedFile[] = [];
  for (const path of paths) {
    const file = Bun.file(resolve(repoRoot, path));
    if (!(await file.exists())) continue;
    const extension = extname(path).toLowerCase();
    // Secret-bearing PEMs and source/config files are small. Avoid loading large vendored binary
    // assets merely to search for a line-oriented PEM header; forbidden key containers are still
    // rejected by name regardless of size.
    if (FORBIDDEN_SIGNING_KEY_EXTENSIONS.has(extension) || file.size <= 2 * 1024 * 1024) {
      files.push({ path, bytes: new Uint8Array(await file.arrayBuffer()) });
    }
  }

  return {
    failures: trackedSigningSecretFailures(files),
    scannedFiles: files.length,
  };
}

if (import.meta.main) {
  const repoRoot = resolve(import.meta.dir, '../..');
  const result = await checkTrackedSigningSecrets(repoRoot);
  if (result.failures.length > 0) {
    console.error('Tracked signing-secret policy failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(
    `Tracked signing-secret policy passed (${result.scannedFiles} candidate files scanned).`,
  );
}
