#!/usr/bin/env bun

import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

export const COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH = 'infra/compose.community-traefik.yml';
export const COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH = 'infra/compose.community-traefik-custom.yml';
export const COMMUNITY_EDGE_LOCAL_COMPOSE_PATH = 'infra/compose.community-traefik-local.yml';
export const COMMUNITY_EDGE_ACME_DYNAMIC_PATH = 'infra/traefik/dynamic-acme.yml';
export const COMMUNITY_EDGE_CUSTOM_DYNAMIC_PATH = 'infra/traefik/dynamic-custom.yml';

const TRAEFIK_IMAGE_EXPRESSION = '${TALOS_TRAEFIK_IMAGE:-traefik:latest}';
const TRAEFIK_ADDRESS_EXPRESSION = '${TALOS_TRAEFIK_IPV4:-172.31.240.2}';
const EDGE_SUBNET_EXPRESSION = '${TALOS_EDGE_SUBNET:-172.31.240.0/24}';
const ACME_VOLUME_NAME = 'talos-community_talos_traefik_acme';
const REJECTED_ENCODED_CHARACTER_FLAGS = ['web', 'websecure'].flatMap((entrypoint) =>
  ['slash', 'backslash', 'nullcharacter', 'semicolon', 'percent', 'questionmark', 'hash'].map(
    (character) =>
      `--entrypoints.${entrypoint}.http.encodedcharacters.allowencoded${character}=false`,
  ),
);

const REVIEWED_FLOATING_LATEST_PATHS = new Set([
  COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH,
  COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH,
  COMMUNITY_EDGE_LOCAL_COMPOSE_PATH,
]);

type ComposeService = {
  image?: unknown;
  pull_policy?: unknown;
  build?: unknown;
  command?: unknown;
  environment?: unknown;
  ports?: unknown;
  volumes?: unknown;
  networks?: unknown;
  privileged?: unknown;
  container_name?: unknown;
  read_only?: unknown;
  cap_drop?: unknown;
  cap_add?: unknown;
  security_opt?: unknown;
  healthcheck?: unknown;
};

type ComposeDocument = {
  services?: Record<string, ComposeService>;
  networks?: Record<string, Record<string, unknown> | null>;
  volumes?: Record<string, unknown>;
};

export type ReleaseInputSource = Readonly<{
  path: string;
  contents: string;
}>;

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === 'string');
}

function parseCompose(source: string, label: string, failures: string[]): ComposeDocument {
  try {
    const parsed = Bun.YAML.parse(source);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      failures.push(`${label} must contain one Compose mapping`);
      return {};
    }
    return parsed as ComposeDocument;
  } catch {
    failures.push(`${label} must be valid YAML`);
    return {};
  }
}

function exactKeys(
  value: Record<string, unknown> | undefined,
  expected: readonly string[],
  label: string,
  failures: string[],
): void {
  const actual = Object.keys(value ?? {}).sort();
  if (JSON.stringify(actual) !== JSON.stringify([...expected].sort())) {
    failures.push(`${label} must contain exactly: ${[...expected].sort().join(', ')}`);
  }
}

function requireSnippets(contents: string, snippets: readonly string[], label: string): string[] {
  return snippets
    .filter((snippet) => !contents.includes(snippet))
    .map((snippet) => `${label} is missing edge protection: ${snippet}`);
}

