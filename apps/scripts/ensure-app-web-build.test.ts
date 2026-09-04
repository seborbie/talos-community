import { describe, expect, test } from 'bun:test';
import { frontendBuildArgumentHashParts } from './ensure-app-web-build';

describe('app web build hashing', () => {
  test('matches the Compose defaults for every baked frontend URL', () => {
    expect(frontendBuildArgumentHashParts({})).toEqual([
      'PUBLIC_API_URL=http://localhost:3001',
      'PUBLIC_RMM_API_URL=http://localhost:3002',
    ]);
  });

  test('changes when either public build argument changes', () => {
    expect(
      frontendBuildArgumentHashParts({
        PUBLIC_API_URL: 'https://api.community.example',
        PUBLIC_RMM_API_URL: 'https://rmm.community.example',
      }),
    ).toEqual([
      'PUBLIC_API_URL=https://api.community.example',
      'PUBLIC_RMM_API_URL=https://rmm.community.example',
    ]);
  });
});
