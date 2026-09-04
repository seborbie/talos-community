ALTER TABLE public.rmm_patch_policy
  ADD COLUMN IF NOT EXISTS target_os_family TEXT NOT NULL DEFAULT 'all';

UPDATE public.rmm_patch_policy
SET target_os_family = 'all'
WHERE target_os_family IS NULL
   OR target_os_family NOT IN ('all', 'windows', 'linux', 'macos');

ALTER TABLE public.rmm_patch_policy
  DROP CONSTRAINT IF EXISTS rmm_patch_policy_target_os_family_check;

ALTER TABLE public.rmm_patch_policy
  ADD CONSTRAINT rmm_patch_policy_target_os_family_check
  CHECK (target_os_family IN ('all', 'windows', 'linux', 'macos'));
