import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { relative, resolve, sep } from 'node:path';

export const PROJECT_LICENSE = 'AGPL-3.0-only';

const REPO_ROOT = resolve(import.meta.dir, '../..');
const APPS_ROOT = resolve(REPO_ROOT, 'apps');
const ROOT_LICENSE_SHA256 = '0d96a4ff68ad6d4b6f1f30f713b18d5184912ba8dd389f86aa7710db079abcb0';

const FIRST_PARTY_JAVASCRIPT_MANIFESTS = [
  'apps/package.json',
  'apps/api_backend/package.json',
  'apps/frontend/package.json',
  'apps/talos_permissions_helper/package.json',
  'apps/talos_protocol_types/package.json',
  'apps/talos_viewer/package.json',
  'apps/talos_worker_chat/package.json',
] as const;

const FIRST_PARTY_CARGO_MANIFESTS = new Map<string, string>([
  ['talos_appliance', 'apps/talos_appliance/Cargo.toml'],
  ['talos_ai_runner', 'apps/talos_ai_runner/Cargo.toml'],
  ['talos_collector', 'apps/talos_collector/Cargo.toml'],
  ['talos_log_util', 'apps/talos_log_util/Cargo.toml'],
  ['talos_permissions_helper', 'apps/talos_permissions_helper/src-tauri/Cargo.toml'],
  ['talos_protocol', 'apps/talos_protocol/Cargo.toml'],
  ['talos_relay', 'apps/talos_relay/Cargo.toml'],
  ['talos_server', 'apps/talos_server/Cargo.toml'],
  ['talos_supervisor', 'apps/talos_supervisor/Cargo.toml'],
  ['talos_telemetry_consumer', 'apps/talos_telemetry_consumer/Cargo.toml'],
  ['talos_telemetry_producer', 'apps/talos_telemetry_producer/Cargo.toml'],
  ['talos_update_common', 'apps/talos_update_common/Cargo.toml'],
  ['talos_viewer', 'apps/talos_viewer/src-tauri/Cargo.toml'],
  ['talos_viewer_updater', 'apps/talos_viewer_updater/Cargo.toml'],
  ['talos_worker', 'apps/talos_worker/Cargo.toml'],
  ['talos_worker_chat', 'apps/talos_worker_chat/src-tauri/Cargo.toml'],
  ['talos_worker_helper', 'apps/talos_worker_helper/Cargo.toml'],
]);

type LocalThirdPartyPackage = {
  manifest: string;
  version: string;
  license: string;
  licenseFile?: string;
  licenseFileSha256?: string;
};

const LOCAL_THIRD_PARTY_CARGO = new Map<string, LocalThirdPartyPackage>([
  [
    'dxgi-capture-rs',
    {
      manifest: 'apps/vendor/dxgi-capture-rs/Cargo.toml',
      version: '1.1.7',
      license: 'MIT',
      licenseFile: 'apps/vendor/dxgi-capture-rs/LICENSE',
      licenseFileSha256: 'e187671b3afebf4f9d85a0a0b87f8b1ba4aa56dae1cc6f97d52e83cd45ddd04f',
    },
  ],
  [
    'permission-flow',
    {
      manifest: 'apps/vendor/permission-flow/Cargo.toml',
      version: '0.1.40',
      license: 'MIT',
      licenseFile: 'apps/vendor/permission-flow/PermissionFlow/LICENSE',
      licenseFileSha256: 'd65f906c8116c14921f841867969d6dc3f9dd7b99fca34071671ff5896a3fa94',
    },
  ],
  [
    'tauri-plugin-permission-flow',
    {
      manifest: 'apps/vendor/tauri-plugin-permission-flow/Cargo.toml',
      version: '0.1.40',
      license: 'MIT',
      licenseFile: 'apps/vendor/permission-flow/PermissionFlow/LICENSE',
      licenseFileSha256: 'd65f906c8116c14921f841867969d6dc3f9dd7b99fca34071671ff5896a3fa94',
    },
  ],
  [
    'samsa',
    {
      manifest: 'apps/vendor/samsa/Cargo.toml',
      version: '0.1.8',
      license: 'Apache-2.0',
      licenseFile: 'apps/vendor/samsa/LICENSE',
      licenseFileSha256: '106f9576da64a4d8240850a3fee0672f275f6041fd47901e7e06dcfeb3f0b9b9',
    },
  ],
  [
    'vpx-encode',
    {
      manifest: 'apps/vpx-encode/Cargo.toml',
      version: '0.6.5',
      license: 'MIT',
    },
  ],
]);

