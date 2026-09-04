ALTER TABLE "public"."rmm_devices"
  ADD COLUMN "linux_shell_username" TEXT,
  ADD COLUMN "linux_shell_password_enc" TEXT,
  ADD COLUMN "linux_shell_credential_id" TEXT,
  ADD COLUMN "linux_shell_credential_version" INTEGER,
  ADD COLUMN "linux_shell_password_updated_at" TIMESTAMPTZ(3);

CREATE INDEX "rmm_devices_linux_shell_credential_id_idx"
  ON "public"."rmm_devices"("linux_shell_credential_id");
