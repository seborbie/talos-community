import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';

const helper = await Bun.file(
  new URL('../../scripts/install-community-buildx.sh', import.meta.url),
).text();
const reviewedSha = '982ca20490b45ed1ec8d99795974d3d874a358f75938c9c237305010e6b7e548';
const fixtureBytes = 'verified Buildx fixture\n';
const fixtureSha = createHash('sha256').update(fixtureBytes).digest('hex');

// Medium tests: run the actual helper in Bash with a disposable filesystem and shell
// functions replacing network/platform/Docker boundaries. Real SHA-256 verification runs.
describe('Community Buildx acquisition', () => {
  let root: string;
  beforeEach(async () => {
    root = await mkdtemp(resolve(tmpdir(), 'talos-buildx-'));
    await mkdir(resolve(root, 'temp with spaces'));
  });
  afterEach(async () => {
    await rm(root, { recursive: true, force: true });
  });

  const stubs = `
uname() { if [[ "$1" = -s ]]; then printf '%s\\n' "\${TEST_OS}"; else printf 'x86_64\\n'; fi; }
curl() {
  local output=''
  while [[ $# -gt 0 ]]; do
    if [[ "$1" = --output ]]; then output="$2"; shift; fi
    shift
  done
  [[ "\${TEST_DOWNLOAD}" != error ]] || return 22
  if [[ "\${TEST_DOWNLOAD}" = tampered ]]; then printf 'tampered\\n' > "$output";
  else printf 'verified Buildx fixture\\n' > "$output"; fi
}
install() {
  command install "$@" || return
  if [[ "\${TEST_INSTALL}" = corrupt ]]; then printf 'corrupt\\n' > "$4"; fi
}
docker() {
  printf '%s\\n' "$*" >> docker-calls
  [[ "\${TEST_DOCKER}" != unavailable ]] || return 1
  if [[ "$*" = 'buildx version' ]]; then
    if [[ "\${TEST_DOCKER}" = wrong-version ]]; then printf 'github.com/docker/buildx v0.1.0 abc\\n';
    else printf 'github.com/docker/buildx v0.37.2 abc\\n'; fi
  fi
}
`;

  function run(overrides: Record<string, string> = {}) {
    // The production Ubuntu runner provides GNU sha256sum; macOS's command differs.
    // shasum performs the same real checksum verification for this local fixture.
    const checksumAdapter =
      process.platform === 'darwin' ? 'sha256sum() { command shasum -a 256 "$@"; }\n' : '';
    return Bun.spawnSync(
      ['bash', '-c', checksumAdapter + stubs + '\n' + helper.replace(reviewedSha, fixtureSha)],
      {
        cwd: root,
        env: {
          ...process.env,
          RUNNER_TEMP: './temp with spaces',
          DOCKER_CONFIG: './docker with spaces',
          TEST_OS: 'Linux',
          TEST_DOWNLOAD: 'valid',
          TEST_INSTALL: 'valid',
          TEST_DOCKER: 'available',
          ...overrides,
        },
      },
    );
  }

  test('retains the reviewed upstream version and published binary checksum', () => {
    expect(helper).toContain('version=v0.37.2\n');
    expect(helper).toContain('sha256=' + reviewedSha + '\n');
    expect(helper).toContain(
      'https://github.com/docker/buildx/releases/download/${version}/buildx-${version}.linux-amd64',
    );
  });

  test('verifies before probing Docker, installs selected plugin and removes its download', async () => {
    const result = run();
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    expect(
      await readFile(resolve(root, 'docker with spaces/cli-plugins/docker-buildx'), 'utf8'),
    ).toBe(fixtureBytes);
    expect(await readFile(resolve(root, 'docker-calls'), 'utf8')).toBe('buildx\nbuildx version\n');
    expect(await readdir(resolve(root, 'temp with spaces'))).toEqual([]);
  });

  test.each(['tampered', 'error'])(
    'rejects %s downloads before Docker execution',
    async (download) => {
      expect(run({ TEST_DOWNLOAD: download }).exitCode).not.toBe(0);
      expect(await Bun.file(resolve(root, 'docker-calls')).exists()).toBe(false);
      expect(
        await Bun.file(resolve(root, 'docker with spaces/cli-plugins/docker-buildx')).exists(),
      ).toBe(false);
      expect(await readdir(resolve(root, 'temp with spaces'))).toEqual([]);
    },
  );

  test('rejects corruption introduced during installation before Docker execution', async () => {
    expect(run({ TEST_INSTALL: 'corrupt' }).exitCode).not.toBe(0);
    expect(await Bun.file(resolve(root, 'docker-calls')).exists()).toBe(false);
  });

  test.each(['unavailable', 'wrong-version'])('stops on an %s Docker plugin', (state) => {
    expect(run({ TEST_DOCKER: state }).exitCode).not.toBe(0);
  });

  test('rejects unsupported platforms without invoking Docker', async () => {
    expect(run({ TEST_OS: 'Darwin' }).exitCode).not.toBe(0);
    expect(await Bun.file(resolve(root, 'docker-calls')).exists()).toBe(false);
  });
});
