import { describe, expect, test } from 'bun:test';
import {
  assertLocalDockerEngineResponsive,
  parseEnvironmentFile,
  type DockerProbeSpawner,
} from './docker-local-env';

describe('Docker environment file parsing', () => {
  test('parses the Compose interpolation values used by apps/.env', () => {
    expect(
      parseEnvironmentFile(`
# comment
RMM_RELAY_URL=localhost:17443
RMM_RELAY_TLS_CERT_HOST_PATH="../private relay/chain.pem"
export RMM_RELAY_TLS_CERT_PATH='/.certs/chain.pem'
EMPTY=
WITH_COMMENT=value # explanation
INVALID KEY=ignored
`),
    ).toEqual({
      RMM_RELAY_URL: 'localhost:17443',
      RMM_RELAY_TLS_CERT_HOST_PATH: '../private relay/chain.pem',
      RMM_RELAY_TLS_CERT_PATH: '/.certs/chain.pem',
      EMPTY: '',
      WITH_COMMENT: 'value',
    });
  });

  test('accepts a responsive Docker engine', async () => {
    let killed = false;
    const spawn: DockerProbeSpawner = () => ({
      exited: Promise.resolve(0),
      kill: () => {
        killed = true;
      },
    });

    await expect(assertLocalDockerEngineResponsive(50, spawn)).resolves.toBeUndefined();
    expect(killed).toBeFalse();
  });

  test('rejects a failed Docker engine probe', async () => {
    const spawn: DockerProbeSpawner = () => ({
      exited: Promise.resolve(1),
      kill: () => {},
    });

    await expect(assertLocalDockerEngineResponsive(50, spawn)).rejects.toThrow(
      'Docker engine is unavailable (docker info exited with code 1)',
    );
  });

  test('bounds an unresponsive Docker engine probe and terminates its CLI process', async () => {
    let killed = false;
    const spawn: DockerProbeSpawner = () => ({
      exited: new Promise(() => {}),
      kill: () => {
        killed = true;
      },
    });

    await expect(assertLocalDockerEngineResponsive(1, spawn)).rejects.toThrow(
      'Docker engine did not respond within 1 seconds',
    );
    expect(killed).toBeTrue();
  });
});
