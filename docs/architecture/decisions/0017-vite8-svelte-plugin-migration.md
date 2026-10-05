# ADR-0017: Coordinate Vite 8 with the supported Svelte plugin

- Status: proposed; awaiting PR integration
- Date: 2026-10-02
- Owner: Talos maintainers

## Context

PR #43 proposed Vite 8.3.0 for the web frontend and three native frontends while retaining
`@sveltejs/vite-plugin-svelte` 6.2.4. That plugin declares only Vite 6/7 peers. Vite 8 replaces
the JavaScript transformer with Oxc and the production bundler with Rolldown. The previous
preprocessor drops value imports referenced only in Svelte markup, including imported stores
used with `$store`. Four added regression tests fail on that graph and pass with the released
upstream plugin fix.

The complete web build also revealed that Vite's SSR CSS resolver returned the bare
`tailwindcss` package identifier instead of its CSS file. Client resolution worked. A focused
resolver regression reproduces this independently of the build and passes with the configuration
below.

## Options considered

1. Keep Vite 7.3.6 and the current plugin. This is the rollback path, but defers the supported
   build-tool migration and retains the generated annotation warning exception.
2. Override transformer behavior or patch the old plugin locally. This would leave a peer-
   incompatible graph and add maintenance Talos does not need.
3. Upgrade all four Vite consumers and the shared Svelte plugin together, using its released
   template-import fix and narrow SSR CSS resolution configuration.

## Decision

Use exact Vite 8.3.0 pins in all four consumers and the shared plugin catalog at 7.3.1. The
plugin's peer requirements accept the existing Svelte 5.57.1 and Vite 8 versions. Keep
`vitePreprocess({ script: true })`: Talos still needs TypeScript preprocessing, and disabling it
would hide the import bug rather than verify the supported path.

Add `ssr.noExternal: ['tailwindcss']` to the web build configuration. This makes the CSS package
resolve to its file before PostCSS processing; it does not broadly bundle server dependencies.
Use explicit file extensions in local Vite configuration imports to satisfy its native-loader
compatibility check. Add the changed preprocessing and CSS resolver tests to the formatting gate.

Remove the obsolete DR-011 custom logger and exception only after a complete build with the
normal Vite logger confirms the generated annotation/source-map diagnostics no longer occur.
Ordinary Vite and Rolldown diagnostics remain visible.

## Consequences and compatibility

The bundler, transformer, generated chunks and plugin graph change. Frozen installs, licence and
advisory policy, zero-diagnostic Svelte checks, all production frontend builds, native platform CI
and browser checks remain required. Dependency and lockfile pins must move together. The focused
tests verify template imports, type erasure, valid compiled stores, and CSS file resolution in
both SSR and client modes. They do not establish every desktop or authenticated user journey.

There is no change to API authorization, production endpoints, secret sourcing, database schemas,
remote command execution or update policy. Native packaging and signed installer execution remain
separate release gates. Development HMR and production SSR should both receive browser verification.

## Rollout

The October 2 repair was stacked on grouped Bun PR #47. On October 5, Dependabot closed #47
unmerged and deleted its branch, which also closed #43. Recover the preserved Vite repair in a
maintainer-owned draft stacked on replacement #52, retaining the shared #48/#49 ancestry and
ADR-0018 native coordination. Branch cleanup is not evidence of integration.

The regression failures on the original plugin were verified before the repair. Verify the
combined graph and all applicable gates on its exact published revision. Re-request review of
the previously rejected Vite proposal; carry forward its review context without dismissing it.
After prerequisites merge, retarget to main and refresh the final diff and CI before integration.
Retarget dependents before deleting a merged prerequisite branch.

## Rollback

Restore Vite 7.3.6 and plugin 6.2.4 with their root lockfile together. Restore the prior constrained
DR-011 logger and time-bounded register entry if its warnings return. The new preprocessing
regressions remain useful on Vite 7; remove the Vite-8-specific SSR configuration only after
checking CSS resolution on the restored graph. No application-data migration is involved.

## References

- [Vite 8 migration](https://vite.dev/guide/migration)
- [Official Svelte plugin 7 releases and peer migration](https://github.com/sveltejs/vite-plugin-svelte/releases)
- [Upstream template-import fix #1326](https://github.com/sveltejs/vite-plugin-svelte/pull/1326)
- [Vite SSR externalization options](https://vite.dev/config/ssr-options#ssr-noexternal)
- [Vite configuration loading](https://vite.dev/config/#config-loading)
