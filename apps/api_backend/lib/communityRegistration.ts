import bcrypt from 'bcrypt';
import { prisma } from './prisma';

const COMMUNITY_BOOTSTRAP_LOCK_ID = 847_260_119;
const MIN_PASSWORD_LENGTH = 12;
const MAX_PASSWORD_LENGTH = 128;
const MAX_EMAIL_LENGTH = 254;

export type CommunityRegistrationStatus = {
  registrationOpen: boolean;
  mode: 'first_user' | 'closed';
};

export class RegistrationInputError extends Error {}
export class RegistrationClosedError extends Error {}

export function parseRegistrationInput(body: unknown): { email: string; password: string } {
  if (!body || typeof body !== 'object' || Array.isArray(body)) {
    throw new RegistrationInputError('Email and password are required');
  }

  const record = body as Record<string, unknown>;
  const email = typeof record.email === 'string' ? record.email.trim().toLowerCase() : '';
  const password = typeof record.password === 'string' ? record.password : '';

  if (!email || email.length > MAX_EMAIL_LENGTH || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    throw new RegistrationInputError('A valid email address is required');
  }
  if (password.length < MIN_PASSWORD_LENGTH || password.length > MAX_PASSWORD_LENGTH) {
    throw new RegistrationInputError(
      `Password must be between ${MIN_PASSWORD_LENGTH} and ${MAX_PASSWORD_LENGTH} characters`,
    );
  }

  return { email, password };
}

export async function getCommunityRegistrationStatus(): Promise<CommunityRegistrationStatus> {
  const userCount = await prisma.user.count();
  return userCount === 0
    ? { registrationOpen: true, mode: 'first_user' }
    : { registrationOpen: false, mode: 'closed' };
}

export async function registerFirstCommunityUser(input: { email: string; password: string }) {
  const status = await getCommunityRegistrationStatus();
  if (!status.registrationOpen) {
    throw new RegistrationClosedError('Registration is closed');
  }

  const passwordHash = await bcrypt.hash(input.password, 10);
  return prisma.$transaction(async (tx) => {
    // Serialize the zero-user check without adding a durable bootstrap secret or singleton row.
    // The lock is scoped to this transaction and released automatically on commit or rollback.
    // PostgreSQL returns void; cast it so Prisma can deserialize the lock result.
    await tx.$queryRaw`SELECT pg_advisory_xact_lock(${COMMUNITY_BOOTSTRAP_LOCK_ID})::text`;

    if ((await tx.user.count()) !== 0) {
      throw new RegistrationClosedError('Registration is closed');
    }

    return tx.user.create({
      data: {
        email: input.email,
        password: passwordHash,
      },
    });
  });
}
