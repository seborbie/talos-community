CREATE TABLE IF NOT EXISTS public.rmm_installer_short_link (
  code VARCHAR(8) PRIMARY KEY,
  token_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  customer_id TEXT,
  site_id TEXT,
  registration_token TEXT NOT NULL,
  server_url TEXT NOT NULL,
  issued_by TEXT NOT NULL,
  expires_at TIMESTAMPTZ(3) NOT NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT rmm_installer_short_link_code_check CHECK (code ~ '^[a-z0-9]{8}$'),
  CONSTRAINT rmm_installer_short_link_token_id_fkey FOREIGN KEY (token_id)
    REFERENCES public.rmm_installer_enrollment_token(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_short_link_profile_id_fkey FOREIGN KEY (profile_id)
    REFERENCES public.rmm_installer_profile(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_short_link_organization_id_fkey FOREIGN KEY (organization_id)
    REFERENCES public."Organization"(id) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_short_link_customer_id_fkey FOREIGN KEY (customer_id)
    REFERENCES public.customers(id) ON DELETE SET NULL ON UPDATE CASCADE,
  CONSTRAINT rmm_installer_short_link_site_id_fkey FOREIGN KEY (site_id)
    REFERENCES public.rmm_sites(id) ON DELETE SET NULL ON UPDATE CASCADE
);

CREATE INDEX IF NOT EXISTS rmm_installer_short_link_expires_at_idx
  ON public.rmm_installer_short_link (expires_at);

CREATE INDEX IF NOT EXISTS rmm_installer_short_link_token_id_idx
  ON public.rmm_installer_short_link (token_id);

CREATE INDEX IF NOT EXISTS rmm_installer_short_link_organization_id_created_at_idx
  ON public.rmm_installer_short_link (organization_id, created_at DESC);
