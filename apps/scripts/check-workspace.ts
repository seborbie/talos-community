import { lstat, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';

type RootManifest = {
  packageManager?: string;
  workspaces?: {
    packages?: string[];
  };
};

type PackageManifest = {
  name?: string;
  private?: boolean;
};

const workspaceDir = resolve(import.meta.dir, '..');
const repoRoot = resolve(workspaceDir, '..');
const failures: string[] = [];

async function pathExists(path: string): Promise<boolean> {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

const rootManifest = (await Bun.file(resolve(workspaceDir, 'package.json')).json()) as RootManifest;
const workspacePackages = rootManifest.workspaces?.packages ?? [];

if (!/^bun@\d+\.\d+\.\d+$/.test(rootManifest.packageManager ?? '')) {
  failures.push(
    'package.json must pin packageManager to an exact bun@<major>.<minor>.<patch> version',
  );
}

if (workspacePackages.length === 0) {
  failures.push('package.json must declare at least one workspace package');
}

if (new Set(workspacePackages).size !== workspacePackages.length) {
  failures.push('package.json contains duplicate workspace package paths');
}

const bunfig = await Bun.file(resolve(workspaceDir, 'bunfig.toml')).text();
if (!/^linker\s*=\s*["']isolated["']\s*$/m.test(bunfig)) {
  failures.push('bunfig.toml must set [install] linker = "isolated"');
}
if (!/^hoist\s*=\s*false\s*$/m.test(bunfig)) {
  failures.push('bunfig.toml must set [install] hoist = false for strict dependency resolution');
}
if (!/^hoistPattern\s*=\s*\[\s*\]\s*$/m.test(bunfig)) {
  failures.push(
    'bunfig.toml must set [install] hoistPattern = [] for the pinned Bun compatibility path',
  );
}

const fallbackHoistPath = resolve(workspaceDir, 'node_modules', '.bun', 'node_modules');
if (await pathExists(fallbackHoistPath)) {
  const fallbackEntries = await readdir(fallbackHoistPath);
  if (fallbackEntries.length > 0) {
    failures.push(
      `node_modules/.bun/node_modules contains ${fallbackEntries.length} fallback entries; ` +
        'perform a clean bun install after enabling strict hoist settings',
    );
  }
}

const dockerignore = await Bun.file(resolve(repoRoot, '.dockerignore')).text();
for (const requiredSecretPattern of ['apps/certs/**', '*.pem', '*.p12']) {
  if (!dockerignore.split(/\r?\n/).includes(requiredSecretPattern)) {
    failures.push(
      `.dockerignore must contain ${requiredSecretPattern} so local credentials cannot enter Docker build contexts`,
    );
  }
}

if (!(await Bun.file(resolve(workspaceDir, 'bun.lock')).exists())) {
  failures.push('the workspace root bun.lock is missing');
}

const immediatePackageDirs: string[] = [];
for (const entry of await readdir(workspaceDir, { withFileTypes: true })) {
  if (!entry.isDirectory() || entry.name === 'node_modules' || entry.name === 'vendor') continue;
  if (await Bun.file(resolve(workspaceDir, entry.name, 'package.json')).exists()) {
    immediatePackageDirs.push(entry.name);
  }
}

for (const packageDir of immediatePackageDirs) {
  if (!workspacePackages.includes(packageDir)) {
    failures.push(`${packageDir}/package.json is not declared in the root workspace`);
  }
}

const packageNames = new Set<string>();
for (const packageDir of workspacePackages) {
  if (packageDir.includes('*') || packageDir.includes('..') || packageDir.startsWith('/')) {
    failures.push(
      `workspace path ${JSON.stringify(packageDir)} must be an explicit child directory`,
    );
    continue;
  }

  const manifestPath = resolve(workspaceDir, packageDir, 'package.json');
  const manifestFile = Bun.file(manifestPath);
  if (!(await manifestFile.exists())) {
    failures.push(`${packageDir}/package.json is missing`);
    continue;
  }

  const manifest = (await manifestFile.json()) as PackageManifest;
  if (!manifest.name) {
    failures.push(`${packageDir}/package.json must declare a package name`);
  } else if (packageNames.has(manifest.name)) {
    failures.push(`workspace package name ${JSON.stringify(manifest.name)} is duplicated`);
  } else {
    packageNames.add(manifest.name);
  }

  if (manifest.private !== true) {
    failures.push(`${packageDir}/package.json must set private to true`);
  }

  if (await Bun.file(resolve(workspaceDir, packageDir, 'bun.lock')).exists()) {
    failures.push(
      `${packageDir}/bun.lock is forbidden; update apps/bun.lock from the workspace root`,
    );
  }
}

if (failures.length > 0) {
  console.error('Bun workspace integrity check failed:\n');
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  `Bun workspace integrity check passed (${workspacePackages.length} packages, strict isolated linker, one first-party lockfile).`,
);
