import { randomInt, randomUUID } from "crypto";
import { Prisma } from "@prisma/client";
import { decryptSecret, encryptSecret } from "./crypto";
import { generateMemorablePassword, type PasswordOptions } from "./passwordGenerator";
import { prisma } from "./prisma";

const NOTE_CODE_ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789";
const DEFAULT_EXPIRY_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_CREATE_ATTEMPTS = 8;

export type SecretSurface = "shell" | "desktop" | "note_only";

export type GeneratedSecretSummary = {
  secretHandle: string;
  shellReference: string | null;
  desktopReference: string | null;
  secureNoteUrl: string;
  expiresAt: string;
  purpose: string | null;
};

export type SecureNoteCheck =
  | { status: "available"; expiresAt: string; recipientEmail: string | null }
  | { status: "not_found" | "expired" | "viewed" | "unauthorized" };

export type SecureNoteReveal =
  | { status: "revealed"; content: string; destroyedAt: string }
  | { status: "not_found" | "expired" | "viewed" | "unauthorized" };

type SecureNoteRow = {
  id: string;
  code: string;
  secretHandle: string;
  organizationId: string;
  createdByUserId: string;
  recipientUserId: string;
  recipientEmail: string | null;
  contentEnc: string | null;
  shellReference: string | null;
  desktopReference: string | null;
  purpose: string | null;
  expiresAt: Date;
  viewedAt: Date | null;
  destroyedAt: Date | null;
};

type AttachableSecureNoteRow = SecureNoteRow & {
  jobId: string | null;
  agentId: string | null;
};

function randomCode(length: number): string {
  let out = "";
  for (let index = 0; index < length; index += 1) {
    out += NOTE_CODE_ALPHABET[randomInt(0, NOTE_CODE_ALPHABET.length)];
  }
  return out;
}

function secureNotePublicBaseUrl(): string | null {
  const value = (
    process.env.RMM_INSTALLER_PUBLIC_FRONTEND_URL ||
    process.env.FRONTEND_PUBLIC_URL ||
    process.env.PUBLIC_APP_URL ||
    process.env.APP_PUBLIC_URL ||
    process.env.FRONTEND_URL ||
    ""
  ).trim();
  return value ? value.replace(/\/+$/, "") : null;
}

function noteUrl(code: string): string {
  const path = `/SN/${code}`;
  const baseUrl = secureNotePublicBaseUrl();
  return baseUrl ? `${baseUrl}${path}` : path;
}

function shellReferenceFor(handle: string): string {
  return `$__talos_secret_${handle.slice(-6)}`;
}

function desktopReferenceFor(handle: string): string {
  return `desktop_secret_${handle.slice(-6)}`;
}

function rowToSummary(row: SecureNoteRow): GeneratedSecretSummary {
  return {
    secretHandle: row.secretHandle,
    shellReference: row.shellReference,
    desktopReference: row.desktopReference,
    secureNoteUrl: noteUrl(row.code),
    expiresAt: row.expiresAt.toISOString(),
    purpose: row.purpose,
  };
}

function isUniqueConstraintError(error: unknown): boolean {
  if (error instanceof Prisma.PrismaClientKnownRequestError) {
    return error.code === "P2002" || (error.code === "P2010" && String(error.message).includes("23505"));
  }
  return String(error).includes("23505") || String(error).includes("duplicate key");
}

async function assertRecipientMembership(organizationId: string, userId: string) {
  const membership = await prisma.organizationMember.findFirst({
    where: { organizationId, userId },
    include: { user: { select: { email: true } } },
  });
  if (!membership) {
    throw new Error("Secure note recipient is not a member of the organization");
  }
  return membership.user?.email ?? null;
}

