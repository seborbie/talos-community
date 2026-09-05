import { describe, expect, test } from 'bun:test';
import { readFile } from 'node:fs/promises';

// Supplement RustSec with the patched versions reviewed in issues #30 and #14. This is a
// regression guard for that remediation, not a replacement for current audits.
const reviewedRanges = [
  { name: 'openssl', vulnerable: '>=0.9.0 <0.10.80', patched: '0.10.80' },
  { name: 'serde_with', vulnerable: '>=2.3.0 <3.21.0', patched: '3.21.0' },
  { name: 'rand', vulnerable: '>=0.7.0 <0.8.6', patched: '0.8.6' },
  { name: 'rand', vulnerable: '>=0.9.0 <0.9.3', patched: '0.9.3' },
  { name: 'rand', vulnerable: '=0.10.0', patched: '0.10.1' },
  { name: 'chacha20', vulnerable: '>=0.10.0 <0.10.2', patched: '0.10.2' },
];

function violations(lockfile: string): string[] {
  const lock = Bun.TOML.parse(lockfile) as { package?: unknown };
  if (!Array.isArray(lock.package) || lock.package.length === 0) {
    throw new Error('Cargo lockfile has no package entries');
  }
  const findings: string[] = [];
  for (const entry of lock.package) {
    if (
      typeof entry !== 'object' ||
      entry === null ||
      !('name' in entry) ||
      !('version' in entry) ||
      typeof entry.name !== 'string' ||
      typeof entry.version !== 'string'
    ) {
      throw new Error('Invalid Cargo lockfile package entry');
    }
    for (const rule of reviewedRanges.filter((rule) => rule.name === entry.name)) {
      if (
        !/^\d+\.\d+\.\d+$/.test(entry.version) ||
        Bun.semver.satisfies(entry.version, rule.vulnerable)
      ) {
        findings.push(`${entry.name}@${entry.version}: reviewed fix ${rule.patched}`);
      }
    }
  }
  return findings;
}

const fixture = (name: string, version: string) =>
  `version = 4\n[[package]]\nname = "${name}"\nversion = "${version}"\n`;

describe('reviewed Cargo security fixes', () => {
  test('the workspace lock does not restore vulnerable package versions', async () => {
    const lock = await readFile(new URL('../Cargo.lock', import.meta.url), 'utf8');
    expect(violations(lock)).toEqual([]);
  });

  test('recognizes the affected release lines and patched boundaries', () => {
    for (const [name, version] of [
      ['openssl', '0.10.75'],
      ['openssl', '0.10.79'],
      ['serde_with', '3.16.1'],
      ['rand', '0.8.5'],
      ['rand', '0.9.2'],
      ['rand', '0.10.0'],
      ['chacha20', '0.10.0'],
      ['chacha20', '0.10.1'],
    ] as const) {
      expect(violations(fixture(name, version)).length).toBeGreaterThan(0);
    }
    for (const { name, patched } of reviewedRanges) {
      expect(violations(fixture(name, patched))).toEqual([]);
    }
  });

  test('fails closed on an empty lock or an unreviewed prerelease', () => {
    expect(() => violations('version = 4')).toThrow();
    expect(violations(fixture('openssl', '0.10.80-beta.1')).length).toBeGreaterThan(0);
  });
});
