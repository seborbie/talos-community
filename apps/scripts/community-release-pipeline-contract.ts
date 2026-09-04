import { resolve } from 'node:path';

export const PINNED_GITLEAKS_IMAGE =
  'ghcr.io/gitleaks/gitleaks@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f';

type GitleaksAllowlist = {
  description?: unknown;
  targetRules?: unknown;
  condition?: unknown;
  regexTarget?: unknown;
  paths?: unknown;
  regexes?: unknown;
};

type GitleaksConfiguration = {
  minVersion?: unknown;
  allowlists?: unknown;
};

export type CommunityReleasePipelineSources = {
  candidateWorkflow: string;
  publishWorkflow: string;
  promotionWorkflow: string;
  gitleaksConfiguration: string;
};

const SIGNER_STEP_NAME = 'Build clients with protected updater-manifest signer';
const PFX_SECRET = '${{ secrets.TALOS_MANIFEST_SIGNING_PFX_BASE64 }}';
const PASSWORD_SECRET = '${{ secrets.TALOS_MANIFEST_SIGNING_PFX_PASSWORD }}';
const EXPECTED_FINGERPRINT_VARIABLE = '${{ vars.TALOS_EXPECTED_MANIFEST_KEY_SHA256 }}';
const EMPTY_SIGNING_ALLOWLIST_DESCRIPTION =
  'Empty legacy macOS signing-key state, never a key value';
const EMPTY_SIGNING_PATH_PATTERN = '(^|/)scripts/build-linux-agent\\.sh$';
const EMPTY_SIGNING_MATCH_PATTERN = '^SIGNING_PRIVATE_KEY_PATH=""\\nSIGNING_PRIVATE_KEY_IS_TEMP=0$';

function count(contents: string, needle: string): number {
  return contents.split(needle).length - 1;
}

function requireSnippets(contents: string, snippets: readonly string[], label: string): string[] {
  return snippets
    .filter((snippet) => !contents.includes(snippet))
    .map((snippet) => `${label} is missing required release protection: ${snippet}`);
}

function manualOnlyFailures(contents: string, label: string): string[] {
  const failures: string[] = [];
  if (!/^on:\s*\n  workflow_dispatch:/m.test(contents)) {
    failures.push(`${label} must be manually dispatched`);
  }
  for (const trigger of ['pull_request', 'push', 'schedule', 'workflow_run']) {
    if (new RegExp(`^  ${trigger}:`, 'm').test(contents)) {
      failures.push(`${label} must not run from ${trigger}`);
    }
  }
  return failures;
}

