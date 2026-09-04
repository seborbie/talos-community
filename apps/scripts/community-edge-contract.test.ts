import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  COMMUNITY_EDGE_ACME_DYNAMIC_PATH,
  COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH,
  COMMUNITY_EDGE_CUSTOM_DYNAMIC_PATH,
  COMMUNITY_EDGE_LOCAL_COMPOSE_PATH,
  COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH,
  checkCommunityEdgeContract,
  communityEdgeContractFailures,
  floatingLatestReleaseInputFailures,
} from './community-edge-contract';

const repoRoot = resolve(import.meta.dir, '..', '..');
const sources = {
  publicCompose: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH), 'utf8'),
  customCompose: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH), 'utf8'),
  localCompose: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_LOCAL_COMPOSE_PATH), 'utf8'),
  acmeDynamic: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_ACME_DYNAMIC_PATH), 'utf8'),
  customDynamic: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_CUSTOM_DYNAMIC_PATH), 'utf8'),
  baseCompose: readFileSync(resolve(repoRoot, 'infra/compose.community.yml'), 'utf8'),
  riskRegister: readFileSync(
    resolve(repoRoot, 'docs/architecture/dependency-risk-register.md'),
    'utf8',
  ),
  releaseInputs: [
    {
      path: COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH,
      contents: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH), 'utf8'),
    },
    {
      path: COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH,
      contents: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH), 'utf8'),
    },
    {
      path: COMMUNITY_EDGE_LOCAL_COMPOSE_PATH,
      contents: readFileSync(resolve(repoRoot, COMMUNITY_EDGE_LOCAL_COMPOSE_PATH), 'utf8'),
    },
  ],
};

describe('Community Traefik edge contract', () => {
  test('the tracked edge modes satisfy the contract', async () => {
    expect((await checkCommunityEdgeContract(repoRoot)).failures).toEqual([]);
  });

  test('rejects Docker authority and an insecure dashboard', () => {
    const unsafePublic = sources.publicCompose
      .replace('--api.insecure=false', '--api.insecure=true')
      .replace(
        '      - talos_traefik_acme:/acme',
        '      - talos_traefik_acme:/acme\n      - /var/run/docker.sock:/var/run/docker.sock',
      );

    const failures = communityEdgeContractFailures({ ...sources, publicCompose: unsafePublic });
    expect(failures).toContain('public ACME edge must not enable the Traefik API or dashboard');
    expect(failures).toContain(
      'public ACME edge must not grant Traefik container-engine discovery or a Docker socket',
    );
  });

  test('rejects broader API proxy trust and direct app publication', () => {
    const unsafePublic = sources.publicCompose.replace(
      'API_TRUSTED_PROXIES: "${TALOS_TRAEFIK_IPV4:-172.31.240.2}"',
      'API_TRUSTED_PROXIES: "uniquelocal"\n    ports:\n      - "0.0.0.0:3001:3001"',
    );

    const failures = communityEdgeContractFailures({ ...sources, publicCompose: unsafePublic });
    expect(failures).toContain(
      "public ACME edge must make the API trust only Traefik's exact private address",
    );
    expect(failures).toContain(
      'public ACME edge must not publish or mount anything into api_backend',
    );
  });

  test('rejects permissive encoded-delimiter handling', () => {
    const unsafePublic = sources.publicCompose.replace(
      '--entrypoints.websecure.http.encodedcharacters.allowencodedslash=false',
      '--entrypoints.websecure.http.encodedcharacters.allowencodedslash=true',
    );

    expect(communityEdgeContractFailures({ ...sources, publicCompose: unsafePublic })).toContain(
      'public ACME edge Traefik command is missing edge protection: --entrypoints.websecure.http.encodedcharacters.allowencodedslash=false',
    );
  });

  test('rejects catch-all SNI and loss of strict SNI checking', () => {
    const unsafeDynamic = sources.acmeDynamic
      .replace('HostSNI({{ env "TALOS_RELAY_DOMAIN" | quote }})', 'HostSNI(`*`)')
      .replace('sniStrict: true', 'sniStrict: false');

    const failures = communityEdgeContractFailures({ ...sources, acmeDynamic: unsafeDynamic });
    expect(failures).toContain(
      'public ACME dynamic config must not contain a catch-all host, SNI, or path router',
    );
    expect(failures).toContain(
      'public ACME dynamic config is missing edge protection: sniStrict: true',
    );
  });

  test('rejects ACME material in custom-certificate mode', () => {
    const unsafeCustom = sources.customCompose.replace(
      './traefik/dynamic-custom.yml:/etc/traefik/dynamic/talos.yml:ro',
      './traefik/dynamic-custom.yml:/etc/traefik/dynamic/talos.yml:ro\n      - talos_traefik_acme:/acme',
    );

    expect(communityEdgeContractFailures({ ...sources, customCompose: unsafeCustom })).toContain(
      'custom-certificate edge must not initialize or mount ACME state',
    );
  });

  test('rejects a project-derived ACME volume name that stopped-state backups cannot address', () => {
    const unsafePublic = sources.publicCompose.replace(
      '    name: talos-community_talos_traefik_acme',
      '    # stable name omitted',
    );

    expect(communityEdgeContractFailures({ ...sources, publicCompose: unsafePublic })).toContain(
      'public ACME edge must use the stable named volume talos-community_talos_traefik_acme',
    );
  });

  test('allows latest only for the reviewed Traefik edge input', () => {
    expect(
      floatingLatestReleaseInputFailures([
        {
          path: COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH,
          contents: 'image: "${TALOS_TRAEFIK_IMAGE:-traefik:latest}"',
        },
      ]),
    ).toEqual([]);
    expect(
      floatingLatestReleaseInputFailures([
        { path: 'infra/compose.example.yml', contents: 'image: nginx:latest' },
        { path: '.github/workflows/example.yml', contents: 'uses: vendor/action@latest' },
      ]),
    ).toEqual([
      'infra/compose.example.yml contains unreviewed floating release input: nginx:latest',
      '.github/workflows/example.yml contains unreviewed floating release input: @latest',
    ]);
  });
});
