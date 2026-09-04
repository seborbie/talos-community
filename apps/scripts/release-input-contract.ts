import { resolve } from 'node:path';
import {
  authenticodeInstallerSequenceFailures,
  wixToolManifestFailures,
} from './installer-signing-contract';

const RUSTUP_INIT_URL =
  'https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init';
const RUSTUP_INIT_SHA256 = '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c';
const LIBVPX_SHA256 = 'cb2a393c9c1fae7aba76b950bb0ad393ba105409fe1a147ccd61b0aaa1501066';
const VC_REDIST_X86_SHA256 = '0c09f2611660441084ce0df425c51c11e147e6447963c3690f97e0b25c55ed64';
const VC_REDIST_X64_SHA256 = 'cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b';
const VCPKG_COMMIT = 'd015e31e90838a4c9dfa3eed45979bc70d9357fc';
const BUN_VERSION = '1.3.14';
const RUST_VERSION = '1.95.0';

function requireSnippets(contents: string, snippets: string[], label: string): string[] {
  return snippets
    .filter((snippet) => !contents.includes(snippet))
    .map((snippet) => `${label} is missing release-input protection: ${snippet}`);
}

export function macosViewerReleaseInputFailures(
  packageJson: string,
  buildScript: string,
): string[] {
  const failures: string[] = [];
  try {
    const manifest = JSON.parse(packageJson) as {
      devDependencies?: Record<string, string>;
    };
    if (manifest.devDependencies?.['@tauri-apps/cli'] !== '2.10.1') {
      failures.push('talos_viewer must declare @tauri-apps/cli exactly at 2.10.1');
    }
  } catch {
    failures.push('talos_viewer/package.json must be valid JSON');
  }

  failures.push(
    ...requireSnippets(
      buildScript,
      [
        'TAURI_CLI_VERSION="2.10.1"',
        'bun install --frozen-lockfile --filter talos_viewer',
        'node_modules/@tauri-apps/cli/tauri.js',
        'bun --bun "$local_tauri_cli" build',
        '-- --locked',
      ],
      'build-macos-viewer.sh',
    ),
  );
  if (/bun\s+x[^\n]*@tauri-apps\/cli/.test(buildScript)) {
    failures.push('build-macos-viewer.sh must not fetch the Tauri CLI dynamically with bun x');
  }
  return failures;
}

export function macosLibvpxReleaseInputFailures(
  buildScript: string,
  documentation: string,
): string[] {
  return [
    ...requireSnippets(
      buildScript,
      [
        `DEFAULT_MACOS_LIBVPX_SHA256="${LIBVPX_SHA256}"`,
        'require_macos_libvpx_digest',
        'verify_macos_libvpx_archive "$archive"',
        'verify_macos_libvpx_archive "$archive.tmp"',
        '.talos-source-sha256',
        'must be an explicit 64-character SHA-256 digest',
      ],
      'build-macos-agent.sh',
    ),
    ...requireSnippets(
      documentation,
      [
        'MACOS_LIBVPX_SHA256',
        'refuses a custom source without that digest',
        'notarization',
        'release blocker',
      ],
      'macOS build documentation',
    ),
  ];
}

export function linuxRustupReleaseInputFailures(
  linuxBuildScript: string,
  windowsBuildScript: string,
): string[] {
  const failures: string[] = [];
  for (const [label, contents] of [
    ['build-linux-agent.sh', linuxBuildScript],
    ['build-installers.ps1', windowsBuildScript],
  ] as const) {
    failures.push(
      ...requireSnippets(
        contents,
        [RUSTUP_INIT_URL, RUSTUP_INIT_SHA256, 'sha256sum -c -', '/tmp/rustup-init'],
        label,
      ),
    );
    if (contents.includes('https://sh.rustup.rs')) {
      failures.push(`${label} must not bootstrap Rust through sh.rustup.rs`);
    }
    if (/curl[^\n]*\|\s*(?:ba)?sh/.test(contents)) {
      failures.push(`${label} must not pipe downloaded content into a shell`);
    }
  }
  if (!windowsBuildScript.includes('--platform linux/amd64')) {
    failures.push('build-installers.ps1 must force linux/amd64 for its x86-64 rustup-init builder');
  }
  return failures;
}

export function linuxBuildContextIsolationFailures(
  linuxBuildScript: string,
  windowsBuildScript: string,
): string[] {
  const failures = [
    ...requireSnippets(
      linuxBuildScript,
      [
        'git -C "$REPO_ROOT" ls-files --cached --others --exclude-standard -z',
        '"$source_mount:/workspace:ro"',
        '"$output_mount:/talos-build-output"',
        '"$payload_mount:/talos-payload:ro"',
        'apps/installer/tmp/manifest_public_key.der',
      ],
      'build-linux-agent.sh',
    ),
    ...requireSnippets(
      windowsBuildScript,
      [
        'function New-LinuxSanitizedBuildContext',
        'Test-LinuxBuildContextPathIsSensitive',
        'ls-files --cached --others --exclude-standard',
        '$sourceMount = "${sourceContext}:/workspace:ro"',
        '$outputMount = "${outputRoot}:/talos-build-output"',
        'apps\\installer\\tmp\\manifest_public_key.der',
      ],
      'build-installers.ps1',
    ),
  ];

  if (linuxBuildScript.includes('$repo_mount:/workspace')) {
    failures.push(
      'build-linux-agent.sh must not mount the repository into a Linux build container',
    );
  }
  if (windowsBuildScript.includes('${RepoRoot}:/workspace')) {
    failures.push(
      'build-installers.ps1 must not mount the repository into a Linux build container',
    );
  }
  return failures;
}

