# Licensing and provenance inventory

- Status: **engineering inventory; qualified legal and owner review still required**
- Selected first-party source licence: `AGPL-3.0-only`
- Last reviewed: 2026-08-28

This inventory separates first-party Talos work from generated output, third-party source, binary
inputs, and media. It is intended to prevent a root licence from being applied accidentally to
material Talos does not own.

The JPEG files under `docs/screenshots/community-edition/` were captured and supplied by the Talos
project owner on 2026-08-29 from an alpha development build containing synthetic demonstration
data. They carry no embedded EXIF metadata. Their exact SHA-256 values are recorded in the public
export policy so changed or additional screenshots require a fresh privacy and rights review.

## First-party source and documentation

Publishable Talos-authored source and documentation outside the exceptions below use
`AGPL-3.0-only`. JavaScript package manifests and first-party Cargo workspace members carry matching
SPDX metadata. The canonical GNU AGPL version 3 text is the root `LICENSE` file (SHA-256
`0d96a4ff68ad6d4b6f1f30f713b18d5184912ba8dd389f86aa7710db079abcb0`).

The owner confirmed the first-party copyright-holder name as **Sebastian Orbe** on 2026-09-04.
The selected public corresponding-source URL is https://github.com/seborbie/talos-community.
These values are recorded in `.config/publication-identity.json`. The owner approved the final
README and source-alpha publication on 2026-09-04. Outstanding qualified reviews remain tracked in
[PUB-001](https://github.com/seborbie/talos-community/issues/1); approval is not legal certification. Adding AGPL rights does not transfer
copyright ownership. Third-party and contributed work retains its respective copyright holders.

External contributions use inbound-equals-outbound `AGPL-3.0-only` terms. Contributors retain
copyright in their work and grant no separate proprietary relicensing rights merely by opening a
pull request; see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

## Generated material

| Material                                                                             | Source of truth                               | Publication treatment                                                                                                                                                                 |
| ------------------------------------------------------------------------------------ | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `apps/talos_protocol_types/src/index.ts`                                             | `apps/talos_protocol` export generator        | First-party generated source under `AGPL-3.0-only`; regenerate and verify with `bun --cwd apps run contracts:check`.                                                                  |
| Prisma client under `apps/node_modules/.bun/**/node_modules/.prisma/`                | Prisma schema and the pinned Prisma toolchain | Generated dependency/build output; do not include `node_modules` in a source snapshot. The licence gate excludes this generated package manifest and reviews `@prisma/client` itself. |
| JavaScript `dist/`, Svelte/Tauri build trees, Cargo `target/`, WiX `bin/` and `obj/` | Their source and pinned build tools           | Generated output; exclude unless a release process intentionally packages it with licence, notices, checksums, source location, and provenance.                                       |

## Vendored and copied third-party source

The authoritative human-readable notices are in [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
The licence gate verifies the following identities and retained evidence:

| Repository path                             | Package                                       | Evidence                                                                                                                                                                                                                                              | Status                                                                                                                                                                                                                                                                                   |
| ------------------------------------------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `apps/vendor/dxgi-capture-rs/`              | `dxgi-capture-rs` 1.1.7                       | revision `4d7e1a651afd248d4d1ef4401033d60efbdf5a91`; MIT licence SHA-256 `e187671b3afebf4f9d85a0a0b87f8b1ba4aa56dae1cc6f97d52e83cd45ddd04f`                                                                                                           | Licence and upstream revision retained; local changes need a release diff review.                                                                                                                                                                                                        |
| `apps/vendor/permission-flow/`              | `permission-flow` 0.1.40                      | revision `b6e15e34b6892598a576d4f0a50ebf7a6af71382`; MIT licence SHA-256 `d65f906c8116c14921f841867969d6dc3f9dd7b99fca34071671ff5896a3fa94`                                                                                                           | Upstream package metadata records a dirty source; document the exact local patch before release.                                                                                                                                                                                         |
| `apps/vendor/tauri-plugin-permission-flow/` | `tauri-plugin-permission-flow` 0.1.40         | same revision and shared upstream MIT licence                                                                                                                                                                                                         | Document the exact local patch before release.                                                                                                                                                                                                                                           |
| `apps/vendor/samsa/`                        | `samsa` 0.1.8                                 | revision `33062fce28a7b14a631496409841e252ef7937c4`; Apache-2.0 licence SHA-256 `106f9576da64a4d8240850a3fee0672f275f6041fd47901e7e06dcfeb3f0b9b9`                                                                                                    | Licence retained; review and describe the Talos consumer-group patch.                                                                                                                                                                                                                    |
| `apps/vpx-encode/`                          | patched `vpx-encode`, locally versioned 0.6.5 | upstream tag `vpx-encode/v0.6.2`, revision `5519bec430184208ee33221ddc727ccb9429b88e`; published crate SHA-256 `cd1f41af42de7667cbdba44e8c38c36e9ede970f1291c32ea57c0ded8eb6f4b6`; Cargo manifest names the upstream project/authors and declares MIT | Exact source baseline and output-file hashes recovered. The reconstructed tree is excluded from the public snapshot; `.config/third-party-acquisition.json` plus `apps/third-party-patches/vpx-encode-0.6.2-talos.patch` fetch and reproduce it. Qualified notice review remains required before distributing the reconstructed source or linked binaries because upstream ships no standalone copyright/licence file. |

The acquisition command accepts only the crates.io 0.6.2 archive above, verifies the patch SHA-256
`da4a296ccaef077b6eb68b15fa67cee0cbd296cf5c18dc6590c250236956aaf1`, extracts three reviewed
files, applies the patch, and verifies each resulting file. It refuses symlinks, additions, changed
files, and unexpected pre-existing output. The result is byte-for-byte identical to the private
workspace copy. This closes the reproducible-source engineering gap without copying the fork into
the public snapshot or fabricating a notice.

Upstream derives the encoder from Ram Kaniyur's `quadrupleslap/srs` repository, whose reviewed head
is `ee32d68928fb16b631ecce09f758cc58b2857d2f`; that source also declares MIT in `Cargo.toml`
without shipping a standalone licence file. This establishes provenance, but not a missing
copyright notice. A qualified reviewer or authoritative upstream notice is still required before
redistributing reconstructed source or linked release binaries.

The two binary WebAssembly fixtures under `apps/vendor/samsa/testdata/` are byte-identical to
CallistoLabsNYC/samsa revision `33062fce28a7b14a631496409841e252ef7937c4`:

- `add_one.wasm`: SHA-256
  `8f581f8899890a7479c20494f53cc5d60156a4d42e39d6a587a9f3089f198fe0`;
- `redpanda-identity.wasm`: SHA-256
  `ecf48344f9205472b9eae291119c1bacbd198fb389b20e00584873f698915a50`.

They remain covered by samsa's retained Apache-2.0 licence and are the only WebAssembly
source-tree fixtures permitted by the Community export policy.

`apps/vendor/tauri-plugin-permission-flow/package-lock.json` and the two vendored Cargo lockfiles are
upstream material, not first-party workspace lockfiles. They are excluded from Bun workspace rules
but must be assessed when deciding whether to retain complete upstream package trees in the public
snapshot.

## Tracked binary and installer material

Historical binary/cache files remain in the private worktree, but the public exporter excludes
them. They are not active public-source inputs and are replaced by the deterministic acquisition
policy below.

| Path                                                                                                                | SHA-256                                                                                                                                                                                                                                                                                                         | Required action                                                                                                                                                                                                                                                               |
| ------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `apps/installer/.wix/extensions/WixToolset.Bal.wixext/6.0.2/wixext6/WixToolset.BootstrapperApplications.wixext.dll` | `0a0bdeec4e6378a7311cd5ad41157e0688a7764be57e87640ac2cb8cf122b48f`                                                                                                                                                                                                                                              | Unused historical 6.0.2 cache; excluded. Active projects and tool manifest use digest-pinned 6.0.0 packages acquired into an ignored local feed. |
| `apps/installer/artifacts/sfx/7zSD.sfx`                                                                             | `d47d81ca4233816b5bf66977c92c55773ae8241d03d108e4477abb49333c2522`                                                                                                                                                                                                                                              | Byte-identical to `bin/7zSD.sfx` from official LZMA SDK 26.00; excluded and recreated from the verified upstream archive. |
| `apps/installer/msi/_ui.wixlib`                                                                                     | `82cc47b2fd3e70742ec8b5c4fbb052113016af2bde91df15ff8eb141fa2b9851`                                                                                                                                                                                                                                              | Generated WiX output; excluded and regenerated from the pinned toolchain. |
| `apps/installer/msi/_ui_extract/`                                                                                   | `uica.dll` (Arm64) `70db7a0e19297631bffc0d4b475f9989a3ed7bd17b0151809ad6b63d06f64e63`; `uica.dll-1` (x64) `4c9a379f1ef9df659bd410a7b7d530378cd03c943799ad0efe97742af5120ca8`; `uica.dll-2` (x86) `8fbbc51e46e5a17db60348032fe55868d2fe26bfc371166dc9be8c691147d310`; plus extracted JSON/bitmap/icon/RTF assets | Generated/extracted WiX output; excluded and not a build input. |

The official LZMA SDK 26.00 archive is pinned by SHA-256
`6b7d0c8ed1a67112d5337e4532ecdcb9fd2eab8b1f6bb54199f9b6a627b506cc`; its exact SFX member
and public-domain notice are verified. The official 7-Zip Extra 26.00 archive is pinned by SHA-256
`1cc38a9e3777ce0e4bbf84475672888a581d400633b0448fd973a7a6aa56cfdc`; its `7za.exe` build
tool and LGPL/BSD notice are verified.

The six official WiX 6.0.0 NuGet packages are bound to upstream revision
`8c7432e50072e009353ea5f2c956ccf453476f71`, individually SHA-256 pinned, and restored only from
the generated local feed. They embed `OSMFEULA.txt` with SHA-256
`d7383eccaa4f11f856ce5767864315265f164f85f03e50a5be8cf1e7da302996`. That agreement states
maintenance-fee conditions for revenue-generating use of official WiX binary releases; builders
must review and satisfy its current terms. A readable copy is retained at
`apps/installer/third-party/wix-6.0.0/OSMFEULA.txt` rather than treating WiX as AGPL-covered.

Installer builds also download or compile third-party inputs that are not committed: Microsoft VC++
Redistributables, the evergreen WebView2 bootstrapper, libvpx 1.13.0, Rust toolchains, and target
build tools. Authentication, digest pinning, and risk exceptions are documented elsewhere, but
release packaging must additionally carry the vendor-required notices and source offers. Do not
copy a vendor binary into the source repository merely to make installation convenient.

## Media and validation artifacts

The Community Edition uses the deterministic first-party icon source at
`apps/brand/talos-app-icon.svg`. It carries an `AGPL-3.0-only` SPDX identifier and is converted by
the Tauri CLI into the three native formats copied into each Tauri application. The canonical
generated hashes are:

- SVG source: `67af677ac516efc85822b4a6ffdb7365f82f3e97a7d49a6f11ab9dd87b231c28`;
- ICNS: `c0cbc0dda1835cdbb623497cd9733e83e94e1fe7f69a23d8aadfa360c5ead7c6`;
- ICO: `1f53ad648f5e129f277b0f8ed2d58b11e637f5da3f738b2eec84bb9e6746ec4a`;
- PNG: `108e8652e711173fd1dbf11023dfd40b6fdd3e5428289ab84931b5a74962e361`.

The export policy binds the source and all nine generated copies to these reviewed hashes.
Documentation screenshots under `docs/pr-artifacts/` and `docs/validation/` still require a visual
review for customer data, personal information, credentials, hostnames, and third-party marks
before they enter the public snapshot.

The five scaffold SVGs under `apps/frontend/public/` are byte-identical to the matching
`packages/create-next-app/templates/app/ts/public/` assets in `vercel/next.js` revision
`357e514cf7bdc276bfd830003c38027c17d9de05`. That revision's MIT notice is retained at
`apps/frontend/public/NEXTJS_TEMPLATE_ASSET_LICENSE.md` (SHA-256
`ee765244e2d59f5234d474f62e0766fa0c8b99af967fdd4c0cb8dcb0c76ea224`). Their exact hashes are
allowlisted by the exporter:

| Reviewed Next.js template artwork | SHA-256                                                            |
| --------------------------------- | ------------------------------------------------------------------ |
| `apps/frontend/public/file.svg`   | `2b67812c325c199a02536cdbeea0c593a72f707d323b72ee3e08dbab06753bd4` |
| `apps/frontend/public/globe.svg`  | `b614b9bf183925957661ac851498fe1d8029fd43a62fbfed86f9e2624a57e7cf` |
| `apps/frontend/public/next.svg`   | `55995dfad6ecb4945a1e856ddca03c5e16aa5bf13fd21b4df6a74ae79357bcfc` |
| `apps/frontend/public/vercel.svg` | `f081337b2fee635b455b63275406a3e7f39d6a014e25ad90dab5a67e62a12ac4` |
| `apps/frontend/public/window.svg` | `644768c4aaeb4767bce293344eeb0c125fb804a94d801440424072202d85e3a1` |

The former provenance-unverified cursor artwork is no longer a build input. Live desktop frames
use the MIT-licensed `MousePointer2` component from the pinned `lucide-svelte` dependency, and the
exporter retains a blocked-path guard against reintroducing the old file.

## Dependency discovery and drift control

From `apps/`, run:

```sh
bun install --frozen-lockfile
bun run license:check
```

The gate reads all first-party JavaScript manifests, discovers installed Bun package licence
metadata, runs locked Cargo metadata, rejects Git/URL/path dependency drift, and verifies known
vendored licence files by digest. The reviewed expression set currently covers the frozen graph;
it is intentionally exact so a new expression requires review.

This automation does **not** decide legal compatibility, generate a complete attribution document,
or prove that a package's metadata is correct. Before a public binary/container release, obtain
qualified review of the allowlist, generate an SPDX or CycloneDX SBOM plus complete dependency
notices, and compare those outputs with the actual archive/container contents.

## Remaining owner/publication inputs

- final README review before publication;
- private security and conduct contacts plus public support/sponsorship destinations; and
- qualified legal confirmation of the third-party licence/source policy.
