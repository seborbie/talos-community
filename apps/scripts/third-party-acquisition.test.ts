import { afterEach, describe, expect, test } from 'bun:test';
import { cp, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import {
  acquireVpxEncode,
  loadThirdPartyAcquisitionPolicy,
  validateThirdPartyAcquisitionPolicy,
  verifyInstallerNotices,
  verifyPatchedVpxTree,
  type ThirdPartyAcquisitionPolicy,
} from './third-party-acquisition';

const repoRoot = resolve(import.meta.dir, '../..');
const policyPath = resolve(repoRoot, '.config/third-party-acquisition.json');
const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true })),
  );
});

async function policy(): Promise<ThirdPartyAcquisitionPolicy> {
  return loadThirdPartyAcquisitionPolicy(policyPath);
}

async function vpxFixture(): Promise<string> {
  const temporary = await mkdtemp(resolve(tmpdir(), 'talos-vpx-policy-test-'));
  temporaryDirectories.push(temporary);
  const output = resolve(temporary, 'vpx-encode');
  await cp(resolve(repoRoot, 'apps/vpx-encode'), output, {
    recursive: true,
    filter: (source) => !source.endsWith('.DS_Store'),
  });
  return output;
}

describe('third-party acquisition policy', () => {
  test('accepts the reviewed repository policy and exact current vpx patch result', async () => {
    const reviewed = await policy();
    expect(reviewed.vpxEncode.version).toBe('0.6.2');
    expect(reviewed.sevenZip.version).toBe('26.00');
    expect(reviewed.wix.version).toBe('6.0.0');
    await expect(
      verifyPatchedVpxTree(resolve(repoRoot, 'apps/vpx-encode'), reviewed),
    ).resolves.toBe(undefined);
    await expect(verifyInstallerNotices(repoRoot, reviewed)).resolves.toBe(undefined);
  });

  test('rejects floating downloads and unknown archive members', async () => {
    const reviewed = await policy();
    const floating = structuredClone(reviewed);
    floating.vpxEncode.archiveUrl = 'https://example.invalid/vpx-encode-latest.crate';
    expect(() => validateThirdPartyAcquisitionPolicy(floating)).toThrow(
      'must not use a floating latest URL',
    );

    const unknownArchive = structuredClone(reviewed);
    unknownArchive.sevenZip.members[0]!.archiveId = 'unreviewed';
    expect(() => validateThirdPartyAcquisitionPolicy(unknownArchive)).toThrow(
      'member refers to unknown archive',
    );
  });

  test('rejects changed or additional vpx source', async () => {
    const reviewed = await policy();
    const changed = await vpxFixture();
    await writeFile(resolve(changed, 'Cargo.toml'), '[package]\nname = "changed"\n');
    await expect(verifyPatchedVpxTree(changed, reviewed)).rejects.toThrow('SHA-256 mismatch');

    const additional = await vpxFixture();
    await writeFile(resolve(additional, 'build.rs'), 'fn main() {}\n');
    await expect(verifyPatchedVpxTree(additional, reviewed)).rejects.toThrow(
      'differs from reviewed file set',
    );
  });

  test('rejects a symlinked vpx source root', async () => {
    const reviewed = await policy();
    const temporary = await mkdtemp(resolve(tmpdir(), 'talos-vpx-symlink-test-'));
    temporaryDirectories.push(temporary);
    const link = resolve(temporary, 'vpx-encode');
    await symlink(resolve(repoRoot, 'apps/vpx-encode'), link);
    await expect(verifyPatchedVpxTree(link, reviewed)).rejects.toThrow(
      'root is not a regular directory',
    );
  });

  test('verifies patch and retained-notice bytes, not only policy syntax', async () => {
    const changedPatch = await policy();
    changedPatch.vpxEncode.patchSha256 = '0'.repeat(64);
    await expect(acquireVpxEncode({ repoRoot, policy: changedPatch })).rejects.toThrow(
      'vpx-encode patch SHA-256 mismatch',
    );

    const changedNotice = await policy();
    changedNotice.wix.retainedNotice.sha256 = '0'.repeat(64);
    await expect(verifyInstallerNotices(repoRoot, changedNotice)).rejects.toThrow(
      'retained notice apps/installer/third-party/wix-6.0.0/OSMFEULA.txt SHA-256 mismatch',
    );
  });
});

