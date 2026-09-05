# Community source export

Talos Community must be published from a new, reviewed snapshot. Never push the private repository,
rewrite its history in place, or use a clone of its private object database as the public repository.

The canonical policy is [`.config/public-export-policy.json`](../.config/public-export-policy.json).
It starts from an explicit set of public roots, then removes generated/local state and withholds
known provenance blockers. The exporter considers only files known to Git as tracked or
non-ignored/untracked, refuses symlinks and submodules, rejects unreviewed binary content, verifies
the exact hashes and provenance records of explicitly permitted WebAssembly/SVG assets, and never
copies `.git`.

Reconstructible third-party source and binary inputs are excluded. The exact generated
`apps/vpx-encode/Cargo.toml` is retained as dependency metadata for Dependabot; its implementation
and README remain excluded. App source directories are explicitly allowlisted so this metadata
exception cannot admit other files from the reconstructed dependency. New app directories require
an export-policy update. The machine policy at
`.config/third-party-acquisition.json` binds the `vpx-encode` upstream archive and Talos patch,
7-Zip/LZMA SDK archives and selected members, WiX packages, and retained notices to exact digests.
Acquisition requires HTTPS plus `git` and `tar` (or an explicit 7-Zip bootstrap for installer
inputs), and refuses changed pre-existing files rather than overwriting them.

## Review an export plan

From `apps/`:

```sh
bun run public:export:review
```

`--allow-incomplete` is deliberately conspicuous. It allows preparation and review while the
manifest remains `publicationReady: false`; it does not waive a blocker or authorize publication.
Without that flag, the command fails closed whenever the source is dirty, an external owner gate is
recorded, or any blocked path would be omitted.

Once every recorded gate is resolved, `bun run public:export:check` exercises the same policy
without the incomplete-review override.

## Generate a disposable candidate

Use a new path outside the private checkout:

```sh
candidate_dir="$(mktemp -d)/talos-community"
bun ./scripts/public-source-export.ts \
  --repo-root .. \
  --output "$candidate_dir" \
  --allow-incomplete
```

The destination must not already exist. The command does not initialize Git or create a commit.
Review `.talos-export-manifest.json` first. It records:

- the private source `HEAD` commit/tree and a hash of its status without exposing excluded paths;
- the export-policy path and SHA-256;
- a deterministic candidate-set and exported-content tree SHA-256;
- each exported path, executable mode, size, and SHA-256; and
- every external gate and provenance-blocked omission.

Two exports from the same source state and policy have identical file content, modes, and manifest.
The manifest intentionally has no wall-clock timestamp. A future initial public commit must cite the
manifest's source commit, policy digest, and content-tree digest.

## Required validation sequence

Run validation inside the disposable candidate, not only in the private checkout:

1. confirm the manifest says `publicationReady: true` and has no omitted blockers;
2. run the digest-pinned Gitleaks command used by the release workflow against the directory;
3. initialize a new disposable Git repository, make one local snapshot commit, and scan that new
   object database as well;
4. run `bun ci`, then `bun run third-party:vpx:prepare` to reconstruct the reviewed path dependency;
5. run the canonical quality gates, licence/source policy, and source release-bundle tests;
6. build `talos-server` for Linux x64 and Windows x64, then exercise its no-Bun install/status flow
   with real digest-qualified images on clean hosts;
7. run clean bundled-PostgreSQL and external-PostgreSQL Compose smoke tests, including restart,
   backup, and restore; and
8. inspect the source archive, release bundle, SBOMs, container layers, checksums, and workflow logs
   for excluded material.

Do not initialize or push the real public repository until the exact owner/name, copyright holder,
contribution terms, public/private contact destinations, and explicit publication authorization are
recorded. Repository security settings that require owner access are listed in
`.github/REPOSITORY_SETTINGS.md`.
