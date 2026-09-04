import { describe, expect, test } from 'bun:test';
import {
  createDatabasePreflightPlan,
  validateProductionDatabaseUrl,
} from './productionDatabasePreflight';

describe('production database preflight', () => {
  test('accepts a bounded TLS external PostgreSQL connection', () => {
    const parsed = validateProductionDatabaseUrl(
      'postgresql://talos:secret@db.example.test/talos?sslmode=verify-full&connect_timeout=5',
      'external',
    );

    expect(parsed.hostname).toBe('db.example.test');
    expect(parsed.searchParams.get('sslmode')).toBe('verify-full');
  });

  test('rejects external connections without enforced TLS or a bounded timeout', () => {
    expect(() =>
      validateProductionDatabaseUrl(
        'postgresql://talos:secret@db.example.test/talos?connect_timeout=5',
        'external',
      ),
    ).toThrow('sslmode=require, verify-ca, or verify-full');
    expect(() =>
      validateProductionDatabaseUrl(
        'postgresql://talos:secret@db.example.test/talos?sslmode=prefer&connect_timeout=5',
        'external',
      ),
    ).toThrow('sslmode=require, verify-ca, or verify-full');
    expect(() =>
      validateProductionDatabaseUrl(
        'postgresql://talos:secret@db.example.test/talos?sslmode=verify-full&connect_timeout=0',
        'external',
      ),
    ).toThrow('connect_timeout must be between 1 and 30 seconds');
  });

  test('keeps sensitive connection values out of validation errors', () => {
    const secret = 'do-not-print-this-password';
    let message = '';

    try {
      validateProductionDatabaseUrl(
        `postgresql://talos:${secret}@db.example.test/talos?sslmode=disable&connect_timeout=5`,
        'external',
      );
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }

    expect(message).not.toContain(secret);
    expect(message).not.toContain('db.example.test');
  });

  test('allows the private bundled database but no other bundled host', () => {
    expect(() =>
      validateProductionDatabaseUrl(
        'postgresql://talos:secret@postgres:5432/talos?connect_timeout=5',
        'bundled',
      ),
    ).not.toThrow();
    expect(() =>
      validateProductionDatabaseUrl(
        'postgresql://talos:secret@elsewhere:5432/talos?connect_timeout=5',
        'bundled',
      ),
    ).toThrow('private postgres service');
  });

  test('runs a non-destructive query through Prisma without putting the URL in argv', () => {
    const databaseUrl =
      'postgresql://talos:secret@db.example.test/talos?sslmode=require&connect_timeout=5';
    const plan = createDatabasePreflightPlan(
      {
        DATABASE_URL: databaseUrl,
        TALOS_DATABASE_MODE: 'external',
      },
      '/workspace/apps/api_backend',
    );

    expect(plan.argv).toEqual([
      'bun',
      'x',
      '--bun',
      'prisma',
      'db',
      'execute',
      '--stdin',
      '--schema',
      './prisma/schema.prisma',
    ]);
    expect(plan.argv.join(' ')).not.toContain(databaseUrl);
    expect(plan.stdin).toBe('SELECT 1;\n');
  });
});
