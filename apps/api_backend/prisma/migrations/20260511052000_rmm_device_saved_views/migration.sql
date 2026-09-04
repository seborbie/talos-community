CREATE TABLE IF NOT EXISTS "rmm_device_saved_views" (
  "id" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "user_id" TEXT NOT NULL,
  "name" TEXT NOT NULL,
  "filters" JSONB NOT NULL DEFAULT '{}'::jsonb,
  "sort_by" TEXT NOT NULL DEFAULT 'lastSeen',
  "sort_direction" TEXT NOT NULL DEFAULT 'desc',
  "page_size" INTEGER NOT NULL DEFAULT 50,
  "created_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT "rmm_device_saved_views_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX IF NOT EXISTS "rmm_device_saved_views_organization_id_user_id_name_key"
  ON "rmm_device_saved_views"("organization_id", "user_id", "name");

CREATE INDEX IF NOT EXISTS "rmm_device_saved_views_organization_id_user_id_updated_at_idx"
  ON "rmm_device_saved_views"("organization_id", "user_id", "updated_at");

ALTER TABLE "rmm_device_saved_views"
  ADD CONSTRAINT "rmm_device_saved_views_organization_id_fkey"
  FOREIGN KEY ("organization_id") REFERENCES "Organization"("id")
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE "rmm_device_saved_views"
  ADD CONSTRAINT "rmm_device_saved_views_user_id_fkey"
  FOREIGN KEY ("user_id") REFERENCES "User"("id")
  ON DELETE CASCADE ON UPDATE CASCADE;
