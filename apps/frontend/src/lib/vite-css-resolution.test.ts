import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { resolveConfig } from 'vite';
import { frontendViteConfig } from '../../vite.config.ts';

test('Tailwind CSS package imports resolve to files in both SSR and client builds', async () => {
  const root = fileURLToPath(new URL('../..', import.meta.url));
  const config = await resolveConfig(
    {
      ...frontendViteConfig,
      // Exercise Vite's CSS resolver in isolation; production builds cover SvelteKit's hooks.
      plugins: [],
      root,
      configFile: false,
      envDir: false,
      logLevel: 'silent',
    },
    'build',
  );
  const resolveCss = config.createResolver({
    extensions: ['.css'],
    mainFields: ['style'],
    conditions: ['style'],
    tryIndex: false,
    preferRelative: true,
  });

  for (const ssr of [false, true]) {
    const resolved = await resolveCss('tailwindcss', `${root}/src/app.css`, false, ssr);
    assert.ok(resolved, `Tailwind CSS import must resolve for ssr=${ssr}`);
    assert.match(resolved, /[/\\]tailwindcss[/\\]index\.css$/);
    assert.equal(await Bun.file(resolved).exists(), true);
  }
});
