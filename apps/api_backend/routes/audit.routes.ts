import { Router } from 'express';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import {
  AuditEventInput,
  auditRequest,
  getAuditRequestMetadata,
  writeAuditEvent
} from '../lib/audit';
import { requireAuth, AuthedRequest } from '../middleware/auth';
import { attachRmmServerAuth, requireRmmServer, RmmServerRequest } from '../middleware/rmmServerKey';

export const auditRouter = Router();
auditRouter.use(attachRmmServerAuth);

const ACTOR_TYPES = new Set(['user', 'machine', 'agent', 'service', 'system', 'unknown']);
const RESULTS = new Set(['success', 'failure', 'blocked']);

async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { organization: true, user: { select: { id: true, email: true } } }
  });
}

function assertUser(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== 'user') {
    res.status(403).json({ error: 'Machine tokens are not allowed' });
    return false;
  }
  return true;
}

function readString(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function readDate(value: unknown, fieldName: string): Date | null {
  const raw = readString(value);
  if (!raw) return null;
  const parsed = new Date(raw);
  if (Number.isNaN(parsed.getTime())) {
    throw Object.assign(new Error(`${fieldName} must be an ISO-8601 timestamp`), { status: 400 });
  }
  return parsed;
}

function readCursor(value: unknown): bigint | null {
  const raw = readString(value);
  if (!raw) return null;
  try {
    return BigInt(raw);
  } catch {
    throw Object.assign(new Error('cursor must be a bigint string'), { status: 400 });
  }
}

function readLimit(value: unknown, fallback = 100, max = 500): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(Math.max(Math.floor(parsed), 1), max);
}

function readEnum<T extends string>(value: unknown, allowed: Set<string>, fallback: T, fieldName: string): T {
  const raw = readString(value) ?? fallback;
  if (!allowed.has(raw)) {
    throw Object.assign(new Error(`${fieldName} must be one of: ${[...allowed].join(', ')}`), { status: 400 });
  }
  return raw as T;
}

function readOptionalNumber(value: unknown, fieldName: string): number | null {
  if (value === null || value === undefined || value === '') return null;
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw Object.assign(new Error(`${fieldName} must be a number`), { status: 400 });
  }
  return parsed;
}

function mapAuditEvent(row: any) {
  return {
    id: row.id.toString(),
    organizationId: row.organizationId,
    customerId: row.customerId,
    siteId: row.siteId,
    agentId: row.agentId,
    actorType: row.actorType,
    userId: row.userId,
    userEmail: row.userEmail,
    actionType: row.actionType,
    targetType: row.targetType,
    targetId: row.targetId,
    targetName: row.targetName,
    result: row.result,
    statusCode: row.statusCode,
    errorMessage: row.errorMessage,
    requestMethod: row.requestMethod,
    requestPath: row.requestPath,
    clientIp: row.clientIp,
    userAgent: row.userAgent,
    correlationId: row.correlationId,
    sessionId: row.sessionId,
    metadata: row.metadata,
    occurredAt: row.occurredAt.toISOString(),
    createdAt: row.createdAt.toISOString()
  };
}

function csvValue(value: unknown): string {
  if (value === null || value === undefined) return '';
  const text = typeof value === 'object' ? JSON.stringify(value) : String(value);
  return `"${text.replace(/"/g, '""')}"`;
}

function auditEventsToCsv(items: ReturnType<typeof mapAuditEvent>[]): string {
  const headers = [
    'occurredAt',
    'result',
    'actionType',
    'actorType',
    'userEmail',
    'organizationId',
    'customerId',
    'siteId',
    'agentId',
    'targetType',
    'targetId',
    'targetName',
    'sessionId',
    'correlationId',
    'clientIp',
    'errorMessage',
    'metadata'
  ];
  const rows = items.map((item) => headers.map((header) => csvValue((item as any)[header])).join(','));
  return `${headers.join(',')}\n${rows.join('\n')}\n`;
}

