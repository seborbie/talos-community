import { afterEach, describe, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import {
  classifyPublicPath,
  exportPublicSource,
  PUBLIC_EXPORT_MANIFEST,
  type PublicExportPolicy,
  validatePublicExportPolicy,
} from './public-source-export';

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

function digest(bytes: Uint8Array | string): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function fixturePolicy(binaryBytes = new Uint8Array([0, 97, 115, 109])): PublicExportPolicy {
  return {
    schemaVersion: 1,
    name: 'fixture',
    maximumFileBytes: 2 * 1024 * 1024,
    externalGates: [],
    requiredFiles: ['README.md'],
    includePatterns: ['.config/export.json', 'README.md', 'src/**'],
    excludePatterns: ['src/generated/**'],
    blockedOmissions: [
      {
        id: 'PX-TEST',
        patterns: ['src/vendor/**'],
        reason: 'fixture provenance is unresolved',
      },
    ],
    forbiddenBinaryExtensions: ['.dll', '.wasm'],
    permittedBinaryFiles: [
      {
        path: 'src/test.wasm',
        sha256: digest(binaryBytes),
        provenance: 'byte-exact local test fixture',
      },
    ],
  };
}

async function git(root: string, ...args: string[]): Promise<void> {
  const process = Bun.spawn(['git', '-C', root, ...args], {
    stdout: 'pipe',
    stderr: 'pipe',
    env: {
      ...Bun.env,
      GIT_AUTHOR_NAME: 'Talos export test',
      GIT_AUTHOR_EMAIL: 'export-test@example.invalid',
      GIT_COMMITTER_NAME: 'Talos export test',
      GIT_COMMITTER_EMAIL: 'export-test@example.invalid',
    },
  });
  const [stderr, code] = await Promise.all([new Response(process.stderr).text(), process.exited]);
  if (code !== 0) throw new Error(`git ${args.join(' ')} failed: ${stderr}`);
}

async function createFixture(): Promise<{
  base: string;
  repo: string;
  policyPath: string;
  binaryBytes: Uint8Array;
}> {
  const base = await mkdtemp(resolve(tmpdir(), 'talos-public-export-'));
  temporaryDirectories.push(base);
  const repo = resolve(base, 'private');
  const binaryBytes = new Uint8Array([0, 97, 115, 109]);
  await mkdir(resolve(repo, '.config'), { recursive: true });
  await mkdir(resolve(repo, 'src/generated'), { recursive: true });
  await mkdir(resolve(repo, 'src/vendor'), { recursive: true });
  await writeFile(resolve(repo, 'README.md'), 'fixture\n');
  await writeFile(resolve(repo, 'src/app.ts'), 'export const value = 1;\n');
  await chmod(resolve(repo, 'src/app.ts'), 0o755);
  await writeFile(resolve(repo, 'src/generated/output.js'), 'generated\n');
  await writeFile(resolve(repo, 'src/vendor/provenance.dll'), new Uint8Array([77, 90, 0]));
  await writeFile(resolve(repo, 'src/test.wasm'), binaryBytes);
  const policyPath = resolve(repo, '.config/export.json');
  await writeFile(policyPath, `${JSON.stringify(fixturePolicy(binaryBytes), null, 2)}\n`);
  await git(repo, 'init', '-b', 'main');
  await git(repo, 'add', '.');
  await git(repo, 'commit', '-m', 'fixture');
  await writeFile(resolve(repo, 'src/uncommitted.ts'), 'export const dirty = true;\n');
  return { base, repo, policyPath, binaryBytes };
}

describe('public source export policy', () => {
  test('classifies explicit source, generated output, and provenance blockers separately', () => {
    const policy = validatePublicExportPolicy(fixturePolicy());
    expect(classifyPublicPath('src/app.ts', policy)).toEqual({ kind: 'include' });
    expect(classifyPublicPath('src/generated/output.js', policy)).toEqual({ kind: 'exclude' });
    expect(classifyPublicPath('private.txt', policy)).toEqual({ kind: 'not-allowlisted' });
    expect(classifyPublicPath('src/vendor/provenance.dll', policy)).toEqual({
      kind: 'blocker',
      blocker: policy.blockedOmissions[0],
    });
  });

  test('rejects broad patterns and digest-free binary exceptions', () => {
    expect(() =>
      validatePublicExportPolicy({ ...fixturePolicy(), includePatterns: ['**'] }),
    ).toThrow('too broad');
    expect(() =>
      validatePublicExportPolicy({
        ...fixturePolicy(),
        permittedBinaryFiles: [{ path: 'src/test.wasm', sha256: 'unknown', provenance: 'fixture' }],
      }),
    ).toThrow('exact lowercase SHA-256');
    expect(() =>
      validatePublicExportPolicy({
        ...fixturePolicy(),
        excludePatterns: ['src/generated/**', 'src/test.wasm'],
      }),
    ).toThrow('permitted binary is excluded');
  });
});

describe('public source export', () => {
  test('writes deterministic incomplete snapshots without private history or blocked material', async () => {
    const fixture = await createFixture();
    const first = resolve(fixture.base, 'export-a');
    const second = resolve(fixture.base, 'export-b');
    const options = {
      repoRoot: fixture.repo,
      policyPath: fixture.policyPath,
      allowIncomplete: true,
      write: true,
    };

    const firstManifest = await exportPublicSource({ ...options, outputDirectory: first });
    const secondManifest = await exportPublicSource({ ...options, outputDirectory: second });

    expect(firstManifest).toEqual(secondManifest);
    expect(firstManifest.snapshot.publicationReady).toBe(false);
    expect(firstManifest.source.clean).toBe(false);
    expect(firstManifest.readinessFailures.map((failure) => failure.id)).toEqual([
      'SOURCE-DIRTY',
      'PX-TEST',
    ]);
    expect(firstManifest.omittedBlockers[0]?.paths).toEqual(['src/vendor/provenance.dll']);
    expect(await Bun.file(resolve(first, 'src/app.ts')).exists()).toBe(true);
    expect(await Bun.file(resolve(first, 'src/uncommitted.ts')).exists()).toBe(true);
    expect(
      Array.from(new Uint8Array(await Bun.file(resolve(first, 'src/test.wasm')).arrayBuffer())),
    ).toEqual(Array.from(fixture.binaryBytes));
    expect(await Bun.file(resolve(first, 'src/generated/output.js')).exists()).toBe(false);
    expect(await Bun.file(resolve(first, 'src/vendor/provenance.dll')).exists()).toBe(false);
    expect(await Bun.file(resolve(first, '.git/config')).exists()).toBe(false);
    expect(JSON.parse(await readFile(resolve(first, PUBLIC_EXPORT_MANIFEST), 'utf8'))).toEqual(
      firstManifest,
    );
    expect(await readFile(resolve(first, PUBLIC_EXPORT_MANIFEST), 'utf8')).toBe(
      await readFile(resolve(second, PUBLIC_EXPORT_MANIFEST), 'utf8'),
    );

    await expect(
      exportPublicSource({
        ...options,
        outputDirectory: undefined,
        allowIncomplete: false,
        write: false,
      }),
    ).rejects.toThrow('public export is not ready');
  });

  test('fails closed on a newly allowlisted binary even when its extension looks textual', async () => {
    const fixture = await createFixture();
    await writeFile(resolve(fixture.repo, 'src/unknown.ts'), new Uint8Array([0, 1, 2, 3]));
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        allowIncomplete: true,
        write: false,
      }),
    ).rejects.toThrow('unreviewed binary content is allowlisted: src/unknown.ts');
  });

  test('fails closed when a reviewed binary changes', async () => {
    const fixture = await createFixture();
    await writeFile(resolve(fixture.repo, 'src/test.wasm'), new Uint8Array([0, 97, 115, 110]));
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        allowIncomplete: true,
        write: false,
      }),
    ).rejects.toThrow('src/test.wasm differs from its reviewed binary digest');
  });

  test('fails closed when a reviewed binary disappears', async () => {
    const fixture = await createFixture();
    await rm(resolve(fixture.repo, 'src/test.wasm'));
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        allowIncomplete: true,
        write: false,
      }),
    ).rejects.toThrow('reviewed binary/media file is missing: src/test.wasm');
  });

  test('refuses a symlink instead of copying its target', async () => {
    const fixture = await createFixture();
    await symlink('/etc/hosts', resolve(fixture.repo, 'src/host-link.ts'));
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        allowIncomplete: true,
        write: false,
      }),
    ).rejects.toThrow('public export refuses symlinks: src/host-link.ts');
  });

  test('keeps output outside the source and refuses to overwrite a destination', async () => {
    const fixture = await createFixture();
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        outputDirectory: resolve(fixture.repo, 'export'),
        allowIncomplete: true,
        write: true,
      }),
    ).rejects.toThrow('destination must be outside');

    const output = resolve(fixture.base, 'already-exists');
    await mkdir(output);
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        outputDirectory: output,
        allowIncomplete: true,
        write: true,
      }),
    ).rejects.toThrow('destination already exists');

    const redirectedParent = resolve(fixture.base, 'redirected-parent');
    await symlink(fixture.repo, redirectedParent, 'dir');
    await expect(
      exportPublicSource({
        repoRoot: fixture.repo,
        policyPath: fixture.policyPath,
        outputDirectory: resolve(redirectedParent, 'export-through-symlink'),
        allowIncomplete: true,
        write: true,
      }),
    ).rejects.toThrow('destination must be outside');
  });
});
