import { Prisma } from '@prisma/client';
import { defaultPatchPolicyConfig } from './patchDecisionEngine';
import { DEFAULT_PATCH_POLICY_PRIORITY } from './patchManagement';
import { prisma } from './prisma';

export const DEFAULT_PATCH_POLICY_SCOPE_KEY = '__talos_default_patch_policy__';
export const DEFAULT_PATCH_POLICY_NAME = 'Default patch policy';

export function defaultPatchPolicyId(organizationId: string) {
  return `patch-default:${organizationId}`;
}

export async function ensureDefaultPatchPolicy(organizationId: string, createdBy = 'system') {
  const policyConfig = defaultPatchPolicyConfig(0);

  await prisma.$executeRaw(Prisma.sql`
    WITH updated AS (
      UPDATE public.rmm_patch_policy
      SET
        scope_type = 'organization',
        scope_key = ${DEFAULT_PATCH_POLICY_SCOPE_KEY},
        target_os_family = 'all',
        customer_id = NULL,
        site_id = NULL,
        agent_id = NULL,
        enabled = true,
        is_default = true,
        priority = ${DEFAULT_PATCH_POLICY_PRIORITY},
        updated_at = CASE
          WHEN public.rmm_patch_policy.scope_type IS DISTINCT FROM 'organization'
            OR public.rmm_patch_policy.scope_key IS DISTINCT FROM ${DEFAULT_PATCH_POLICY_SCOPE_KEY}
            OR public.rmm_patch_policy.target_os_family IS DISTINCT FROM 'all'
            OR public.rmm_patch_policy.customer_id IS NOT NULL
            OR public.rmm_patch_policy.site_id IS NOT NULL
            OR public.rmm_patch_policy.agent_id IS NOT NULL
            OR public.rmm_patch_policy.enabled IS DISTINCT FROM true
            OR public.rmm_patch_policy.is_default IS DISTINCT FROM true
            OR public.rmm_patch_policy.priority IS DISTINCT FROM ${DEFAULT_PATCH_POLICY_PRIORITY}
          THEN NOW()
          ELSE public.rmm_patch_policy.updated_at
        END
      WHERE organization_id = ${organizationId}
        AND is_default = true
      RETURNING id
    )
    INSERT INTO public.rmm_patch_policy
      (
        id, organization_id, scope_type, scope_key, customer_id, site_id, agent_id,
        name, target_os_family, approval_mode, maintenance_window_start, maintenance_window_end,
        maintenance_window_timezone, reboot_behavior, deferral_days,
        managed_mode, native_windows_update_control, policy_config_jsonb, priority, enabled,
        is_default, created_by, created_at, updated_at
      )
    SELECT
      ${defaultPatchPolicyId(organizationId)}, ${organizationId}, 'organization', ${DEFAULT_PATCH_POLICY_SCOPE_KEY},
      NULL, NULL, NULL,
      ${DEFAULT_PATCH_POLICY_NAME}, 'all', 'auto_approve_all', NULL, NULL,
      'UTC', 'allow', 0,
      true, true, ${JSON.stringify(policyConfig)}::jsonb, ${DEFAULT_PATCH_POLICY_PRIORITY}, true,
      true, ${createdBy}, NOW(), NOW()
    WHERE NOT EXISTS (SELECT 1 FROM updated)
    ON CONFLICT (id)
    DO UPDATE SET
      scope_type = 'organization',
      scope_key = ${DEFAULT_PATCH_POLICY_SCOPE_KEY},
      customer_id = NULL,
      site_id = NULL,
      agent_id = NULL,
      name = EXCLUDED.name,
      target_os_family = EXCLUDED.target_os_family,
      approval_mode = EXCLUDED.approval_mode,
      maintenance_window_start = EXCLUDED.maintenance_window_start,
      maintenance_window_end = EXCLUDED.maintenance_window_end,
      maintenance_window_timezone = EXCLUDED.maintenance_window_timezone,
      reboot_behavior = EXCLUDED.reboot_behavior,
      deferral_days = EXCLUDED.deferral_days,
      managed_mode = EXCLUDED.managed_mode,
      native_windows_update_control = EXCLUDED.native_windows_update_control,
      policy_config_jsonb = EXCLUDED.policy_config_jsonb,
      priority = EXCLUDED.priority,
      enabled = true,
      is_default = true,
      updated_at = NOW()
  `);
}
