import { describe, expect, test } from 'bun:test';
import {
  checkReleaseInputContract,
  linuxBuildContextIsolationFailures,
  linuxRustupReleaseInputFailures,
  macosLibvpxReleaseInputFailures,
  macosViewerReleaseInputFailures,
  windowsPrerequisiteReleaseInputFailures,
  windowsDevelopmentToolchainFailures,
  windowsVcpkgReleaseInputFailures,
} from './release-input-contract';

const validWixToolManifest = JSON.stringify({
  version: 1,
  isRoot: true,
  tools: { wix: { version: '6.0.0', commands: ['wix'] } },
});

describe('release input contract', () => {
  test('rejects a dynamically fetched Tauri CLI without a locked Cargo build', () => {
    const failures = macosViewerReleaseInputFailures(
      JSON.stringify({ devDependencies: {} }),
      'bun x @tauri-apps/cli build',
    );
    expect(failures).toContain('talos_viewer must declare @tauri-apps/cli exactly at 2.10.1');
    expect(failures).toContain(
      'build-macos-viewer.sh must not fetch the Tauri CLI dynamically with bun x',
    );
  });

  test('requires custom libvpx source digest and cache verification', () => {
    const failures = macosLibvpxReleaseInputFailures('MACOS_LIBVPX_URL="custom"', 'notarization');
    expect(failures).toContain(
      'build-macos-agent.sh is missing release-input protection: require_macos_libvpx_digest',
    );
    expect(failures).toContain(
      'build-macos-agent.sh is missing release-input protection: verify_macos_libvpx_archive "$archive"',
    );
  });

  test('rejects curl-piped Rust bootstraps', () => {
    const formerBootstrap = 'curl https://sh.rustup.rs | sh';
    const failures = linuxRustupReleaseInputFailures(formerBootstrap, formerBootstrap);
    expect(failures).toContain('build-linux-agent.sh must not bootstrap Rust through sh.rustup.rs');
    expect(failures).toContain(
      'build-installers.ps1 must not pipe downloaded content into a shell',
    );
    expect(failures).toContain(
      'build-installers.ps1 must force linux/amd64 for its x86-64 rustup-init builder',
    );
  });

  test('rejects broad repository mounts in Linux build containers', () => {
    const formerShellBuild = '-v "$repo_mount:/workspace"';
    const formerWindowsBuild = '$repoMount = "${RepoRoot}:/workspace"';
    const failures = linuxBuildContextIsolationFailures(formerShellBuild, formerWindowsBuild);

    expect(failures).toContain(
      'build-linux-agent.sh must not mount the repository into a Linux build container',
    );
    expect(failures).toContain(
      'build-installers.ps1 must not mount the repository into a Linux build container',
    );
  });

  test('rejects floating VC++ downloads and a fail-open release profile', () => {
    const failures = windowsPrerequisiteReleaseInputFailures(
      'https://aka.ms/vs/17/release/vc_redist.x64.exe',
      'https://aka.ms/vs/17/release/vc_redist.x64.exe',
      '',
      '',
      validWixToolManifest,
    );
    expect(failures).toContain(
      'VC++ redistributables must not use the floating aka.ms release URLs',
    );
    expect(failures).toContain(
      'build-installers.ps1 is missing release-input protection: $releaseSigningRequested -eq $releaseUnsignedRequested',
    );
  });

  test('rejects a release pipeline that signs MSI outputs after Burn construction', () => {
    const outOfOrderSigning = `
      $authenticodeSigningEnabled -and ($buildSupervisorArtifacts -or $buildViewerArtifacts)
      $authenticodeSigningEnabled -and $buildSupervisorArtifacts
      Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $Cert.Thumbprint
      Authenticode verification failed after signing
      dotnet build $wixBuild.ProjectPath
      $wixMsiPaths += @($msiX86, $msiX64)
      $wixMsiPaths += $viewerMsiX64
      dotnet build $bundleProject
      Sign-Binaries -Paths $wixMsiPaths
      Sign-Binaries -Paths @($bundleExe)
      Write-Step "Publishing installer artifacts to profile folder"
      bundle = Get-ArtifactMetadata -Path $publishedBundleExe
    `;

    const failures = windowsPrerequisiteReleaseInputFailures(
      outOfOrderSigning,
      '',
      '',
      '',
      validWixToolManifest,
    );
    expect(failures).toContain(
      'final WiX MSI outputs must be signed before the Burn bundle embeds them',
    );
  });

  test('rejects a floating WiX CLI in the release tool manifest', () => {
    const failures = windowsPrerequisiteReleaseInputFailures(
      '',
      '',
      '',
      '',
      JSON.stringify({
        version: 1,
        isRoot: true,
        tools: { wix: { version: 'latest', commands: ['wix'] } },
      }),
    );

    expect(failures).toContain('.config/dotnet-tools.json must pin the WiX CLI exactly at 6.0.0');
  });

  test('requires one exact vcpkg baseline for setup, release, and CI', () => {
    const failures = windowsVcpkgReleaseInputFailures(
      'git clone "https://github.com/microsoft/vcpkg" C:\\vcpkg',
      'VPX_LIB_DIR="C:\\vcpkg\\installed"; winget install --id NASM.NASM',
      'vcpkg install libvpx:x64-windows',
      '',
    );
    expect(failures).toContain(
      'Setup-DevEnviroment.ps1 is missing release-input protection: d015e31e90838a4c9dfa3eed45979bc70d9357fc',
    );
    expect(failures).toContain(
      'build-installers.ps1 is missing release-input protection: talos-vcpkg-baseline.txt',
    );
    expect(failures).toContain(
      'quality.yml is missing release-input protection: git checkout --detach $env:VCPKG_COMMIT',
    );
    expect(failures).toContain(
      'VcpkgProvenance.ps1 is missing release-input protection: function Get-TalosDirectorySha256',
    );
    expect(failures).toContain(
      'build-installers.ps1 must not install NASM from a floating Winget package',
    );
  });

  test('rejects a moving Windows development toolchain', () => {
    const failures = windowsDevelopmentToolchainFailures(
      'rustup default stable',
      JSON.stringify({ packageManager: 'bun@latest' }),
      '[toolchain]\nchannel = "stable"',
    );

    expect(failures).toContain(
      'Setup-DevEnviroment.ps1 must not select the moving Rust stable channel',
    );
    expect(failures).toContain('apps/package.json must pin packageManager to bun@1.3.14');
    expect(failures).toContain('apps/rust-toolchain.toml must pin Rust 1.95.0');
  });

  test('the tracked release inputs satisfy the contract', async () => {
    expect((await checkReleaseInputContract()).failures).toEqual([]);
  });
});