// This is deliberately an exact-expression review list, not a permissive SPDX parser. A new or
// differently expressed dependency licence therefore requires an explicit provenance review.
const REVIEWED_BUN_LICENSE_EXPRESSIONS = new Set([
  '0BSD',
  'Apache-2.0',
  'Apache-2.0 OR MIT',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'BlueOak-1.0.0',
  'ISC',
  'MIT',
  'MIT OR Apache-2.0',
  'MPL-2.0',
]);

const REVIEWED_CARGO_LICENSE_EXPRESSIONS = new Set([
  '(Apache-2.0 OR MIT) AND BSD-3-Clause',
  '(MIT OR Apache-2.0) AND Unicode-3.0',
  '0BSD OR MIT OR Apache-2.0',
  'Apache-2.0',
  'Apache-2.0 / MIT',
  'Apache-2.0 AND ISC',
  'Apache-2.0 AND MIT',
  'Apache-2.0 OR BSL-1.0',
  'Apache-2.0 OR ISC OR MIT',
  'Apache-2.0 OR MIT',
  'Apache-2.0 WITH LLVM-exception',
  'Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT',
  'Apache-2.0/MIT',
  'BSD-2-Clause OR Apache-2.0 OR MIT',
  'BSD-3-Clause',
  'BSD-3-Clause AND MIT',
  'BSD-3-Clause OR MIT OR Apache-2.0',
  'BSD-3-Clause/MIT',
  'CC0-1.0',
  'CC0-1.0 OR MIT-0 OR Apache-2.0',
  'CDLA-Permissive-2.0',
  'ISC',
  'ISC AND (Apache-2.0 OR ISC)',
  'ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)',
  'MIT',
  'MIT AND BSD-3-Clause',
  'MIT OR Apache-2.0',
  'MIT OR Apache-2.0 OR LGPL-2.1-or-later',
  'MIT OR Apache-2.0 OR Zlib',
  'MIT OR BSD-3-Clause',
  'MIT OR Zlib OR Apache-2.0',
  'MIT/Apache-2.0',
  'MPL-2.0',
  'Unicode-3.0',
  'Unlicense OR MIT',
  'Unlicense/MIT',
  'Zlib',
  'Zlib OR Apache-2.0 OR MIT',
]);

export type DependencyLicense = {
  ecosystem: 'bun' | 'cargo';
  name: string;
  version: string;
  license: string | undefined;
  source: string;
  manifest?: string;
  licenseFile?: string;
};

export type LicensePolicyInputs = {
  rootLicenseSha256: string;
  javascriptManifestLicenses: Map<string, string | undefined>;
  bunDependencies: DependencyLicense[];
  bunLockText: string;
  cargoDependencies: DependencyLicense[];
  evidenceFileSha256: Map<string, string>;
};

function toRepoPath(path: string): string {
  return relative(REPO_ROOT, path).split(sep).join('/');
}

function describeDependency(dependency: DependencyLicense): string {
  return `${dependency.ecosystem} dependency ${dependency.name}@${dependency.version}`;
}

