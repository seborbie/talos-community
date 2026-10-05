import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { copyFile, mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';

const repoRoot = resolve(import.meta.dir, '../..');
type ValidationStep = { name?: string; run?: string; 'working-directory'?: string };

// Keep fixture creation/cleanup in separately bounded hooks: Windows Git/Bash startup
// made the combined setup + validation exceed the default five-second test budget.
// Each hook and the actual validation retain Bun's default timeout; no retries or skips.
// Medium test: execute the actual workflow's local validation in Bash and a disposable Git repo.
describe.each([
  ['community-v1.2.3', true, 'tag'],
  ['community-v1.2.3-rc.1', true, 'tag'],
  ['community-v01.2.3', false, 'tag'],
  ['community-v1.2.3+metadata', false, 'tag'],
  ['community-v1.2.3', false, 'commit'],
] as const)(
  'promotion validates %s (expected success: %s, object: %s)',
  (tag, accepted, objectType) => {
    let fixture: string | undefined;
    beforeEach(async () => {
      fixture = await mkdtemp(resolve(tmpdir(), 'talos-promotion-'));
      const scripts = resolve(fixture, 'apps/scripts');
      await mkdir(scripts, { recursive: true });
      await copyFile(
        resolve(repoRoot, 'apps/scripts/community-release-version.ts'),
        resolve(scripts, 'community-release-version.ts'),
      );
      const git = (...args: string[]) => {
        const result = Bun.spawnSync(
          ['git', '-c', 'user.name=Talos test', '-c', 'user.email=test@example.test', ...args],
          { cwd: fixture },
        );
        expect(result.exitCode).toBe(0);
      };
      git('init', '-q');
      git('commit', '--allow-empty', '-qm', 'Test release');
      if (objectType === 'tag') git('tag', '-a', tag, '-m', 'Test tag');
      else git('tag', tag);
    });
    afterEach(async () => {
      if (fixture) await rm(fixture, { recursive: true, force: true });
      fixture = undefined;
    });

    test('executes local workflow validation with an isolated Git fixture', async () => {
      if (!fixture) throw new Error('promotion fixture is missing');
      const workflow = Bun.YAML.parse(
        await readFile(
          resolve(repoRoot, '.github/workflows/community-release-promote.yml'),
          'utf8',
        ),
      ) as {
        jobs: { validate: { steps: ValidationStep[] } };
      };
      const step = workflow.jobs.validate.steps.find(
        (entry) =>
          entry.name === 'Validate explicit prerelease authorization and reviewed evidence',
      );
      if (!step?.run) throw new Error('promotion validation step is missing');
      const remoteCheck = step.run.indexOf('run_json=');
      expect(remoteCheck).toBeGreaterThan(0);
      const output = resolve(fixture, 'github-output');
      const result = Bun.spawnSync(['bash', '-c', step.run.slice(0, remoteCheck)], {
        cwd: resolve(fixture, step['working-directory'] ?? '.'),
        env: {
          ...process.env,
          RELEASE_TAG: tag,
          CONFIRM_PRERELEASE: `PRERELEASE ${tag}`,
          TEST_EVIDENCE_URL: 'https://example.test/evidence',
          TEST_EVIDENCE_SHA256: 'a'.repeat(64),
          GITHUB_OUTPUT: output,
        },
      });
      expect(result.exitCode === 0, result.stderr.toString()).toBe(accepted);
      if (accepted) {
        expect(await readFile(output, 'utf8')).toContain(
          `version=${tag.slice('community-v'.length)}\n`,
        );
      } else {
        expect(result.stderr.toString()).toContain(
          objectType === 'commit' ? 'require an annotated tag' : 'release tag must match',
        );
        expect(await Bun.file(output).exists()).toBe(false);
      }
    });
  },
);
