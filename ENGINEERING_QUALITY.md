# Talos engineering quality contract

Status: repository policy
Applies to: production code, tests, build and release tooling, infrastructure, documentation, and
generated artifacts
Audience: maintainers, contributors, reviewers, and automated coding agents

## Purpose and interpretation

This document turns established engineering guidance into enforceable Talos policy. The words
**MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** describe repository policy.

The references fall into four classes:

- **External conformance specifications:** IETF RFCs, W3C Recommendations, OpenAPI, and SPDX/ISO.
  Their normative language applies when Talos implements or claims conformance.
- **Security baselines and frameworks:** NIST SSDF, OWASP ASVS, and SLSA. Talos chooses how and when
  to adopt them; citing them does not prove compliance.
- **Official ecosystem guidance:** Bun, Rust, TypeScript, Svelte, OpenTelemetry, and Google
  engineering guidance.
- **Talos policy:** the enforceable rules in this document.

References explain the rationale for this policy. Their presence does not establish compliance.
Do not claim ASVS, SLSA, WCAG, OpenAPI, SPDX, or reproducible-build conformance until the applicable
requirements have been explicitly audited and verified.

## Definition of done

A change is complete only when:

1. Its intended behavior and important failure modes are explicit.
2. The design has an appropriate boundary and does not worsen known structural debt.
3. Trust-boundary inputs are validated and authorization is enforced server-side.
4. Changed behavior is covered by meaningful automated tests.
5. Applicable formatting, static analysis, test, contract, and build gates pass without increasing
   the warning baseline.
6. Operational effects, compatibility, migration, rollout, and rollback are addressed where
   relevant.
7. Documentation, contracts, and generated artifacts agree with the implementation.
8. The complete diff has been reviewed and all skipped verification is disclosed.

Passing tests alone is not proof of completion when those tests do not cover the stated behavior.

## Change discipline and architecture

- Changes MUST be coherent and reviewable. Mechanical movement, formatting, dependency upgrades,
  generated-code changes, and behavior changes SHOULD be separated whenever practical.
- Every change MUST leave overall code health no worse. Avoid drive-by edits and speculative
  abstractions.
- Decisions affecting process boundaries, protocols, durable-state ownership, data models,
  security boundaries, major dependencies, or build/release architecture MUST add or update an
  architecture decision record in `docs/architecture/decisions/`.
- An ADR MUST record context, options considered, decision, consequences, rollout, and rollback.
- New behavior defaults to an in-process module. A new process or service requires a concrete
  isolation, security, scaling, ownership, or deployment need.
- Mutable state MUST be classified as transient connection state, reconstructable cache, or
  durable/shared state. Durable or cross-replica state MUST NOT live only in a process-local map.
- State owners MUST document restart, reconnect, expiry, concurrency, and multi-instance semantics.
- Known god files are ratchets: new features MUST NOT be added directly when a coherent module can
  be extracted. Except for an emergency fix, a change touching one SHOULD make it smaller or create
  a tested seam toward decomposition.
- Remote calls MUST define cancellation, timeouts, retryability, bounded attempts, and failure
  behavior. Retry only safe or idempotent operations, at one layer, with backoff and jitter.

