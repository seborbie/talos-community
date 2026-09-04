import { resolve } from 'node:path';

type WorkspaceManifest = {
  packageManager?: string;
};

const workspaceDir = resolve(import.meta.dir, '..');
const manifest = (await Bun.file(
  resolve(workspaceDir, 'package.json'),
).json()) as WorkspaceManifest;
const packageManager = manifest.packageManager ?? '';
const match = /^bun@(.+)$/.exec(packageManager);

if (!match) {
  console.error('apps/package.json must pin packageManager as bun@<version>.');
  process.exit(1);
}

const requiredVersion = match[1];
const actualVersion = process.versions.bun;

if (actualVersion !== requiredVersion) {
  console.error(
    `Talos requires Bun ${requiredVersion}; current Bun is ${actualVersion ?? 'unknown'}. ` +
      'Install the pinned version before changing dependencies or running release builds.',
  );
  process.exit(1);
}

console.log(`Bun ${actualVersion} matches the workspace toolchain pin.`);