function composeModeFailures(
  document: ComposeDocument,
  source: string,
  label: string,
  expectedPorts: readonly string[],
  failures: string[],
): void {
  exactKeys(document.services, ['api_backend', 'traefik'], `${label} service set`, failures);
  const api = document.services?.api_backend;
  const traefik = document.services?.traefik;
  const apiEnvironment = asRecord(api?.environment);
  const traefikNetworks = asRecord(traefik?.networks);
  const edgeNetwork = asRecord(document.networks?.talos_edge);
  const ipam = asRecord(edgeNetwork.ipam);
  const ipamConfigs = Array.isArray(ipam.config) ? ipam.config : [];
  const firstIpamConfig = asRecord(ipamConfigs[0]);

  if (apiEnvironment.API_TRUSTED_PROXIES !== TRAEFIK_ADDRESS_EXPRESSION) {
    failures.push(`${label} must make the API trust only Traefik's exact private address`);
  }
  if (Object.hasOwn(api ?? {}, 'ports') || Object.hasOwn(api ?? {}, 'volumes')) {
    failures.push(`${label} must not publish or mount anything into api_backend`);
  }
  if (traefik?.image !== TRAEFIK_IMAGE_EXPRESSION || traefik.pull_policy !== 'missing') {
    failures.push(
      `${label} must use the reviewed Traefik image expression with pull_policy missing`,
    );
  }
  if (Object.hasOwn(traefik ?? {}, 'build')) {
    failures.push(`${label} Traefik must use the publisher image rather than a source build`);
  }
  if (traefik?.privileged === true || Object.hasOwn(traefik ?? {}, 'container_name')) {
    failures.push(`${label} Traefik must not be privileged or use a global container name`);
  }
  if (JSON.stringify(asStringArray(traefik?.ports)) !== JSON.stringify(expectedPorts)) {
    failures.push(`${label} must publish only its reviewed HTTP and HTTPS bindings`);
  }
  if (asRecord(traefikNetworks.talos_edge).ipv4_address !== TRAEFIK_ADDRESS_EXPRESSION) {
    failures.push(`${label} must assign Traefik the address trusted by api_backend`);
  }
  if (firstIpamConfig.subnet !== EDGE_SUBNET_EXPRESSION) {
    failures.push(`${label} must define the reviewed private edge subnet contract`);
  }
  if (traefik?.read_only !== true) {
    failures.push(`${label} Traefik must use a read-only root filesystem`);
  }
  if (
    JSON.stringify(asStringArray(traefik?.cap_drop)) !== '["ALL"]' ||
    JSON.stringify(asStringArray(traefik?.cap_add)) !== '["NET_BIND_SERVICE"]'
  ) {
    failures.push(`${label} Traefik must retain only NET_BIND_SERVICE`);
  }
  if (!asStringArray(traefik?.security_opt).includes('no-new-privileges:true')) {
    failures.push(`${label} Traefik must enable no-new-privileges`);
  }

  const command = asStringArray(traefik?.command).join('\n');
  failures.push(
    ...requireSnippets(
      command,
      [
        '--api.dashboard=false',
        '--api.insecure=false',
        '--core.stricttlsoptions=true',
        '--entrypoints.web.forwardedheaders.insecure=false',
        '--entrypoints.websecure.forwardedheaders.insecure=false',
        '--entrypoints.web.http.aliasheadersstrategy=delete',
        '--entrypoints.websecure.http.aliasheadersstrategy=delete',
        ...REJECTED_ENCODED_CHARACTER_FLAGS,
        '--providers.file.filename=/etc/traefik/dynamic/talos.yml',
        '--providers.file.debugloggeneratedtemplate=false',
        '--accesslog.fields.headers.defaultmode=drop',
        '--ping.entrypoint=traefik',
      ],
      `${label} Traefik command`,
    ),
  );
  if (/providers\.(?:docker|swarm)/i.test(command) || /docker\.sock/i.test(source)) {
    failures.push(`${label} must not grant Traefik container-engine discovery or a Docker socket`);
  }
  if (/--api\.(?:dashboard|insecure)=true/i.test(command)) {
    failures.push(`${label} must not enable the Traefik API or dashboard`);
  }
}

export function floatingLatestReleaseInputFailures(
  sources: readonly ReleaseInputSource[],
): string[] {
  const failures: string[] = [];
  for (const source of sources) {
    const matches =
      source.contents.match(/(?:[A-Za-z0-9][A-Za-z0-9._/-]*:latest|@latest)\b/g) ?? [];
    for (const match of matches) {
      if (!REVIEWED_FLOATING_LATEST_PATHS.has(source.path) || match !== 'traefik:latest') {
        failures.push(`${source.path} contains unreviewed floating release input: ${match}`);
      }
    }
    if (REVIEWED_FLOATING_LATEST_PATHS.has(source.path)) {
      const reviewedMatches = matches.filter((match) => match === 'traefik:latest');
      if (reviewedMatches.length !== 1) {
        failures.push(`${source.path} must contain exactly one reviewed traefik:latest default`);
      }
    }
  }
  return failures;
}

