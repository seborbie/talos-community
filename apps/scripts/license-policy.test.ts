import { describe, expect, test } from 'bun:test';
import {
  checkLicensePolicy,
  isBunStorePackageManifest,
  licensePolicyFailures,
  type LicensePolicyInputs,
} from './license-policy';

const rootLicenseSha256 = '0d96a4ff68ad6d4b6f1f30f713b18d5184912ba8dd389f86aa7710db079abcb0';

function minimalInputs(): LicensePolicyInputs {
  return {
    rootLicenseSha256,
    javascriptManifestLicenses: new Map([
      ['apps/package.json', 'AGPL-3.0-only'],
      ['apps/api_backend/package.json', 'AGPL-3.0-only'],
      ['apps/frontend/package.json', 'AGPL-3.0-only'],
      ['apps/talos_permissions_helper/package.json', 'AGPL-3.0-only'],
      ['apps/talos_protocol_types/package.json', 'AGPL-3.0-only'],
      ['apps/talos_viewer/package.json', 'AGPL-3.0-only'],
      ['apps/talos_worker_chat/package.json', 'AGPL-3.0-only'],
    ]),
    bunDependencies: [
      {
        ecosystem: 'bun',
        name: 'example-package',
        version: '1.0.0',
        license: 'MIT',
        source: 'bun-registry-install',
      },
    ],
    bunLockText: '',
    cargoDependencies: [],
    evidenceFileSha256: new Map(),
  };
}

describe('dependency licence/source policy', () => {
  test('rejects licence drift and unreviewed dependency sources', () => {
    const input = minimalInputs();
    input.javascriptManifestLicenses.set('apps/frontend/package.json', 'UNLICENSED');
    input.bunDependencies.push({
      ecosystem: 'bun',
      name: 'unknown-license',
      version: '2.0.0',
      license: undefined,
      source: 'git+https://example.invalid/dependency.git',
    });
    input.bunLockText = '"unknown-license": "git+https://example.invalid/dependency.git"';

    const failures = licensePolicyFailures(input);
    expect(failures).toContain('apps/frontend/package.json must declare AGPL-3.0-only');
    expect(failures).toContain('bun dependency unknown-license@2.0.0 has no declared licence');
    expect(failures).toContain(
      'bun dependency unknown-license@2.0.0 uses unreviewed source git+https://example.invalid/dependency.git',
    );
    expect(failures).toContain(
      'apps/bun.lock contains an unreviewed non-registry dependency source',
    );
  });

  test('rejects a new Cargo licence expression until it is reviewed', () => {
    const input = minimalInputs();
    input.cargoDependencies.push({
      ecosystem: 'cargo',
      name: 'new-crate',
      version: '1.2.3',
      license: 'LicenseRef-Proprietary',
      source: 'registry+https://github.com/rust-lang/crates.io-index',
    });

    expect(licensePolicyFailures(input)).toContain(
      'cargo dependency new-crate@1.2.3 uses unreviewed licence expression LicenseRef-Proprietary',
    );
  });

  test('does not relicense copied third-party source as first-party AGPL', () => {
    const input = minimalInputs();
    input.cargoDependencies.push({
      ecosystem: 'cargo',
      name: 'vpx-encode',
      version: '0.6.5',
      license: 'AGPL-3.0-only',
      source: 'local',
      manifest: 'apps/vpx-encode/Cargo.toml',
    });

    expect(licensePolicyFailures(input)).toContain(
      'apps/vpx-encode/Cargo.toml must retain reviewed licence MIT',
    );
  });

  test('the installed frozen dependency graph and first-party manifests satisfy policy', async () => {
    const result = await checkLicensePolicy();
    expect(result.bunDependencyCount).toBeGreaterThan(250);
    expect(result.cargoDependencyCount).toBeGreaterThan(700);
    expect(result.failures).toEqual([]);
  });
});

test('licence scan recognizes Windows and Unix store paths without matching nested fixtures', () => {
  for (const path of [
    'pkg@1.0.0/node_modules/pkg/package.json',
    '@scope+pkg@1.0.0/node_modules/@scope/pkg/package.json',
  ]) {
    expect(isBunStorePackageManifest(path)).toBe(true);
    expect(isBunStorePackageManifest(path.replaceAll('/', '\\'))).toBe(true);
  }
  expect(isBunStorePackageManifest('pkg@1/node_modules/pkg/test/package.json')).toBe(false);
});
