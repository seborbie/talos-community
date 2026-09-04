import { describe, expect, test } from 'bun:test';
import {
  NATIVE_DEBUG_REQUIRED_ENVIRONMENT_VARIABLES,
  NATIVE_DEBUG_REQUIRED_CORS_ORIGINS,
  assertNativeDebugEnvironment,
  mergeDebugEnvironment,
  resolveNativeDebugCredentials,
  resolveNativeDebugCorsAllowedOrigins,
  resolveDebugJwtSecret,
  resolveDebugRelayUrl,
  serializeNativeDebugSecrets,
  withNativeDebugProcessEnvironment,
  withNativeRelayCertificates,
} from './debug-environment';

describe('native debug environment', () => {
  const completeCredentialEnvironment = {
    JWT_SECRET: 'generated-debug-jwt',
    APP_ENCRYPTION_KEY: 'generated-debug-encryption-key',
    TOKEN_TTL: '1h',
    MACHINE_TOKEN_TTL: '30d',
    SERVICE_KEY: 'generated-debug-api-service-key',
    RMM_SERVER_API_KEY: 'generated-debug-rmm-server-key',
    RMM_TELEMETRY_SERVICE_KEY: 'generated-debug-telemetry-key',
    TALOS_AI_RUNNER_SERVICE_KEY: 'generated-debug-ai-runner-key',
  };

  test('generates a fresh high-entropy JWT secret instead of a public fallback', () => {
    const first = resolveDebugJwtSecret({});
    const second = resolveDebugJwtSecret({});

    expect(first).toStartWith('talos_debug_jwt_');
    expect(first.length).toBeGreaterThanOrEqual(58);
    expect(second).not.toBe(first);
    expect(resolveDebugJwtSecret({ JWT_SECRET: ' configured-secret ' })).toBe('configured-secret');
  });

  test('gives an explicit shell value precedence over env files', () => {
    expect(
      mergeDebugEnvironment(
        { SOURCE: 'apps', APPS_ONLY: 'yes' },
        { SOURCE: 'api', API_ONLY: 'yes' },
        { SOURCE: 'shell', SHELL_ONLY: 'yes' },
      ),
    ).toEqual({
      SOURCE: 'shell',
      APPS_ONLY: 'yes',
      API_ONLY: 'yes',
      SHELL_ONLY: 'yes',
    });
  });

  test('preserves configured CORS origins and admits the reviewed HTTPS dev frontend', () => {
    expect(NATIVE_DEBUG_REQUIRED_CORS_ORIGINS).toEqual([
      'http://localhost:3000',
      'http://127.0.0.1:3000',
      'https://dev.talos.cloud',
    ]);
    expect(
      resolveNativeDebugCorsAllowedOrigins({
        CORS_ALLOWED_ORIGINS:
          'https://operator.example, https://dev.talos.cloud, http://localhost:3000',
      }).split(','),
    ).toEqual([
      'https://operator.example',
      'https://dev.talos.cloud',
      'http://localhost:3000',
      'http://127.0.0.1:3000',
    ]);
  });

  test('maps the Compose certificate selection to native host files and terminates TLS', () => {
    const result = withNativeRelayCertificates(
      {
        RMM_RELAY_TLS_TERMINATED: 'true',
        RMM_RELAY_TLS_CERT_PATH: '/.certs/container-chain.pem',
        RMM_RELAY_TLS_KEY_PATH: '/.certs/container-key.pem',
      },
      {
        certificateFile: '/repo/apps/certs/host-chain.pem',
        keyFile: '/repo/apps/certs/host-key.pem',
      },
    );

    expect(result.RMM_RELAY_TLS_TERMINATED).toBe('false');
    expect(result.RMM_RELAY_TLS_CERT_PATH).toBe('/repo/apps/certs/host-chain.pem');
    expect(result.RMM_RELAY_TLS_KEY_PATH).toBe('/repo/apps/certs/host-key.pem');
  });

  test('uses an IPv4 Redpanda advertisement and one color-control convention', () => {
    expect(
      withNativeDebugProcessEnvironment({
        NO_COLOR: '1',
        TALOS_REDPANDA_ADVERTISED_HOST: 'localhost',
      }),
    ).toEqual({
      FORCE_COLOR: '1',
      TALOS_REDPANDA_ADVERTISED_HOST: '127.0.0.1',
    });
  });

  test('uses the fresh local relay URL when the env file value is absent or empty', () => {
    expect(resolveDebugRelayUrl({}, 'localhost:17443')).toBe('localhost:17443');
    expect(resolveDebugRelayUrl({ RMM_RELAY_URL: '   ' }, 'localhost:17443')).toBe(
      'localhost:17443',
    );
    expect(
      resolveDebugRelayUrl({ RMM_RELAY_URL: ' relay.community.example:443 ' }, 'localhost:17443'),
    ).toBe('relay.community.example:443');
  });

  test('fails preflight unless every started service has its credential', () => {
    expect(NATIVE_DEBUG_REQUIRED_ENVIRONMENT_VARIABLES).toEqual([
      'JWT_SECRET',
      'APP_ENCRYPTION_KEY',
      'TOKEN_TTL',
      'MACHINE_TOKEN_TTL',
      'SERVICE_KEY',
      'RMM_SERVER_API_KEY',
      'RMM_TELEMETRY_SERVICE_KEY',
      'TALOS_AI_RUNNER_SERVICE_KEY',
    ]);
    expect(() => assertNativeDebugEnvironment(completeCredentialEnvironment)).not.toThrow();

    for (const credential of [
      'APP_ENCRYPTION_KEY',
      'SERVICE_KEY',
      'RMM_SERVER_API_KEY',
      'RMM_TELEMETRY_SERVICE_KEY',
      'TALOS_AI_RUNNER_SERVICE_KEY',
    ] as const) {
      expect(() =>
        assertNativeDebugEnvironment({
          ...completeCredentialEnvironment,
          [credential]: '',
        }),
      ).toThrow(credential);
    }
  });

  test('rejects published credential markers before starting infrastructure', () => {
    expect(() =>
      assertNativeDebugEnvironment({
        ...completeCredentialEnvironment,
        RMM_SERVER_API_KEY: 'replace_with_shared_rmm_server_key',
      }),
    ).toThrow('public example credentials are still configured: RMM_SERVER_API_KEY');
  });

  test('repairs the screenshot case with stable local secrets without weakening preflight', () => {
    const resolution = resolveNativeDebugCredentials(
      {
        ...completeCredentialEnvironment,
        APP_ENCRYPTION_KEY: '',
        RMM_AGENT_TOKEN: 'replace_with_shared_agent_token',
      },
      {},
    );

    expect(resolution.repairedVariables).toEqual(['APP_ENCRYPTION_KEY', 'RMM_AGENT_TOKEN']);
    expect(resolution.environment.APP_ENCRYPTION_KEY).toStartWith(
      'talos_debug_app_encryption_key_',
    );
    expect(resolution.environment.RMM_AGENT_TOKEN).toStartWith('talos_debug_rmm_agent_token_');
    expect(resolution.environment.APP_ENCRYPTION_KEY).not.toBe(resolution.environment.JWT_SECRET);
    expect(() => assertNativeDebugEnvironment(resolution.environment)).not.toThrow();
  });

  test('reuses generated debug credentials across restarts and preserves valid operator values', () => {
    const configured = {
      TOKEN_TTL: '1h',
      MACHINE_TOKEN_TTL: '30d',
      SERVICE_KEY: 'operator-service-key',
    };
    const first = resolveNativeDebugCredentials(configured, {});
    const second = resolveNativeDebugCredentials(configured, first.persistedSecrets);

    for (const variable of [
      'JWT_SECRET',
      'APP_ENCRYPTION_KEY',
      'RMM_SERVER_API_KEY',
      'RMM_TELEMETRY_SERVICE_KEY',
      'TALOS_AI_RUNNER_SERVICE_KEY',
    ]) {
      expect(second.environment[variable]).toBe(first.environment[variable]);
    }
    expect(second.environment.SERVICE_KEY).toBe('operator-service-key');
    expect(second.persistedSecrets.SERVICE_KEY).toBeUndefined();
    expect(() => assertNativeDebugEnvironment(second.environment)).not.toThrow();
  });

  test('removes unusable optional provider markers and serializes only local secret assignments', () => {
    const resolution = resolveNativeDebugCredentials(
      {
        ...completeCredentialEnvironment,
        OPENAI_API_KEY: 'replace_with_openai_api_key',
        API_SERVICE_KEY: 'replace_with_shared_service_key',
        TALOS_AI_RUNNER_RMM_SERVER_KEY: 'replace_with_shared_rmm_server_key',
      },
      {},
    );

    expect(resolution.environment.OPENAI_API_KEY).toBeUndefined();
    expect(resolution.environment.API_SERVICE_KEY).toBe(completeCredentialEnvironment.SERVICE_KEY);
    expect(resolution.environment.TALOS_AI_RUNNER_RMM_SERVER_KEY).toBe(
      completeCredentialEnvironment.RMM_SERVER_API_KEY,
    );
    const serialized = serializeNativeDebugSecrets({
      APP_ENCRYPTION_KEY: 'persisted-debug-encryption-key',
    });
    expect(serialized).toContain('APP_ENCRYPTION_KEY=persisted-debug-encryption-key');
    expect(serialized).not.toContain('OPENAI_API_KEY');
  });
});
