import { describe, expect, test } from 'bun:test';
import {
  artifactIntegrityFailures,
  authenticodeInstallerSequenceFailures,
  authenticodeResolutionFailures,
  bootstrapFailures,
  checkInstallerSigningContract,
  manifestPfxWorkflowFailures,
  updaterManifestEncodingFailures,
  unsignedReleaseSurfaceFailures,
  viewerManifestKeyEmbeddingFailures,
  wixToolManifestFailures,
} from './installer-signing-contract';

const validFinalInstallerSigningSequence = `
  function Sign-BurnBundle
  dotnet tool restore --tool-manifest $WixToolManifestPath
  dotnet tool run wix -- burn detach $BundlePath -engine $detachedEnginePath
  Sign-Binaries -Paths @($detachedEnginePath)
  dotnet tool run wix -- burn reattach $BundlePath -engine $detachedEnginePath -o $reattachedBundlePath
  Sign-Binaries -Paths @($BundlePath)
  dotnet tool run wix -- burn detach $BundlePath -engine $verificationEnginePath
  Test-BinaryHasExpectedSignature -Path $verificationEnginePath -ExpectedThumbprint $ExpectedThumbprint
  The final Burn bundle does not contain a valid engine signature
  $authenticodeSigningEnabled -and ($buildSupervisorArtifacts -or $buildViewerArtifacts)
  $authenticodeSigningEnabled -and $buildSupervisorArtifacts
  Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint
  throw "Authenticode verification failed after signing"
  dotnet build $wixBuild.ProjectPath
  $wixMsiPaths += @($msiX86, $msiX64)
  $wixMsiPaths += $viewerMsiX64
  Sign-Binaries -Paths $wixMsiPaths
  $needsBundle = $authenticodeSigningEnabled -or -not
  dotnet build $bundleProject -c Release --no-restore --no-incremental
  Sign-BurnBundle -BundlePath $bundleExe
  Write-Step "Publishing installer artifacts to profile folder"
  bundle = Get-ArtifactMetadata -Path $publishedBundleExe
`;

