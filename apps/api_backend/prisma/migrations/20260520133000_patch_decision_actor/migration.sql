ALTER TABLE public.rmm_patch_decision_log
  ADD COLUMN IF NOT EXISTS actor_type TEXT NOT NULL DEFAULT 'system',
  ADD COLUMN IF NOT EXISTS actor_user_id TEXT NULL,
  ADD COLUMN IF NOT EXISTS actor_email TEXT NULL;

CREATE INDEX IF NOT EXISTS rmm_patch_decision_log_org_actor_type_idx
  ON public.rmm_patch_decision_log (organization_id, actor_type);
