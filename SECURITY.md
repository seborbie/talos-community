# Security policy

## Supported versions

Talos Community Edition is published as a source alpha. Until a version is
published and named here, no branch or binary should be represented as a supported production
release. Security fixes are developed on the current default branch and will receive release notes
when a supported release line exists.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or include secrets, exploit details,
customer information, or private endpoints in public discussions.

Use GitHub's [private vulnerability reporting form](../../security/advisories/new). This relative
link resolves to the current Talos repository regardless of its GitHub owner. Private vulnerability reporting is enabled for the official repository. The signed-in
non-maintainer intake check remains tracked in [PUB-001](https://github.com/seborbie/talos-community/issues/1).
Fork owners must enable reporting and verify their own intake destination. Do not publish vulnerability details in an
issue or discussion.

When the private destination is available, include:

- the affected version, commit, component, and platform;
- reproduction steps or a minimal proof of concept;
- realistic impact and prerequisites;
- any suggested mitigation; and
- a safe way to contact you for follow-up.

If the private-report form is unavailable, do not substitute a public issue; contact the repository
owner through a separately verified private channel.

Maintainers will acknowledge a complete report, investigate it, coordinate a fix and disclosure,
and credit reporters who want attribution. Exact timelines depend on severity and fix complexity;
please avoid public disclosure until a coordinated remediation is available.

Operational emergencies in a self-hosted deployment remain the responsibility of that deployment's
operator. Rotate exposed credentials and signing keys immediately and preserve relevant audit
evidence without publishing it.
