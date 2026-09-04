CREATE TABLE IF NOT EXISTS public.feature_upgrade_iso_media (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  os_family TEXT NOT NULL DEFAULT 'windows',
  product TEXT NOT NULL,
  version TEXT NOT NULL,
  edition TEXT,
  architecture TEXT NOT NULL,
  language TEXT,
  sha256 TEXT,
  size_bytes BIGINT,
  container_name TEXT NOT NULL,
  blob_name TEXT NOT NULL,
  active BOOLEAN NOT NULL DEFAULT true,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS feature_upgrade_iso_media_container_blob_key
  ON public.feature_upgrade_iso_media (container_name, blob_name);

CREATE INDEX IF NOT EXISTS feature_upgrade_iso_media_active_os_family_idx
  ON public.feature_upgrade_iso_media (active, os_family);

CREATE INDEX IF NOT EXISTS feature_upgrade_iso_media_product_version_idx
  ON public.feature_upgrade_iso_media (product, version);
