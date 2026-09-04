import { describe, expect, test } from 'bun:test';
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