export function windowsPrerequisiteReleaseInputFailures(
  buildScript: string,
  bundleProject: string,
  documentation: string,
  riskRegister: string,
  wixToolManifest = '',
): string[] {
  const failures = [
    ...authenticodeInstallerSequenceFailures(buildScript),
    ...wixToolManifestFailures(wixToolManifest),
    ...requireSnippets(
      buildScript,
      [
        VC_REDIST_X86_SHA256,
        VC_REDIST_X64_SHA256,
        'download.visualstudio.microsoft.com/download/pr/',
        'function Assert-ExpectedSha256',
        'function Assert-MicrosoftAuthenticodeSignature',
        'System.Management.Automation.SignatureStatus]::Valid',
        'O=Microsoft Corporation',
        'Verifying cached $Label',
        '-ExpectedSha256 $vcRedistX86Sha256 -RequireMicrosoftAuthenticode $true',
        '-ExpectedSha256 $vcRedistX64Sha256 -RequireMicrosoftAuthenticode $true',
        '-RequireMicrosoftAuthenticode $true',
        '$BuildProfile -eq "release"',
        '$releaseSigningRequested -eq $releaseUnsignedRequested',
        'requires exactly one explicit Authenticode choice',
      ],
      'build-installers.ps1',
    ),
    ...requireSnippets(
      bundleProject,
      [
        '0C09F2611660441084CE0DF425C51C11E147E6447963C3690F97E0B25C55ED64/VC_redist.x86.exe',
        'CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B/VC_redist.x64.exe',
      ],
      'Talos.Agent.Bundle.wixproj',
    ),
    ...requireSnippets(
      documentation,
      [
        'immutable Microsoft Download URLs',
        'currently valid Authenticode signature',
        'Passing both or neither fails',
        'DR-010',
        'do not currently perform Apple notarization',
      ],
      'installer documentation',
    ),
    ...requireSnippets(
      riskRegister,
      [
        'DR-010',
        'mutable release input',
        'Expiry: 2026-11-17',
        'valid Authenticode status',
        'Convert this key to a public',
      ],
      'dependency risk register',
    ),
  ];
  if (
    buildScript.includes('https://aka.ms/vs/17/release/vc_redist') ||
    bundleProject.includes('https://aka.ms/vs/17/release/vc_redist')
  ) {
    failures.push('VC++ redistributables must not use the floating aka.ms release URLs');
  }
  return failures;
}

