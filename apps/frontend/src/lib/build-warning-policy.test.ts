import { describe, expect, test } from 'bun:test';
import {
  createFrontendBuildWarningGate,
  isRegisteredGeneratedSvelteWarning,
} from './build-warning-policy';

const registeredFile = 'src/routes/dashboard/devices/+page.svelte';
const registeredAnnotation =
  `${registeredFile} (58:6): A comment\n\n"/* @__PURE__ */"\n\n` +
  `in "${registeredFile}" contains an annotation that Rollup cannot interpret due to the ` +
  'position of the comment. The comment will be removed to avoid issues.';
const registeredSourceMap =
  `${registeredFile} (58:6): Error when using sourcemap for reporting an error: ` +
  "Can't resolve original location of error.";

describe('frontend build warning policy', () => {
  test('recognizes only the registered generated Svelte diagnostics', () => {
    expect(isRegisteredGeneratedSvelteWarning(registeredAnnotation)).toBe(true);
    expect(isRegisteredGeneratedSvelteWarning(registeredSourceMap)).toBe(true);
    expect(
      isRegisteredGeneratedSvelteWarning(
        registeredAnnotation.replace(registeredFile, 'src/routes/new-page.svelte'),
      ),
    ).toBe(false);
    expect(
      isRegisteredGeneratedSvelteWarning(
        registeredAnnotation.replace('Rollup cannot interpret', 'Rollup found a different issue'),
      ),
    ).toBe(false);
    expect(isRegisteredGeneratedSvelteWarning('src/source.ts (1:1): another warning')).toBe(false);
  });

  test('forwards every unregistered warning to Vite', () => {
    const forwarded: string[] = [];
    const handleWarning = createFrontendBuildWarningGate();

    handleWarning(registeredAnnotation, (message) => forwarded.push(message));
    handleWarning('circular dependency detected', (message) => forwarded.push(message));

    expect(forwarded).toEqual(['circular dependency detected']);
  });

  test('fails when the registered warning baseline grows', () => {
    const handleWarning = createFrontendBuildWarningGate(1);
    handleWarning(registeredAnnotation, () => undefined);

    expect(() => handleWarning(registeredSourceMap, () => undefined)).toThrow(
      'Registered frontend build-warning baseline grew beyond 1',
    );
  });
});