export function communityEdgeContractFailures(input: {
  publicCompose: string;
  customCompose: string;
  localCompose: string;
  acmeDynamic: string;
  customDynamic: string;
  baseCompose: string;
  riskRegister: string;
  releaseInputs: readonly ReleaseInputSource[];
}): string[] {
  const failures: string[] = [];
  const publicDocument = parseCompose(
    input.publicCompose,
    COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH,
    failures,
  );
  const customDocument = parseCompose(
    input.customCompose,
    COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH,
    failures,
  );
  const localDocument = parseCompose(
    input.localCompose,
    COMMUNITY_EDGE_LOCAL_COMPOSE_PATH,
    failures,
  );

  composeModeFailures(
    publicDocument,
    input.publicCompose,
    'public ACME edge',
    [
      '${TALOS_EDGE_BIND_ADDRESS:-0.0.0.0}:${TALOS_EDGE_HTTP_PORT:-80}:80/tcp',
      '${TALOS_EDGE_BIND_ADDRESS:-0.0.0.0}:${TALOS_EDGE_HTTPS_PORT:-443}:443/tcp',
    ],
    failures,
  );
  composeModeFailures(
    customDocument,
    input.customCompose,
    'custom-certificate edge',
    [
      '${TALOS_EDGE_BIND_ADDRESS:-0.0.0.0}:${TALOS_EDGE_HTTP_PORT:-80}:80/tcp',
      '${TALOS_EDGE_BIND_ADDRESS:-0.0.0.0}:${TALOS_EDGE_HTTPS_PORT:-443}:443/tcp',
    ],
    failures,
  );
  composeModeFailures(
    localDocument,
    input.localCompose,
    'local self-signed edge',
    [
      '127.0.0.1:${TALOS_EDGE_HTTP_PORT:-80}:80/tcp',
      '127.0.0.1:${TALOS_EDGE_HTTPS_PORT:-443}:443/tcp',
    ],
    failures,
  );

  const publicTraefik = publicDocument.services?.traefik;
  const publicCommand = asStringArray(publicTraefik?.command).join('\n');
  const publicVolumes = asStringArray(publicTraefik?.volumes);
  failures.push(
    ...requireSnippets(
      publicCommand,
      [
        '--certificatesresolvers.letsencrypt.acme.email=${TALOS_ACME_EMAIL:?',
        '--certificatesresolvers.letsencrypt.acme.storage=/acme/acme.json',
        '--certificatesresolvers.letsencrypt.acme.httpchallenge.entrypoint=web',
        '--certificatesresolvers.letsencrypt.acme.caserver=${TALOS_ACME_CA_SERVER:-https://acme-v02.api.letsencrypt.org/directory}',
      ],
      'public ACME command',
    ),
  );
  if (!Object.hasOwn(publicDocument.volumes ?? {}, 'talos_traefik_acme')) {
    failures.push('public ACME edge must declare the durable talos_traefik_acme volume');
  }
  const publicAcmeVolume = asRecord(publicDocument.volumes?.talos_traefik_acme);
  if (publicAcmeVolume.name !== ACME_VOLUME_NAME) {
    failures.push(`public ACME edge must use the stable named volume ${ACME_VOLUME_NAME}`);
  }
  if (
    !publicVolumes.includes('talos_traefik_acme:/acme') ||
    !publicVolumes.includes('./traefik/dynamic-acme.yml:/etc/traefik/dynamic/talos.yml:ro')
  ) {
    failures.push('public ACME edge must mount only its dynamic file and durable ACME state');
  }

  for (const [label, document, certVariablePrefix] of [
    ['custom-certificate edge', customDocument, 'TALOS_CUSTOM_TLS'] as const,
    ['local self-signed edge', localDocument, 'TALOS_LOCAL_TLS'] as const,
  ]) {
    const traefik = document.services?.traefik;
    const command = asStringArray(traefik?.command).join('\n');
    const volumes = asStringArray(traefik?.volumes).join('\n');
    if (/certificatesresolvers|\/acme(?:\/|\s|$)/i.test(command + volumes)) {
      failures.push(`${label} must not initialize or mount ACME state`);
    }
    failures.push(
      ...requireSnippets(
        volumes,
        [
          './traefik/dynamic-custom.yml:/etc/traefik/dynamic/talos.yml:ro',
          `\${${certVariablePrefix}_CERT_PATH:?`,
          `\${${certVariablePrefix}_KEY_PATH:?`,
          ':/certificates/talos-fullchain.pem:ro',
          ':/certificates/talos-key.pem:ro',
        ],
        `${label} mounts`,
      ),
    );
  }

  for (const [label, source] of [
    ['public ACME dynamic config', input.acmeDynamic],
    ['custom-certificate dynamic config', input.customDynamic],
  ] as const) {
    failures.push(
      ...requireSnippets(
        source,
        [
          'env "TALOS_FRONTEND_DOMAIN" | quote',
          'env "TALOS_API_DOMAIN" | quote',
          'env "TALOS_CONTROL_DOMAIN" | quote',
          'env "TALOS_RELAY_DOMAIN" | quote',
          'port: \'{{ env "TALOS_EDGE_HTTPS_PORT" }}\'',
          'url: http://frontend:3000',
          'url: http://api_backend:3001',
          'url: http://talos_server:17110',
          'address: talos_relay:443',
          'minVersion: VersionTLS12',
          'sniStrict: true',
        ],
        label,
      ),
    );
    if (/HostSNI\s*\(\s*[`'"]\*|HostRegexp|PathPrefix\s*\(\s*[`'"]\//.test(source)) {
      failures.push(`${label} must not contain a catch-all host, SNI, or path router`);
    }
    if (/passthrough:\s*true/i.test(source)) {
      failures.push(`${label} must terminate relay TLS at Traefik`);
    }
  }
  if ((input.acmeDynamic.match(/certResolver:\s*letsencrypt/g) ?? []).length !== 4) {
    failures.push('public ACME dynamic config must request certificates for all four DNS names');
  }
  failures.push(
    ...requireSnippets(
      input.customDynamic,
      ['certFile: /certificates/talos-fullchain.pem', 'keyFile: /certificates/talos-key.pem'],
      'custom-certificate dynamic config',
    ),
  );

  if (/^\s+ports:/m.test(input.baseCompose)) {
    failures.push('production base must not bypass the edge with direct host ports');
  }
  failures.push(
    ...requireSnippets(
      input.baseCompose,
      ['RMM_RELAY_TLS_TERMINATED: "true"', 'RMM_RELAY_BIND_ADDR: 0.0.0.0:443'],
      'production base relay boundary',
    ),
    ...requireSnippets(
      input.riskRegister,
      ['DR-012', 'traefik:latest', 'Expiry: 2027-08-28', 'No other Community image may float'],
      'dependency risk register',
    ),
    ...floatingLatestReleaseInputFailures(input.releaseInputs),
  );

  const combined = [
    input.publicCompose,
    input.customCompose,
    input.localCompose,
    input.acmeDynamic,
    input.customDynamic,
  ].join('\n');
  if (/docker\.sock|providers:\s*\n\s+docker:/i.test(combined)) {
    failures.push('Community edge files must not mount a Docker socket or enable Docker discovery');
  }

  return failures;
}

async function collectFiles(directory: string, repoRoot: string): Promise<ReleaseInputSource[]> {
  const results: ReleaseInputSource[] = [];
  let entries;
  try {
    entries = await readdir(resolve(repoRoot, directory), { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return results;
    throw error;
  }
  for (const entry of entries) {
    const relativePath = `${directory}/${entry.name}`;
    if (entry.isDirectory()) {
      results.push(...(await collectFiles(relativePath, repoRoot)));
      continue;
    }
    if (
      entry.isFile() &&
      (/\.ya?ml$/i.test(entry.name) || /^Dockerfile(?:\.|$)/.test(entry.name))
    ) {
      results.push({
        path: relativePath,
        contents: await readFile(resolve(repoRoot, relativePath), 'utf8'),
      });
    }
  }
  return results;
}

export async function checkCommunityEdgeContract(
  repoRoot = resolve(import.meta.dir, '..', '..'),
): Promise<{ failures: string[] }> {
  const [
    publicCompose,
    customCompose,
    localCompose,
    acmeDynamic,
    customDynamic,
    baseCompose,
    riskRegister,
    infraInputs,
    workflowInputs,
    appInputs,
  ] = await Promise.all([
    readFile(resolve(repoRoot, COMMUNITY_EDGE_PUBLIC_COMPOSE_PATH), 'utf8'),
    readFile(resolve(repoRoot, COMMUNITY_EDGE_CUSTOM_COMPOSE_PATH), 'utf8'),
    readFile(resolve(repoRoot, COMMUNITY_EDGE_LOCAL_COMPOSE_PATH), 'utf8'),
    readFile(resolve(repoRoot, COMMUNITY_EDGE_ACME_DYNAMIC_PATH), 'utf8'),
    readFile(resolve(repoRoot, COMMUNITY_EDGE_CUSTOM_DYNAMIC_PATH), 'utf8'),
    readFile(resolve(repoRoot, 'infra/compose.community.yml'), 'utf8'),
    readFile(resolve(repoRoot, 'docs/architecture/dependency-risk-register.md'), 'utf8'),
    collectFiles('infra', repoRoot),
    collectFiles('.github/workflows', repoRoot),
    collectFiles('apps', repoRoot),
  ]);

  return {
    failures: communityEdgeContractFailures({
      publicCompose,
      customCompose,
      localCompose,
      acmeDynamic,
      customDynamic,
      baseCompose,
      riskRegister,
      releaseInputs: [...infraInputs, ...workflowInputs, ...appInputs],
    }),
  };
}

if (import.meta.main) {
  const { failures } = await checkCommunityEdgeContract();
  if (failures.length > 0) {
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log('Community edge contract is valid.');
}