export async function createGeneratedSecret(input: {
  organizationId: string;
  userId: string;
  kind: "password";
  surface: SecretSurface;
  purpose?: string | null;
  jobId?: string | null;
  agentId?: string | null;
  passwordOptions?: PasswordOptions;
  expiresInMs?: number;
}): Promise<GeneratedSecretSummary> {
  if (input.kind !== "password") {
    throw new Error("Only password secret generation is supported");
  }
  const recipientEmail = await assertRecipientMembership(input.organizationId, input.userId);
  const secret = generateMemorablePassword(input.passwordOptions ?? {});
  const contentEnc = encryptSecret(secret);
  if (!contentEnc) {
    throw new Error("Failed to encrypt generated secret");
  }
  const expiresAt = new Date(Date.now() + Math.max(1_000, input.expiresInMs ?? DEFAULT_EXPIRY_MS));

  for (let attempt = 0; attempt < MAX_CREATE_ATTEMPTS; attempt += 1) {
    const code = randomCode(8);
    const secretHandle = `sec_${randomCode(16)}`;
    const shellReference = input.surface === "shell" ? shellReferenceFor(secretHandle) : null;
    const desktopReference = input.surface === "desktop" ? desktopReferenceFor(secretHandle) : null;
    try {
      const rows = await prisma.$queryRaw<SecureNoteRow[]>`
        INSERT INTO command_center.secure_notes (
          id,
          code,
          secret_handle,
          organization_id,
          created_by_user_id,
          recipient_user_id,
          recipient_email,
          kind,
          surface,
          purpose,
          content_enc,
          content_length,
          shell_reference,
          desktop_reference,
          job_id,
          agent_id,
          expires_at
        )
        VALUES (
          ${randomUUID()},
          ${code},
          ${secretHandle},
          ${input.organizationId},
          ${input.userId},
          ${input.userId},
          ${recipientEmail},
          ${input.kind},
          ${input.surface},
          ${input.purpose?.trim() || null},
          ${contentEnc},
          ${secret.length},
          ${shellReference},
          ${desktopReference},
          ${input.jobId?.trim() || null},
          ${input.agentId?.trim() || null},
          ${expiresAt}
        )
        RETURNING
          id,
          code,
          secret_handle AS "secretHandle",
          organization_id AS "organizationId",
          created_by_user_id AS "createdByUserId",
          recipient_user_id AS "recipientUserId",
          recipient_email AS "recipientEmail",
          content_enc AS "contentEnc",
          shell_reference AS "shellReference",
          desktop_reference AS "desktopReference",
          purpose,
          expires_at AS "expiresAt",
          viewed_at AS "viewedAt",
          destroyed_at AS "destroyedAt"
      `;
      return rowToSummary(rows[0]);
    } catch (error) {
      if (isUniqueConstraintError(error)) {
        continue;
      }
      throw error;
    }
  }
  throw new Error("Failed to allocate a unique secure note code");
}

export async function attachGeneratedSecretsToRunnerJob(input: {
  organizationId: string;
  userId: string;
  jobId: string;
  agentId?: string | null;
  secretHandles: string[];
}): Promise<GeneratedSecretSummary[]> {
  const handles = [...new Set(input.secretHandles.map((value) => value.trim()).filter(Boolean))];
  if (handles.length === 0) return [];
  for (const handle of handles) {
    if (!isSecretHandle(handle)) {
      throw new Error(`Invalid generated secret handle: ${handle}`);
    }
  }

  return prisma.$transaction(async (tx) => {
    const jobs = await tx.$queryRaw<Array<{
      id: string;
      organizationId: string;
      userId: string;
      agentId: string;
    }>>`
      SELECT
        id,
        organization_id AS "organizationId",
        user_id AS "userId",
        agent_id AS "agentId"
      FROM command_center.ai_runner_jobs
      WHERE id = ${input.jobId}
      LIMIT 1
    `;
    const job = jobs[0];
    if (!job) {
      throw new Error("AI runner job not found");
    }
    if (job.organizationId !== input.organizationId || job.userId !== input.userId) {
      throw new Error("Generated secret job ownership mismatch");
    }
    if (input.agentId && job.agentId !== input.agentId) {
      throw new Error("Generated secret job agent mismatch");
    }

    const rows = await tx.$queryRaw<AttachableSecureNoteRow[]>(
      Prisma.sql`
        SELECT
          id,
          code,
          secret_handle AS "secretHandle",
          organization_id AS "organizationId",
          created_by_user_id AS "createdByUserId",
          recipient_user_id AS "recipientUserId",
          recipient_email AS "recipientEmail",
          content_enc AS "contentEnc",
          shell_reference AS "shellReference",
          desktop_reference AS "desktopReference",
          job_id AS "jobId",
          agent_id AS "agentId",
          purpose,
          expires_at AS "expiresAt",
          viewed_at AS "viewedAt",
          destroyed_at AS "destroyedAt"
        FROM command_center.secure_notes
        WHERE secret_handle IN (${Prisma.join(handles)})
        FOR UPDATE
      `,
    );

    const byHandle = new Map(rows.map((row) => [row.secretHandle, row]));
    for (const handle of handles) {
      const note = byHandle.get(handle);
      if (!note) {
        throw new Error(`Generated secret not found: ${handle}`);
      }
      if (note.organizationId !== input.organizationId || note.recipientUserId !== input.userId) {
        throw new Error(`Generated secret is not available to this operator: ${handle}`);
      }
      if (note.expiresAt.getTime() <= Date.now()) {
        await tx.$executeRaw`DELETE FROM command_center.secure_notes WHERE id = ${note.id}`;
        throw new Error(`Generated secret has expired: ${handle}`);
      }
      if (!note.contentEnc || note.destroyedAt || note.viewedAt) {
        throw new Error(`Generated secret is no longer available: ${handle}`);
      }
      if (note.jobId && note.jobId !== input.jobId) {
        throw new Error(`Generated secret is already bound to another runner job: ${handle}`);
      }
      if (note.agentId && note.agentId !== job.agentId) {
        throw new Error(`Generated secret is bound to a different agent: ${handle}`);
      }
    }

    const updated = await tx.$queryRaw<SecureNoteRow[]>(
      Prisma.sql`
        UPDATE command_center.secure_notes
        SET job_id = ${input.jobId},
            agent_id = COALESCE(agent_id, ${job.agentId}),
            updated_at = NOW()
        WHERE secret_handle IN (${Prisma.join(handles)})
        RETURNING
          id,
          code,
          secret_handle AS "secretHandle",
          organization_id AS "organizationId",
          created_by_user_id AS "createdByUserId",
          recipient_user_id AS "recipientUserId",
          recipient_email AS "recipientEmail",
          content_enc AS "contentEnc",
          shell_reference AS "shellReference",
          desktop_reference AS "desktopReference",
          purpose,
          expires_at AS "expiresAt",
          viewed_at AS "viewedAt",
          destroyed_at AS "destroyedAt"
      `,
    );

    const summaries = new Map(updated.map((row) => [row.secretHandle, rowToSummary(row)]));
    return handles.map((handle) => summaries.get(handle)).filter((value): value is GeneratedSecretSummary => Boolean(value));
  });
}

