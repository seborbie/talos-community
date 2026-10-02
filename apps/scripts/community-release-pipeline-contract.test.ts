import { describe, expect, test } from 'bun:test';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import {
  checkCommunityReleasePipelineContract,
  communityReleasePipelineFailures,
  emptyLegacySigningStateIsAllowlisted,
  gitleaksReleaseConfigurationFailures,
} from './community-release-pipeline-contract';

async function trackedSources() {
  const repoRoot = resolve(import.meta.dir, '../..');
  const [candidateWorkflow, publishWorkflow, promotionWorkflow, gitleaksConfiguration] =
    await Promise.all([
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-candidate.yml')).text(),
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-publish.yml')).text(),
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-promote.yml')).text(),
      Bun.file(resolve(repoRoot, '.gitleaks.toml')).text(),
    ]);
  return { candidateWorkflow, publishWorkflow, promotionWorkflow, gitleaksConfiguration };
}

describe('Community release pipeline contract', () => {
  test('the tracked manual candidate and publication workflows satisfy the contract', async () => {
    expect((await checkCommunityReleasePipelineContract()).failures).toEqual([]);
  });

  test('rejects a raw source archive or an incomplete public-export bypass', async () => {
    const sources = await trackedSources();
    const failures = communityReleasePipelineFailures({
      ...sources,
      candidateWorkflow:
        sources.candidateWorkflow.replace(
          'bun ./scripts/public-source-export.ts --repo-root .. --output "${exported}"',
          'bun ./scripts/public-source-export.ts --repo-root .. --output "${exported}" --allow-incomplete',
        ) + '\n# former bypass\ngit archive HEAD > source.tar\n',
    });

    expect(failures).toContain(
      'candidate workflow must never bypass unresolved public export gates',
    );
    expect(failures).toContain(
      'candidate workflow must not archive the raw checkout around public export policy',
    );
  });

  test('rejects signing secrets outside the single protected signer step', async () => {
    const sources = await trackedSources();
    const failures = communityReleasePipelineFailures({
      ...sources,
      candidateWorkflow: sources.candidateWorkflow.replace(
        '    steps:\n      - uses: actions/checkout@',
        '    env:\n      EXPOSED_PFX: ${{ secrets.TALOS_MANIFEST_SIGNING_PFX_BASE64 }}\n    steps:\n      - uses: actions/checkout@',
      ),
    });

    expect(failures).toContain(
      'signing secret expression must occur only in the protected signer step: ${{ secrets.TALOS_MANIFEST_SIGNING_PFX_BASE64 }}',
    );
    expect(failures).toContain(
      'manifest signing secret names must not escape the protected signer step',
    );
  });

  test('requires exact protected release-line key continuity before artifact handoff', async () => {
    const sources = await trackedSources();
    const failures = communityReleasePipelineFailures({
      ...sources,
      candidateWorkflow: sources.candidateWorkflow.replace(
        '$observedManifestKeySha256 -cne $expectedManifestKeySha256',
        '$false',
      ),
    });

    expect(failures).toContain(
      'protected signer step is missing key-continuity enforcement: $observedManifestKeySha256 -cne $expectedManifestKeySha256',
    );
  });

  test('reconstructs reviewed vpx source before running release quality gates', async () => {
    const sources = await trackedSources();
    const failures = communityReleasePipelineFailures({
      ...sources,
      candidateWorkflow: sources.candidateWorkflow.replace(
        'bun ci\n          bun run third-party:vpx:prepare\n          bun run quality',
        'bun ci\n          bun run quality',
      ),
    });

    expect(failures).toContain(
      'candidate workflow must reconstruct reviewed vpx source after install and before quality gates',
    );
  });

  test('rejects credentialed public-read evidence and removal of the startup gate', async () => {
    const sources = await trackedSources();
    const failures = communityReleasePipelineFailures({
      ...sources,
      publishWorkflow: sources.publishWorkflow
        .replace(
          'docker run --rm "${SKOPEO_IMAGE}" inspect --raw',
          'docker run --rm --volume "${HOME}/.docker/config.json:/auth.json" "${SKOPEO_IMAGE}" inspect --raw --authfile /auth.json',
        )
        .replace('needs: [validate, bundle]', 'needs: validate'),
    });
    expect(failures).toContain(
      'publication must verify the exact public digest without registry credentials',
    );
    expect(failures).toContain(
      'publication must gate success on the verified released-bundle Linux startup smoke',
    );
  });
});

describe('exact synthetic Gitleaks allowlist', () => {
  test('allows only the empty two-line state at the reviewed path', async () => {
    const { gitleaksConfiguration } = await trackedSources();
    expect(gitleaksReleaseConfigurationFailures(gitleaksConfiguration)).toEqual([]);
    expect(
      emptyLegacySigningStateIsAllowlisted(
        gitleaksConfiguration,
        'scripts/build-linux-agent.sh',
        'SIGNING_PRIVATE_KEY_PATH=""\nSIGNING_PRIVATE_KEY_IS_TEMP=0',
      ),
    ).toBe(true);
    expect(
      emptyLegacySigningStateIsAllowlisted(
        gitleaksConfiguration,
        'scripts/build-linux-agent.sh',
        'SIGNING_PRIVATE_KEY_PATH="/tmp/real-signing-key.pem"\nSIGNING_PRIVATE_KEY_IS_TEMP=0',
      ),
    ).toBe(false);
    expect(
      emptyLegacySigningStateIsAllowlisted(
        gitleaksConfiguration,
        'scripts/another-file.sh',
        'SIGNING_PRIVATE_KEY_PATH=""\nSIGNING_PRIVATE_KEY_IS_TEMP=0',
      ),
    ).toBe(false);
  });
});

