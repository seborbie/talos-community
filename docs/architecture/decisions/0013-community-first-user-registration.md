# ADR-0013: Serialize Community first-user registration in PostgreSQL

- Status: accepted
- Date: 2026-09-04
- Owner: Sebastian Orbe

## Context and options

A Community installation has no shipped account or default password. Its first user registers
through the API, then creates an organization as SUPER_ADMIN. Later users are provisioned by an
administrator. An application-local flag would reset on restart and cannot coordinate independent
database connections. A separate bootstrap row or credential would add a migration and another
operator-managed secret. A PostgreSQL transaction lock can coordinate the existing empty-user check.

## Decision

Keep the global zero-user check and insertion in one PostgreSQL transaction, protected by the
same transaction-scoped advisory lock for every registration request. Use a parameterized query
and cast the lock function's void result to text because Prisma cannot deserialize PostgreSQL void.
The fast pre-check avoids password hashing after registration closes; the check under the lock
is authoritative. All contenders other than the first successful insertion receive HTTP 403.

The database owns the registration state. Committing the first user closes registration across
connections and process restarts. Rollback releases the lock and leaves registration available.
Deleting every user reopens registration, so that action is not a routine account-recovery path.
PostgreSQL default READ COMMITTED isolation allows a waiting contender to see the committed user
before its protected count. Do not change transaction isolation without retesting that behavior.

## Trust boundary and consequences

Registration is unauthenticated only while there are no users. Credential validation, password
hashing, request limits, and database serialization are server-side. Passwords and tokens must not
be logged. The lock prevents two winners; it does not establish that the first caller owns the
deployment. Operators must complete bootstrap on a trusted local/restricted network before making
the installation publicly reachable. A future remote unattended bootstrap needs a separately
reviewed ownership credential or enrollment mechanism.

This design requires PostgreSQL, introduces no schema change, and preserves the existing HTTP
request/response shapes. The real-database regression exercises first registration, login,
registration closure, and simultaneous registration attempts. It runs in the disposable PostgreSQL
CI job; mocked route tests remain useful for token configuration and invalid inputs.

## Rollout and rollback

Apply the normal migrations, deploy the corrected API, and verify first registration on a fresh
test database. Existing installations with users remain closed. Roll back the API only to a build
that preserves the transaction lock and a Prisma-supported result type; the former void-returning
query makes fresh registration fail. No database rollback or credential rotation is required.
