import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import type { Server } from 'http';
import type { AddressInfo } from 'net';
import { app } from '../server';

let server: Server;
let baseUrl: string;

beforeAll(async () => {
  await new Promise<void>((resolve) => {
    server = app.listen(0, '127.0.0.1', () => {
      const address = server.address() as AddressInfo;
      baseUrl = `http://127.0.0.1:${address.port}`;
      resolve();
    });
  });
});

afterAll(async () => {
  await new Promise<void>((resolve, reject) => {
    server.close((error) => {
      if (error) {
        reject(error);
        return;
      }
      resolve();
    });
  });
});

async function preflight(path: string, origin: string): Promise<Response> {
  return fetch(`${baseUrl}${path}`, {
    method: 'OPTIONS',
    headers: {
      Origin: origin,
      'Access-Control-Request-Method': 'POST',
      'Access-Control-Request-Headers': 'content-type'
    }
  });
}

describe('AI Assist CORS', () => {
  test('allows packaged Tauri macOS origin for shell assist preflight', async () => {
    const response = await preflight('/rmm/ai/shell-assist', 'tauri://localhost');

    expect(response.status).toBe(204);
    expect(response.headers.get('access-control-allow-origin')).toBe('tauri://localhost');
  });

  test('allows packaged Tauri macOS origin for desktop task preflight', async () => {
    const response = await preflight('/rmm/ai/desktop-task/start', 'tauri://localhost');

    expect(response.status).toBe(204);
    expect(response.headers.get('access-control-allow-origin')).toBe('tauri://localhost');
  });

  test('rejects unknown origins for AI Assist preflight', async () => {
    const response = await preflight('/rmm/ai/shell-assist', 'tauri://evil.example');

    expect(response.ok).toBe(false);
    expect(response.headers.get('access-control-allow-origin')).toBeNull();
  });

  test('does not retain the former hosted development origin as an implicit default', async () => {
    const response = await preflight('/rmm/ai/shell-assist', 'https://hosted-default.example.test');

    expect(response.ok).toBe(false);
    expect(response.headers.get('access-control-allow-origin')).toBeNull();
  });
});
