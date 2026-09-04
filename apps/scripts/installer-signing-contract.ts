import { resolve } from 'node:path';

function occurrenceCount(contents: string, literal: string): number {
  return contents.split(literal).length - 1;
}

export function authenticodeResolutionFailures(contents: string): string[] {
  const failures: string[] = [];
  const resolution = '$cert = Get-CodeSigningCert -Thumbprint $authenticodeExpectedThumbprint';

  if (
    !contents.includes(
      '$authenticodeSigningEnabled = $SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning',
    )
  ) {
    failures.push(
      'build-installers.ps1 must derive Authenticode certificate resolution from the sign/skip switches',
    );
  }
  if (occurrenceCount(contents, resolution) !== 1) {
    failures.push(
      'build-installers.ps1 must resolve the Authenticode certificate at exactly one guarded location',
    );
  }
  if (!contents.includes('if ($externalAuthenticodeSignerPath) {')) {
    failures.push(
      'build-installers.ps1 must resolve a local Authenticode certificate only when signing is enabled without an external adapter',
    );
  }

  const externalSignerSnippets = [
    '[string]$ExternalAuthenticodeSignerPath',
    'function Get-ExternalAuthenticodeSignerPath',
    '-FilePath $pendingPaths',
    '-ExpectedCertificateThumbprint $normalizedExpectedThumbprint',
    '-TimestampServer $TimestampUrl',
    'Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint',
  ];
  for (const snippet of externalSignerSnippets) {
    if (!contents.includes(snippet)) {
      failures.push(
        `build-installers.ps1 is missing external Authenticode signer protection: ${snippet}`,
      );
    }
  }
  for (const snippet of [
    'if (-not $cert.HasPrivateKey)',
    '$now -lt $cert.NotBefore -or $now -gt $cert.NotAfter',
    '$codeSigningOid = "1.3.6.1.5.5.7.3.3"',
  ]) {
    if (!contents.includes(snippet)) {
      failures.push(
        `build-installers.ps1 is missing local Authenticode identity validation: ${snippet}`,
      );
    }
  }
  if (/ExternalAuthenticode[^\n]*(?:Password|PrivateKey|Pfx)/i.test(contents)) {
    failures.push(
      'the external Authenticode adapter contract must not accept private-key material',
    );
  }

  return failures;
}

