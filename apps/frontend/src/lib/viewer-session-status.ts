import { rmmApi } from '$lib/api';
import type {
  RmmViewerConnectionSummary,
  RmmViewerSessionKind,
  RmmViewerSessionStatus
} from '$lib/types';

const VIEWER_KIND_DISPLAY_ORDER: Record<RmmViewerSessionKind, number> = {
  remote_desktop: 0,
  shell: 1,
  file_transfer: 2,
  remote_registry: 3,
  chat: 4
};

const viewerKindRank = (kind: RmmViewerSessionKind) => VIEWER_KIND_DISPLAY_ORDER[kind] ?? 99;

export function formatViewerSessionKind(kind: RmmViewerSessionKind): string {
  switch (kind) {
    case 'remote_desktop':
      return 'remote desktop';
    case 'shell':
      return 'shell';
    case 'file_transfer':
      return 'file transfer';
    case 'remote_registry':
      return 'remote registry';
    case 'chat':
      return 'chat';
  }
}

/** Stable order for UI lists that poll repeatedly (API order is not guaranteed). */
export function sortViewerConnectionsForDisplay(
  connections: RmmViewerConnectionSummary[]
): RmmViewerConnectionSummary[] {
  return [...connections].sort((a, b) => {
    const userA = (a.userEmail ?? a.userId ?? '').toLowerCase();
    const userB = (b.userEmail ?? b.userId ?? '').toLowerCase();
    if (userA !== userB) return userA.localeCompare(userB);
    const kindDiff = viewerKindRank(a.kind) - viewerKindRank(b.kind);
    if (kindDiff !== 0) return kindDiff;
    return a.sessionId.localeCompare(b.sessionId);
  });
}

export const VIEWER_LAUNCH_TIMEOUT_MS = 5000;
export const VIEWER_LAUNCH_POLL_MS = 1500;
export const VIEWER_LAUNCH_SLOW_POLL_MS = 5000;
export const VIEWER_LAUNCH_RATE_LIMIT_BACKOFF_MS = 10000;
export const VIEWER_CONNECTION_POLL_MS = 5000;
const VIEWER_CONNECTION_FALLBACK_EVERY = 3;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const extractStatusCode = (err: unknown): number | null => {
  if (typeof err !== 'object' || !err || !('statusCode' in err)) {
    return null;
  }
  const statusCode = Number((err as { statusCode?: number }).statusCode);
  return Number.isFinite(statusCode) ? statusCode : null;
};

export type WaitForViewerSessionOptions = {
  agentId?: string;
  timeoutMs?: number;
  pollMs?: number;
  onTimeout?: () => void;
  shouldCancel?: () => boolean;
};

export async function waitForViewerSessionConnected(
  sessionId: string,
  options: WaitForViewerSessionOptions = {}
): Promise<RmmViewerSessionStatus | null> {
  const timeoutMs = options.timeoutMs ?? VIEWER_LAUNCH_TIMEOUT_MS;
  const pollMs = options.pollMs ?? VIEWER_LAUNCH_POLL_MS;
  const startedAt = Date.now();
  let timeoutRaised = false;
  let attempt = 0;
  while (!options.shouldCancel?.()) {
    if (!timeoutRaised && Date.now() - startedAt >= timeoutMs) {
      timeoutRaised = true;
      options.onTimeout?.();
    }

    let status: RmmViewerSessionStatus | null = null;
    let statusCode: number | null = null;
    try {
      status = await rmmApi.getViewerSessionStatus(sessionId);
    } catch (err) {
      statusCode = extractStatusCode(err);
      if (statusCode !== 404 && statusCode !== 429) {
        throw err;
      }
    }
    if (!status) {
      const shouldCheckConnections =
        Boolean(options.agentId) &&
        statusCode !== 429 &&
        (attempt % VIEWER_CONNECTION_FALLBACK_EVERY === 0 || timeoutRaised);

      if (shouldCheckConnections && options.agentId) {
        try {
          const connections = await rmmApi.getViewerConnections(options.agentId);
          const matched = connections.find((connection) => connection.sessionId === sessionId);
          if (matched) {
            return {
              sessionId,
              kind: matched.kind,
              agentId: matched.agentId,
              userId: matched.userId,
              userEmail: matched.userEmail,
              state: 'connected',
              connected: true,
              attached: true,
              connectedAt: matched.connectedAt,
              lastHeartbeatAt: matched.lastHeartbeatAt
            };
          }
        } catch (err) {
          const fallbackStatusCode = extractStatusCode(err);
          if (fallbackStatusCode !== 429) {
            throw err;
          }
        }
      }

      const delay =
        statusCode === 429
          ? VIEWER_LAUNCH_RATE_LIMIT_BACKOFF_MS
          : timeoutRaised
            ? VIEWER_LAUNCH_SLOW_POLL_MS
            : pollMs;
      attempt += 1;
      await sleep(delay);
      continue;
    }
    if (status.connected || status.state === 'connected') {
      return status;
    }
    const delay = timeoutRaised ? VIEWER_LAUNCH_SLOW_POLL_MS : pollMs;
    attempt += 1;
    await sleep(delay);
  }
  return null;
}

export function groupViewerConnectionsByAgent(
  connections: RmmViewerConnectionSummary[]
): Map<string, RmmViewerConnectionSummary[]> {
  const grouped = new Map<string, RmmViewerConnectionSummary[]>();
  for (const connection of connections) {
    const existing = grouped.get(connection.agentId) ?? [];
    existing.push(connection);
    grouped.set(connection.agentId, existing);
  }
  for (const [agentId, list] of grouped) {
    grouped.set(agentId, sortViewerConnectionsForDisplay(list));
  }
  return grouped;
}