export function licensePolicyFailures(input: LicensePolicyInputs): string[] {
  const failures: string[] = [];

  if (input.rootLicenseSha256 !== ROOT_LICENSE_SHA256) {
    failures.push('root LICENSE must be the unmodified GNU AGPL version 3 text');
  }

  for (const manifest of FIRST_PARTY_JAVASCRIPT_MANIFESTS) {
    const license = input.javascriptManifestLicenses.get(manifest);
    if (license !== PROJECT_LICENSE) {
      failures.push(`${manifest} must declare ${PROJECT_LICENSE}`);
    }
  }

  if (/(?:git\+https?:|github:|https?:\/\/|file:|link:)/i.test(input.bunLockText)) {
    failures.push('apps/bun.lock contains an unreviewed non-registry dependency source');
  }

  for (const dependency of input.bunDependencies) {
    const description = describeDependency(dependency);
    if (!dependency.license) {
      failures.push(`${description} has no declared licence`);
    } else if (!REVIEWED_BUN_LICENSE_EXPRESSIONS.has(dependency.license)) {
      failures.push(`${description} uses unreviewed licence expression ${dependency.license}`);
    }
    if (dependency.source !== 'bun-registry-install') {
      failures.push(`${description} uses unreviewed source ${dependency.source}`);
    }
  }

  const seenFirstPartyCargo = new Set<string>();
  const seenLocalThirdPartyCargo = new Set<string>();
  for (const dependency of input.cargoDependencies) {
    const description = describeDependency(dependency);
    const firstPartyManifest = FIRST_PARTY_CARGO_MANIFESTS.get(dependency.name);
    const localThirdParty = LOCAL_THIRD_PARTY_CARGO.get(dependency.name);

    if (dependency.source === 'local') {
      if (!dependency.manifest) {
        failures.push(`${description} is local but has no manifest path`);
        continue;
      }

      if (firstPartyManifest === dependency.manifest) {
        seenFirstPartyCargo.add(dependency.name);
        if (dependency.license !== PROJECT_LICENSE) {
          failures.push(`${dependency.manifest} must declare ${PROJECT_LICENSE}`);
        }
        continue;
      }

      if (localThirdParty?.manifest === dependency.manifest) {
        seenLocalThirdPartyCargo.add(dependency.name);
        if (dependency.version !== localThirdParty.version) {
          failures.push(
            `${dependency.manifest} version ${dependency.version} does not match reviewed version ${localThirdParty.version}`,
          );
        }
        const retainsDeclaredLicense = dependency.license === localThirdParty.license;
        const retainsLicenseFile =
          localThirdParty.licenseFile !== undefined &&
          dependency.licenseFile === localThirdParty.licenseFile;
        if (!retainsDeclaredLicense && !retainsLicenseFile) {
          failures.push(
            `${dependency.manifest} must retain reviewed licence ${localThirdParty.license}`,
          );
        }
        continue;
      }

      failures.push(`${description} is an unreviewed local/path package at ${dependency.manifest}`);
      continue;
    }

    if (!dependency.source.startsWith('registry+')) {
      failures.push(`${description} uses unreviewed source ${dependency.source}`);
    }
    if (!dependency.license) {
      failures.push(`${description} has no declared licence`);
    } else if (!REVIEWED_CARGO_LICENSE_EXPRESSIONS.has(dependency.license)) {
      failures.push(`${description} uses unreviewed licence expression ${dependency.license}`);
    }
  }

  for (const [name, manifest] of FIRST_PARTY_CARGO_MANIFESTS) {
    if (!seenFirstPartyCargo.has(name)) {
      failures.push(`Cargo metadata is missing first-party package ${name} at ${manifest}`);
    }
  }
  for (const [name, provenance] of LOCAL_THIRD_PARTY_CARGO) {
    if (!seenLocalThirdPartyCargo.has(name)) {
      failures.push(`Cargo metadata is missing reviewed third-party package ${name}`);
    }
    if (provenance.licenseFile && provenance.licenseFileSha256) {
      const actual = input.evidenceFileSha256.get(provenance.licenseFile);
      if (actual !== provenance.licenseFileSha256) {
        failures.push(`${provenance.licenseFile} does not match its reviewed licence evidence`);
      }
    }
  }

  return [...new Set(failures)].sort();
}

async function sha256(path: string): Promise<string> {
  return createHash('sha256')
    .update(new Uint8Array(await Bun.file(path).arrayBuffer()))
    .digest('hex');
}

type PackageJson = {
  name?: string;
  version?: string;
  license?: string;
  licenses?: Array<string | { type?: string }> | string;
};

function packageJsonLicense(manifest: PackageJson): string | undefined {
  if (typeof manifest.license === 'string') return manifest.license;
  if (typeof manifest.licenses === 'string') return manifest.licenses;
  if (Array.isArray(manifest.licenses)) {
    const licenses = manifest.licenses
      .map((entry) => (typeof entry === 'string' ? entry : entry.type))
      .filter((entry): entry is string => Boolean(entry));
    return licenses.length > 0 ? licenses.join(' OR ') : undefined;
  }
  return undefined;
}

