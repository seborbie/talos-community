import { readFile } from 'node:fs/promises';
import { dirname, posix, resolve } from 'node:path';

type Table = Record<string, unknown>;
function table(value: unknown): Table {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Table)
    : {};
}

// Follow the local manifest graph that Dependabot fetches before running Cargo.
// Reading only Git-visible files prevents an already-bootstrapped tree hiding a broken checkout.
export async function checkCargoManifestGraph(
  read: (path: string) => Promise<string>,
  entry = 'apps/Cargo.toml',
): Promise<string[]> {
  const visited = new Set<string>();
  async function visit(path: string): Promise<void> {
    path = posix.normalize(path);
    if (path.startsWith('../') || posix.isAbsolute(path)) {
      throw new Error(`Cargo manifest escapes repository: ${path}`);
    }
    if (visited.has(path)) return;
    visited.add(path);
    const manifest = table(Bun.TOML.parse(await read(path)));
    const workspace = table(manifest.workspace);
    const paths: string[] = [];
    for (const member of (workspace.members ?? []) as string[]) paths.push(member);
    function dependencies(value: unknown): void {
      const section = table(value);
      for (const kind of ['dependencies', 'dev-dependencies', 'build-dependencies']) {
        for (const dependency of Object.values(table(section[kind]))) {
          const local = table(dependency).path;
          if (typeof local === 'string') paths.push(local);
        }
      }
    }
    dependencies(manifest);
    dependencies(workspace);
    for (const target of Object.values(table(manifest.target))) dependencies(target);
    for (const registry of Object.values(table(manifest.patch))) {
      for (const dependency of Object.values(table(registry))) {
        const local = table(dependency).path;
        if (typeof local === 'string') paths.push(local);
      }
    }
    for (const dependency of Object.values(table(manifest.replace))) {
      const local = table(dependency).path;
      if (typeof local === 'string') paths.push(local);
    }
    for (const local of paths) {
      if (/[?*\[\]{}]/.test(local)) {
        throw new Error(
          `Expand Cargo member glob in dependency contract before using it: ${local}`,
        );
      }
      await visit(posix.join(posix.dirname(path), local, 'Cargo.toml'));
    }
  }
  await visit(entry);
  return [...visited];
}

export async function checkDependabotRepository(repoRoot: string): Promise<string[]> {
  const child = Bun.spawn(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], {
    cwd: repoRoot,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [output, error, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (code !== 0) throw new Error(`Cannot enumerate Git-visible manifests: ${error}`);
  const files = new Set(output.split('\0'));
  return checkCargoManifestGraph(async (path) => {
    if (!files.has(path))
      throw new Error(`Dependabot cannot fetch untracked/ignored manifest: ${path}`);
    return readFile(resolve(repoRoot, path), 'utf8');
  });
}

if (import.meta.main) {
  const manifests = await checkDependabotRepository(resolve(dirname(import.meta.path), '../..'));
  console.log(`Dependabot can fetch all ${manifests.length} local Cargo manifests.`);
}
