const GENERATED_PURE_ANNOTATION = '/* @__PURE__ */';

const REGISTERED_GENERATED_SVELTE_FILES = new Set([
  'src/lib/components/FeatureUpgradePreflightChecklist.svelte',
  'src/routes/dashboard/+page.svelte',
  'src/routes/dashboard/devices/+page.svelte',
  'src/routes/dashboard/rmm/patches/+page.svelte',
  'src/routes/dashboard/rmm/patches/feature-upgrades/+page.svelte',
]);

export const MAX_REGISTERED_FRONTEND_BUILD_WARNINGS = 45;

type ParsedSvelteWarning = {
  file: string;
  detail: string;
};

function parseSvelteWarning(message: string): ParsedSvelteWarning | null {
  const match = /^(src\/[^\n]+\.svelte) \(\d+:\d+\): ([\s\S]+)$/.exec(message);
  if (!match) {
    return null;
  }

  return { file: match[1], detail: match[2] };
}

/**
 * Svelte 5.55 currently emits pure annotations in transformed component output at positions that
 * Rollup 4 cannot retain. Vite reports both the annotation and a secondary source-map warning.
 * Keep the exception constrained to the exact generated messages and the five affected sources.
 */
export function isRegisteredGeneratedSvelteWarning(message: string): boolean {
  const parsed = parseSvelteWarning(message);
  if (!parsed || !REGISTERED_GENERATED_SVELTE_FILES.has(parsed.file)) {
    return false;
  }

  const annotationDetail =
    `A comment\n\n"${GENERATED_PURE_ANNOTATION}"\n\n` +
    `in "${parsed.file}" contains an annotation that Rollup cannot interpret due to the ` +
    'position of the comment. The comment will be removed to avoid issues.';
  const sourceMapDetail =
    "Error when using sourcemap for reporting an error: Can't resolve original location of error.";

  return parsed.detail === annotationDetail || parsed.detail === sourceMapDetail;
}

export function createFrontendBuildWarningGate(
  maxRegisteredWarnings = MAX_REGISTERED_FRONTEND_BUILD_WARNINGS,
): (message: string, defaultHandler: (message: string) => void) => void {
  let registeredWarningCount = 0;

  return (message, defaultHandler) => {
    if (!isRegisteredGeneratedSvelteWarning(message)) {
      defaultHandler(message);
      return;
    }

    registeredWarningCount += 1;
    if (registeredWarningCount > maxRegisteredWarnings) {
      throw new Error(
        `Registered frontend build-warning baseline grew beyond ${maxRegisteredWarnings}. ` +
          'Inspect the generated output before changing the exception.',
      );
    }
  };
}
