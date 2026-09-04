import { describe, expect, test } from 'bun:test';
import {
  buildScriptVersionFailures,
  buildTargetsFailures,
  checkInstallerVersionContract,
  parseCargoPackageVersion,
  windowsInstallerVersionFailures,
  wixSourceVersionFailures,
} from './installer-version-contract';

describe('Windows installer version contract', () => {
  test('reads only the Cargo package version', () => {
    expect(
      parseCargoPackageVersion(
        `[package]\nname = "example"\nversion = "1.2.3"\n\n[dependencies]\nother = { version = "9.9.9" }\n`,
        'Cargo.toml',
      ),
    ).toBe('1.2.3');
  });

  test('enforces Windows Installer version syntax and numeric bounds', () => {
    expect(windowsInstallerVersionFailures('255.255.65535', 'agent')).toEqual([]);
    expect(windowsInstallerVersionFailures('1.2.3-beta.1', 'agent')).not.toEqual([]);
    expect(windowsInstallerVersionFailures('1.2.3.4', 'agent')).not.toEqual([]);
    expect(windowsInstallerVersionFailures('256.0.0', 'agent')).not.toEqual([]);
    expect(windowsInstallerVersionFailures('1.256.0', 'agent')).not.toEqual([]);
    expect(windowsInstallerVersionFailures('1.2.65536', 'agent')).not.toEqual([]);
  });

  test('rejects a hand-maintained WiX product version', () => {
    expect(wixSourceVersionFailures('<Package Version="1.2.3" />', 'Agent.x64.wxs')).not.toEqual(
      [],
    );
    expect(
      wixSourceVersionFailures('<Package Version="$(var.ProductVersion)" />', 'Agent.x64.wxs'),
    ).toEqual([]);
    expect(
      wixSourceVersionFailures('<Bundle Version="$(var.ProductVersion)" />', 'Bundle.wxs'),
    ).toEqual([]);
    expect(
      wixSourceVersionFailures(
        '<Package Version="1.2.3"><File Version="$(var.ProductVersion)" /></Package>',
        'Agent.x64.wxs',
      ),
    ).not.toEqual([]);
  });

  test('rejects a build script that drops the MSBuild version property', () => {
    expect(buildScriptVersionFailures('dotnet build installer.wixproj')).not.toEqual([]);
  });

  test('rejects sequential conditions that append the WiX constant twice', () => {
    expect(
      buildTargetsFailures(`
        <DefineConstants Condition="'$(DefineConstants)' == ''">ProductVersion=$(ProductVersion)</DefineConstants>
        <DefineConstants Condition="'$(DefineConstants)' != ''">$(DefineConstants);ProductVersion=$(ProductVersion)</DefineConstants>
        <Target Name="ValidateInstallerProductVersion">
          <Error Condition="'$(ProductVersion)' == ''" />
        </Target>
      `),
    ).not.toEqual([]);
  });

  test('the tracked release inputs satisfy the contract', async () => {
    const result = await checkInstallerVersionContract();
    expect(result.failures).toEqual([]);
    expect(result.versions.agent).toMatch(/^\d+\.\d+\.\d+$/);
    expect(result.versions.viewer).toMatch(/^\d+\.\d+\.\d+$/);
  });
});
