import { $ } from 'bun';

/** True if `docker image inspect` succeeds for this reference (image present locally). */
export async function dockerImageExists(imageRef: string): Promise<boolean> {
  const r = await $`docker image inspect ${imageRef}`.quiet().nothrow();
  return r.exitCode === 0;
}
