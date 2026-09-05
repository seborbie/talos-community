import { expect, test } from 'bun:test';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { checkCargoManifestGraph, checkDependabotRepository } from './dependabot-contract';
import { classifyPublicPath, validatePublicExportPolicy } from './public-source-export';

const repoRoot = resolve(import.meta.dir, '../..');

test('public Git-visible Cargo graph is complete without acquiring excluded source', async () => {
  const manifests = await checkDependabotRepository(repoRoot);
  expect(manifests).toContain('apps/vpx-encode/Cargo.toml');
  const policy = validatePublicExportPolicy(
    JSON.parse(await readFile(resolve(repoRoot, '.config/public-export-policy.json'), 'utf8')),
  );
  for (const manifest of manifests) {
    expect(classifyPublicPath(manifest, policy).kind).toBe('include');
  }
  expect(policy.requiredFiles).toContain('apps/vpx-encode/Cargo.toml');
  for (const path of [
    'apps/vpx-encode/src/lib.rs',
    'apps/vpx-encode/README.md',
    'apps/vpx-encode/build.rs',
  ]) {
    expect(classifyPublicPath(path, policy).kind).not.toBe('include');
  }
});

test('missing local manifests fail even for transitive, target-specific and patched dependencies', async () => {
  for (const declaration of [
    '[dependencies]\nlocal = { path = "../missing" }',
    '[target.\'cfg(windows)\'.dependencies]\nlocal = { path = "../missing" }',
    '[patch.crates-io]\nlocal = { path = "../missing" }',
  ]) {
    const files: Record<string, string> = {
      'apps/Cargo.toml': '[workspace]\nmembers = ["worker"]',
      'apps/worker/Cargo.toml': declaration,
    };
    await expect(
      checkCargoManifestGraph(async (path) => {
        if (!(path in files)) throw new Error(`Missing ${path}`);
        return files[path]!;
      }),
    ).rejects.toThrow('Missing apps/missing/Cargo.toml');
  }
});

test('Dependabot covers both application workspaces and actions without suppressing security updates', async () => {
  const config = Bun.YAML.parse(
    await readFile(resolve(repoRoot, '.github/dependabot.yml'), 'utf8'),
  ) as {
    updates: Array<Record<string, unknown>>;
  };
  for (const [ecosystem, directory] of [
    ['bun', '/apps'],
    ['cargo', '/apps'],
    ['github-actions', '/'],
  ]) {
    const entry = config.updates.find((update) => update['package-ecosystem'] === ecosystem);
    expect(entry?.directory).toBe(directory);
    expect(entry?.['target-branch']).toBeUndefined();
    expect(entry?.ignore).toBeUndefined();
    expect(entry?.['exclude-paths']).toBeUndefined();
    expect(entry?.['open-pull-requests-limit']).toBeGreaterThan(0);
  }
  const cargo = config.updates.find((update) => update['package-ecosystem'] === 'cargo');
  expect(cargo?.allow).toEqual([{ 'dependency-type': 'all' }]);
});
