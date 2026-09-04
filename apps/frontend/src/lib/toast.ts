import { writable } from 'svelte/store';

export type ToastVariant = 'default' | 'destructive';

export interface Toast {
  id: string;
  title?: string;
  description?: string;
  variant?: ToastVariant;
  duration?: number;
}

const TOAST_LIMIT = 3;
const DEFAULT_DURATION = 4000;

export const toasts = writable<Toast[]>([]);

const createId = () => {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
};

export const dismiss = (id: string) => {
  toasts.update((items) => items.filter((toast) => toast.id !== id));
};

export const toast = (data: Omit<Toast, 'id'>) => {
  const id = createId();
  const entry: Toast = {
    id,
    variant: data.variant ?? 'default',
    duration: data.duration ?? DEFAULT_DURATION,
    ...data,
  };

  toasts.update((items) => [entry, ...items].slice(0, TOAST_LIMIT));

  if (entry.duration && entry.duration > 0) {
    setTimeout(() => dismiss(id), entry.duration);
  }

  return id;
};