export function authenticodeInstallerSequenceFailures(contents: string): string[] {
  const failures: string[] = [];
  const agentMsiPaths = '$wixMsiPaths += @($msiX86, $msiX64)';
  const viewerMsiPath = '$wixMsiPaths += $viewerMsiX64';
  const msiBuild = 'dotnet build $wixBuild.ProjectPath';
  const msiSigning = 'Sign-Binaries -Paths $wixMsiPaths';
  const bundleBuild = 'dotnet build $bundleProject';
  const bundleSigning = 'Sign-BurnBundle -BundlePath $bundleExe';
  const publication = 'Write-Step "Publishing installer artifacts to profile folder"';
  const artifactHashing = 'bundle = Get-ArtifactMetadata -Path $publishedBundleExe';
  const engineDetach = 'dotnet tool run wix -- burn detach $BundlePath -engine $detachedEnginePath';
  const engineSigning = 'Sign-Binaries -Paths @($detachedEnginePath)';
  const engineReattach =
    'dotnet tool run wix -- burn reattach $BundlePath -engine $detachedEnginePath -o $reattachedBundlePath';
  const outerBundleSigning = 'Sign-Binaries -Paths @($BundlePath)';
  const verificationDetach =
    'dotnet tool run wix -- burn detach $BundlePath -engine $verificationEnginePath';
  const embeddedEngineVerification =
    'Test-BinaryHasExpectedSignature -Path $verificationEnginePath -ExpectedThumbprint $ExpectedThumbprint';

  const requiredSnippets = [
    agentMsiPaths,
    viewerMsiPath,
    msiBuild,
    msiSigning,
    bundleBuild,
    '$needsBundle = $authenticodeSigningEnabled -or -not',
    'dotnet build $bundleProject -c Release --no-restore --no-incremental',
    bundleSigning,
    publication,
    artifactHashing,
    '$authenticodeSigningEnabled -and ($buildSupervisorArtifacts -or $buildViewerArtifacts)',
    '$authenticodeSigningEnabled -and $buildSupervisorArtifacts',
    'Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint',
    'Authenticode verification failed after signing',
    'function Sign-BurnBundle',
    'dotnet tool restore --tool-manifest $WixToolManifestPath',
    engineDetach,
    engineSigning,
    engineReattach,
    outerBundleSigning,
    verificationDetach,
    embeddedEngineVerification,
    'The final Burn bundle does not contain a valid engine signature',
  ];
  for (const snippet of requiredSnippets) {
    if (!contents.includes(snippet)) {
      failures.push(
        `build-installers.ps1 is missing final-installer signing protection: ${snippet}`,
      );
    }
  }

  const msiBuildIndex = contents.indexOf(msiBuild);
  const agentMsiPathsIndex = contents.indexOf(agentMsiPaths);
  const viewerMsiPathIndex = contents.indexOf(viewerMsiPath);
  const msiSigningIndex = contents.indexOf(msiSigning);
  const bundleBuildIndex = contents.indexOf(bundleBuild);
  const bundleSigningIndex = contents.indexOf(bundleSigning);
  const publicationIndex = contents.indexOf(publication);
  const artifactHashingIndex = contents.indexOf(artifactHashing);
  const engineDetachIndex = contents.indexOf(engineDetach);
  const engineSigningIndex = contents.indexOf(engineSigning);
  const engineReattachIndex = contents.indexOf(engineReattach);
  const outerBundleSigningIndex = contents.indexOf(outerBundleSigning);
  const verificationDetachIndex = contents.indexOf(verificationDetach);
  const embeddedEngineVerificationIndex = contents.indexOf(embeddedEngineVerification);

  if (
    [
      engineDetachIndex,
      engineSigningIndex,
      engineReattachIndex,
      outerBundleSigningIndex,
      verificationDetachIndex,
      embeddedEngineVerificationIndex,
    ].every((index) => index >= 0) &&
    !(
      engineDetachIndex < engineSigningIndex &&
      engineSigningIndex < engineReattachIndex &&
      engineReattachIndex < outerBundleSigningIndex &&
      outerBundleSigningIndex < verificationDetachIndex &&
      verificationDetachIndex < embeddedEngineVerificationIndex
    )
  ) {
    failures.push(
      'Burn signing must detach and sign the engine, reattach it, sign the outer bundle, then verify the embedded engine',
    );
  }

  if (msiBuildIndex >= 0 && msiSigningIndex >= 0 && !(msiBuildIndex < msiSigningIndex)) {
    failures.push('final WiX MSI outputs must be signed after MSI construction');
  }
  if (
    agentMsiPathsIndex >= 0 &&
    viewerMsiPathIndex >= 0 &&
    msiSigningIndex >= 0 &&
    !(agentMsiPathsIndex < msiSigningIndex && viewerMsiPathIndex < msiSigningIndex)
  ) {
    failures.push(
      'Agent x86/x64 and Viewer MSI outputs must all be selected before the MSI signing phase',
    );
  }
  if (msiSigningIndex >= 0 && bundleBuildIndex >= 0 && !(msiSigningIndex < bundleBuildIndex)) {
    failures.push('final WiX MSI outputs must be signed before the Burn bundle embeds them');
  }
  if (
    bundleBuildIndex >= 0 &&
    bundleSigningIndex >= 0 &&
    !(bundleBuildIndex < bundleSigningIndex)
  ) {
    failures.push('the final Burn bundle must be signed after Burn constructs it');
  }
  if (
    bundleSigningIndex >= 0 &&
    publicationIndex >= 0 &&
    !(bundleSigningIndex < publicationIndex)
  ) {
    failures.push('the final Burn bundle must be signed before installer publication');
  }
  if (
    bundleSigningIndex >= 0 &&
    artifactHashingIndex >= 0 &&
    !(bundleSigningIndex < artifactHashingIndex)
  ) {
    failures.push('the final Burn bundle must be signed before artifact manifest hashing');
  }

  return failures;
}

export function wixToolManifestFailures(contents: string): string[] {
  try {
    const manifest = JSON.parse(contents) as {
      version?: number;
      isRoot?: boolean;
      tools?: Record<string, { version?: string; commands?: string[] }>;
    };
    if (manifest.version !== 1 || manifest.isRoot !== true) {
      return ['.config/dotnet-tools.json must be a version-1 root tool manifest'];
    }
    const wix = manifest.tools?.wix;
    if (wix?.version !== '6.0.0') {
      return ['.config/dotnet-tools.json must pin the WiX CLI exactly at 6.0.0'];
    }
    if (!Array.isArray(wix.commands) || wix.commands.length !== 1 || wix.commands[0] !== 'wix') {
      return ['.config/dotnet-tools.json must expose only the pinned wix command'];
    }
    return [];
  } catch {
    return ['.config/dotnet-tools.json must be valid JSON'];
  }
}

