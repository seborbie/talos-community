import { afterEach, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const repoRoot = resolve(import.meta.dir, '../..');
const buildScript = resolve(repoRoot, 'scripts/build-linux-agent.sh');
const temporaryDirectories: string[] = [];

function temporaryDirectory(prefix: string): string {
  const directory = mkdtempSync(join(tmpdir(), prefix));
  temporaryDirectories.push(directory);
  return directory;
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe('Linux Docker source isolation', () => {
  test('copies working-tree source but excludes ignored environment and signing material', () => {
    const fixture = temporaryDirectory('talos-build-context-fixture-');
    mkdirSync(join(fixture, 'apps/certs'), { recursive: true });
    mkdirSync(join(fixture, 'apps/example/src'), { recursive: true });
    writeFileSync(join(fixture, '.gitignore'), '.env\n*.pem\n*.pfx\napps/installer/tmp/\n');
    writeFileSync(join(fixture, 'apps/example/Cargo.toml'), "[package]\nname='example'\n");
    writeFileSync(join(fixture, 'apps/example/src/lib.rs'), 'pub fn tracked() {}\n');
    writeFileSync(join(fixture, 'apps/example/src/untracked.rs'), 'pub fn untracked() {}\n');
    writeFileSync(join(fixture, '.env'), 'FAKE_SECRET=must-not-copy\n');
    writeFileSync(join(fixture, 'apps/certs/relay.pem'), 'fake relay key\n');
    writeFileSync(join(fixture, 'apps/certs/update.pfx'), 'fake updater key\n');
    const publicKey = join(fixture, 'manifest-public.der');
    writeFileSync(publicKey, 'fake public key\n');

    const initialize = Bun.spawnSync(['git', 'init', '--quiet'], {
      cwd: fixture,
      stdout: 'pipe',
      stderr: 'pipe',
    });
    expect(initialize.exitCode).toBe(0);
    const stage = Bun.spawnSync(
      ['git', 'add', '.gitignore', 'apps/example/Cargo.toml', 'apps/example/src/lib.rs'],
      { cwd: fixture, stdout: 'pipe', stderr: 'pipe' },
    );
    expect(stage.exitCode).toBe(0);

    const verify = Bun.spawnSync(
      [
        'bash',
        '-c',
        `set -euo pipefail
source "$BUILD_SCRIPT"
REPO_ROOT="$FIXTURE_ROOT"
MANIFEST_PUBLIC_KEY_DER_PATH="$PUBLIC_KEY"
prepare_sanitized_build_context
test -f "$SANITIZED_BUILD_CONTEXT/apps/example/Cargo.toml"
test -f "$SANITIZED_BUILD_CONTEXT/apps/example/src/lib.rs"
test -f "$SANITIZED_BUILD_CONTEXT/apps/example/src/untracked.rs"
test -f "$SANITIZED_BUILD_CONTEXT/apps/installer/tmp/manifest_public_key.der"
test ! -e "$SANITIZED_BUILD_CONTEXT/.env"
test ! -e "$SANITIZED_BUILD_CONTEXT/apps/certs/relay.pem"
test ! -e "$SANITIZED_BUILD_CONTEXT/apps/certs/update.pfx"`,
      ],
      {
        env: {
          ...process.env,
          BUILD_SCRIPT: buildScript,
          FIXTURE_ROOT: fixture,
          PUBLIC_KEY: publicKey,
        },
        stdout: 'pipe',
        stderr: 'pipe',
      },
    );

    expect(new TextDecoder().decode(verify.stderr)).toBe('');
    expect(verify.exitCode).toBe(0);
  });
});
