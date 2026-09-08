# ADR-0015: Review OpenAI SDK upgrades against the Bun transport contract

- Status: proposed; awaiting PR integration
- Date: 2026-09-07
- Owner: Talos maintainers

## Context

PR #39 updates the API backend SDK from OpenAI 6.33.0 to 7.9.0. The upstream
[7.0 release](https://github.com/openai/openai-node/releases/tag/v7.0.0) raises the supported
Node.js minimum to 22. Talos runs this backend with pinned Bun 1.3.14 in development and its
Docker image, rather than Node. The SDK's 7.9.0 README supports Bun 1.0 or later.
Talos uses Responses creation, tool-result continuation, streaming events and final responses.
Some existing consumers use dynamic response types, so type checking alone is insufficient.

## Decision

Keep the Bun runtime and use the exact locked SDK release, subject to the existing review and CI
gates. Add network-free SDK transport tests covering function-call payloads, previous-response
continuation, streamed text and final responses, and cancellation before any request is sent.
Use injected fetch and synthetic data, never real credentials or model calls in these tests.

The full PR dependency diff adds optional peer declarations, not installed runtime dependencies.
Talos does not invoke the removed SDK CLI. Node-only deployments are not validated by this decision;
a future runtime change must revisit the Node 22 minimum.

## Alternatives and consequences

Keeping 6.33.0 would postpone the runtime review but also defer upstream fixes. Changing Talos to
Node merely to follow the SDK's Node support baseline would expand scope unnecessarily. Bun is an
explicitly supported upstream runtime and remains the deployment runtime.

Existing API-key sourcing, configured timeouts, model choice, tool authorization and remote-command
validation remain unchanged. The SDK continues to own HTTP serialization and event parsing. These
mocked transport checks verify compatibility, not live model behavior, every API surface, or the
entire remote-command authorization path. Existing application and security tests remain required.
No new privileges, endpoint configuration or logging of credential material are introduced.

## Rollout and rollback

Integrate only after reviewing the current PR revision and successful required checks. Verify main
CI after merge. Rollback restores the previous manifest and root Bun lockfile together; the added
transport tests should remain useful for subsequent candidate upgrades. No database or protocol
migration is introduced.
