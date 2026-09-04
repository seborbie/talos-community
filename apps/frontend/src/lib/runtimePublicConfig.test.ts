import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { resolveRuntimePublicServiceUrls } from './runtimePublicConfig';

describe('runtime public service configuration', () => {
  test('resolves installation-specific endpoints without a frontend rebuild', () => {
    expect(
      resolveRuntimePublicServiceUrls({
        PUBLIC_API_URL: 'https://api.one.example',
        PUBLIC_RMM_API_URL: 'https://control.one.example',
      }),
    ).toEqual({
      apiUrl: 'https://api.one.example',
      rmmApiUrl: 'https://control.one.example',
    });
    expect(
      resolveRuntimePublicServiceUrls({
        PUBLIC_API_URL: 'https://api.two.example',
        PUBLIC_RMM_API_URL: 'https://control.two.example',
      }),
    ).toEqual({
      apiUrl: 'https://api.two.example',
      rmmApiUrl: 'https://control.two.example',
    });
  });

  test('requires an API URL and allows RMM browser APIs to be disabled explicitly', () => {
    expect(() => resolveRuntimePublicServiceUrls({})).toThrow('PUBLIC_API_URL is not configured');
    expect(resolveRuntimePublicServiceUrls({ PUBLIC_API_URL: 'http://127.0.0.1:3001' })).toEqual({
      apiUrl: 'http://127.0.0.1:3001',
      rmmApiUrl: null,
    });
  });

  test('rejects unsafe or ambiguous public endpoint values', () => {
    expect(() =>
      resolveRuntimePublicServiceUrls({ PUBLIC_API_URL: 'javascript:alert(1)' }),
    ).toThrow('absolute HTTP(S) URL');
    expect(() =>
      resolveRuntimePublicServiceUrls({
        PUBLIC_API_URL: 'https://user:secret@example.test',
      }),
    ).toThrow('without credentials or a fragment');
    expect(() =>
      resolveRuntimePublicServiceUrls({
        PUBLIC_API_URL: 'https://api.example.test/#unexpected',
      }),
    ).toThrow('without credentials or a fragment');
  });

  test('the production image reads endpoints dynamically instead of baking build arguments', () => {
    const apiSource = readFileSync(resolve(import.meta.dir, 'api.ts'), 'utf8');
    const viewerSource = readFileSync(resolve(import.meta.dir, 'viewer-launcher.ts'), 'utf8');
    const dockerfile = readFileSync(resolve(import.meta.dir, '..', '..', 'Dockerfile'), 'utf8');

    expect(apiSource).toContain("from '$env/dynamic/public'");
    expect(viewerSource).toContain("from '$env/dynamic/public'");
    expect(apiSource).not.toContain("from '$env/static/public'");
    expect(viewerSource).not.toContain("from '$env/static/public'");
    expect(dockerfile).not.toContain('ARG PUBLIC_API_URL');
    expect(dockerfile).not.toContain('ARG PUBLIC_RMM_API_URL');
  });
});
