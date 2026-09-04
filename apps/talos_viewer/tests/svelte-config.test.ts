import assert from 'node:assert/strict';
import { test } from 'node:test';
import { preprocess } from 'svelte/compiler';
import config from '../svelte.config.js';

test('Svelte build configuration preprocesses TypeScript before Rollup parses it', async () => {
  const source = `<script lang="ts">
    async function stop(expected?: string): Promise<string | undefined> { return expected; }
  </script>`;

  assert.ok(config.preprocess, 'Svelte preprocess configuration must be present');
  const processed = await preprocess(source, config.preprocess, {
    filename: 'typescript-preprocess-regression.svelte'
  });

  assert.doesNotMatch(processed.code, /expected\?: string/);
  assert.doesNotMatch(processed.code, /Promise<string \| undefined>/);
});