export function manifestPfxWorkflowFailures(contents: string): string[] {
  const failures: string[] = [];
  const requiredSnippets = [
    '[string]$ManifestCertificatePath',
    '[System.Security.SecureString]$ManifestCertificatePassword',
    'function Get-ManifestSigningCert',
    'Resolve-Path -LiteralPath $CertificatePath',
    'Manifest signing PFX must remain outside the Talos repository',
    'X509KeyStorageFlags]::EphemeralKeySet',
    'Assert-ManifestSigningCertificate -Cert $cert',
    '-Label "-ManifestCertificateThumbprint"',
    '$rsa.KeySize -lt 2048 -or $rsa.KeySize -gt 8192',
    'Export-PublicKeyDer -Cert $manifestCert',
    'Manifest public key SHA-256:',
    'Sign-ManifestFile -ManifestPath',
  ];
  for (const snippet of requiredSnippets) {
    if (!contents.includes(snippet)) {
      failures.push(`build-installers.ps1 is missing manifest-signing protection: ${snippet}`);
    }
  }
  if (
    contents.includes('SkipManifestSigning') ||
    contents.includes('DisableManifestVerification')
  ) {
    failures.push('manifest signing and verification must not gain a bypass switch');
  }
  if (contents.includes('-FallbackThumbprint $CertificateThumbprint')) {
    failures.push(
      'skipping Authenticode must not make the Authenticode thumbprint an implicit manifest-key input',
    );
  }
  return failures;
}

export function updaterManifestEncodingFailures(contents: string): string[] {
  const failures: string[] = [];
  for (const snippet of [
    'function Write-Utf8NoBomJson([string]$Path, [string]$Json)',
    '$Json + [Environment]::NewLine',
    '[System.Text.UTF8Encoding]::new($false)',
  ]) {
    if (!contents.includes(snippet)) {
      failures.push(`build-installers.ps1 is missing UTF-8 manifest protection: ${snippet}`);
    }
  }
  if (/Set-Content[^\r\n]*-Encoding\s+UTF8/i.test(contents)) {
    failures.push(
      'build-installers.ps1 must not use Windows PowerShell UTF8 encoding that adds a manifest BOM',
    );
  }
  const jsonDocuments = occurrenceCount(contents, 'ConvertTo-Json -Depth 8');
  const noBomWrites = occurrenceCount(contents, 'Write-Utf8NoBomJson -Path');
  if (jsonDocuments === 0 || noBomWrites !== jsonDocuments) {
    failures.push(
      'every installer JSON document must be persisted through the UTF-8-no-BOM writer',
    );
  }
  return failures;
}

export function viewerManifestKeyEmbeddingFailures(
  buildScript: string,
  keySupport: string,
): string[] {
  const failures: string[] = [];
  for (const snippet of [
    'env::var_os("RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH")',
    'persist_validated_pkcs1_rsa_public_key',
    'manifest signing public key cannot be embedded',
    'expect("write empty embedded manifest public key DER")',
  ]) {
    if (!buildScript.includes(snippet)) {
      failures.push(`viewer build.rs is missing fail-closed key embedding: ${snippet}`);
    }
  }
  for (const snippet of [
    'fs::read(source)',
    'validate_pkcs1_rsa_public_key_der(&bytes)',
    'fs::write(target, &bytes)',
    'fs::read(target)',
    'if persisted != bytes',
    '(2048..=8192).contains(&modulus_bits)',
    'exponent < 3 || exponent % 2 == 0',
  ]) {
    if (!keySupport.includes(snippet)) {
      failures.push(`viewer manifest-key support is missing validation: ${snippet}`);
    }
  }
  if (/let _ = fs::write|if let Ok\(bytes\) = fs::read/.test(buildScript)) {
    failures.push('viewer build.rs must not replace a configured unreadable key with empty bytes');
  }
  return failures;
}

export function bootstrapFailures(contents: string): string[] {
  const failures: string[] = [];
  const requiredSnippets = [
    '[System.Security.SecureString]$Password',
    '$Password.Length -lt 16',
    'Refusing to overwrite existing manifest signing key',
    'Refusing to create a manifest signing private key inside the Talos repository',
    '-KeyAlgorithm RSA',
    '-KeyLength 3072',
    '-HashAlgorithm SHA256',
    '-KeyUsage DigitalSignature',
    'Export-PfxCertificate',
    '-DeleteKey',
    'Move-Item -LiteralPath $temporaryPfxPath -Destination $fullOutputPath',
    'Manifest public key SHA-256:',
  ];
  for (const snippet of requiredSnippets) {
    if (!contents.includes(snippet)) {
      failures.push(`Community manifest-key bootstrap is missing protection: ${snippet}`);
    }
  }
  return failures;
}

export function signingDocumentationFailures(contents: string): string[] {
  const requiredSnippets = [
    '-SkipAuthenticodeSigning',
    '-ManifestCertificatePath',
    '-ManifestCertificatePassword',
    'does not disable manifest signing',
    'pinned public key',
    'New-CommunityManifestSigningCertificate.ps1',
    'Agent x86/x64 and Viewer MSI outputs',
    'before Burn embeds',
    'completed Burn bundle',
    'fails closed',
    'Burn requires two Authenticode signatures',
    '.config/dotnet-tools.json',
    'detaches and signs the engine',
    'https://docs.firegiant.com/wix/tools/signing/#signing-bundles',
    '-ExternalAuthenticodeSignerPath',
    'UNSIGNED-BINARIES.txt',
    'SHA256SUMS',
  ];
  return requiredSnippets
    .filter((snippet) => !contents.includes(snippet))
    .map((snippet) => `installer signing documentation is missing: ${snippet}`);
}

