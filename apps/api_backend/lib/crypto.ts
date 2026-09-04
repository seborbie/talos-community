import crypto from 'crypto';

const ALGO = 'aes-256-gcm';

function getKey(): Buffer {
  const key = process.env.APP_ENCRYPTION_KEY?.trim();
  if (!key) {
    throw new Error('Missing APP_ENCRYPTION_KEY for encryption');
  }
  // Derive 32-byte key from the application encryption secret using SHA-256
  return crypto.createHash('sha256').update(key).digest();
}

export function encryptSecret(plainText?: string | null): string | null {
  if (!plainText) return null;
  const iv = crypto.randomBytes(12);
  const key = getKey();
  const cipher = crypto.createCipheriv(ALGO, key, iv);
  const ciphertext = Buffer.concat([cipher.update(plainText, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([iv, tag, ciphertext]).toString('base64');
}

export function decryptSecret(payload?: string | null): string | null {
  if (!payload) return null;
  const raw = Buffer.from(payload, 'base64');
  const iv = raw.subarray(0, 12);
  const tag = raw.subarray(12, 28);
  const data = raw.subarray(28);
  const key = getKey();
  const decipher = crypto.createDecipheriv(ALGO, key, iv);
  decipher.setAuthTag(tag);
  const plain = Buffer.concat([decipher.update(data), decipher.final()]);
  return plain.toString('utf8');
}

