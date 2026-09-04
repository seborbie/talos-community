import type { Request } from 'express';
import type { Prisma } from '@prisma/client';
import { prisma } from './prisma';
import { createLogger } from './logger';
import { getTrustedClientIp } from './requestTrust';

const log = createLogger('api_backend::audit');

export type AuditActorType = 'user' | 'machine' | 'agent' | 'service' | 'system' | 'unknown';
export type AuditResult = 'success' | 'failure' | 'blocked';

type AuditClient = typeof prisma | Prisma.TransactionClient;

export type AuditRequestMetadata = {
  requestMethod?: string | null;
  requestPath?: string | null;
  clientIp?: string | null;
  userAgent?: string | null;
  correlationId?: string | null;
};

export type AuditEventInput = AuditRequestMetadata & {
  organizationId?: string | null;
  customerId?: string | null;
  siteId?: string | null;
  agentId?: string | null;
  actorType?: AuditActorType;
  userId?: string | null;
  userEmail?: string | null;
  actionType: string;
  targetType: string;
  targetId?: string | null;
  targetName?: string | null;
  result?: AuditResult;
  statusCode?: number | null;
  errorMessage?: string | null;
  sessionId?: string | null;
  occurredAt?: Date | null;
  metadata?: Record<string, unknown> | null;
};

function cleanString(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function cleanOptionalNumber(value: unknown): number | null {
  if (value === null || value === undefined) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function toInputJsonObject(value: Record<string, unknown> | null | undefined): Prisma.InputJsonObject {
  if (!value) return {};
  return JSON.parse(JSON.stringify(value, (_key, item) => (item === undefined ? null : item)));
}

export function getAuditRequestMetadata(req: Request): AuditRequestMetadata {
  const correlationId =
    cleanString(req.header('x-correlation-id')) ||
    cleanString(req.header('x-request-id')) ||
    cleanString(req.header('traceparent'));

  return {
    requestMethod: cleanString(req.method),
    requestPath: cleanString(req.originalUrl || req.path),
    clientIp: getTrustedClientIp(req),
    userAgent: cleanString(req.header('user-agent')),
    correlationId
  };
}

export function buildAuditCreateData(input: AuditEventInput): Prisma.AuditEventCreateInput {
  return {
    organizationId: cleanString(input.organizationId),
    customerId: cleanString(input.customerId),
    siteId: cleanString(input.siteId),
    agentId: cleanString(input.agentId),
    actorType: input.actorType || 'user',
    userId: cleanString(input.userId),
    userEmail: cleanString(input.userEmail),
    actionType: cleanString(input.actionType) || 'unknown.action',
    targetType: cleanString(input.targetType) || 'unknown',
    targetId: cleanString(input.targetId),
    targetName: cleanString(input.targetName),
    result: input.result || 'success',
    statusCode: cleanOptionalNumber(input.statusCode),
    errorMessage: cleanString(input.errorMessage),
    requestMethod: cleanString(input.requestMethod),
    requestPath: cleanString(input.requestPath),
    clientIp: cleanString(input.clientIp),
    userAgent: cleanString(input.userAgent),
    correlationId: cleanString(input.correlationId),
    sessionId: cleanString(input.sessionId),
    metadata: toInputJsonObject(input.metadata),
    ...(input.occurredAt ? { occurredAt: input.occurredAt } : {})
  };
}

export async function writeAuditEvent(input: AuditEventInput, client: AuditClient = prisma) {
  return client.auditEvent.create({
    data: buildAuditCreateData(input)
  });
}

export async function tryWriteAuditEvent(input: AuditEventInput, client: AuditClient = prisma): Promise<void> {
  try {
    await writeAuditEvent(input, client);
  } catch (error) {
    log.warn('audit event write failed', {
      actionType: input.actionType,
      targetType: input.targetType,
      error: error instanceof Error ? error.message : String(error)
    });
  }
}

export function auditRequest(req: Request, input: AuditEventInput): AuditEventInput {
  return {
    ...getAuditRequestMetadata(req),
    ...input
  };
}