export async function deleteExpiredSecureNote(code: string): Promise<boolean> {
  const result = await prisma.$executeRaw`
    DELETE FROM command_center.secure_notes
    WHERE code = ${code}
      AND expires_at <= NOW()
  `;
  return result > 0;
}

async function findNoteByCode(code: string): Promise<SecureNoteRow | null> {
  const rows = await prisma.$queryRaw<SecureNoteRow[]>`
    SELECT
      id,
      code,
      secret_handle AS "secretHandle",
      organization_id AS "organizationId",
      created_by_user_id AS "createdByUserId",
      recipient_user_id AS "recipientUserId",
      recipient_email AS "recipientEmail",
      content_enc AS "contentEnc",
      shell_reference AS "shellReference",
      desktop_reference AS "desktopReference",
      purpose,
      expires_at AS "expiresAt",
      viewed_at AS "viewedAt",
      destroyed_at AS "destroyedAt"
    FROM command_center.secure_notes
    WHERE code = ${code}
    LIMIT 1
  `;
  return rows[0] ?? null;
}

export async function checkSecureNoteForUser(code: string, userId: string): Promise<SecureNoteCheck> {
  const note = await findNoteByCode(code);
  if (!note) return { status: "not_found" };
  if (note.expiresAt.getTime() <= Date.now()) {
    await deleteExpiredSecureNote(code);
    return { status: "expired" };
  }
  if (note.recipientUserId !== userId) return { status: "unauthorized" };
  if (note.destroyedAt || note.viewedAt || !note.contentEnc) return { status: "viewed" };
  return {
    status: "available",
    expiresAt: note.expiresAt.toISOString(),
    recipientEmail: note.recipientEmail,
  };
}

export async function revealSecureNoteForUser(code: string, userId: string): Promise<SecureNoteReveal> {
  return prisma.$transaction(async (tx) => {
    const rows = await tx.$queryRaw<SecureNoteRow[]>`
      SELECT
        id,
        code,
        secret_handle AS "secretHandle",
        organization_id AS "organizationId",
        created_by_user_id AS "createdByUserId",
        recipient_user_id AS "recipientUserId",
        recipient_email AS "recipientEmail",
        content_enc AS "contentEnc",
        shell_reference AS "shellReference",
        desktop_reference AS "desktopReference",
        purpose,
        expires_at AS "expiresAt",
        viewed_at AS "viewedAt",
        destroyed_at AS "destroyedAt"
      FROM command_center.secure_notes
      WHERE code = ${code}
      FOR UPDATE
    `;
    const note = rows[0];
    if (!note) return { status: "not_found" };
    if (note.expiresAt.getTime() <= Date.now()) {
      await tx.$executeRaw`DELETE FROM command_center.secure_notes WHERE id = ${note.id}`;
      return { status: "expired" };
    }
    if (note.recipientUserId !== userId) return { status: "unauthorized" };
    if (note.destroyedAt || note.viewedAt || !note.contentEnc) return { status: "viewed" };

    const content = decryptSecret(note.contentEnc);
    if (!content) {
      throw new Error("Secure note content could not be decrypted");
    }
    const destroyedAt = new Date();
    await tx.$executeRaw`
      UPDATE command_center.secure_notes
      SET content_enc = NULL,
          viewed_at = ${destroyedAt},
          destroyed_at = ${destroyedAt},
          updated_at = ${destroyedAt}
      WHERE id = ${note.id}
    `;
    return {
      status: "revealed",
      content,
      destroyedAt: destroyedAt.toISOString(),
    };
  });
}