auditRouter.post('/events', requireRmmServer, async (req: RmmServerRequest, res, next) => {
  try {
    const body = req.body || {};
    const auditEvent: AuditEventInput = auditRequest(req, {
      organizationId: readString(body.organizationId),
      customerId: readString(body.customerId),
      siteId: readString(body.siteId),
      agentId: readString(body.agentId),
      actorType: readEnum(body.actorType, ACTOR_TYPES, 'service', 'actorType'),
      userId: readString(body.userId),
      userEmail: readString(body.userEmail),
      actionType: readString(body.actionType) || '',
      targetType: readString(body.targetType) || '',
      targetId: readString(body.targetId),
      targetName: readString(body.targetName),
      result: readEnum(body.result, RESULTS, 'success', 'result'),
      statusCode: readOptionalNumber(body.statusCode, 'statusCode'),
      errorMessage: readString(body.errorMessage),
      correlationId: readString(body.correlationId) || getAuditRequestMetadata(req).correlationId,
      sessionId: readString(body.sessionId),
      metadata: body.metadata && typeof body.metadata === 'object' && !Array.isArray(body.metadata) ? body.metadata : {}
    });

    if (!auditEvent.actionType || !auditEvent.targetType) {
      return res.status(400).json({ error: 'actionType and targetType are required' });
    }

    const created = await writeAuditEvent(auditEvent);
    return res.status(201).json({ id: created.id.toString() });
  } catch (error) {
    return next(error);
  }
});

auditRouter.get('/events', requireAuth, async (req: AuthedRequest, res, next) => {
  try {
    if (!assertUser(req, res)) return;
    const membership = await getCurrentMembership(req.jwt!.sub);
    if (!membership) return res.status(404).json({ error: 'No organization', needsOnboarding: true });

    const limit = readLimit(req.query.limit, readString(req.query.format) === 'csv' ? 1000 : 100);
    const cursor = readCursor(req.query.cursor);
    const query = readString(req.query.q)?.slice(0, 200) ?? null;
    const actionType = readString(req.query.actionType);
    const result = readString(req.query.result);
    const agentId = readString(req.query.agentId);
    const userId = readString(req.query.userId);
    const customerId = readString(req.query.customerId);
    const siteId = readString(req.query.siteId);
    const from = readDate(req.query.from, 'from');
    const to = readDate(req.query.to, 'to');

    const where: Prisma.AuditEventWhereInput = {
      organizationId: membership.organizationId,
      ...(cursor ? { id: { lt: cursor } } : {}),
      ...(actionType ? { actionType } : {}),
      ...(result && result !== 'all' ? { result } : {}),
      ...(agentId ? { agentId } : {}),
      ...(userId ? { userId } : {}),
      ...(customerId ? { customerId } : {}),
      ...(siteId ? { siteId } : {}),
      ...(from || to
        ? {
            occurredAt: {
              ...(from ? { gte: from } : {}),
              ...(to ? { lte: to } : {})
            }
          }
        : {}),
      ...(query
        ? {
            OR: [
              { actionType: { contains: query, mode: 'insensitive' } },
              { targetType: { contains: query, mode: 'insensitive' } },
              { targetName: { contains: query, mode: 'insensitive' } },
              { userEmail: { contains: query, mode: 'insensitive' } },
              { agentId: { contains: query, mode: 'insensitive' } },
              { sessionId: { contains: query, mode: 'insensitive' } },
              { correlationId: { contains: query, mode: 'insensitive' } },
              { errorMessage: { contains: query, mode: 'insensitive' } }
            ]
          }
        : {})
    };

    const rows = await prisma.auditEvent.findMany({
      where,
      orderBy: [{ occurredAt: 'desc' }, { id: 'desc' }],
      take: limit
    });
    const items = rows.map(mapAuditEvent);

    if (readString(req.query.format) === 'csv') {
      const csv = auditEventsToCsv(items);
      res.setHeader('Content-Type', 'text/csv; charset=utf-8');
      res.setHeader('Content-Disposition', `attachment; filename="talos-audit-events.csv"`);
      return res.status(200).send(csv);
    }

    const nextCursor = rows.length === limit ? rows[rows.length - 1]!.id.toString() : null;
    return res.json({ items, nextCursor });
  } catch (error) {
    return next(error);
  }
});