export function artifactIntegrityFailures(contents: string): string[] {
  const requiredSnippets = [
    'function Write-UnsignedArtifactNotice',
    'function Write-ArtifactChecksums',
    'UNSIGNED-BINARIES.txt',
    'SHA256SUMS',
    'build-provenance.json',
    'local-build-metadata-not-cryptographic-attestation',
    'windowsAuthenticodeStatus',
    'updaterManifestPublicKeySha256',
  ];
  return requiredSnippets
    .filter((snippet) => !contents.includes(snippet))
    .map((snippet) => `installer artifact integrity output is missing: ${snippet}`);
}

export function unsignedReleaseSurfaceFailures(
  rootReadme: string,
  installerPage: string,
  releaseTemplate: string,
  releaseGuide: string,
): string[] {
  const failures: string[] = [];
  for (const [label, contents, snippets] of [
    [
      'root README',
      rootReadme,
      ['intentionally **unsigned**', 'SmartScreen', 'SHA256SUMS', 'do not disable'],
    ],
    [
      'installer download page',
      installerPage,
      ['intentionally unsigned', 'SmartScreen', 'SHA256SUMS', 'not disable security controls'],
    ],
    [
      'release notes template',
      releaseTemplate,
      ['Unsigned binaries', 'Unknown publisher', 'SHA256SUMS', 'Secret scan'],
    ],
    [
      'release signing guide',
      releaseGuide,
      [
        'Updater-manifest release-line key',
        'separate protected systems',
        'ExternalAuthenticodeSignerPath',
        'not a SLSA claim',
        'Never lend or share another organisation',
        "share another organisation's signing identity",
      ],
    ],
  ] as const) {
    const normalizedContents = contents.replace(/\s+/g, ' ');
    for (const snippet of snippets) {
      if (!normalizedContents.includes(snippet)) {
        failures.push(`${label} is missing unsigned-release guidance: ${snippet}`);
      }
    }
  }
  return failures;
}

export type InstallerSigningContractResult = {
  failures: string[];
};

export async function checkInstallerSigningContract(
  repoRoot = resolve(import.meta.dir, '../..'),
): Promise<InstallerSigningContractResult> {
  const [
    buildScript,
    bootstrapScript,
    documentation,
    wixToolManifest,
    rootReadme,
    installerPage,
    releaseTemplate,
    releaseGuide,
    viewerBuildScript,
    viewerManifestKeySupport,
  ] = await Promise.all([
    Bun.file(resolve(repoRoot, 'scripts/build-installers.ps1')).text(),
    Bun.file(resolve(repoRoot, 'scripts/New-CommunityManifestSigningCertificate.ps1')).text(),
    Bun.file(resolve(repoRoot, 'apps/installer/README.md')).text(),
    Bun.file(resolve(repoRoot, '.config/dotnet-tools.json')).text(),
    Bun.file(resolve(repoRoot, 'README.md')).text(),
    Bun.file(
      resolve(repoRoot, 'apps/frontend/src/routes/dashboard/rmm/installers/+page.svelte'),
    ).text(),
    Bun.file(resolve(repoRoot, '.github/RELEASE_TEMPLATE.md')).text(),
    Bun.file(resolve(repoRoot, 'docs/release-signing.md')).text(),
    Bun.file(resolve(repoRoot, 'apps/talos_viewer/src-tauri/build.rs')).text(),
    Bun.file(resolve(repoRoot, 'apps/talos_viewer/src-tauri/build_manifest_public_key.rs')).text(),
  ]);

  return {
    failures: [
      ...authenticodeResolutionFailures(buildScript),
      ...authenticodeInstallerSequenceFailures(buildScript),
      ...wixToolManifestFailures(wixToolManifest),
      ...manifestPfxWorkflowFailures(buildScript),
      ...updaterManifestEncodingFailures(buildScript),
      ...viewerManifestKeyEmbeddingFailures(viewerBuildScript, viewerManifestKeySupport),
      ...bootstrapFailures(bootstrapScript),
      ...signingDocumentationFailures(documentation),
      ...artifactIntegrityFailures(buildScript),
      ...unsignedReleaseSurfaceFailures(rootReadme, installerPage, releaseTemplate, releaseGuide),
    ],
  };
}

if (import.meta.main) {
  const result = await checkInstallerSigningContract();
  if (result.failures.length > 0) {
    console.error('Installer signing contract check failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log('Installer signing contract passed.');
}