// Medium tests execute the actual workflow's JSON checks rather than reproducing them in TS.
test.each([
  [{ distribution: 'controller' }, true],
  [{ distribution: 'full' }, true],
  [{ distribution: 'other' }, false],
  [{ distribution: null }, false],
  [{ distribution: 'controller', sourceSha: 'b'.repeat(40) }, false],
  [{ distribution: 'controller', releaseVersion: '9.9.9' }, false],
  [{ distribution: 'controller', releaseTag: 'community-v9.9.9' }, false],
  [{ distribution: 'controller', schemaVersion: 2 }, false],
] as const)('publication rejects mismatched candidate identity %j', async (override, accepted) => {
  const fixture = await mkdtemp(resolve(tmpdir(), 'talos-release-identity-'));
  try {
    await writeFile(
      resolve(fixture, 'release-identity.json'),
      JSON.stringify({
        schemaVersion: 1,
        releaseVersion: '1.2.3',
        releaseTag: 'community-v1.2.3',
        sourceSha: 'a'.repeat(40),
        ...override,
      }),
    );
    const sources = await trackedSources();
    const workflow = Bun.YAML.parse(sources.publishWorkflow) as {
      jobs: { validate: { steps: { name?: string; run?: string }[] } };
    };
    const run = workflow.jobs.validate.steps.find(
      (step) => step.name === 'Verify distribution identity',
    )?.run;
    if (!run) throw new Error('distribution verification step missing');
    const localChecks = run
      .slice(run.indexOf('jq -e'))
      .replaceAll('${{ steps.identity.outputs.version }}', '1.2.3')
      .replaceAll('${{ steps.identity.outputs.source_sha }}', 'a'.repeat(40));
    const result = Bun.spawnSync(['bash', '-c', `set -euo pipefail\n${localChecks}`], {
      env: {
        ...process.env,
        identity: fixture,
        RELEASE_TAG: 'community-v1.2.3',
        GITHUB_OUTPUT: resolve(fixture, 'output'),
      },
    });
    expect(result.exitCode === 0).toBe(accepted);
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});

test.each([
  ['controller', null, true],
  ['controller', 'a'.repeat(64), false],
  ['full', 'a'.repeat(64), true],
  ['full', null, false],
  [null, null, false],
  ['unknown', 'a'.repeat(64), false],
] as const)(
  'promotion enforces signing requirements for %s',
  async (distribution, key, accepted) => {
    const { promotionWorkflow } = await trackedSources();
    const start = promotionWorkflow.indexOf('          fingerprint="$(jq -r');
    const end = promotionWorkflow.indexOf('          notes=', start);
    expect(start).toBeGreaterThan(0);
    expect(end).toBeGreaterThan(start);
    const result = Bun.spawnSync(
      ['bash', '-c', `set -euo pipefail\n${promotionWorkflow.slice(start, end)}`],
      {
        env: {
          ...process.env,
          manifest: JSON.stringify({
            distribution,
            signing: { updaterManifestPublicKeySha256: key },
          }),
        },
      },
    );
    expect(result.exitCode === 0).toBe(accepted);
  },
);

test.each([true, false])(
  'promotion selects the exact Linux bundle instead of a source archive (present: %s)',
  async (includeBundle) => {
    const fixture = await mkdtemp(resolve(tmpdir(), 'talos-promotion-archive-'));
    try {
      const root = resolve(fixture, 'talos-community-1.2.3');
      await mkdir(root);
      await writeFile(resolve(root, 'release-manifest.json'), '{"distribution":"controller"}');
      // A source archive can appear first; it must never be selected as the release bundle.
      await writeFile(resolve(fixture, 'talos-community-1.2.3-source.tar.gz'), 'source fixture');
      if (includeBundle) {
        const tar = Bun.spawnSync([
          'tar',
          '-czf',
          resolve(fixture, 'talos-community-1.2.3-linux-x86_64.tar.gz'),
          '-C',
          fixture,
          'talos-community-1.2.3',
        ]);
        expect(tar.exitCode).toBe(0);
      }
      const { promotionWorkflow } = await trackedSources();
      const start = promotionWorkflow.indexOf('          archive=');
      const end = promotionWorkflow.indexOf('          fingerprint=', start);
      expect(start).toBeGreaterThan(0);
      expect(end).toBeGreaterThan(start);
      const checks = promotionWorkflow
        .slice(start, end)
        .replaceAll('${{ needs.validate.outputs.version }}', '1.2.3');
      // Git Bash's GNU tar treats a drive-colon path as a remote archive. The workflow runs on
      // Linux; translate only this Windows fixture to MSYS's local drive path for the real check.
      const bashFixture =
        process.platform === 'win32'
          ? fixture
              .replaceAll('\\', '/')
              .replace(/^([a-z]):/i, (_, drive: string) => `/${drive.toLowerCase()}`)
          : fixture;
      const result = Bun.spawnSync(['bash', '-c', `set -euo pipefail\n${checks}`], {
        env: { ...process.env, release: bashFixture },
      });
      if (includeBundle && result.exitCode !== 0) {
        throw new Error(`Archive selection failed: ${new TextDecoder().decode(result.stderr)}`);
      }
      expect(result.exitCode === 0).toBe(includeBundle);
    } finally {
      await rm(fixture, { recursive: true, force: true });
    }
  },
);
