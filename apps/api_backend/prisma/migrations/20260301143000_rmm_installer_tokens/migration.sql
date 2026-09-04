DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_type t
    JOIN pg_namespace n ON n.oid = t.typnamespace
    WHERE t.typname = 'RmmInstallerScopeType'
      AND n.nspname = 'public'
  ) THEN
    CREATE TYPE public."RmmInstallerScopeType" AS ENUM ('ORGANIZATION', 'CUSTOMER', 'SITE');
  END IF;
END $$;

CREATE TABLE IF NOT EXISTS public.rmm_installer_profile (
  id TEXT PRIMARY KEY,
  organization_id TEXT NOT NULL,
  customer_id TEXT NULL,
  site_id TEXT NULL,
  scope_type public."RmmInstallerScopeType" NOT NULL,
  name TEXT NOT NULL,
  expires_at TIMESTAMPTZ(3) NULL,
  max_uses INTEGER NULL,
  created_by TEXT NOT NULL,
  revoked_at TIMESTAMPTZ(3) NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT rmm_installer_profile_max_uses_check CHECK (max_uses IS NULL OR max_uses > 0),
  CONSTRAINT rmm_installer_profile_scope_check CHECK (
    (scope_type = 'ORGANIZATION' AND customer_id IS NULL AND site_id IS NULL)
    OR (scope_type = 'CUSTOMER' AND customer_id IS NOT NULL AND site_id IS NULL)
    OR (scope_type = 'SITE' AND customer_id IS NOT NULL AND site_id IS NOT NULL)
  ),
  CONSTRAINT rmm_installer_profile_organization_id_fkey FOREIGN KEY (organization_id)
    REFERENCES public."Organization"(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_profile_customer_id_fkey FOREIGN KEY (customer_id)
    REFERENCES public.customers(id) ON DELETE SET NULL ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_profile_site_id_fkey FOREIGN KEY (site_id)
    REFERENCES public.rmm_sites(id) ON DELETE SET NULL ON UPDATE CASCADE
);

CREATE INDEX IF NOT EXISTS rmm_installer_profile_organization_id_created_at_idx
  ON public.rmm_installer_profile (organization_id, created_at DESC);
CREATE INDEX IF NOT EXISTS rmm_installer_profile_customer_id_idx
  ON public.rmm_installer_profile (customer_id);
CREATE INDEX IF NOT EXISTS rmm_installer_profile_site_id_idx
  ON public.rmm_installer_profile (site_id);
CREATE INDEX IF NOT EXISTS rmm_installer_profile_revoked_at_idx
  ON public.rmm_installer_profile (revoked_at);

CREATE TABLE IF NOT EXISTS public.rmm_installer_enrollment_token (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  customer_id TEXT NULL,
  site_id TEXT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  token_prefix TEXT NOT NULL,
  expires_at TIMESTAMPTZ(3) NULL,
  max_uses INTEGER NULL,
  used_count INTEGER NOT NULL DEFAULT 0,
  issued_by TEXT NOT NULL,
  revoked_at TIMESTAMPTZ(3) NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_used_at TIMESTAMPTZ(3) NULL,
  CONSTRAINT rmm_installer_enrollment_token_max_uses_check CHECK (max_uses IS NULL OR max_uses > 0),
  CONSTRAINT rmm_installer_enrollment_token_used_count_check CHECK (used_count >= 0),
  CONSTRAINT rmm_installer_enrollment_token_uses_bounds_check CHECK (max_uses IS NULL OR used_count <= max_uses),
  CONSTRAINT rmm_installer_enrollment_token_profile_id_fkey FOREIGN KEY (profile_id)
    REFERENCES public.rmm_installer_profile(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_enrollment_token_organization_id_fkey FOREIGN KEY (organization_id)
    REFERENCES public."Organization"(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_enrollment_token_customer_id_fkey FOREIGN KEY (customer_id)
    REFERENCES public.customers(id) ON DELETE SET NULL ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_enrollment_token_site_id_fkey FOREIGN KEY (site_id)
    REFERENCES public.rmm_sites(id) ON DELETE SET NULL ON UPDATE CASCADE
);

CREATE INDEX IF NOT EXISTS rmm_installer_enrollment_token_profile_id_created_at_idx
  ON public.rmm_installer_enrollment_token (profile_id, created_at DESC);
CREATE INDEX IF NOT EXISTS rmm_installer_enrollment_token_organization_id_created_at_idx
  ON public.rmm_installer_enrollment_token (organization_id, created_at DESC);
CREATE INDEX IF NOT EXISTS rmm_installer_enrollment_token_revoked_at_expires_at_idx
  ON public.rmm_installer_enrollment_token (revoked_at, expires_at);

CREATE TABLE IF NOT EXISTS public.rmm_installer_token_use (
  id BIGSERIAL PRIMARY KEY,
  token_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  first_seen_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_seen_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT rmm_installer_token_use_token_id_fkey FOREIGN KEY (token_id)
    REFERENCES public.rmm_installer_enrollment_token(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_token_use_profile_id_fkey FOREIGN KEY (profile_id)
    REFERENCES public.rmm_installer_profile(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_token_use_organization_id_fkey FOREIGN KEY (organization_id)
    REFERENCES public."Organization"(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_token_use_token_id_agent_id_key UNIQUE (token_id, agent_id)
);

CREATE INDEX IF NOT EXISTS rmm_installer_token_use_organization_id_agent_id_idx
  ON public.rmm_installer_token_use (organization_id, agent_id);
CREATE INDEX IF NOT EXISTS rmm_installer_token_use_profile_id_first_seen_at_idx
  ON public.rmm_installer_token_use (profile_id, first_seen_at DESC);

CREATE TABLE IF NOT EXISTS public.rmm_installer_download_audit (
  id BIGSERIAL PRIMARY KEY,
  profile_id TEXT NOT NULL,
  token_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  customer_id TEXT NULL,
  site_id TEXT NULL,
  user_id TEXT NOT NULL,
  user_email TEXT NULL,
  client_ip TEXT NULL,
  user_agent TEXT NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT rmm_installer_download_audit_profile_id_fkey FOREIGN KEY (profile_id)
    REFERENCES public.rmm_installer_profile(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_download_audit_token_id_fkey FOREIGN KEY (token_id)
    REFERENCES public.rmm_installer_enrollment_token(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_download_audit_organization_id_fkey FOREIGN KEY (organization_id)
    REFERENCES public."Organization"(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_download_audit_customer_id_fkey FOREIGN KEY (customer_id)
    REFERENCES public.customers(id) ON DELETE SET NULL ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_download_audit_site_id_fkey FOREIGN KEY (site_id)
    REFERENCES public.rmm_sites(id) ON DELETE SET NULL ON UPDATE CASCADE
);

CREATE INDEX IF NOT EXISTS rmm_installer_download_audit_organization_id_created_at_idx
  ON public.rmm_installer_download_audit (organization_id, created_at DESC);
CREATE INDEX IF NOT EXISTS rmm_installer_download_audit_profile_id_created_at_idx
  ON public.rmm_installer_download_audit (profile_id, created_at DESC);
CREATE INDEX IF NOT EXISTS rmm_installer_download_audit_token_id_created_at_idx
  ON public.rmm_installer_download_audit (token_id, created_at DESC);