function pinnedActionFailures(contents: string, label: string): string[] {
  const failures: string[] = [];
  const uses = [...contents.matchAll(/^\s*- uses:\s+([^\s#]+).*$/gm)].map(
    (match) => match[1] ?? '',
  );
  if (uses.length === 0) failures.push(`${label} must use reviewed actions`);
  for (const action of uses) {
    const separator = action.lastIndexOf('@');
    const revision = separator >= 0 ? action.slice(separator + 1) : '';
    if (!/^[0-9a-f]{40}$/.test(revision)) {
      failures.push(`${label} action is not pinned to a lowercase commit SHA: ${action}`);
    }
  }
  return failures;
}

function stepBody(contents: string, stepName: string): string | undefined {
  const startMarker = `      - name: ${stepName}`;
  const start = contents.indexOf(startMarker);
  if (start < 0) return undefined;
  const remaining = contents.slice(start + startMarker.length);
  const next = remaining.search(/^      - /m);
  return next < 0
    ? contents.slice(start)
    : contents.slice(start, start + startMarker.length + next);
}

function jobBody(contents: string, jobName: string): string | undefined {
  const startMarker = `\n  ${jobName}:\n`;
  const start = contents.indexOf(startMarker);
  if (start < 0) return undefined;
  const remaining = contents.slice(start + startMarker.length);
  const next = remaining.search(/^  [a-zA-Z0-9_-]+:\s*$/m);
  return next < 0
    ? contents.slice(start)
    : contents.slice(start, start + startMarker.length + next);
}

function parseGitleaksConfiguration(contents: string): GitleaksConfiguration | undefined {
  try {
    return Bun.TOML.parse(contents) as GitleaksConfiguration;
  } catch {
    return undefined;
  }
}

function emptySigningAllowlist(contents: string): GitleaksAllowlist | undefined {
  const configuration = parseGitleaksConfiguration(contents);
  if (!configuration || !Array.isArray(configuration.allowlists)) return undefined;
  return (configuration.allowlists as GitleaksAllowlist[]).find(
    (allowlist) => allowlist.description === EMPTY_SIGNING_ALLOWLIST_DESCRIPTION,
  );
}

export function emptyLegacySigningStateIsAllowlisted(
  gitleaksConfiguration: string,
  path: string,
  match: string,
): boolean {
  const allowlist = emptySigningAllowlist(gitleaksConfiguration);
  if (!allowlist || !Array.isArray(allowlist.paths) || !Array.isArray(allowlist.regexes)) {
    return false;
  }
  return (
    allowlist.condition === 'AND' &&
    allowlist.regexTarget === 'match' &&
    allowlist.paths.some(
      (pattern) => typeof pattern === 'string' && new RegExp(pattern).test(path),
    ) &&
    allowlist.regexes.some(
      (pattern) => typeof pattern === 'string' && new RegExp(pattern).test(match),
    )
  );
}

export function gitleaksReleaseConfigurationFailures(contents: string): string[] {
  const failures: string[] = [];
  const configuration = parseGitleaksConfiguration(contents);
  if (!configuration) return ['.gitleaks.toml must be valid TOML'];
  if (configuration.minVersion !== 'v8.30.1') {
    failures.push('.gitleaks.toml must require Gitleaks v8.30.1');
  }
  if (!Array.isArray(configuration.allowlists)) {
    return [...failures, '.gitleaks.toml must contain reviewed allowlists'];
  }
  for (const allowlist of configuration.allowlists as GitleaksAllowlist[]) {
    if (
      allowlist.condition !== 'AND' ||
      !Array.isArray(allowlist.targetRules) ||
      allowlist.targetRules.length === 0 ||
      !Array.isArray(allowlist.paths) ||
      allowlist.paths.length === 0 ||
      !Array.isArray(allowlist.regexes) ||
      allowlist.regexes.length === 0
    ) {
      failures.push(
        `.gitleaks.toml allowlist is not constrained by rule, path, and exact value: ${String(allowlist.description)}`,
      );
    }
  }

  const exact = (configuration.allowlists as GitleaksAllowlist[]).filter(
    (allowlist) => allowlist.description === EMPTY_SIGNING_ALLOWLIST_DESCRIPTION,
  );
  if (exact.length !== 1) {
    failures.push('.gitleaks.toml must contain exactly one empty legacy signing-state allowlist');
  } else {
    const allowlist = exact[0];
    if (
      JSON.stringify(allowlist?.targetRules) !== JSON.stringify(['generic-api-key']) ||
      allowlist?.condition !== 'AND' ||
      allowlist.regexTarget !== 'match' ||
      JSON.stringify(allowlist.paths) !== JSON.stringify([EMPTY_SIGNING_PATH_PATTERN]) ||
      JSON.stringify(allowlist.regexes) !== JSON.stringify([EMPTY_SIGNING_MATCH_PATTERN])
    ) {
      failures.push(
        'the empty legacy signing-state allowlist must retain its exact reviewed scope',
      );
    }
  }

  const emptyState = 'SIGNING_PRIVATE_KEY_PATH=""\nSIGNING_PRIVATE_KEY_IS_TEMP=0';
  const nonemptyState =
    'SIGNING_PRIVATE_KEY_PATH="/tmp/actual-signing-key.pem"\nSIGNING_PRIVATE_KEY_IS_TEMP=0';
  if (!emptyLegacySigningStateIsAllowlisted(contents, 'scripts/build-linux-agent.sh', emptyState)) {
    failures.push('the exact empty legacy signing state must be recognized as synthetic');
  }
  if (
    emptyLegacySigningStateIsAllowlisted(contents, 'scripts/build-linux-agent.sh', nonemptyState)
  ) {
    failures.push('a nonempty signing-key path must never be allowlisted');
  }
  return failures;
}

export function communityReleasePipelineFailures(
  sources: CommunityReleasePipelineSources,
): string[] {
  const {
    candidateWorkflow: candidate,
    publishWorkflow: publish,
    promotionWorkflow: promotion,
  } = sources;
  const failures = [
    ...manualOnlyFailures(candidate, 'candidate workflow'),
    ...manualOnlyFailures(publish, 'publication workflow'),
    ...manualOnlyFailures(promotion, 'promotion workflow'),
    ...pinnedActionFailures(candidate, 'candidate workflow'),
    ...pinnedActionFailures(publish, 'publication workflow'),
    ...pinnedActionFailures(promotion, 'promotion workflow'),
    ...gitleaksReleaseConfigurationFailures(sources.gitleaksConfiguration),
  ];

  for (const [label, contents] of [
    ['candidate workflow', candidate],
    ['publication workflow', publish],
    ['promotion workflow', promotion],
  ] as const) {
    if (!contents.includes(PINNED_GITLEAKS_IMAGE)) {
      failures.push(`${label} must use the reviewed digest-pinned Gitleaks v8.30.1 image`);
    }
    if (/(?:^|[\s"'])[^@\s"']+:latest(?:[\s"']|$)/m.test(contents)) {
      failures.push(`${label} must not use a floating latest release tool or image`);
    }
  }

  failures.push(
    ...requireSnippets(
      candidate,
      [
        'bun ./scripts/public-source-export.ts --repo-root .. --output "${exported}"',
        'git -C "${exported}" init -q -b community-source',
        '"${GITLEAKS_IMAGE}" git --config /repo/.gitleaks.toml',
        '"${GITLEAKS_IMAGE}" dir --config /repo/.gitleaks.toml',
        'bun ci',
        'bun run third-party:vpx:prepare',
        'bun run quality',
        'cargo build --locked --release -p talos_appliance',
        'outputs: type=oci',
        'platforms: linux/amd64,linux/arm64',
        'provenance: mode=max',
        'sbom: true',
        'push: false',
        'environment: community-manifest-signing',
        'runs-on: [self-hosted, windows, x64, talos-release]',
        'UNSIGNED-BINARIES.txt',
        'build-provenance.json',
        'updaterManifestPublicKeySha256',
        'windowsAuthenticodeStatus',
        'Remove-Item $pfx -Force -ErrorAction SilentlyContinue',
        'Scan protected native artifact handoff for secrets',
      ],
      'candidate workflow',
    ),
  );
  if (/public-source-export\.ts[^\n]*--allow-incomplete/.test(candidate)) {
    failures.push('candidate workflow must never bypass unresolved public export gates');
  }
  if (/\bgit\s+archive\b/.test(candidate)) {
    failures.push(
      'candidate workflow must not archive the raw checkout around public export policy',
    );
  }
  if (/^\s+packages:\s+write\s*$/m.test(candidate)) {
    failures.push('candidate workflow must not have registry write permission');
  }
  if (
    !candidate.includes(
      'bun ci\n          bun run third-party:vpx:prepare\n          bun run quality',
    )
  ) {
    failures.push(
      'candidate workflow must reconstruct reviewed vpx source after install and before quality gates',
    );
  }

  const signerStep = stepBody(candidate, SIGNER_STEP_NAME);
  if (!signerStep) {
    failures.push(`candidate workflow is missing protected step: ${SIGNER_STEP_NAME}`);
  } else {
    for (const secret of [PFX_SECRET, PASSWORD_SECRET]) {
      if (count(candidate, secret) !== 1 || !signerStep.includes(secret)) {
        failures.push(
          `signing secret expression must occur only in the protected signer step: ${secret}`,
        );
      }
    }
    if (
      count(candidate, EXPECTED_FINGERPRINT_VARIABLE) !== 1 ||
      !signerStep.includes(EXPECTED_FINGERPRINT_VARIABLE)
    ) {
      failures.push(
        `expected manifest-key fingerprint variable must occur only in the protected signer step: ${EXPECTED_FINGERPRINT_VARIABLE}`,
      );
    }
    for (const snippet of [
      "$expectedManifestKeySha256 -cnotmatch '^[0-9a-f]{64}$'",
      '$observedManifestKeySha256 -cne $expectedManifestKeySha256',
      '[string]$artifactManifest.signing.updaterManifests.publicKeySha256 -cne $expectedManifestKeySha256',
      'Built updater-manifest public key does not match the protected expected release-line fingerprint.',
    ]) {
      if (!signerStep.includes(snippet)) {
        failures.push(`protected signer step is missing key-continuity enforcement: ${snippet}`);
      }
    }
    const prefix = candidate.slice(0, candidate.indexOf(signerStep));
    const suffix = candidate.slice(candidate.indexOf(signerStep) + signerStep.length);
    if (/TALOS_MANIFEST_SIGNING_PFX_(?:BASE64|PASSWORD)/.test(`${prefix}\n${suffix}`)) {
      failures.push('manifest signing secret names must not escape the protected signer step');
    }
    if (/TALOS_EXPECTED_MANIFEST_KEY_SHA256/.test(`${prefix}\n${suffix}`)) {
      failures.push('expected manifest-key fingerprint must not escape the protected signer step');
    }
  }

  failures.push(
    ...requireSnippets(
      publish,
      [
        'environment: community-release-publish',
        'test "${CONFIRM_REGISTRY_WRITE}" = "${RELEASE_TAG}"',
        'Community release candidate',
        'run-id: ${{ inputs.candidate_run_id }}',
        'github-token: ${{ github.token }}',
        'sha256sum --check SHA256SUMS',
        '"${SKOPEO_IMAGE}" copy --all --preserve-digests',
        'refusing to overwrite existing release tag ${tag}',
        'could not establish whether ${tag} already exists',
        'test "${source_digest}" = "${published_digest}"',
        'immutable_reference="ghcr.io/${owner}/${{ matrix.image }}@${published_digest}"',
        '--arg reference "${immutable_reference}"',
        'reference:$reference,digest:$digest',
        'bun ./scripts/community-release-bundle.ts',
        'launcher-windows.spdx.json',
        'native-clients.spdx.json',
        'RELEASE_NOTES.md',
        'actions/attest-build-provenance@',
        'Scan candidate workflow logs',
      ],
      'publication workflow',
    ),
  );
  for (const key of ['api_backend', 'frontend', 'relay', 'control_server']) {
    if (count(publish, `key: ${key}`) !== 1) {
      failures.push(`publication workflow must publish exactly one ${key} image`);
    }
  }
  const publishImages = jobBody(publish, 'publish-images');
  if (!publishImages || !/^\s+packages:\s+write\s*$/m.test(publishImages)) {
    failures.push('only the protected image publication job must receive packages: write');
  }
  if (count(publish, 'packages: write') !== 1) {
    failures.push('packages: write must occur exactly once in the publication workflow');
  }
  if (/^\s+contents:\s+write\s*$/m.test(publish)) {
    failures.push('image/bundle publication workflow must not create a GitHub release');
  }
  if (/\bgh\s+release\s+create\b/.test(publish)) {
    failures.push('image/bundle publication must not bypass separate prerelease approval');
  }

  failures.push(
    ...requireSnippets(
      promotion,
      [
        'environment: community-release-promotion',
        'test "${CONFIRM_PRERELEASE}" = "PRERELEASE ${RELEASE_TAG}"',
        'TEST_EVIDENCE_URL',
        'TEST_EVIDENCE_SHA256',
        '[[ "${TEST_EVIDENCE_SHA256}" =~ ^[0-9a-f]{64}$ ]]',
        'Community release image and bundle publication',
        'run-id: ${{ inputs.publication_run_id }}',
        'sha256sum --check SHA256SUMS',
        'gh attestation verify "${artifact}"',
        'publication-logs',
        '--max-archive-depth 8 --redact --no-banner --no-color /release',
        'gh release create "${RELEASE_TAG}"',
        '--prerelease --verify-tag',
        'a GitHub release already exists',
      ],
      'promotion workflow',
    ),
  );
  const promotionJob = jobBody(promotion, 'promote');
  if (!promotionJob || !/^\s+contents:\s+write\s*$/m.test(promotionJob)) {
    failures.push('only the protected prerelease promotion job must receive contents: write');
  }
  if (count(promotion, 'contents: write') !== 1) {
    failures.push('contents: write must occur exactly once in the promotion workflow');
  }
  if (/^\s+packages:\s+write\s*$/m.test(promotion)) {
    failures.push('prerelease promotion must not have registry write permission');
  }
  if (count(promotion, 'gh release create "${RELEASE_TAG}"') !== 1) {
    failures.push('promotion must contain exactly one explicit GitHub prerelease creation');
  }

  return failures;
}

export async function checkCommunityReleasePipelineContract(
  repoRoot = resolve(import.meta.dir, '../..'),
): Promise<{ failures: string[] }> {
  const [candidateWorkflow, publishWorkflow, promotionWorkflow, gitleaksConfiguration] =
    await Promise.all([
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-candidate.yml')).text(),
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-publish.yml')).text(),
      Bun.file(resolve(repoRoot, '.github/workflows/community-release-promote.yml')).text(),
      Bun.file(resolve(repoRoot, '.gitleaks.toml')).text(),
    ]);
  return {
    failures: communityReleasePipelineFailures({
      candidateWorkflow,
      publishWorkflow,
      promotionWorkflow,
      gitleaksConfiguration,
    }),
  };
}

if (import.meta.main) {
  const result = await checkCommunityReleasePipelineContract();
  if (result.failures.length > 0) {
    console.error('Community release pipeline contract failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log('Community release pipeline contract passed.');
}
