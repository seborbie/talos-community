import { expect, test } from 'bun:test';

test('the required dependency-security check runs for every pull request', async () => {
  const workflow = Bun.YAML.parse(
    await Bun.file(new URL('../../.github/workflows/security.yml', import.meta.url)).text(),
  ) as { on: Record<string, { paths?: string[]; 'paths-ignore'?: string[] } | null> };
  expect(Object.hasOwn(workflow.on, 'pull_request')).toBe(true);
  expect(workflow.on.pull_request?.paths).toBeUndefined();
  expect(workflow.on.pull_request?.['paths-ignore']).toBeUndefined();
});
