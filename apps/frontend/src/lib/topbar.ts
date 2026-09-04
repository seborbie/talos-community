import { writable } from 'svelte/store';

export type TopbarAction = {
  label: string;
  disabled?: boolean;
  run: () => void | Promise<void>;
};

export type TopbarConfig = {
  title: string;
  action?: TopbarAction;
} | null;

export const topbarConfig = writable<TopbarConfig>(null);