Architecture decision records follow Michael Nygard's original
[ADR guidance](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions). Retry policy is
informed by the [AWS Well-Architected retry guidance](https://docs.aws.amazon.com/wellarchitected/latest/framework/rel_mitigate_interaction_failure_limit_retries.html).

## Testing

- Logic changes MUST include tests in the same change.
- A bug fix MUST include a regression test that fails without the fix.
- A behavior-preserving refactor without adequate coverage MUST first add characterization tests.
- Tests MUST assert externally meaningful behavior, not implementation trivia.
- Tests MUST be deterministic, order-independent, and parallel-safe. They MUST NOT depend on
  production services, uncontrolled time or randomness, unbounded waits, or fixed sleeps as
  synchronization.
- Small tests use no network, database, real clock, or external process. Medium tests may use
  localhost, the filesystem, or disposable dependencies. Large tests exercise a deployed or broad
  integration. Test documentation and CI MUST identify the size when it affects execution.
- Process and protocol boundaries require contract tests. Critical user journeys require a small
  number of end-to-end tests.
- Coverage is evidence, not a substitute for useful assertions.
- Flaky tests are defects. A quarantine MUST be tracked, owned, justified, and time-bounded; silently
  rerunning until green is forbidden.

Sources: [Google test sizes](https://testing.googleblog.com/2010/12/test-sizes.html),
[Google review guidance for tests](https://google.github.io/eng-practices/review/reviewer/looking-for.html),
and [Svelte testing guidance](https://svelte.dev/docs/svelte/testing).

## Review and automated-agent behavior

- Reviewers MUST assess design, correctness, edge cases, concurrency, security, complexity, tests,
  naming, documentation, compatibility, and operational impact.
- Authentication, authorization, cryptography, update/install flows, remote command execution,
  unsafe Rust, schema migrations, and release pipelines require qualified human review.
- Before editing, agents MUST read repository instructions, inspect relevant code and tests, and
  inspect `git status`.
- Agents MUST preserve unrelated worktree changes and MUST NOT expand external side effects beyond
  the user's authorization.
- After editing, agents MUST review the full diff, run focused checks first and applicable root
  gates afterward, and report exact results, skipped checks, and remaining risk.
- Generated output MUST be changed through its source or generator, never hand-edited when a
  generator exists.
- A gate MUST NOT be disabled, weakened, or reclassified merely to make a change pass.
- Emergency exceptions MUST be minimal, documented, and followed by a tracked correction.

Sources: Google's [code-review standard](https://google.github.io/eng-practices/review/reviewer/standard.html),
[review checklist](https://google.github.io/eng-practices/review/reviewer/looking-for.html), and
[small-change guidance](https://google.github.io/eng-practices/review/developer/small-cls.html).

## Rust

- The supported toolchain and each package's `rust-version` MUST be pinned before release.
- Shared lints belong in the Cargo workspace and SHOULD become stricter over time.
- CI MUST run `cargo fmt --all -- --check`, Clippy for supported targets/features with warnings
  denied, and tests including documentation tests using locked resolution.
- Safe Rust is the default. `unsafe` MUST be necessary, minimal, isolated behind a safe interface,
  accompanied by a precise `SAFETY` explanation, tested, and specially reviewed.
- Recoverable request handling, protocol parsing, persistence, and long-running services MUST NOT
  use `unwrap`, `expect`, or `panic`. A proven invariant MAY use `expect` with a message explaining
  the invariant.
- Errors MUST be typed or carry structured context. Code MUST NOT branch on human-readable error
  strings. Public APIs document errors, panics, and safety contracts.
- Shared/public crates SHOULD follow the Rust API Guidelines.

Sources: [Clippy CI usage](https://doc.rust-lang.org/clippy/usage.html),
[rustfmt](https://github.com/rust-lang/rustfmt),
[the Rust Book's unsafe guidance](https://doc.rust-lang.org/stable/book/ch20-01-unsafe-rust.html),
[Cargo workspace lints](https://doc.rust-lang.org/stable/cargo/reference/workspaces.html), and the
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/).

## TypeScript and Svelte

- Packages MUST extend or match the shared strict TypeScript baseline. New packages enable
  `strict`, `noUncheckedIndexedAccess`, and `exactOptionalPropertyTypes`; existing packages ratchet
  toward them without weakening current checks.
- HTTP, WebSocket, Kafka, environment, database, filesystem, and native-bridge values MUST be
  treated as `unknown` until runtime validation succeeds.
- Avoid `any`, unchecked assertions, non-null assertions, `@ts-ignore`, and blanket suppressions.
  Necessary exceptions MUST be narrow and explain why they are safe.
- Formatting belongs to an automated formatter. `bun run format:check` is the current ratchet for
  repository tooling, the hand-authored contract facade, extracted TypeScript seams, and migrated
  Svelte surfaces; new or materially migrated TypeScript/Svelte surfaces MUST join that scope. Run
  `bun run format` to apply the pinned formatter. Generated artifacts remain exclusively owned by
  their generator and MUST pass their drift check instead of being reformatted afterward.
  Type-aware linting SHOULD enforce correctness.
- New Svelte code uses current Svelte 5 patterns: derived values instead of state-setting effects,
  effects only for genuine external synchronization, stable domain keys, and scoped state/context.
- Components own one coherent UI responsibility. Protocol, data-access, and domain logic belong in
  typed modules rather than large components.
- Svelte checks MUST run with no new warnings. Accessibility suppressions require an inline reason
  and focused verification.
- User-facing UI targets WCAG 2.2 Level AA, without claiming conformance before a complete manual
  and automated evaluation.

Sources: TypeScript [`strict`](https://www.typescriptlang.org/tsconfig/strict),
[`noUncheckedIndexedAccess`](https://www.typescriptlang.org/tsconfig/noUncheckedIndexedAccess.html),
[`exactOptionalPropertyTypes`](https://www.typescriptlang.org/tsconfig/exactOptionalPropertyTypes.html),
[typescript-eslint configurations](https://typescript-eslint.io/users/configs/),
[Prettier's rationale and formatting contract](https://prettier.io/docs/en/why-prettier.html),
[Svelte best practices](https://svelte.dev/docs/svelte/best-practices),
[`sv check`](https://svelte.dev/docs/cli/sv-check), and the normative
[WCAG 2.2 Recommendation](https://www.w3.org/TR/WCAG22/).

## API and protocol contracts

- Every new externally consumed HTTP operation, and every legacy operation whose request or response
  shape is materially changed, MUST gain one versioned, machine-readable source of truth. The
  supported OpenAPI version MUST be pinned with the generator/toolchain.
- Clients and DTOs for a migrated contract MUST be generated or mechanically verified from it.
  Equivalent new paths and payload definitions MUST NOT be independently maintained in Rust and
  TypeScript.
- New or migrated trust-boundary requests require runtime validation. Their tests MUST validate
  responses against the contract, and CI MUST lint those contracts, detect breaking changes, and
  run applicable cross-language fixtures.
- HTTP APIs MUST use appropriate HTTP semantics and a stable machine-readable error shape based on
  RFC 9457. Human detail text MUST NOT be parsed as a machine code or reveal secrets.
- Breaking changes require an explicit versioning and migration plan. Retried mutations require
  idempotency semantics.
- Message and WebSocket protocols require explicit versions, compatibility rules, size limits,
  authentication/authorization behavior, and malformed-message tests.

The migrated interactive-session slice is generated from `talos_protocol` into the private
`@talos/protocol-types` Bun workspace. Change the Rust source and run `bun run contracts:generate`;
CI runs `bun run contracts:check` and rejects hand-edited or stale output. ADR-0002 records the
scope and the legacy OpenAPI/runtime-validation baseline. Unmigrated endpoints are technical debt,
not evidence of conformance; touching one must either move that slice forward under this ratchet or
record a scoped exception under the process below.

Normative sources: [OpenAPI Specification](https://spec.openapis.org/oas/latest.html) and
[RFC 9457 Problem Details for HTTP APIs](https://www.rfc-editor.org/rfc/rfc9457.html).

## Secure development

- Talos uses [NIST SSDF 1.1](https://csrc.nist.gov/pubs/sp/800/218/final) as its development-process
  baseline.
- New or changed web, API, and control-plane surfaces MUST satisfy applicable
  [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/)
  Level 2 requirements. Existing gaps and non-applicable requirements must be recorded; this does
  not constitute a whole-product ASVS claim.
- Security-sensitive changes require a written trust-boundary/threat analysis covering identity,
  authorization, abuse cases, failure modes, data sensitivity, and least privilege.
- Authorization MUST be enforced at the trusted service layer. Inputs require allowlisted shape,
  type, range, size, and business-invariant validation. Client validation is usability only.
- Secrets, credentials, session material, tokens, personal data, and command contents MUST NOT be
  committed or logged. Redaction happens before emission.
- New dependencies require necessity, maintenance, provenance, license, and vulnerability review.
  Ignored advisories require a rationale, owner, and expiry.
- Security controls require negative and abuse-case tests, not only happy paths.

## Workspace, dependencies, and repeatability

- JavaScript packages use Bun-native workspaces with the `isolated` linker, `hoist = false`, and an
  empty `hoistPattern` compatibility setting for the pinned Bun release. Together these provide
  Bun's strict pnpm-like resolution mode: the fallback store contains no package links and
  undeclared imports fail instead of resolving according to unrelated installed packages.
- The repository maintains exactly one first-party Bun lockfile at `apps/bun.lock`. Workspace leaf
  lockfiles are forbidden, and every workspace declares every dependency it imports. Lockfiles
  retained inside immutable vendored upstream source are not part of the first-party workspace.
- Root dependencies are limited to repository-wide tooling. Use `workspace:` references and Bun
  catalogs for shared versions where appropriate.
- Local bootstrap uses `bun install` from `apps/`. CI and release builds use `bun ci` or
  `bun install --frozen-lockfile`.
- The Cargo workspace maintains one lockfile. CI and release operations use `--locked`.
- Bun, Rust, CI actions, base images, and release toolchains MUST be pinned. Release inputs MUST NOT
  use floating `latest` tags. CI actions SHOULD be pinned to reviewed full commit SHAs and
  containers to immutable digests.
- Trusted dependency lifecycle scripts require explicit review.
- CI runs JavaScript and Rust advisory checks. Ignored findings follow the exception process.
  Dependency licenses and non-registry or Git sources MUST be reviewed before a public release.
  `bun run license:check` enforces the selected `AGPL-3.0-only` first-party metadata, exact
  dependency-expression review list, source classes, and retained vendor-licence evidence. This is
  a drift gate, not legal advice; qualified review of compatibility and complete release notices
  remains required.
- Generated build output MUST NOT be committed unless it is an intentional versioned release input
  with a documented source and reproducible generation command.
- Releases SHOULD produce an SPDX or CycloneDX SBOM and SLSA provenance. “Reproducible build” may be
  claimed only after independent builds are verified bit-for-bit identical.

Sources: Bun [workspaces](https://bun.com/docs/pm/workspaces),
[isolated installs](https://bun.com/docs/pm/isolated-installs),
[frozen CI installs](https://bun.com/docs/pm/cli/install),
[lockfiles](https://bun.com/docs/pm/lockfile), and [`bun audit`](https://bun.com/docs/pm/cli/audit);
Cargo [lockfile guidance](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html);
RustSec [`cargo audit`](https://github.com/RustSec/rustsec/tree/main/cargo-audit);
GitHub's [Dependabot ecosystem reference](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-options-reference);
the [reproducible-build definition](https://reproducible-builds.org/docs/definition/),
[SLSA build requirements](https://slsa.dev/spec/v1.2/build-requirements), and
[SPDX specification status](https://spdx.dev/use/specifications/).

## Observability and operations

- Services MUST emit structured, machine-parseable logs with timestamp, severity, stable event
  name, service name/version, relevant operation identifiers, and trace/span identifiers.
- W3C Trace Context SHOULD be propagated across HTTP and supported process boundaries; correlation
  MUST be preserved across queues, WebSockets, and agent sessions.
- A propagated error is recorded once at the layer that owns handling or reporting it, not at every
  layer.
- New service operations expose latency distributions, traffic, error rate/type, and saturation.
  Distributed pipelines additionally expose queue lag, retries, drops, reconnects, and oldest-work
  age.
- Health checks distinguish liveness from readiness. Services support graceful shutdown and make
  incomplete work and reconnect behavior observable.
- Alerts must be actionable and tied to user impact or imminent exhaustion.

Sources: the normative [W3C Trace Context Recommendation](https://www.w3.org/TR/trace-context/),
OpenTelemetry [log data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/) and
[semantic conventions](https://opentelemetry.io/docs/specs/semconv/), and Google's
[four golden signals](https://sre.google/sre-book/monitoring-distributed-systems/).

## Repository quality gates

`cd apps && bun run quality` is the canonical local aggregate gate. CI uses the same component
commands in an operating-system matrix and additionally verifies:

- workspace manifest, strict isolated-linker, fallback-hoist, and lockfile integrity
  (`bun run workspace:check`);
- first-party SPDX metadata plus dependency licence/source drift (`bun run license:check`);
- generated-code and contract drift;
- format and static-analysis checks;
- unit, contract, and currently available integration tests;
- supported production builds;
- dependency vulnerability policy and the documented exception baseline;
- Linux-, macOS-, and Windows-specific native checks.

Complete generated attribution/SBOM validation, installer execution, Docker runtime smoke tests, and
end-to-end system tests remain release gates to add where the required platform or policy decision
exists; they are not claimed as current CI coverage.

No warning baseline may silently grow. A suppression or gate exception must be narrow, linked to a
tracked issue, owned, justified, and given an expiry date.

## Exception process

When a rule cannot be met, the change MUST record:

1. The exact unmet rule and evidence.
2. Why compliant alternatives are currently impractical.
3. Scope and risk of the exception.
4. Compensating controls.
5. An owner, tracked issue, and expiry date. Before the public issue tracker exists, a unique
   dependency-risk-register key may serve as the tracking reference, but it MUST be converted to a
   public issue before the Community release.

An undocumented exception is a defect, not precedent.
