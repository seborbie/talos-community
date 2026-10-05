# October 2026 rustls advisory triage

The October 2 pinned RustSec audit found `rustls` 0.23.36 in the Community Cargo lockfile.
[RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html) affects 0.23.13
through 0.23.44 and is fixed in 0.23.45. Rustls could accept a TLS 1.3 handshake message
at the wrong encryption level after a key change instead of closing the connection.
The handshake transcript remains authenticated; the advisory does not establish that an
on-path attacker can forge or complete a handshake.

Talos uses rustls directly in the relay, protocol transport, and worker, and transitively
through network clients. These are network trust boundaries. The owner of identity and
authorization remains the Talos service and its authenticated session protocols; this
dependency update does not change credentials, certificate verification settings, or
authorization. A malicious peer might exploit the affected TLS state handling, although
no Talos-specific exploit was demonstrated. The reviewed fix raises each direct rustls
minimum to 0.23.45 and updates the one workspace lockfile, without an audit exception.

The focused lockfile regression rejects the affected release range. The pinned RustSec
audit must still run to detect future advisories. Reverting the lockfile or manifest
minimums would restore the affected version and requires an explicit security review.
Qualified review and cross-platform TLS/build checks are required before integration.
