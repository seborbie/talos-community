UPDATE public.rmm_patch_update_catalog
SET category = 'uwp_app',
    updated_at = NOW()
WHERE category = 'other'
  AND (
    lower(title) LIKE '%uwp%'
    OR lower(title) LIKE '%appx%'
    OR lower(title) LIKE '%msix%'
    OR lower(title) LIKE '%microsoft.windowsappruntime%'
    OR lower(title) LIKE '%microsoft.windows.devhome%'
    OR lower(title) LIKE '%microsoft.vclibs%'
    OR lower(title) LIKE '%microsoftwindows.crossdevice%'
    OR lower(update_key) ~ '^[a-z0-9]{12}-microsoft\.'
  );

UPDATE public.rmm_patch_device_update_state
SET category = 'uwp_app',
    updated_at = NOW()
WHERE category = 'other'
  AND (
    lower(title) LIKE '%uwp%'
    OR lower(title) LIKE '%appx%'
    OR lower(title) LIKE '%msix%'
    OR lower(title) LIKE '%microsoft.windowsappruntime%'
    OR lower(title) LIKE '%microsoft.windows.devhome%'
    OR lower(title) LIKE '%microsoft.vclibs%'
    OR lower(title) LIKE '%microsoftwindows.crossdevice%'
    OR lower(update_key) ~ '^[a-z0-9]{12}-microsoft\.'
  );

UPDATE public.rmm_patch_policy
SET policy_config_jsonb = jsonb_set(
  jsonb_set(
    COALESCE(policy_config_jsonb, '{}'::jsonb),
    '{categories}',
    CASE
      WHEN jsonb_typeof(policy_config_jsonb->'categories') = 'object' THEN policy_config_jsonb->'categories'
      ELSE '{}'::jsonb
    END,
    true
  ),
  '{categories,uwp_app}',
  jsonb_build_object(
    'approval', 'manual',
    'installAfterDays', GREATEST(COALESCE(deferral_days, 0), 0),
    'forceInstallByDays', NULL,
    'forceRebootByDays', NULL
  ),
  true
)
WHERE policy_config_jsonb #> '{categories,uwp_app}' IS NULL;
