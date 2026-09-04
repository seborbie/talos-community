/**
 * Logger that formats output to match Rust tracing_subscriber so api_backend
 * logs look consistent when run alongside talos_server/talos_worker in one CLI.
 * Format: TIMESTAMP  LEVEL target: message [key=value ...]
 * Uses same ANSI colors as tracing (level + target in green/yellow/red).
 * Supports forced color in piped/multiplexed CLIs via FORCE_COLOR/CLICOLOR_FORCE.
 */

const DEFAULT_TARGET = 'api_backend';

const ANSI = {
  reset: '\x1b[0m',
  gray: '\x1b[90m',
  green: '\x1b[32m',
  blue: '\x1b[34m',
  yellow: '\x1b[33m',
  red: '\x1b[31m',
} as const;

function forceColorEnabled(): boolean {
  const force = process.env.FORCE_COLOR ?? process.env.CLICOLOR_FORCE;
  if (!force) return false;
  const normalized = force.trim().toLowerCase();
  return normalized !== '0' && normalized !== 'false' && normalized !== 'no';
}

function colorDisabled(): boolean {
  return typeof process.env.NO_COLOR === 'string';
}

function useColor(stream: NodeJS.WriteStream): boolean {
  if (colorDisabled()) return false;
  if (forceColorEnabled()) return true;
  return stream.isTTY === true;
}

function isoNow(): string {
  return new Date().toISOString();
}

function colorizeLevel(level: string, stream: NodeJS.WriteStream): string {
  if (!useColor(stream)) return level.padEnd(5);
  const padded = level.padEnd(5);
  switch (level) {
    case 'DEBUG':
      return `${ANSI.blue}${padded}${ANSI.reset}`;
    case 'INFO':
      return `${ANSI.green}${padded}${ANSI.reset}`;
    case 'WARN':
      return `${ANSI.yellow}${padded}${ANSI.reset}`;
    case 'ERROR':
      return `${ANSI.red}${padded}${ANSI.reset}`;
    default:
      return padded;
  }
}

function colorizeTarget(target: string, stream: NodeJS.WriteStream): string {
  if (!useColor(stream)) return target;
  return `${ANSI.gray}${target}${ANSI.reset}`;
}

function colorizeTimestamp(timestamp: string, stream: NodeJS.WriteStream): string {
  if (!useColor(stream)) return timestamp;
  return `${ANSI.gray}${timestamp}${ANSI.reset}`;
}

function formatMessage(
  level: string,
  target: string,
  message: string,
  fields?: Record<string, unknown>,
  stream?: NodeJS.WriteStream
): string {
  const timestamp = isoNow();
  const streamOut = stream ?? process.stdout;
  const timestampStr = colorizeTimestamp(timestamp, streamOut);
  const levelStr = colorizeLevel(level, streamOut);
  const targetStr = colorizeTarget(target, streamOut);
  let line = `${timestampStr}  ${levelStr} ${targetStr}: ${message}`;
  if (fields && Object.keys(fields).length > 0) {
    const parts = Object.entries(fields).map(([k, v]) => {
      const val = v instanceof Error ? v.message : String(v);
      return `${k}=${val}`;
    });
    line += ' ' + parts.join(' ');
  }
  return line;
}

function write(level: string, target: string, message: string, fields?: Record<string, unknown>, isError = false): void {
  const stream = isError ? process.stderr : process.stdout;
  const line = formatMessage(level, target, message, fields, stream);
  stream.write(line + '\n');
}

export function createLogger(target: string = DEFAULT_TARGET) {
  return {
    debug(message: string, fields?: Record<string, unknown>): void {
      write('DEBUG', target, message, fields, false);
    },
    info(message: string, fields?: Record<string, unknown>): void {
      write('INFO', target, message, fields, false);
    },
    warn(message: string, fields?: Record<string, unknown>): void {
      write('WARN', target, message, fields, false);
    },
    error(message: string, fields?: Record<string, unknown>): void {
      write('ERROR', target, message, fields, true);
    },
  };
}

const defaultLog = createLogger(DEFAULT_TARGET);

export const log = defaultLog;
export default defaultLog;
