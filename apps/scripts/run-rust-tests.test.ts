import { describe, expect, test } from 'bun:test';
import { rustTestEnvironment } from './run-rust-tests';

describe('rustTestEnvironment', () => {
  test('adds the macOS Swift runtime runner for Apple silicon', () => {
    const environment = rustTestEnvironment('darwin', 'arm64', { PATH: '/bin' });

    expect(environment.CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER).toBe(
      'env DYLD_LIBRARY_PATH=/usr/lib/swift',
    );
  });

  test('adds the macOS Swift runtime runner for Intel', () => {
    const environment = rustTestEnvironment('darwin', 'x64', {});

    expect(environment.CARGO_TARGET_X86_64_APPLE_DARWIN_RUNNER).toBe(
      'env DYLD_LIBRARY_PATH=/usr/lib/swift',
    );
  });

  test('preserves an explicitly configured Cargo runner', () => {
    const environment = rustTestEnvironment('darwin', 'arm64', {
      CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER: '/opt/talos/custom-runner',
    });

    expect(environment.CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER).toBe('/opt/talos/custom-runner');
  });

  test('does not add Apple runners on other platforms', () => {
    const environment = rustTestEnvironment('linux', 'arm64', { PATH: '/bin' });

    expect(environment).toEqual({ PATH: '/bin' });
  });
});
