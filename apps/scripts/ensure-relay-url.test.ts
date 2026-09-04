import { describe, expect, test } from 'bun:test';
import { DEV_RELAY_URL, ensureRelayUrlContents } from './ensure-relay-url';

describe('relay URL setup', () => {
  test('preserves an explicit operator endpoint', () => {
    expect(ensureRelayUrlContents('RMM_RELAY_URL=relay.community.example\n')).toEqual({
      contents: 'RMM_RELAY_URL=relay.community.example\n',
      changed: false,
      configuredUrl: 'relay.community.example',
    });
  });

  test('fills an empty setting with the local Community default', () => {
    expect(ensureRelayUrlContents('RMM_RELAY_URL=\n')).toEqual({
      contents: `RMM_RELAY_URL=${DEV_RELAY_URL}\n`,
      changed: true,
      configuredUrl: DEV_RELAY_URL,
    });
  });

  test('adds a missing setting without rewriting other values', () => {
    expect(ensureRelayUrlContents('JWT_SECRET=keep-me\n')).toEqual({
      contents: `JWT_SECRET=keep-me\n\nRMM_RELAY_URL=${DEV_RELAY_URL}`,
      changed: true,
      configuredUrl: DEV_RELAY_URL,
    });
  });
});
