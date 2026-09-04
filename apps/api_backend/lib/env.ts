import { createLogger } from './logger';
import {
  API_REQUIRED_ENVIRONMENT_VARIABLES,
  assertSecureEnvironment,
  parseApiTrustedProxies,
} from './environmentPolicy';

const log = createLogger('api_backend::env');

function validateEnv() {
  try {
    assertSecureEnvironment(process.env, API_REQUIRED_ENVIRONMENT_VARIABLES, 'API');
    parseApiTrustedProxies(process.env.API_TRUSTED_PROXIES);
  } catch (error) {
    log.error('invalid startup configuration', {
      error: error instanceof Error ? error.message : String(error),
    });
    log.info('create apps/.env with unique credentials and reviewed proxy configuration');
    process.exit(1);
  }
}

// Validate environment variables on module load
validateEnv();

export const env = {
    jwtSecret: process.env.JWT_SECRET as string,
    tokenTtl: process.env.TOKEN_TTL ?? '1h',
    machineTtl: process.env.MACHINE_TOKEN_TTL ?? '30d',
    serviceKey: process.env.SERVICE_KEY,
    appEncryptionKey: process.env.APP_ENCRYPTION_KEY as string,
    rmmServerApiKey: process.env.RMM_SERVER_API_KEY,
    rmmServerUrl: process.env.RMM_SERVER_HTTP_URL || process.env.PUBLIC_RMM_API_URL || process.env.RMM_API_URL,
    aiRunnerUrl: process.env.TALOS_AI_RUNNER_URL,
    aiRunnerServiceKey: process.env.TALOS_AI_RUNNER_SERVICE_KEY || process.env.SERVICE_KEY,
    aiRunnerCallbackBaseUrl: process.env.TALOS_AI_RUNNER_CALLBACK_BASE_URL || process.env.API_BACKEND_URL || process.env.PUBLIC_API_URL,
    telemetryProducerUrl: process.env.RMM_TELEMETRY_PRODUCER_URL,
    featureUpgradeIsoContainer: process.env.FEATURE_UPGRADE_ISO_CONTAINER,
    featureUpgradeIsoPublicBlobEndpoint: process.env.FEATURE_UPGRADE_ISO_PUBLIC_BLOB_ENDPOINT,
    featureUpgradeIsoSasTtlSeconds: process.env.FEATURE_UPGRADE_ISO_SAS_TTL_SECONDS,
    azureStorageConnectionString: process.env.AZURE_STORAGE_CONNECTION_STRING,
    apiTrustedProxies: parseApiTrustedProxies(process.env.API_TRUSTED_PROXIES),
  };
