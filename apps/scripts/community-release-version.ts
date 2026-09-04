#!/usr/bin/env bun

import { appendFile } from 'node:fs/promises';

const RELEASE_TAG_PREFIX = 'community-v';
const SEMVER_IDENTIFIER = '(?:0|[1-9][0-9]*|[A-Za-z-][0-9A-Za-z-]*)';
const RELEASE_TAG_PATTERN = new RegExp(
  `^${RELEASE_TAG_PREFIX}(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:-(${SEMVER_IDENTIFIER}(?:\\.${SEMVER_IDENTIFIER})*))?$`,
);
const SHA_PATTERN = /^[0-9a-f]{40}$/;

export type CommunityReleaseIdentity = {
  tag: string;
  version: string;
  sourceSha: string;
};

export function parseCommunityReleaseIdentity(input: {
  tag: string;
  sourceSha: string;
  refType: string;
  tagObjectType: string;
}): CommunityReleaseIdentity {
  if (input.refType !== 'tag') {
    throw new Error('Community releases must be dispatched from a tag ref');
  }
  if (input.tagObjectType !== 'tag') {
    throw new Error('Community releases require an annotated tag, not a branch or lightweight tag');
  }
  const match = RELEASE_TAG_PATTERN.exec(input.tag);
  if (!match) {
    throw new Error(`release tag must match ${RELEASE_TAG_PREFIX}<SemVer> without build metadata`);
  }
  if (!SHA_PATTERN.test(input.sourceSha)) {
    throw new Error('source SHA must be a lowercase full 40-character Git commit ID');
  }

  return {
    tag: input.tag,
    version: input.tag.slice(RELEASE_TAG_PREFIX.length),
    sourceSha: input.sourceSha,
  };
}

function option(name: string): string {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  if (!value || value.startsWith('--')) throw new Error(`${name} is required`);
  return value;
}

async function main(): Promise<void> {
  const identity = parseCommunityReleaseIdentity({
    tag: option('--tag'),
    sourceSha: option('--source-sha'),
    refType: option('--ref-type'),
    tagObjectType: option('--tag-object-type'),
  });
  const outputPath = option('--github-output');
  await appendFile(
    outputPath,
    `tag=${identity.tag}\nversion=${identity.version}\nsource_sha=${identity.sourceSha}\n`,
    { encoding: 'utf8' },
  );
}

if (import.meta.main) {
  await main();
}
