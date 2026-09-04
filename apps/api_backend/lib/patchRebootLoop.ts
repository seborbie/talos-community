export interface PostPatchRebootLoopInput {
  previousBootSessionId?: string | null;
  currentBootSessionId?: string | null;
  previousRebootRequired?: boolean | null;
  currentRebootRequired?: boolean | null;
  hadPatchRebootIntent: boolean;
  pendingRebootUpdateKeys: string[];
  previousRebootUpdateKeys: string[];
}

function normalizeKeySet(values: string[]): Set<string> {
  return new Set(values.map((value) => value.trim()).filter(Boolean));
}

export function selectPostPatchRebootLoopFailureKeys(input: PostPatchRebootLoopInput): string[] {
  const previousBoot = input.previousBootSessionId?.trim();
  const currentBoot = input.currentBootSessionId?.trim();
  if (!previousBoot || !currentBoot || previousBoot === currentBoot) return [];
  if (input.currentRebootRequired !== true) return [];
  if (!input.hadPatchRebootIntent) return [];

  const previousKeys = normalizeKeySet(input.previousRebootUpdateKeys);
  return [...normalizeKeySet(input.pendingRebootUpdateKeys)].filter((updateKey) => previousKeys.has(updateKey));
}

export function shouldClearRebootForFailedPendingUpdates(
  pendingRebootUpdateKeys: string[],
  failedPostRebootUpdateKeys: string[]
): boolean {
  const pendingKeys = normalizeKeySet(pendingRebootUpdateKeys);
  if (pendingKeys.size === 0) return false;
  const failedKeys = normalizeKeySet(failedPostRebootUpdateKeys);
  return [...pendingKeys].every((updateKey) => failedKeys.has(updateKey));
}