export function windowsVcpkgReleaseInputFailures(
  setupScript: string,
  buildScript: string,
  qualityWorkflow: string,
  provenanceHelper = '',
): string[] {
  const failures = [
    ...requireSnippets(
      provenanceHelper,
      [
        'function Get-TalosDirectorySha256',
        'Get-ChildItem -LiteralPath $resolvedRoot -File -Recurse',
        'Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256',
        'format=talos-vcpkg-provenance-v1',
        'overlay_sha256=$overlaySha256',
        'function Test-TalosVcpkgProvenanceRecord',
      ],
      'VcpkgProvenance.ps1',
    ),
    ...requireSnippets(
      setupScript,
      [
        VCPKG_COMMIT,
        'git -C $VcpkgRoot checkout --detach $ExpectedCommit',
        'Join-Path $PSScriptRoot "VcpkgProvenance.ps1"',
        'Get-TalosVcpkgProvenanceRecord',
        'Test-TalosVcpkgProvenanceRecord',
        'talos-vcpkg-baseline.txt',
        'Set-Content -LiteralPath $baselineMarker -Value $expectedProvenance',
      ],
      'Setup-DevEnviroment.ps1',
    ),
    ...requireSnippets(
      buildScript,
      [
        VCPKG_COMMIT,
        '$script:RequiredNasmVersion = "3.01"',
        'Join-Path $PSScriptRoot "VcpkgProvenance.ps1"',
        'Get-TalosVcpkgProvenanceRecord',
        'Test-TalosVcpkgProvenanceRecord',
        '$nasmCmd.Source -v',
        '$actualVersion -ne $script:RequiredNasmVersion',
        'talos-vcpkg-baseline.txt',
        'Pinned vcpkg provenance marker is missing',
      ],
      'build-installers.ps1',
    ),
    ...requireSnippets(
      qualityWorkflow,
      [
        `VCPKG_COMMIT: ${VCPKG_COMMIT}`,
        'git checkout --detach $env:VCPKG_COMMIT',
        '.\\bootstrap-vcpkg.bat -disableMetrics',
        '${{ env.VCPKG_COMMIT }}',
        'Verify vcpkg overlay provenance',
        "hashFiles('scripts/vcpkg-overlays/libvpx/**')",
      ],
      'quality.yml',
    ),
  ];
  if (/winget\s+install\s+--id\s+["']?NASM\.NASM/i.test(buildScript)) {
    failures.push('build-installers.ps1 must not install NASM from a floating Winget package');
  }
  return failures;
}

export function windowsDevelopmentToolchainFailures(
  setupScript: string,
  workspaceManifest: string,
  rustToolchain: string,
): string[] {
  const failures = requireSnippets(
    setupScript,
    [
      `$RequiredBunVersion = "${BUN_VERSION}"`,
      `$RequiredRustVersion = "${RUST_VERSION}"`,
      'rustup toolchain install $RequiredRustVersion --profile minimal --component clippy,rustfmt',
      'rustc "+$RequiredRustVersion" -V',
    ],
    'Setup-DevEnviroment.ps1',
  );

  if (/rustup\s+default\s+stable/i.test(setupScript)) {
    failures.push('Setup-DevEnviroment.ps1 must not select the moving Rust stable channel');
  }

  try {
    const manifest = JSON.parse(workspaceManifest) as { packageManager?: string };
    if (manifest.packageManager !== `bun@${BUN_VERSION}`) {
      failures.push(`apps/package.json must pin packageManager to bun@${BUN_VERSION}`);
    }
  } catch {
    failures.push('apps/package.json must be valid JSON');
  }

  if (
    !new RegExp(`^channel\\s*=\\s*["']${RUST_VERSION.replaceAll('.', '\\.')}["']\\s*$`, 'm').test(
      rustToolchain,
    )
  ) {
    failures.push(`apps/rust-toolchain.toml must pin Rust ${RUST_VERSION}`);
  }

  return failures;
}

export async function checkReleaseInputContract(
  repoRoot = resolve(import.meta.dir, '../..'),
): Promise<{ failures: string[] }> {
  const [
    viewerPackage,
    viewerBuild,
    macosAgentBuild,
    linuxBuild,
    windowsBuild,
    bundleProject,
    installerDocs,
    macosDocs,
    riskRegister,
    setupScript,
    qualityWorkflow,
    vcpkgProvenanceHelper,
    workspaceManifest,
    rustToolchain,
    wixToolManifest,
  ] = await Promise.all([
    Bun.file(resolve(repoRoot, 'apps/talos_viewer/package.json')).text(),
    Bun.file(resolve(repoRoot, 'scripts/build-macos-viewer.sh')).text(),
    Bun.file(resolve(repoRoot, 'scripts/build-macos-agent.sh')).text(),
    Bun.file(resolve(repoRoot, 'scripts/build-linux-agent.sh')).text(),
    Bun.file(resolve(repoRoot, 'scripts/build-installers.ps1')).text(),
    Bun.file(resolve(repoRoot, 'apps/installer/bundle/Talos.Agent.Bundle.wixproj')).text(),
    Bun.file(resolve(repoRoot, 'apps/installer/README.md')).text(),
    Bun.file(resolve(repoRoot, 'apps/talos_worker/macos/README.md')).text(),
    Bun.file(resolve(repoRoot, 'docs/architecture/dependency-risk-register.md')).text(),
    Bun.file(resolve(repoRoot, 'scripts/Setup-DevEnviroment.ps1')).text(),
    Bun.file(resolve(repoRoot, '.github/workflows/quality.yml')).text(),
    Bun.file(resolve(repoRoot, 'scripts/VcpkgProvenance.ps1')).text(),
    Bun.file(resolve(repoRoot, 'apps/package.json')).text(),
    Bun.file(resolve(repoRoot, 'apps/rust-toolchain.toml')).text(),
    Bun.file(resolve(repoRoot, '.config/dotnet-tools.json')).text(),
  ]);

  return {
    failures: [
      ...macosViewerReleaseInputFailures(viewerPackage, viewerBuild),
      ...macosLibvpxReleaseInputFailures(macosAgentBuild, macosDocs),
      ...linuxRustupReleaseInputFailures(linuxBuild, windowsBuild),
      ...linuxBuildContextIsolationFailures(linuxBuild, windowsBuild),
      ...windowsPrerequisiteReleaseInputFailures(
        windowsBuild,
        bundleProject,
        installerDocs,
        riskRegister,
        wixToolManifest,
      ),
      ...windowsVcpkgReleaseInputFailures(
        setupScript,
        windowsBuild,
        qualityWorkflow,
        vcpkgProvenanceHelper,
      ),
      ...windowsDevelopmentToolchainFailures(setupScript, workspaceManifest, rustToolchain),
    ],
  };
}

if (import.meta.main) {
  const result = await checkReleaseInputContract();
  if (result.failures.length > 0) {
    console.error('Release input contract check failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log('Release input contract passed.');
}
