ALTER TABLE public.rmm_devices
  ADD COLUMN IF NOT EXISTS macos_update_account_status TEXT,
  ADD COLUMN IF NOT EXISTS macos_update_account_required BOOLEAN,
  ADD COLUMN IF NOT EXISTS macos_update_account_username TEXT,
  ADD COLUMN IF NOT EXISTS macos_update_account_credential_version INTEGER,
  ADD COLUMN IF NOT EXISTS macos_update_account_generated_uid TEXT,
  ADD COLUMN IF NOT EXISTS macos_update_account_failure_code TEXT,
  ADD COLUMN IF NOT EXISTS macos_update_account_failure_message TEXT,
  ADD COLUMN IF NOT EXISTS macos_update_account_last_verified_at TIMESTAMPTZ(3),
  ADD COLUMN IF NOT EXISTS macos_update_account_status_jsonb JSONB;