async function collectBunDependencies(): Promise<DependencyLicense[]> {
  const storeRoot = resolve(APPS_ROOT, 'node_modules/.bun');
  if (!existsSync(storeRoot)) {
    throw new Error('apps/node_modules/.bun is missing; run the frozen workspace install first');
  }

  const dependencies = new Map<string, DependencyLicense>();
  const packageManifest = /^[^/]+\/node_modules\/(?:@[^/]+\/)?[^/]+\/package\.json$/;
  const glob = new Bun.Glob('**/package.json');
  for await (const path of glob.scan({ cwd: storeRoot, dot: true, onlyFiles: true })) {
    if (!packageManifest.test(path)) continue;
    const manifest = (await Bun.file(resolve(storeRoot, path)).json()) as PackageJson;
    if (!manifest.name || !manifest.version) continue;
    const license = packageJsonLicense(manifest);
    const key = `${manifest.name}\u0000${manifest.version}\u0000${license ?? ''}`;
    dependencies.set(key, {
      ecosystem: 'bun',
      name: manifest.name,
      version: manifest.version,
      license,
      source: 'bun-registry-install',
    });
  }
  return [...dependencies.values()].sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`),
  );
}

type CargoMetadataPackage = {
  name: string;
  version: string;
  license: string | null;
  license_file: string | null;
  source: string | null;
  manifest_path: string;
};

async function collectCargoDependencies(): Promise<DependencyLicense[]> {
  const process = Bun.spawn(['cargo', 'metadata', '--locked', '--format-version', '1'], {
    cwd: APPS_ROOT,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(process.stdout).text(),
    new Response(process.stderr).text(),
    process.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`cargo metadata failed (${exitCode}): ${stderr.trim()}`);
  }
  const metadata = JSON.parse(stdout) as { packages: CargoMetadataPackage[] };
  return metadata.packages.map((dependency) => ({
    ecosystem: 'cargo',
    name: dependency.name,
    version: dependency.version,
    license: dependency.license ?? undefined,
    source: dependency.source ?? 'local',
    manifest: toRepoPath(dependency.manifest_path),
    licenseFile: dependency.license_file
      ? toRepoPath(resolve(dependency.manifest_path, '..', dependency.license_file))
      : undefined,
  }));
}

export async function checkLicensePolicy(repoRoot = REPO_ROOT): Promise<{
  failures: string[];
  bunDependencyCount: number;
  cargoDependencyCount: number;
}> {
  if (repoRoot !== REPO_ROOT) {
    throw new Error('custom repository roots are not supported by the dependency discovery gate');
  }

  const javascriptManifestLicenses = new Map<string, string | undefined>();
  for (const manifestPath of FIRST_PARTY_JAVASCRIPT_MANIFESTS) {
    const manifest = (await Bun.file(resolve(REPO_ROOT, manifestPath)).json()) as PackageJson;
    javascriptManifestLicenses.set(manifestPath, packageJsonLicense(manifest));
  }

  const evidenceFileSha256 = new Map<string, string>();
  for (const provenance of LOCAL_THIRD_PARTY_CARGO.values()) {
    if (provenance.licenseFile && !evidenceFileSha256.has(provenance.licenseFile)) {
      evidenceFileSha256.set(
        provenance.licenseFile,
        await sha256(resolve(REPO_ROOT, provenance.licenseFile)),
      );
    }
  }

  const [rootLicenseSha256, bunLockText, bunDependencies, cargoDependencies] = await Promise.all([
    sha256(resolve(REPO_ROOT, 'LICENSE')),
    Bun.file(resolve(APPS_ROOT, 'bun.lock')).text(),
    collectBunDependencies(),
    collectCargoDependencies(),
  ]);

  return {
    failures: licensePolicyFailures({
      rootLicenseSha256,
      javascriptManifestLicenses,
      bunDependencies,
      bunLockText,
      cargoDependencies,
      evidenceFileSha256,
    }),
    bunDependencyCount: bunDependencies.length,
    cargoDependencyCount: cargoDependencies.length,
  };
}

if (import.meta.main) {
  const result = await checkLicensePolicy();
  if (result.failures.length > 0) {
    console.error('Dependency licence/source policy failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(
    `Dependency licence/source policy passed (${result.bunDependencyCount} Bun packages, ${result.cargoDependencyCount} Cargo packages).`,
  );
}
