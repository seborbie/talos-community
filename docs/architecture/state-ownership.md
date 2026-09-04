# State ownership and scaling boundaries

Last reviewed: 2026-08-19
Owner: Talos maintainers

This document distinguishes durable control-plane state from state that exists only to serve a
live connection or correlate one request. A process-local map is acceptable only when losing it has
an explicit reconnect, timeout, or retry outcome. It must not be the source of truth for queued
work or user-visible progress.

## Ownership matrix

| State | Authoritative owner | Lifetime and restart behavior | Scaling boundary |
| --- | --- | --- | --- |
| Agent WebSocket connections and current capabilities | `talos_server::AgentDirectory` | Exists only while an agent socket is connected. A replacement socket owns the entry; a stale socket disconnect cannot remove its replacement. Agents reconnect after a server restart. | One `talos_server` control replica is currently supported. Multiple replicas require connection-aware routing or a shared directory plus message delivery to the owning replica. |
| Pending detail, command, RDP-session, capability, QUIC-reflex, and shell-offer correlations | `talos_server` one-shot channels | Request correlation only. Entries are reaped after 60 seconds and callers have operation-specific timeouts. Restart drops the sender and the request fails; callers may retry. | Requests and their agent connection must reach the same control replica. |
| Active shell, desktop, file-transfer, registry, and chat launch sessions | `talos_server` memory | Contains short-lived transport negotiation data and credentials. Unattached sessions expire after 15 minutes; chat presence expires after a 6-second heartbeat window. Restart invalidates the launch and the viewer must create a new session. | Session creation, viewer attachment, heartbeats, and agent messages must reach the owning replica. Do not replicate raw session keys casually; introduce an explicit encrypted session directory or affinity design first. |
| Snapshot-request throttle | `talos_server` memory; snapshot request record in PostgreSQL | The 30-second per-agent cooldown is reconstructable and resets on restart. The actual request ID/status is registered durably before dispatch. | A shared rate limiter is required before multiple control replicas can enforce one global cooldown. |
| Generic remediation jobs, frozen execution contract, steps, and status | PostgreSQL tables under `rmm_telemetry` | The telemetry projection creates queued rows; an atomic `FOR UPDATE SKIP LOCKED` claim changes `queued` to `running`; aggregate terminal evidence is validated and projected across every durable step in one transaction, and job status is derived from all step states. Duplicate projections cannot rewrite running or terminal payloads. | Safe for concurrent claimers. Generic status reports go directly from `talos_server` to the authenticated API route rather than through the remediation-status Kafka topic. |
| A generic remediation job left `running` by an ambiguous crash | PostgreSQL | Remains visible for explicit operator reconciliation. Talos does not automatically requeue it because arbitrary shell commands are not idempotent and may already have executed. | A future reconciliation policy must distinguish pre-delivery leases from acknowledged execution, or mark timed-out work failed; it must not blindly redeliver commands. |
| Patch jobs and patch/feature-upgrade progress | PostgreSQL, reached directly or through the telemetry producer/consumer path selected by configuration | API rows are authoritative. The server and consumer do not overlay replica-local progress caches. | API transactions and keyed broker partitions provide the concurrency boundary. |
| Telemetry events and progress awaiting projection | Redpanda topics and consumer-group offsets | Durable in the broker according to topic retention; consumers rediscover and assign every partition after restart. Projections are keyed and validated before API writes. | Increase partitions for throughput while preserving a stable entity key. Consumers must not hard-code partition zero or keep an HTTP progress cache. |
| API client and parsed control-server configuration | Immutable `AppState` values | Rebuilt from validated environment configuration at process start. Empty optional producer URLs intentionally select the direct durable API path. | Safe to construct independently in each replica. |

## Supported deployment rule

PostgreSQL and Redpanda-backed work may be scaled according to their own transaction and consumer
semantics. `talos_server` is intentionally a single active control replica until agent and viewer
traffic can be routed to the replica that owns each live connection/session. Adding a second
replica without that routing would produce intermittent "not connected" responses and lost session
correlations even though durable database state remains correct.

When adding new state, record its owner, expiry, restart outcome, and multi-replica behavior here.
Durable work must never be introduced as another `AppState` collection.