describe('installer signing contract', () => {
  test('rejects the former unconditional Authenticode certificate lookup', () => {
    expect(
      authenticodeResolutionFailures(`
        if (-not $isBinaryOnlyBuild) {
          $cert = Get-CodeSigningCert -Thumbprint $CertificateThumbprint
        }
      `),
    ).toContain(
      'build-installers.ps1 must resolve the Authenticode certificate at exactly one guarded location',
    );
  });

  test('requires an external signer adapter to expose only public signing inputs', () => {
    const failures = authenticodeResolutionFailures(`
      $authenticodeSigningEnabled = $SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning
      $cert = Get-CodeSigningCert -Thumbprint $authenticodeExpectedThumbprint
      if ($externalAuthenticodeSignerPath) {}
      [string]$ExternalAuthenticodeSignerPath
      function Get-ExternalAuthenticodeSignerPath {}
      -FilePath $pendingPaths
      -ExpectedCertificateThumbprint $normalizedExpectedThumbprint
      -TimestampServer $TimestampUrl
      Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint
      ExternalAuthenticodePrivateKeyPassword
    `);

    expect(failures).toContain(
      'the external Authenticode adapter contract must not accept private-key material',
    );
  });

  test('requires every final WiX installer output in the signing phases', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace('$wixMsiPaths += $viewerMsiX64', ''),
    );

    expect(failures).toContain(
      'build-installers.ps1 is missing final-installer signing protection: $wixMsiPaths += $viewerMsiX64',
    );
  });

  test('rejects signing MSIs after Burn has embedded them', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace(
        'Sign-Binaries -Paths $wixMsiPaths\n  $needsBundle = $authenticodeSigningEnabled -or -not\n  dotnet build $bundleProject -c Release --no-restore --no-incremental',
        '$needsBundle = $authenticodeSigningEnabled -or -not\n  dotnet build $bundleProject -c Release --no-restore --no-incremental\n  Sign-Binaries -Paths $wixMsiPaths',
      ),
    );

    expect(failures).toContain(
      'final WiX MSI outputs must be signed before the Burn bundle embeds them',
    );
  });

  test('requires a non-incremental Burn rebuild for signed installer builds', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence
        .replace('$needsBundle = $authenticodeSigningEnabled -or -not', '')
        .replace(' --no-incremental', ''),
    );

    expect(failures).toContain(
      'build-installers.ps1 is missing final-installer signing protection: $needsBundle = $authenticodeSigningEnabled -or -not',
    );
    expect(failures).toContain(
      'build-installers.ps1 is missing final-installer signing protection: dotnet build $bundleProject -c Release --no-restore --no-incremental',
    );
  });

  test('rejects signing the Burn output before Burn constructs it', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace(
        '$needsBundle = $authenticodeSigningEnabled -or -not\n  dotnet build $bundleProject -c Release --no-restore --no-incremental\n  Sign-BurnBundle -BundlePath $bundleExe',
        'Sign-BurnBundle -BundlePath $bundleExe\n  $needsBundle = $authenticodeSigningEnabled -or -not\n  dotnet build $bundleProject -c Release --no-restore --no-incremental',
      ),
    );

    expect(failures).toContain('the final Burn bundle must be signed after Burn constructs it');
  });

  test('rejects reattaching the Burn engine before signing it', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace(
        'Sign-Binaries -Paths @($detachedEnginePath)\n  dotnet tool run wix -- burn reattach $BundlePath -engine $detachedEnginePath -o $reattachedBundlePath',
        'dotnet tool run wix -- burn reattach $BundlePath -engine $detachedEnginePath -o $reattachedBundlePath\n  Sign-Binaries -Paths @($detachedEnginePath)',
      ),
    );

    expect(failures).toContain(
      'Burn signing must detach and sign the engine, reattach it, sign the outer bundle, then verify the embedded engine',
    );
  });

  test('rejects signing the Burn output after publication or hashing', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace(
        'Sign-BurnBundle -BundlePath $bundleExe\n  Write-Step "Publishing installer artifacts to profile folder"\n  bundle = Get-ArtifactMetadata -Path $publishedBundleExe',
        'Write-Step "Publishing installer artifacts to profile folder"\n  bundle = Get-ArtifactMetadata -Path $publishedBundleExe\n  Sign-BurnBundle -BundlePath $bundleExe',
      ),
    );

    expect(failures).toContain('the final Burn bundle must be signed before installer publication');
    expect(failures).toContain(
      'the final Burn bundle must be signed before artifact manifest hashing',
    );
  });

  test('requires post-signtool signature verification for fail-closed releases', () => {
    const failures = authenticodeInstallerSequenceFailures(
      validFinalInstallerSigningSequence.replace(
        'Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint',
        '',
      ),
    );

    expect(failures).toContain(
      'build-installers.ps1 is missing final-installer signing protection: Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint',
    );
  });

  test('pins the WiX CLI used for Burn detach and reattach', () => {
    expect(
      wixToolManifestFailures(
        JSON.stringify({
          version: 1,
          isRoot: true,
          tools: { wix: { version: 'latest', commands: ['wix'] } },
        }),
      ),
    ).toContain('.config/dotnet-tools.json must pin the WiX CLI exactly at 6.0.0');
  });

  test('requires an ephemeral, bounded RSA manifest-key input', () => {
    expect(manifestPfxWorkflowFailures('function Get-ManifestSigningCert {}')).toContain(
      'build-installers.ps1 is missing manifest-signing protection: X509KeyStorageFlags]::EphemeralKeySet',
    );
  });

  test('rejects Windows PowerShell UTF-8 BOM output for signed manifests', () => {
    const noBomWriter = `
      function Write-Utf8NoBomJson([string]$Path, [string]$Json) {
        [System.IO.File]::WriteAllText(
          $Path,
          $Json + [Environment]::NewLine,
          [System.Text.UTF8Encoding]::new($false)
        )
      }
      $manifestJson = $manifest | ConvertTo-Json -Depth 8
      Write-Utf8NoBomJson -Path $manifestPath -Json $manifestJson
    `;
    expect(updaterManifestEncodingFailures(noBomWriter)).toEqual([]);

    const failures = updaterManifestEncodingFailures(
      noBomWriter.replace(
        'Write-Utf8NoBomJson -Path $manifestPath -Json $manifestJson',
        'Set-Content -Path $manifestPath -Value $manifestJson -Encoding UTF8',
      ),
    );
    expect(failures).toContain(
      'build-installers.ps1 must not use Windows PowerShell UTF8 encoding that adds a manifest BOM',
    );
    expect(failures).toContain(
      'every installer JSON document must be persisted through the UTF-8-no-BOM writer',
    );
  });

  test('rejects silently embedding an empty viewer update key after configured-key failure', () => {
    const failures = viewerManifestKeyEmbeddingFailures(
      `
        if let Ok(path) = env::var("RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH") {
          if let Ok(bytes) = fs::read(path) {
            let _ = fs::write(&manifest_key_output, bytes);
          } else {
            let _ = fs::write(&manifest_key_output, []);
          }
        }
      `,
      '',
    );

    expect(failures).toContain(
      'viewer build.rs must not replace a configured unreadable key with empty bytes',
    );
    expect(failures).toContain(
      'viewer manifest-key support is missing validation: validate_pkcs1_rsa_public_key_der(&bytes)',
    );
  });

  test('rejects an implicit fallback to the Authenticode certificate', () => {
    expect(
      manifestPfxWorkflowFailures(
        'Get-ManifestSigningCert -FallbackThumbprint $CertificateThumbprint',
      ),
    ).toContain(
      'skipping Authenticode must not make the Authenticode thumbprint an implicit manifest-key input',
    );
  });

  test('requires the bootstrap to refuse key overwrite', () => {
    expect(bootstrapFailures('Export-PfxCertificate')).toContain(
      'Community manifest-key bootstrap is missing protection: Refusing to overwrite existing manifest signing key',
    );
  });

  test('requires unsigned artifacts to carry checksums and explicit trust metadata', () => {
    const failures = artifactIntegrityFailures('function Write-ArtifactChecksums { "SHA256SUMS" }');

    expect(failures).toContain(
      'installer artifact integrity output is missing: function Write-UnsignedArtifactNotice',
    );
    expect(failures).toContain(
      'installer artifact integrity output is missing: build-provenance.json',
    );
  });

  test('rejects download and release surfaces that hide unsigned status', () => {
    const failures = unsignedReleaseSurfaceFailures('', '', '', '');

    expect(failures).toContain(
      'root README is missing unsigned-release guidance: intentionally **unsigned**',
    );
    expect(failures).toContain(
      'installer download page is missing unsigned-release guidance: SHA256SUMS',
    );
  });

  test('the tracked installer signing workflow satisfies the contract', async () => {
    expect((await checkInstallerSigningContract()).failures).toEqual([]);
  });
});
