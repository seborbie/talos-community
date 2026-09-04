import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  configuredInstallerBootstrapUrl,
  installersRouter,
  requireUnsignedScopedInstallerOptIn,
  stampMacosPackagePostinstall,
  unsignedScopedInstallersEnabled
} from '../routes/installers.routes';

describe('Windows scoped installer safety', () => {
  test('does not advertise a phantom bootstrap executable', () => {
    expect(configuredInstallerBootstrapUrl(undefined)).toBeNull();
    expect(configuredInstallerBootstrapUrl('   ')).toBeNull();
    expect(configuredInstallerBootstrapUrl('https://downloads.example.test/talos-agent.exe')).toBe(
      'https://downloads.example.test/talos-agent.exe'
    );
    expect(
      readFileSync(resolve(import.meta.dir, '..', 'routes', 'installers.routes.ts'), 'utf8')
    ).not.toContain('/downloads/rmm-agent-bootstrap.exe');
  });

  test('requires an explicit opt-in before runtime assembly of an unsigned EXE', () => {
    expect(unsignedScopedInstallersEnabled(undefined)).toBe(false);
    expect(unsignedScopedInstallersEnabled('false')).toBe(false);
    expect(unsignedScopedInstallersEnabled('1')).toBe(false);
    expect(unsignedScopedInstallersEnabled(' TRUE ')).toBe(true);
  });

  test('fails closed before the scoped EXE handler can issue a token or read artifacts', () => {
    let statusCode = 0;
    let responseBody: unknown = null;
    let nextCalled = false;
    const response = {
      status(code: number) {
        statusCode = code;
        return this;
      },
      json(body: unknown) {
        responseBody = body;
        return this;
      }
    };

    requireUnsignedScopedInstallerOptIn(
      {} as never,
      response as never,
      (() => {
        nextCalled = true;
      }) as never
    );

    expect(statusCode).toBe(503);
    expect(responseBody).toEqual({
      error:
        'Runtime-assembled scoped EXEs are disabled because modifying an executable invalidates its Authenticode signature',
      code: 'UNSIGNED_SCOPED_INSTALLERS_DISABLED'
    });
    expect(nextCalled).toBe(false);

    const routeLayer = (installersRouter as any).stack.find(
      (layer: any) => layer.route?.path === '/profiles/:id/download-exe'
    );
    expect(routeLayer).toBeDefined();
    expect(routeLayer.route.stack[1]?.handle).toBe(requireUnsignedScopedInstallerOptIn);
  });
});

describe('macOS installer package stamping', () => {
  test('stamps env blocks when postinstall logs before launchctl reload', () => {
    const script = `#!/bin/sh
set -eu

AGENT_ENV_PATH="$ENV_DIR/rmm-agent.env"
SUPERVISOR_ENV_PATH="$ENV_DIR/talos-supervisor.env"
STATE_DIR="/Library/Application Support/Talos"

if [ ! -f "$AGENT_ENV_PATH" ]; then
  cat > "$AGENT_ENV_PATH" <<'EOF_AGENT_ENV'
RMM_SERVER_URL='wss://old.example/agent/ws'
RMM_AGENT_TOKEN='replace-with-enrollment-token'
RMM_AGENT_ID_PATH='/Library/Application Support/Talos/talos_worker_id.txt'
EOF_AGENT_ENV
  chmod 0600 "$AGENT_ENV_PATH"
fi

if [ ! -f "$SUPERVISOR_ENV_PATH" ]; then
  cat > "$SUPERVISOR_ENV_PATH" <<'EOF_SUPERVISOR_ENV'
RMM_UPDATE_BASE_URL='https://old.example/rmm/updates'
RMM_UPDATE_CHANNEL=stable
EOF_SUPERVISOR_ENV
  chmod 0600 "$SUPERVISOR_ENV_PATH"
fi

log_postinstall "reloading Talos LaunchDaemons"
launchctl bootout "system/$WORKER_SERVICE_LABEL" >/dev/null 2>&1 || true
`;

    const stamped = stampMacosPackagePostinstall(script, {
      token: 'token-123',
      serverUrl: 'https://api.example.test',
      updateBaseUrl: 'https://api.example.test/rmm/updates'
    });

    expect(stamped).toContain("RMM_SERVER_URL='wss://api.example.test/agent/ws'");
    expect(stamped).toContain("RMM_AGENT_TOKEN='token-123'");
    expect(stamped).toContain("RMM_UPDATE_BASE_URL='https://api.example.test/rmm/updates'");
    expect(stamped).toContain('log_postinstall "reloading Talos LaunchDaemons"');
    expect(stamped).not.toContain('replace-with-enrollment-token');
    expect(stamped).not.toContain('old.example');
  });
});
