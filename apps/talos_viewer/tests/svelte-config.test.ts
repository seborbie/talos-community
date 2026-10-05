import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compile, preprocess } from 'svelte/compiler';
import config from '../svelte.config.js';

test('Svelte build configuration preprocesses TypeScript before Rollup parses it', async () => {
  const source = `<script lang="ts">
    async function stop(expected?: string): Promise<string | undefined> { return expected; }
  </script>`;

  assert.ok(config.preprocess, 'Svelte preprocess configuration must be present');
  const processed = await preprocess(source, config.preprocess, {
    filename: 'typescript-preprocess-regression.svelte',
  });

  assert.doesNotMatch(processed.code, /expected\?: string/);
  assert.doesNotMatch(processed.code, /Promise<string \| undefined>/);
});

test('TypeScript preprocessing preserves imports used only by template stores and expressions', async () => {
  const source = `<script lang="ts">
    import { templateStore } from './template-store';
    import { formatLabel } from './template-label';
    import type { Readable } from 'svelte/store';
    const count: number = 1;
  </script>
  <p>{$templateStore}: {formatLabel(count)}</p>`;

  const filename = 'template-import-preprocess-regression.svelte';
  assert.ok(config.preprocess, 'Svelte preprocess configuration must be present');
  const processed = await preprocess(source, config.preprocess, { filename });

  assert.match(processed.code, /import\s*\{\s*templateStore\s*\}/);
  assert.match(processed.code, /import\s*\{\s*formatLabel\s*\}/);
  assert.doesNotMatch(processed.code, /import type|Readable|count:\s*number/);
  assert.doesNotThrow(() => compile(processed.code, { filename, generate: 'server' }));
});
