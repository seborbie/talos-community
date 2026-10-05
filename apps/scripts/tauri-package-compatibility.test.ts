import assert from 'node:assert/strict';
import { test } from 'bun:test';

// Tauri's actual build preflight rejects mismatched JS/Rust major or minor versions:
// https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/info/plugins.rs
// Frontend asset builds alone do not exercise that native packaging boundary.
test.each(['talos_viewer', 'talos_permissions_helper', 'talos_worker_chat'])(
  '%s keeps native API and installed plugin versions compatible with locked Rust crates',
  async (frontend) => {
    const lock = Bun.TOML.parse(
      await Bun.file(new URL('../Cargo.lock', import.meta.url)).text(),
    ) as { package?: unknown };
    assert.ok(Array.isArray(lock.package), 'Cargo lock must contain package entries');
    const versions = new Map<string, string[]>();
    for (const entry of lock.package) {
      assert.ok(entry && typeof entry === 'object');
      assert.ok('name' in entry && typeof entry.name === 'string');
      assert.ok('version' in entry && typeof entry.version === 'string');
      versions.set(entry.name, [...(versions.get(entry.name) ?? []), entry.version]);
    }

    const manifest: unknown = JSON.parse(
      await Bun.file(new URL(`../${frontend}/package.json`, import.meta.url)).text(),
    );
    assert.ok(manifest && typeof manifest === 'object' && 'dependencies' in manifest);
    const dependencies = manifest.dependencies;
    assert.ok(dependencies && typeof dependencies === 'object');
    let apiChecked = false;
    for (const [name, version] of Object.entries(dependencies)) {
      const crate =
        name === '@tauri-apps/api'
          ? 'tauri'
          : name.startsWith('@tauri-apps/plugin-')
            ? name.replace('@tauri-apps/', 'tauri-')
            : undefined;
      if (!crate) continue;
      const locked = versions.get(crate);
      // Match upstream: only installed JS/Rust plugin pairs participate.
      if (crate !== 'tauri' && !locked) continue;
      assert.equal(locked?.length, 1, `${crate} must have one reviewed workspace release`);
      assert.ok(typeof version === 'string');
      assert.match(version, /^\d+\.\d+\.\d+$/);
      const rustVersion = locked?.[0];
      assert.ok(rustVersion);
      assert.match(rustVersion, /^\d+\.\d+\.\d+$/);
      assert.equal(
        version.split('.').slice(0, 2).join('.'),
        rustVersion.split('.').slice(0, 2).join('.'),
        `${frontend}: ${name} ${version} is incompatible with ${crate} ${rustVersion}`,
      );
      if (crate === 'tauri') apiChecked = true;
    }
    assert.ok(apiChecked, `${frontend} must retain its checked Tauri API dependency`);
  },
);

// The viewer consumes Wry's controller directly; incompatible COM crate identities
// break its accelerator-key cast even when JavaScript/Rust package versions match.
test('viewer and Wry share the Windows WebView2 COM crate identities', async () => {
  const lock = Bun.TOML.parse(await Bun.file(new URL('../Cargo.lock', import.meta.url)).text()) as {
    package: { name: string; version: string; dependencies?: string[] }[];
  };
  const packageFor = (name: string) => {
    const matches = lock.package.filter((entry) => entry.name === name);
    assert.equal(matches.length, 1, `${name} must have one workspace release`);
    return matches[0];
  };
  const dependencyId = (owner: string, name: string) => {
    const entries = packageFor(owner)?.dependencies?.filter(
      (entry) => entry === name || entry.startsWith(`${name} `),
    );
    assert.equal(entries?.length, 1, `${owner} must resolve exactly one ${name}`);
    const entry = entries?.[0];
    assert.ok(entry);
    return entry === name ? `${name} ${packageFor(name)?.version}` : entry;
  };
  for (const name of ['webview2-com', 'windows-core']) {
    assert.equal(
      dependencyId('talos_viewer', name),
      dependencyId('wry', name),
      `${name}: viewer must use the COM types returned by Wry`,
    );
  }
});
