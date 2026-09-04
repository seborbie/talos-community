CREATE TABLE "command_center"."secure_notes" (
  "id" TEXT NOT NULL,
  "code" TEXT NOT NULL,
  "secret_handle" TEXT NOT NULL,
  "organization_id" TEXT NOT NULL,
  "created_by_user_id" TEXT NOT NULL,
  "recipient_user_id" TEXT NOT NULL,
  "recipient_email" TEXT,
  "kind" TEXT NOT NULL,
  "surface" TEXT NOT NULL,
  "purpose" TEXT,
  "content_enc" TEXT,
  "content_length" INTEGER NOT NULL DEFAULT 0,
  "shell_reference" TEXT,
  "desktop_reference" TEXT,
  "job_id" TEXT,
  "agent_id" TEXT,
  "expires_at" TIMESTAMPTZ(3) NOT NULL,
  "viewed_at" TIMESTAMPTZ(3),
  "destroyed_at" TIMESTAMPTZ(3),
  "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

  CONSTRAINT "secure_notes_pkey" PRIMARY KEY ("id")
);

CREATE UNIQUE INDEX "secure_notes_code_key"
  ON "command_center"."secure_notes"("code");

CREATE UNIQUE INDEX "secure_notes_secret_handle_key"
  ON "command_center"."secure_notes"("secret_handle");

CREATE INDEX "command_center_secure_notes_org_expiry_idx"
  ON "command_center"."secure_notes"("organization_id", "expires_at");

CREATE INDEX "command_center_secure_notes_recipient_expiry_idx"
  ON "command_center"."secure_notes"("recipient_user_id", "expires_at");

CREATE INDEX "command_center_secure_notes_job_idx"
  ON "command_center"."secure_notes"("job_id");

CREATE INDEX "command_center_secure_notes_expiry_idx"
  ON "command_center"."secure_notes"("expires_at");

ALTER TABLE "command_center"."secure_notes"
  ADD CONSTRAINT "secure_notes_code_format"
  CHECK ("code" ~ '^[a-z0-9]{8}$');

ALTER TABLE "command_center"."secure_notes"
  ADD CONSTRAINT "secure_notes_secret_handle_format"
  CHECK ("secret_handle" ~ '^sec_[a-z0-9]{16}$');
