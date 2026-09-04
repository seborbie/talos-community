ALTER TABLE "public"."rmm_patch_action"
  DROP CONSTRAINT IF EXISTS "rmm_patch_action_type_check";

ALTER TABLE "public"."rmm_patch_action"
  ADD CONSTRAINT "rmm_patch_action_type_check"
  CHECK (
    "action_type" IN (
      'scan',
      'approval',
      'install',
      'force_scan',
      'force_download',
      'force_install',
      'force_reboot',
      'defer',
      'defer_reboot',
      'block',
      'approve',
      'emergency_approve',
      'exclude_device',
      'maintenance_mode',
      'break_glass',
      'cancel'
    )
  );
