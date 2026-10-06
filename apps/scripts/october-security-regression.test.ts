import { describe, expect, test } from 'bun:test';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const minimumVersions = new Map([
  ['proxy-addr', '2.0.8'],
  ['source-map-js', '1.2.2'],
]);

function vulnerableCopies(lockfile: string): string[] {
  const lock = Bun.JSONC.parse(lockfile) as { packages?: Record<string, unknown> };
  if (!lock.packages) throw new Error('Bun lockfile has no package entries');
  const findings: string[] = [];
  for (const [name, minimum] of minimumVersions) {
    let found = false;
    for (const [key, entry] of Object.entries(lock.packages)) {
      if (key !== name && !key.endsWith('/' + name)) continue;
      found = true;
      if (!Array.isArray(entry) || typeof entry[0] !== 'string') {
        throw new Error(`Invalid Bun lockfile entry for ${key}`);
      }
      const version = entry[0].slice(name.length + 1);
      if (
        !entry[0].startsWith(name + '@') ||
        !/^\d+\.\d+\.\d+$/.test(version) ||
        !Bun.semver.satisfies(version, '>=' + minimum)
      ) {
        findings.push(`${key}: ${entry[0]}`);
      }
    }
    if (!found) throw new Error(`Bun lockfile has no ${name} entry`);
  }
  return findings;
}

describe('October 2026 dependency security boundaries', () => {
  test('all locked copies exclude the reviewed vulnerable ranges', async () => {
    const lock = await readFile(new URL('../bun.lock', import.meta.url), 'utf8');
    expect(vulnerableCopies(lock)).toEqual([]);
  });

  test('rejects vulnerable nested copies even when a patched root copy exists', () => {
    const patched = {
      'proxy-addr': ['proxy-addr@2.0.8'],
      'source-map-js': ['source-map-js@1.2.2'],
    };
    expect(vulnerableCopies(JSON.stringify({ packages: patched }))).toEqual([]);
    expect(
      vulnerableCopies(
        JSON.stringify({
          packages: {
            ...patched,
            'express/proxy-addr': ['proxy-addr@2.0.7'],
            'vite/postcss/source-map-js': ['source-map-js@1.2.1'],
          },
        }),
      ),
    ).toHaveLength(2);
    expect(() => vulnerableCopies('{"packages":{}}')).toThrow();
  });
});

type SourceMapLibrary = {
  SourceMapConsumer: new (map: unknown) => {
    originalPositionFor(position: { line: number; column: number }): {
      source: string | null;
      line: number | null;
      column: number | null;
    };
  };
};

// Resolve through each real consumer, rather than testing an unrelated root package in the
// isolated workspace. Both PostCSS versions and Tailwind must receive the patched implementation.
const frontendRequire = createRequire(resolve(import.meta.dir, '../frontend/package.json'));
const viteRequire = createRequire(frontendRequire.resolve('vite'));
const tailwindPostcssRequire = createRequire(frontendRequire.resolve('@tailwindcss/postcss'));
const consumers = [
  ['Vite PostCSS', createRequire(viteRequire.resolve('postcss'))],
  ['Tailwind PostCSS', createRequire(tailwindPostcssRequire.resolve('postcss'))],
  ['Tailwind node', createRequire(tailwindPostcssRequire.resolve('@tailwindcss/node'))],
] as const;

const flatMap = { version: 3, sources: ['input.js'], names: [], mappings: 'AAAA' };
const indexedMap = (line: unknown, column: unknown = 0, map: unknown = flatMap) => ({
  version: 3,
  sections: [{ offset: { line, column }, map }],
});

for (const [name, consumerRequire] of consumers) {
  const library = consumerRequire('source-map-js') as SourceMapLibrary;
  describe(name + ' source-map security boundary', () => {
    test('rejects amplification and invalid section offsets without expanding mappings', () => {
      for (const line of [10_000_001, -1, 0.5, '1', Infinity]) {
        expect(() => new library.SourceMapConsumer(indexedMap(line))).toThrow();
      }
      for (const column of [-1, 0.5, '1', Infinity, Number.MAX_SAFE_INTEGER + 1]) {
        expect(() => new library.SourceMapConsumer(indexedMap(0, column))).toThrow();
      }
      expect(
        () =>
          new library.SourceMapConsumer(
            indexedMap(5_000_000, 0, indexedMap(5_000_000, 0, indexedMap(5_000_000))),
          ),
      ).toThrow();
    });

    test('retains source lookups for valid flat and indexed maps at the allowed boundary', () => {
      expect(
        new library.SourceMapConsumer(flatMap).originalPositionFor({ line: 1, column: 0 }),
      ).toMatchObject({ source: 'input.js', line: 1, column: 0 });
      expect(
        new library.SourceMapConsumer(indexedMap(10_000_000)).originalPositionFor({
          line: 10_000_001,
          column: 1,
        }),
      ).toMatchObject({ source: 'input.js', line: 1, column: 0 });
    });
  });
}
