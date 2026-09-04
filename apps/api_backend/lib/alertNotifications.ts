export type AlertNotificationChannel = 'email' | 'webhook' | 'psa';

export type AlertNotificationAdapterResult = {
  channel: AlertNotificationChannel;
  adapter: string;
  status: 'stubbed' | 'skipped';
  detail: string;
  externalRef: string | null;
};

const SUPPORTED_CHANNELS = new Set<AlertNotificationChannel>(['email', 'webhook', 'psa']);

export function normalizeNotificationChannels(value: unknown): AlertNotificationChannel[] {
  if (!Array.isArray(value)) return [];
  const channels: AlertNotificationChannel[] = [];
  for (const item of value) {
    if (typeof item !== 'string') continue;
    const normalized = item.trim().toLowerCase() as AlertNotificationChannel;
    if (SUPPORTED_CHANNELS.has(normalized) && !channels.includes(normalized)) {
      channels.push(normalized);
    }
  }
  return channels;
}

export async function dispatchAlertNotifications(options: {
  alertId: string;
  channels: unknown;
}): Promise<AlertNotificationAdapterResult[]> {
  const channels = normalizeNotificationChannels(options.channels);
  return channels.map((channel) => ({
    channel,
    adapter: channel === 'psa' ? 'psa-placeholder' : `${channel}-placeholder`,
    status: 'stubbed',
    detail: `${channel} notification adapter is stubbed; no credentials are required`,
    externalRef: `${channel}-stub:${options.alertId}`
  }));
}