describe('source and installer build wiring', () => {
  test('each fresh release build prepares excluded Cargo inputs before using them', async () => {
    const workflow = Bun.YAML.parse(
      await readFile(
        resolve(repoRoot, '.github/workflows/community-release-candidate.yml'),
        'utf8',
      ),
    ) as { jobs: Record<string, { steps: Array<{ run?: string; uses?: string }> }> };
    for (const jobName of ['launcher-linux', 'launcher-windows', 'images']) {
      const steps = workflow.jobs[jobName]!.steps;
      const bunSetup = steps.findIndex((step) => step.uses?.startsWith('oven-sh/setup-bun@'));
      const acquire = steps.findIndex((step) =>
        step.run?.includes('bun run third-party:vpx:prepare'),
      );
      const build = steps.findIndex(
        (step) =>
          step.run?.includes('cargo build') || step.uses?.startsWith('docker/build-push-action@'),
      );
      expect(bunSetup).toBeGreaterThanOrEqual(0);
      expect(acquire).toBeGreaterThan(bunSetup);
      expect(build).toBeGreaterThan(acquire);
    }
    const validation = workflow.jobs.validate!.steps.map((step) => step.run ?? '').join('\n');
    expect(validation).toContain(
      'cargo install cargo-audit --version "${CARGO_AUDIT_VERSION}" --locked',
    );
    expect(validation).toContain('libwebkit2gtk-4.1-dev');
  });

  test('normal CI reconstructs excluded Cargo inputs before checks on a fresh checkout', async () => {
    const workflow = Bun.YAML.parse(
      await readFile(resolve(repoRoot, '.github/workflows/quality.yml'), 'utf8'),
    ) as { jobs: Record<string, { steps: Array<{ run?: string }> }> };
    for (const jobName of ['bun', 'rust']) {
      const commands = workflow.jobs[jobName]!.steps.flatMap((step) => step.run?.split('\n') ?? []);
      const install = commands.findIndex((command) => command.trim() === 'bun ci');
      const acquire = commands.findIndex(
        (command) => command.trim() === 'bun run third-party:vpx:prepare',
      );
      const firstGate = commands.findIndex((command) =>
        /bun run (check|test|contracts:check)(\s|$)/.test(command),
      );
      expect(install).toBeGreaterThanOrEqual(0);
      expect(acquire).toBeGreaterThan(install);
      expect(firstGate).toBeGreaterThan(acquire);
    }
  });

  test('uses the fail-closed acquisition path and local-only WiX restores', async () => {
    const buildScript = await readFile(resolve(repoRoot, 'scripts/build-installers.ps1'), 'utf8');
    const packageManifest = JSON.parse(
      await readFile(resolve(repoRoot, 'apps/package.json'), 'utf8'),
    ) as { scripts: Record<string, string> };
    const exportPolicy = JSON.parse(
      await readFile(resolve(repoRoot, '.config/public-export-policy.json'), 'utf8'),
    ) as {
      excludePatterns: string[];
      blockedOmissions: Array<{ id: string }>;
    };

    expect(packageManifest.scripts['third-party:vpx:prepare']).toContain(
      'third-party-acquisition.ts vpx',
    );
    expect(packageManifest.scripts.setup).toContain('third-party:vpx:prepare');
    expect(buildScript).toContain('third-party-acquisition.ts');
    expect(buildScript).toContain('$pinnedInstallerInputs.SevenZipExecutable');
    expect(buildScript).toContain('$pinnedInstallerInputs.SfxSha256');
    const restores = buildScript.match(/dotnet (?:tool )?restore[^\r\n]*/g) ?? [];
    expect(restores.length).toBe(5);
    expect(restores.every((line) => line.includes('--configfile'))).toBe(true);
    expect(exportPolicy.excludePatterns).toContain('apps/vpx-encode/**');
    expect(exportPolicy.excludePatterns).toContain('apps/installer/.wix/**');
    expect(exportPolicy.blockedOmissions.map((entry) => entry.id)).not.toContain('PX-001');
    expect(exportPolicy.blockedOmissions.map((entry) => entry.id)).not.toContain('PX-002');
  });
});
