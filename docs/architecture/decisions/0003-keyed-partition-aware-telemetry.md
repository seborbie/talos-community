# ADR-0003: Keyed, partition-aware telemetry consumption

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

Talos publishes snapshots, events, remediation commands, remediation status, and patch progress to
Redpanda. Producers key each record as `<organizationId>:<agentId>`, but the active consumer
previously declared only partition `0` for every input topic. Extra broker partitions were therefore
never offered to the consumer group, offset validation ignored them, and adding consumer replicas
could not increase useful parallelism.

Kafka ordering is scoped to a topic partition. A stable agent key keeps one agent's records together
within each topic while the topic's partition count is unchanged. It does not establish ordering
between different topics, and increasing a topic's partition count can remap later records for an
agent to a different partition. Consumers must retain idempotency and timestamp/version checks rather
than infer a global cross-topic order.

## Options considered

### Keep one partition per topic

This gives simple ordering but caps each topic at one active consumer-group member and hides topology
bugs until production scale.

### Use multiple partitions with unkeyed or randomly keyed records

This enables parallelism but allows concurrent handling of records for the same agent, weakening the
ordering boundary used by snapshot, event, and remediation processing.

### Use agent-keyed records and discover broker partitions

This retains per-agent, per-topic ordering and lets the consumer group distribute real broker
partitions across replicas without encoding a partition count in application code.

## Decision

Talos keeps `<organizationId>:<agentId>` as the partition key for all telemetry input topics. At
startup, the active telemetry consumer requests broker metadata for every configured input topic and
passes every discovered partition ID to both offset preflight and the Samsa consumer group. Startup
fails if a configured topic is missing or has no partitions.

The development Compose profile creates telemetry and telemetry-DLQ topics with
`RMM_TELEMETRY_TOPIC_PARTITIONS`, defaulting to three. Its Redpanda, Redpanda Console, and Azurite
images are pinned by digest so local topology does not drift when an upstream `latest` tag moves.

Consumer replicas share one group. Samsa's round-robin group assignor spreads the discovered
topic-partitions between members, so useful concurrency is bounded by available partitions and by
the distribution of agent keys. The single-broker development profile exercises partition and group
behavior; it does not provide broker high availability.

Patch progress has one authoritative destination per topology. Without the producer, the control
server writes it directly to the API backend's PostgreSQL projection. With the producer enabled, the
control server writes it once to Redpanda and the owning consumer partition projects it to the same
PostgreSQL endpoint. The former embedded HTTP cache and its private port were removed because they
were replica-local, had no active caller, and caused the full topology to double-write progress.

The API validates every patch-progress record before projecting the batch, locks the referenced
device scope, and persists the parsed RFC3339 event time in nullable
`rmm_patch_action.reported_at`. A nonterminal row accepts an event when the stored timestamp is null
or the incoming timestamp is equal or newer. Rows predating the migration begin null, so their first
valid report establishes the ordering baseline. Equal timestamps are accepted and resolve in
database serialization order because the current wire contract has no sequence field. Completed,
failed, and cancelled states are immutable for an operation ID, including when a later-timestamp
heartbeat or conflicting terminal report arrives. The terminal action row, per-update result
projection, action-result evidence, and one-shot override cleanup commit in one database
transaction, so retry cannot observe a terminal row whose side effects were lost.

## Patch-progress trust boundary

The endpoint still requires the RMM server key, but authentication does not make forwarded agent
JSON trustworthy. Each record must name an existing device and its actual organization; the API
checks and locks that relationship before any batch member is written. Status is allowlisted, phase
and identifiers are bounded, timestamps must be valid timezone-qualified RFC3339 values, batches
are capped at 100 records, and encoded progress/evidence are capped at 256 KiB/128 KiB per record.
An agent timestamp more than ten minutes ahead of API time is rejected: otherwise one hostile or
badly skewed running event could make every legitimate completion appear stale until that future
time arrives. Past events remain valid so broker backlog and offline delivery can drain normally.
Malformed, cross-organization, or oversized input rejects the whole batch without logging its
potentially sensitive evidence. Database or terminal side-effect failure rolls back the whole batch
and returns a retryable server error.

## Consequences

Positive:

- all broker partitions participate in consumption and offset preflight;
- multiple consumer replicas can process different agent partitions concurrently;
- non-contiguous partition IDs are preserved rather than reconstructed from a count;
- local development catches assumptions that only hold for partition `0`;
- progress reads use the durable database projection and do not depend on consumer affinity;
- patch-progress writes validate the device/organization pair and bounded wire fields before any
  batch member is projected. `rmm_patch_action.reported_at` records the parsed producer timestamp;
  an atomic upsert rejects older reports and treats terminal states as immutable, so delayed
  heartbeats and replays cannot regress completed, failed, or cancelled work.

Costs and risks:

- there is no total order across telemetry topics;
- increasing a partition count can move an agent key, so rollout must tolerate overlap between its
  old and new partitions;
- metadata discovery is a startup dependency and deliberately fails closed for missing topics;
- Samsa receives the discovered topology at process startup, so every consumer replica must restart
  after operators expand a topic;
- in the full topology, patch-progress visibility is eventually consistent with consumer lag.

## Rollout

1. Create new environments with at least the configured partition count.
2. For existing environments, explicitly expand topics or recreate disposable development data;
   idempotent topic creation does not resize an existing topic.
3. Restart all telemetry consumer replicas so each member discovers the same topology, then verify
   group assignments, lag, and offset-preflight logs for every partition.
4. Apply the nullable `reported_at` migration before deploying the API code that references it.
5. Confirm the patch-progress projection URL reaches the API backend and monitor projection errors.
6. Scale replicas gradually, stopping when partition ownership or downstream capacity becomes the
   limiting factor.

## Rollback

The consumer can be rolled back to a version that explicitly assigns known partitions, but doing so
would strand messages on omitted partitions. Kafka partition counts cannot be decreased. Disposable
development clusters may be recreated with one partition; persistent environments require new
single-partition topics and a controlled producer/consumer cutover if a topology rollback is needed.
The API may be rolled back while retaining the nullable `reported_at` column; previous versions
ignore it. Dropping the column is unnecessary and should only happen after all newer API instances
are removed.
