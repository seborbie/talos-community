import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const repoRoot = resolve(import.meta.dir, '..', '..');
const setup = readFileSync(resolve(repoRoot, 'scripts', 'Setup-DevEnviroment.ps1'), 'utf8');
const helper = readFileSync(resolve(repoRoot, 'scripts', 'DevEnvironmentSecrets.ps1'), 'utf8');

describe('development credential generation contract', () => {
  test('Windows setup no longer writes repository-wide fixed credentials', () => {
    expect(setup).not.toContain('talos_dev_local_service_key_01');
    expect(setup).not.toContain('talos_dev_jwt_secret_minimum_32_chars_long_');
    expect(setup).not.toContain('talos_dev_agent_token_01');
  });

  test('uses independent cryptographic secrets for each installation boundary', () => {
    expect(helper).toContain('[Security.Cryptography.RandomNumberGenerator]::Create()');
    for (const purpose of [
      'jwt',
      'app_encryption',
      'rmm_server',
      'service',
      'telemetry',
      'ai_runner',
      'agent',
    ]) {
      expect(setup).toContain(`New-TalosRandomSecret -Purpose "${purpose}"`);
    }
    expect(setup).toContain('if ($line -match "^\\s*APP_ENCRYPTION_KEY=")');
    expect(setup).toContain('if ($line -match "^\\s*TALOS_AI_RUNNER_SERVICE_KEY=")');
  });
});
