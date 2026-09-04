import { describe, expect, test } from 'bun:test';
import { parseCommunityReleaseIdentity } from './community-release-version';

const validInput = {
  tag: 'community-v1.2.3-rc.1',
  sourceSha: 'a'.repeat(40),
  refType: 'tag',
  tagObjectType: 'tag',
};

describe('Community release version ownership', () => {
  test('derives the release version from one annotated Community tag', () => {
    expect(parseCommunityReleaseIdentity(validInput)).toEqual({
      tag: 'community-v1.2.3-rc.1',
      version: '1.2.3-rc.1',
      sourceSha: 'a'.repeat(40),
    });
  });

  test('rejects branch dispatches and lightweight tags', () => {
    expect(() => parseCommunityReleaseIdentity({ ...validInput, refType: 'branch' })).toThrow(
      'must be dispatched from a tag ref',
    );
    expect(() => parseCommunityReleaseIdentity({ ...validInput, tagObjectType: 'commit' })).toThrow(
      'require an annotated tag',
    );
  });

  test('rejects unsafe, ambiguous, and manually duplicated version forms', () => {
    for (const tag of [
      'v1.2.3',
      'community-v01.2.3',
      'community-v1.2.3+rebuilt',
      'community-v1.2.3/../../release',
    ]) {
      expect(() => parseCommunityReleaseIdentity({ ...validInput, tag })).toThrow(
        'release tag must match',
      );
    }
    expect(() =>
      parseCommunityReleaseIdentity({ ...validInput, sourceSha: 'A'.repeat(40) }),
    ).toThrow('lowercase full 40-character');
  });
});