export async function resolveGeneratedSecretForRunner(input: {
  jobId: string;
  runnerId?: string | null;
  leaseId?: string | null;
  secretHandle: string;
}): Promise<{
  secret: string;
  shellReference: string | null;
  desktopReference: string | null;
  secureNoteUrl: string;
}> {
  const jobs = await prisma.$queryRaw<Array<{
    id: string;
    organizationId: string;
    runnerId: string | null;
    leaseOwnerRunnerId: string | null;
    leaseId: string | null;
    leaseExpiresAt: Date | null;
  }>>`
    SELECT
      id,
      organization_id AS "organizationId",
      runner_id AS "runnerId",
      lease_owner_runner_id AS "leaseOwnerRunnerId",
      lease_id AS "leaseId",
      lease_expires_at AS "leaseExpiresAt"
    FROM command_center.ai_runner_jobs
    WHERE id = ${input.jobId}
    LIMIT 1
  `;
  const job = jobs[0];
  if (!job) {
    throw new Error("AI runner job not found");
  }
  if (!job.leaseId || job.leaseId !== input.leaseId) {
    throw new Error("AI runner callback lease mismatch");
  }
  if (input.runnerId && job.leaseOwnerRunnerId && job.leaseOwnerRunnerId !== input.runnerId) {
    throw new Error("AI runner callback runner mismatch");
  }
  if (job.leaseExpiresAt && job.leaseExpiresAt.getTime() <= Date.now()) {
    throw new Error("AI runner callback lease expired");
  }

  const rows = await prisma.$queryRaw<SecureNoteRow[]>`
    SELECT
      id,
      code,
      secret_handle AS "secretHandle",
      organization_id AS "organizationId",
      created_by_user_id AS "createdByUserId",
      recipient_user_id AS "recipientUserId",
      recipient_email AS "recipientEmail",
      content_enc AS "contentEnc",
      shell_reference AS "shellReference",
      desktop_reference AS "desktopReference",
      purpose,
      expires_at AS "expiresAt",
      viewed_at AS "viewedAt",
      destroyed_at AS "destroyedAt"
    FROM command_center.secure_notes
    WHERE secret_handle = ${input.secretHandle}
      AND job_id = ${input.jobId}
    LIMIT 1
  `;
  const note = rows[0];
  if (!note) {
    throw new Error("Generated secret not found for runner job");
  }
  if (note.organizationId !== job.organizationId) {
    throw new Error("Generated secret organization mismatch");
  }
  if (note.expiresAt.getTime() <= Date.now()) {
    await deleteExpiredSecureNote(note.code);
    throw new Error("Generated secret has expired");
  }
  if (!note.contentEnc || note.destroyedAt || note.viewedAt) {
    throw new Error("Generated secret is no longer available");
  }
  const secret = decryptSecret(note.contentEnc);
  if (!secret) {
    throw new Error("Generated secret could not be decrypted");
  }
  return {
    secret,
    shellReference: note.shellReference,
    desktopReference: note.desktopReference,
    secureNoteUrl: noteUrl(note.code),
  };
}

export async function listGeneratedSecureNotesForJob(jobId: string): Promise<GeneratedSecretSummary[]> {
  if (!jobId.trim()) return [];
  const rows = await prisma.$queryRaw<SecureNoteRow[]>`
    SELECT
      id,
      code,
      secret_handle AS "secretHandle",
      organization_id AS "organizationId",
      created_by_user_id AS "createdByUserId",
      recipient_user_id AS "recipientUserId",
      recipient_email AS "recipientEmail",
      content_enc AS "contentEnc",
      shell_reference AS "shellReference",
      desktop_reference AS "desktopReference",
      purpose,
      expires_at AS "expiresAt",
      viewed_at AS "viewedAt",
      destroyed_at AS "destroyedAt"
    FROM command_center.secure_notes
    WHERE job_id = ${jobId}
    ORDER BY created_at ASC
  `;
  return rows.map(rowToSummary);
}

export function isSecureNoteCode(value: string): boolean {
  return /^[a-z0-9]{8}$/.test(value);
}

export function isSecretHandle(value: string): boolean {
  return /^sec_[a-z0-9]{16}$/.test(value);
}
