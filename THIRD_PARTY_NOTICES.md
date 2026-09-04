# Third-party notices

Status: **preliminary publication inventory — not release sign-off**

Talos Community Edition includes and depends on third-party software. The root `AGPL-3.0-only`
[`LICENSE`](LICENSE) applies to publishable first-party Talos source only; it does not replace or
alter a third party's licence. Original licence files and notices must remain with the corresponding
material.

## Vendored and copied source

| Component                             | Version or revision evidence                                                                                       | Licence                               | Source and retained notice                                                                                                                                                                                                                                                                                                                                                                                       |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dxgi-capture-rs`                     | 1.1.7; upstream revision `4d7e1a651afd248d4d1ef4401033d60efbdf5a91`                                                | MIT                                   | [Upstream](https://github.com/RobbyV2/dxgi-capture-rs); copyright RobbyV2; [`apps/vendor/dxgi-capture-rs/LICENSE`](apps/vendor/dxgi-capture-rs/LICENSE)                                                                                                                                                                                                                                                          |
| `permission-flow`                     | 0.1.40; packaged from upstream revision `b6e15e34b6892598a576d4f0a50ebf7a6af71382` with local changes              | MIT                                   | [Upstream](https://github.com/veecore/permission-flow); copyright 2026 小弟调调; [`apps/vendor/permission-flow/PermissionFlow/LICENSE`](apps/vendor/permission-flow/PermissionFlow/LICENSE)                                                                                                                                                                                                                      |
| `tauri-plugin-permission-flow`        | 0.1.40; packaged from the same upstream revision with local changes                                                | MIT                                   | [Upstream](https://github.com/veecore/permission-flow); shares the retained upstream licence above                                                                                                                                                                                                                                                                                                               |
| `samsa`                               | 0.1.8; upstream revision `33062fce28a7b14a631496409841e252ef7937c4` with Talos changes                             | Apache-2.0 (`license-file` metadata)  | [Upstream](https://github.com/CallistoLabsNYC/samsa); [`apps/vendor/samsa/LICENSE`](apps/vendor/samsa/LICENSE); upstream supplied no `NOTICE` file in this copy                                                                                                                                                                                                                                                  |
| `vpx-encode`                          | locally versioned 0.6.5 patched from tag `vpx-encode/v0.6.2`, revision `5519bec430184208ee33221ddc727ccb9429b88e`  | MIT declaration in its Cargo manifest | [Upstream](https://github.com/astraw/vpx-encode); published 0.6.2 crate SHA-256 `cd1f41af42de7667cbdba44e8c38c36e9ede970f1291c32ea57c0ded8eb6f4b6`; authors Andrew Straw and Ram Kaniyur are named by upstream. The public snapshot carries only the digest-pinned acquisition policy and reviewed Talos patch, then reconstructs this tree locally. Upstream supplies no standalone copyright/licence file, so qualified notice review remains required before redistributing reconstructed source or binaries. |
| Next.js Create Next App template SVGs | upstream revision `357e514cf7bdc276bfd830003c38027c17d9de05`; five byte-identical files in `apps/frontend/public/` | MIT                                   | [Upstream](https://github.com/vercel/next.js/tree/357e514cf7bdc276bfd830003c38027c17d9de05/packages/create-next-app/templates/app/ts/public); copyright 2025 Vercel, Inc.; [`apps/frontend/public/NEXTJS_TEMPLATE_ASSET_LICENSE.md`](apps/frontend/public/NEXTJS_TEMPLATE_ASSET_LICENSE.md)                                                                                                                      |

The vendored directories retain upstream copyright and licence material. Talos-specific patches do
not relicense those directories under the repository-wide AGPL declaration.

## Registry dependencies

The frozen Bun and Cargo dependency graphs are recorded in `apps/bun.lock` and `apps/Cargo.lock`.
`bun --cwd apps run license:check` discovers installed Bun package metadata and Cargo metadata,
rejects missing/unreviewed licence expressions and non-registry sources, and verifies the retained
vendored licence evidence. Its current expression allowlist is a drift-control mechanism, not a
legal compatibility opinion.

A per-release SBOM and complete machine-generated dependency notice must accompany publishable
binaries and containers. That output is not yet implemented, so the lockfiles and this preliminary
file are not sufficient release notices by themselves.

## Binary prerequisites and build inputs

The following third-party material is acquired as pinned build input and is not copied into the
public source snapshot:

- 7-Zip 26.00 LZMA SDK archive SHA-256
  `6b7d0c8ed1a67112d5337e4532ecdcb9fd2eab8b1f6bb54199f9b6a627b506cc`; its
  `bin/7zSD.sfx` member is SHA-256
  `d47d81ca4233816b5bf66977c92c55773ae8241d03d108e4477abb49333c2522`. The upstream LZMA SDK
  notice places the SDK, including the SFX module, in the public domain and is retained at
  [`apps/installer/third-party/7zip-26.00/LZMA-SDK-NOTICE.txt`](apps/installer/third-party/7zip-26.00/LZMA-SDK-NOTICE.txt).
- 7-Zip Extra 26.00 archive SHA-256
  `1cc38a9e3777ce0e4bbf84475672888a581d400633b0448fd973a7a6aa56cfdc`; its pinned `7za.exe`
  build tool is SHA-256 `392d39caddffb4b078807ee05e69c94719027ac9c9445503806e47116acf6686`.
  Its LGPL/BSD licence information is retained at
  [`apps/installer/third-party/7zip-26.00/License.txt`](apps/installer/third-party/7zip-26.00/License.txt).
- WiX Toolset 6.0.0 CLI, SDK, Bal, Firewall, Util, and UI NuGet packages from upstream revision
  `8c7432e50072e009353ea5f2c956ccf453476f71`. Exact package digests are recorded in
  [`.config/third-party-acquisition.json`](.config/third-party-acquisition.json). Each official
  package requires acceptance of the WiX Open Source Maintenance Fee Agreement; the exact package
  file's SHA-256 is recorded and a readable copy is retained at
  [`apps/installer/third-party/wix-6.0.0/OSMFEULA.txt`](apps/installer/third-party/wix-6.0.0/OSMFEULA.txt).
- Microsoft Visual C++ Redistributables and the Microsoft Edge WebView2 Runtime bootstrapper; and
- libvpx source and libraries used by native video builds.

The acquisition command verifies every archive, selected member, package, patch, reconstructed
source file, and retained notice by SHA-256. It creates an ignored, local-only WiX feed and refuses
to replace unexpected local files. Historical tracked WiX caches/extracts and the SFX stub are
excluded by policy and are not active public-source inputs. The WiX maintenance-fee terms must be
reviewed for the builder's circumstances before using the official packages. Microsoft
prerequisites downloaded during a build are not first-party Talos software and must not be
described as AGPL-licensed.

See [`docs/licensing-and-provenance.md`](docs/licensing-and-provenance.md) for the file-level
inventory, hashes, unresolved media provenance, and publication actions.
