// Bun may evaluate isolated test files in any order. Establish the required API configuration
// before a test imports modules whose production startup guard validates these values.
process.env.JWT_SECRET = "talos-api-test-only-secret";
process.env.TOKEN_TTL = "1h";
process.env.MACHINE_TOKEN_TTL = "30d";
// Bun loads apps/.env automatically. Tests must not inherit operator credentials or public example
// markers from that ignored file, so establish deterministic test-only trust-boundary values.
process.env.APP_ENCRYPTION_KEY = "talos-api-test-only-encryption-key";
process.env.SERVICE_KEY = "talos-api-test-only-service-key";
process.env.API_SERVICE_KEY = "talos-api-test-only-api-service-key";
process.env.RMM_SERVER_API_KEY = "test-rmm-key";
process.env.RMM_TELEMETRY_SERVICE_KEY = "test-service-key";
process.env.TALOS_AI_RUNNER_SERVICE_KEY = "talos-api-test-only-ai-runner-key";
process.env.TALOS_AI_RUNNER_RMM_SERVER_KEY = "talos-api-test-only-ai-rmm-key";
process.env.RMM_AGENT_TOKEN = "talos-api-test-only-agent-token";
process.env.API_TRUSTED_PROXIES = "";
process.env.CORS_ALLOWED_ORIGINS = "http://localhost:3000,http://127.0.0.1:3000";
delete process.env.FRONTEND_URL;
delete process.env.OPENAI_API_KEY;
