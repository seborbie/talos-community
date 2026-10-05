import { describe, expect, test } from 'bun:test';
import { readFile } from 'node:fs/promises';

// Supplement the live advisory audit when registry access is unavailable. All resolved copies
// must exclude the devalue releases affected by the reviewed 2026 advisories.
function vulnerableDevalueCopies(lockfile: string): string[] {
  const lock = Bun.JSONC.parse(lockfile) as { packages?: Record<string, unknown> };
  if (!lock.packages || Object.keys(lock.packages).length === 0) {
    throw new Error('Bun lockfile has no package entries');
  }

  const findings: string[] = [];
  for (const [key, entry] of Object.entries(lock.packages)) {
    if (key !== 'devalue' && !key.endsWith('/devalue')) continue;
    if (!Array.isArray(entry) || typeof entry[0] !== 'string') {
      throw new Error(`Invalid Bun lockfile entry for ${key}`);
    }
    const match = /^devalue@(\d+\.\d+\.\d+)$/.exec(entry[0]);
    if (!match || Bun.semver.satisfies(match[1], '<5.9.3')) {
      findings.push(`${key}: ${entry[0]}`);
    }
  }
  if (Object.keys(lock.packages).every((key) => key !== 'devalue' && !key.endsWith('/devalue'))) {
    throw new Error('Bun lockfile has no devalue entry');
  }
  return findings;
}

const fixture = (entries: Record<string, unknown>) => JSON.stringify({ packages: entries });

describe('reviewed Bun security fix', () => {
  test('all resolved devalue copies are patched', async () => {
    const lock = await readFile(new URL('../bun.lock', import.meta.url), 'utf8');
    expect(vulnerableDevalueCopies(lock)).toEqual([]);
  });

  test('rejects vulnerable root and nested copies while accepting the patched boundary', () => {
    expect(vulnerableDevalueCopies(fixture({ devalue: ['devalue@5.9.0'] }))).toHaveLength(1);
    expect(vulnerableDevalueCopies(fixture({ 'svelte/devalue': ['devalue@5.9.2'] }))).toHaveLength(
      1,
    );
    expect(vulnerableDevalueCopies(fixture({ devalue: ['devalue@5.9.3'] }))).toEqual([]);
  });

  test('fails closed on missing or unreviewed devalue entries', () => {
    expect(() => vulnerableDevalueCopies(fixture({}))).toThrow();
    expect(() => vulnerableDevalueCopies(fixture({ unrelated: ['unrelated@1.0.0'] }))).toThrow();
    expect(vulnerableDevalueCopies(fixture({ devalue: ['devalue@5.9.4-beta.1'] }))).toHaveLength(1);
  });
});
