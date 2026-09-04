# ADR-0006: Cargo-owned Windows installer product versions

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

The Agent x86 MSI, Agent x64 MSI, Agent Burn bundle, Viewer MSI, and Viewer Burn authoring each
contained a manually copied product version. Those copies had already drifted from the versions of
the binaries the installer releases. A release could therefore publish update manifests and native
binaries with one version while Windows Installer used another for upgrade and downgrade ordering.

The Agent MSI initially installs `talos_supervisor`, but the installer represents the complete Agent
product and bootstraps the independently updated worker. Using the lower supervisor component
version would also move backwards from the highest previously authored Agent MSI version, causing
the existing `<MajorUpgrade>` policy to reject the package as a downgrade.

[Windows Installer ProductVersion](https://learn.microsoft.com/en-us/windows/win32/msi/productversion)
accepts a more restrictive product-version form than general Cargo SemVer: three numeric fields,
with major/minor at most 255 and build at most 65,535. It ignores a fourth field. The release path
must fail explicitly when a Cargo version cannot be represented instead of truncating or rewriting
it silently.

## Options considered

### Keep WiX versions synchronized by review

This preserves direct standalone WiX builds, but it is the process that allowed the current drift
and provides no deterministic release invariant.

### Maintain a separate installer release-version file

This removes duplicate values from individual WiX sources, but creates a second release identity
that can still disagree with binary and update-manifest versions.

### Inject Cargo package versions into WiX at build time

This keeps the native package manifest authoritative. WiX projects require an explicit MSBuild
property, while repository checks can verify both the active authoring inputs and build-script
wiring on every supported host.

## Decision

The Agent installer release identity is the `talos_worker` package version. Both Agent MSIs and the
Agent Burn bundle receive that value as `ProductVersion`. `talos_supervisor` remains an
independently versioned implementation component and does not control installer upgrade ordering.

The Viewer installer release identity is the `talos_viewer` Cargo package version. The Viewer MSI
and canonical Viewer Burn project use that value.

Canonical `.wxs` files reference only `$(var.ProductVersion)`. A shared
`apps/installer/Directory.Build.targets` maps the MSBuild property to the WiX preprocessor constant
and rejects a build that omits it. `scripts/build-installers.ps1` reads the Cargo manifests,
requires an unsigned `major.minor.build` value within Windows Installer's numeric limits, passes the
property to every active build, and treats the relevant manifest as a rebuild input.

A platform-neutral Bun check parses each active `.wixproj` compile item, rejects hand-maintained
versions and numbered-copy release inputs, validates current Cargo versions, and verifies the
PowerShell wiring. It runs through the normal repository static-check gate.

## Consequences

Positive:

- binary/update and Windows installation identities have one version source;
- Agent x86, Agent x64, and Burn cannot drift independently;
- missing, pre-release, four-component, or out-of-range versions fail before publishing;
- the contract is checked on Linux and macOS even when a Windows/WiX artifact build is unavailable.

Costs and limitations:

- direct `dotnet build` calls must supply `-p:ProductVersion=<major.minor.build>`;
- Cargo pre-release/build metadata cannot be used for a Windows installer release;
- the check cannot know versions already published outside the repository, so a release reviewer
  must confirm the Cargo version exceeds the highest deployed product version;
- a real MSI/Burn compilation and upgrade test still requires the supported Windows release host.

## Rollout

1. Replace numeric versions in canonical WiX authoring with the required preprocessor constant.
2. Derive Agent and Viewer values from `talos_worker` and `talos_viewer` respectively.
3. Pass those values to MSI/Burn builds and expose them in artifact metadata.
4. Land the cross-platform static check and regression tests before the next Windows release.
5. On Windows, build signed artifacts and verify an upgrade from the highest previously published
   Agent and Viewer installers before promotion.

The checked-in Cargo versions are higher than the previously hard-coded Agent and Viewer WiX
versions, preserving forward upgrade ordering for this transition.

## Rollback

Revert the build wiring and WiX variable use only before publishing artifacts from this decision.
After release, do not lower the stable `UpgradeCode` product version. Roll back application behavior
by reverting the code, incrementing the relevant Cargo package to a new higher version, and
publishing replacement MSI/Burn artifacts.
