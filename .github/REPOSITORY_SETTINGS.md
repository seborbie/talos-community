# Public repository settings checklist

This file records settings that cannot be enforced by source alone. It is not evidence that a
repository has been created or that any setting is active.

For public operation, an owner must:

1. confirm the exact repository owner/name and corresponding-source URL;
2. make `main` the default branch and prohibit deletion and force pushes;
3. require the cross-platform quality, dependency-security, release-policy, and secret-scan checks;
4. require at least one qualified human review for security-sensitive and release changes;
5. restrict workflow-token permissions to read-only by default and approve every write permission;
6. enable Dependabot alerts/updates, secret scanning, push protection, dependency graph, and private
   vulnerability reporting;
7. restrict environments that hold updater-manifest signing material to reviewed release jobs with
   required approvers, and keep PFX/password material out of pull-request and general build jobs;
8. prevent unreviewed Actions and reusable workflows, and retain pinned action commit SHAs;
9. disable automatic release publication: candidate artifacts require a separate human approval;
10. test the private security and conduct-reporting destinations from a non-maintainer account; and
11. verify an anonymous clone can run the documented source and release-bundle checks.

Do not add `CODEOWNERS`, `FUNDING.yml`, contact links, repository URLs, or environment reviewers by
guessing names. Add them only after the owner supplies and verifies those values.

## Initial source publication

The official repository is `seborbie/talos-community`. On 2026-09-04, private reporting,
secret scanning, push protection, Dependabot alerts/updates, and read-only default workflow tokens
were enabled through GitHub. Reporting was enabled on an empty public repository before source
upload because GitHub requires public visibility for the feature. The non-maintainer intake test
and outstanding human reviews remain tracked in
[PUB-001](https://github.com/seborbie/talos-community/issues/1), due 2026-09-11.
No release-signing secrets or supported binary releases were provisioned by source publication.
