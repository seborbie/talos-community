import { expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Medium: exercise Git's Windows-style checkout conversion without requiring Windows.
test('checkout preserves digest-bound patch bytes with core.autocrlf enabled', async () => {
  const root = await mkdtemp(join(tmpdir(), 'talos-git-bytes-'));
  const git = (...args: string[]) => {
    const result = Bun.spawnSync(['git', ...args], { cwd: root });
    expect(result.exitCode).toBe(0);
  };
  try {
    const source = await readFile(
      new URL('../third-party-patches/vpx-encode-0.6.2-talos.patch', import.meta.url),
    );
    const attributes = Bun.file(new URL('../../.gitattributes', import.meta.url));
    if (await attributes.exists())
      await writeFile(join(root, '.gitattributes'), await attributes.text());
    await writeFile(join(root, 'input.patch'), source);
    git('init');
    git('-c', 'core.autocrlf=false', 'add', '.');
    await rm(join(root, 'input.patch'));
    git('-c', 'core.autocrlf=true', 'checkout-index', '-a', '-f');
    expect(await readFile(join(root, 'input.patch'))).toEqual(source);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
