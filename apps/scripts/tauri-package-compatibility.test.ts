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
