# ADR-0005: PostgreSQL-owned generic remediation dispatch

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

Generic remediation commands were projected into PostgreSQL but also copied into two
`talos_server` maps for queued and running work. A restart lost both maps, a second control-server
replica saw different queues, and the database could disagree with what an agent was allowed to
claim. The server also reconstructed status events from the running map, so it could not accept a
valid late report after losing that process-local lookup.

A remediation payload is more than an intent identifier: producers may freeze per-command
execution policy and per-step timeouts. Rebuilding the payload from a mutable intent later would
change already-approved work.

## Options considered

### Keep process-local queues and rebuild them after restart

This retains two sources of truth and introduces replay races. It still requires replica affinity
and cannot distinguish a command that was merely loaded from one actually delivered to an agent.

### Make Redpanda the only command/status store

The broker is suitable for transport and replay, but user-facing queue/status queries and atomic
agent claims still need a materialized owner. Treating consumer memory as that owner recreates the
same replica-local failure mode.

### Claim and update the PostgreSQL projection directly

This gives one queryable source of truth and lets concurrent claimers use database row locks. The
broker remains the full-topology command transport into the projection, while live agent dispatch
uses an authenticated API boundary.

## Decision

PostgreSQL owns generic remediation job and step state. Projection persists a private,
versioned snapshot of the approved execution policy and step extensions alongside public metadata.
The private key is stripped from read and worker responses; database step rows remain authoritative
for commands, current status, and evidence.

An RMM-server-key-only API atomically selects eligible work with `FOR UPDATE SKIP LOCKED` and changes
`queued` to `running`. It excludes patch-install intents, pending approval, missing command IDs, and
jobs without steps. A command ID and dedupe key are immutable projection identities: a replay may
refresh work that is not yet running only when organization, agent, intent, command ID, and dedupe
ownership agree. It cannot re-parent a global command/dedupe collision or rewrite a running or
terminal job's frozen payload.

`talos_server` keeps no remediation queue or running-job map. The enqueue endpoint is a wake-up hint
for currently connected agents. On poll, the server claims from the API and forwards the existing
worker payload shape. Agent status reports are scoped by agent ID and command ID and update the
known steps and derived job state in one transaction. A job remains `running` until every durable
step is terminal; it then resolves to `failed` if any step failed, otherwise `cancelled` if any step
was cancelled, otherwise `completed`. Repeating the same terminal state is idempotent, while
conflicting step or job transitions fail.

Workers send a non-terminal `running` report before each generic step and one terminal report whose
bounded evidence contains the executed step outcomes. The API validates that aggregate against the
durable step indices and atomically projects each outcome. A successful aggregate must cover every
durable step. A failed or cancelled aggregate marks durable steps absent from the report as
cancelled because execution has ended without running them. If terminal evidence has no `steps`
property, it is treated as a direct per-step report and cannot make a multi-step job terminal while
other steps remain non-terminal. A present but malformed or inconsistent `steps` value is rejected
without mutation. Status evidence remains limited to 32 KiB of encoded JSON so the internal 12 MB
telemetry parser cannot be used to grow step rows without bound.

Deploy the API projection behavior before the worker reporting change. Existing workers already
send the aggregate terminal shape, so the new API repairs their multi-step rows immediately. The
new worker's additional `running` reports are accepted by either API version and do not close a
job; deploying the worker first is non-breaking but leaves the old terminal-projection defect in
place until the API is upgraded.

Generic agent status takes the direct authenticated API path instead of publishing to the
remediation-status Kafka topic. The command path in the full topology remains
producer -> Redpanda -> consumer -> PostgreSQL -> wake/claim. Patch dispatch keeps its separate
database-backed routes and progress topology. The retained Kafka status projector uses the same
row-locking transition service for backward compatibility and additionally requires the projected
organization and agent to match. Patch-job status uses that service with an exact patch-intent
selector. The old numeric job-only status route had no repository caller and could not prove tenant
or agent ownership, so it returns `410 Gone` with the scoped replacement path instead of mutating
state.

## Trust boundary and failure analysis

The API accepts the direct route only through the RMM-server-key middleware and scopes every
mutation to the agent ID associated with the worker connection, the command ID, and an exact intent
class. The service-key Kafka compatibility route also requires organization ID. Status projection
never creates a missing job or step; the separately validated command projection owns those rows.
Worker evidence is still untrusted input: encoded size, object/array shape, step index, uniqueness,
known durable membership, status vocabulary, transition legality, and aggregate/job consistency
are validated before mutation. Evidence commands or prose never override the database-owned
command or drive status derivation. Row locks serialize concurrent or replayed reports, and command
output is persisted but not logged by this path.

If the API is unavailable after a command executes, the durable job can remain `running`; the
worker/server wire currently has no end-to-end acknowledgement or safe arbitrary-command retry.
This is visible for reconciliation and intentionally preferable to blind replay. Malformed evidence
fails closed with no mutation, while a valid repeated terminal aggregate is idempotent.

## Consequences

Positive:

- restart no longer loses queued remediation work;
- concurrent control replicas cannot claim the same database row;
- late status validation no longer depends on a process-local copy of the job;
- approved execution and per-step timeout values do not drift with later intent edits;
- internal snapshot metadata cannot leak through normal job reads;
- a terminal multi-step job cannot retain misleading pending or running middle steps.

Costs and risks:

- claim and report require the API backend to be available;
- direct generic status does not inherit Kafka status-event retention;
- claiming marks a job running before the worker confirms receipt;
- a crash in that delivery window is ambiguous, because an arbitrary shell command may or may not
  have executed.

Talos deliberately does not auto-requeue stale `running` generic commands: such commands are not
generally idempotent. Ambiguous work stays durable and visible for explicit reconciliation. A future
lease/acknowledgement design may mark pre-delivery claims failed or distinguish acknowledged
execution, but it must not blindly redeliver unknown shell work.

## Verification

- pure tests round-trip frozen execution and per-step fields while stripping reserved metadata;
- route regressions cover authentication, eligibility, one-time claim, agent scope, known-step
  enforcement, terminal idempotency, transition conflicts, three-step success/failure projection,
  malformed or missing aggregate evidence, legacy consumer compatibility, wrong organization,
  immutable command projection ownership, patch status, and retirement of the unscoped route;
- worker regressions cover three step-start reports, the single atomic terminal outcome, and the
  32 KiB UTF-8-safe evidence bound;
- server tests preserve the worker wire shape and wake-only target selection;
- a disposable migrated PostgreSQL integration exercises scoped terminal projection, replay,
  timestamp preservation, immutable command ownership, the active HTTP compatibility payload, and
  two simultaneous conflicting terminal transactions (exactly one update and one conflict).

## Rollback

Rolling back the control server alone would reintroduce empty local queues because new commands are
already projected durably. A safe rollback must restore the old API/server pair together and replay
only commands whose execution state has been reviewed. Never convert all durable `running` rows
back to `queued` automatically.
