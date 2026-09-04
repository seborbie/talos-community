import { describe, expect, test } from 'bun:test';
import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const repoRoot = resolve(import.meta.dir, '../..');

function read(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), 'utf8');
}

function readSourceTree(relativeDirectory: string, extensions: readonly string[]): string {
  const root = resolve(repoRoot, relativeDirectory);
  const files: string[] = [];
  const visit = (directory: string) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) {
        visit(path);
      } else if (extensions.some((extension) => entry.name.endsWith(extension))) {
        files.push(path);
      }
    }
  };
  visit(root);
  return files
    .sort()
    .map((path) => readFileSync(path, 'utf8'))
    .join('\n');
}

describe('Community runtime network defaults', () => {
  test('keeps host-native service defaults on loopback and container binds explicit', () => {
    const server = read('apps/talos_server/src/config.rs');
    const relay = read('apps/talos_relay/src/main.rs');
    const producer = read('apps/talos_telemetry_producer/src/main.rs');
    const aiRunner = read('apps/talos_ai_runner/src/main.rs');
    const compose = read('infra/docker-compose.dev.yml');

    expect(server).toContain('SocketAddr::from(([127, 0, 0, 1], DEFAULT_BIND_PORT))');
    expect(relay).toContain('const DEFAULT_RELAY_BIND_ADDR: &str = "127.0.0.1:443"');
    expect(producer).toContain('const DEFAULT_PRODUCER_BIND_ADDR: &str = "127.0.0.1:17120"');
    expect(aiRunner).toContain('const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3010"');

    expect(compose).toContain('RMM_BIND_ADDR: 0.0.0.0:17110');
    expect(compose).toContain('RMM_RELAY_BIND_ADDR: 0.0.0.0:443');
    expect(compose).toContain('RMM_TELEMETRY_PRODUCER_BIND_ADDR: 0.0.0.0:17120');
    expect(compose).toContain('TALOS_AI_RUNNER_BIND_ADDR: 0.0.0.0:3010');
  });

  test('does not compile a third-party STUN endpoint into worker or viewer flows', () => {
    const protocol = read('apps/talos_protocol/src/lib.rs');
    const worker = read('apps/talos_worker/src/main.rs');
    const viewer = read('apps/talos_viewer/src-tauri/src/main.rs');
    const viewerChat = read('apps/talos_viewer/src-tauri/src/viewer_chat.rs');
    const runtimeSources = [
      readSourceTree('apps/talos_protocol/src', ['.rs']),
      readSourceTree('apps/talos_worker/src', ['.rs']),
      readSourceTree('apps/talos_viewer/src-tauri/src', ['.rs']),
    ].join('\n');

    expect(runtimeSources).not.toContain('stun.l.google.com');
    expect(protocol).toContain('pub const RMM_STUN_SERVER_ENV: &str = "RMM_STUN_SERVER"');
    expect(protocol).toContain('pub fn parse_stun_server(');
    expect(worker).toContain('talos_protocol::configured_stun_server()');
    expect(viewer).toContain('fn query_configured_stun_reflex(');
    expect(viewerChat).toContain('crate::query_configured_stun_reflex(stun_socket)');
  });

  test('does not fetch web fonts at runtime', () => {
    const frontendCss = read('apps/frontend/src/app.css');
    const viewerHtml = read('apps/talos_viewer/index.html');
    const viewerCss = read('apps/talos_viewer/src/app.css');
    const webInputs = [
      readSourceTree('apps/frontend/src', ['.css', '.html', '.svelte', '.ts']),
      viewerHtml,
      readSourceTree('apps/talos_viewer/src', ['.css', '.html', '.svelte', '.ts']),
    ].join('\n');

    expect(webInputs).not.toContain('fonts.googleapis.com');
    expect(webInputs).not.toContain('fonts.gstatic.com');
    expect(frontendCss).toContain(
      'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
    );
    expect(viewerCss).toContain('BlinkMacSystemFont');
  });

  test('documents disabled-by-default STUN and the relay fallback', () => {
    const communityDocs = read('docs/community-edition.md');

    expect(communityDocs).toContain('RMM_STUN_SERVER');
    expect(communityDocs).toContain('disabled by default');
    expect(communityDocs).toContain('relay fallback');
  });
});
