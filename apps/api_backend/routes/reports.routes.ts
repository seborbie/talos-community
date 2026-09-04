import { randomUUID } from 'crypto';
import { Router } from 'express';
import { Prisma } from '@prisma/client';
import { prisma } from '../lib/prisma';
import {
  generateReportRows,
  nextReportRunAt,
  normalizeReportFilters,
  parseReportFormat,
  parseReportFrequency,
  parseReportId,
  PrismaReportRepository,
  REPORT_DEFINITION_BY_ID,
  REPORT_DEFINITIONS,
  ReportFilters,
  ReportFormat,
  ReportValidationError,
  reportFilename,
  reportFiltersToJson,
  rowsToCsv
} from '../lib/reports';
import { requireAuth, AuthedRequest } from '../middleware/auth';

export const reportsRouter = Router();

const reportRepository = new PrismaReportRepository(prisma);

type ReportRunRow = {
  id: string;
  organization_id: string;
  report_id: string;
  format: string;
  filters_jsonb: unknown;
  status: string;
  row_count: number;
  generated_by: string | null;
  delivery_status: string;
  error_message: string | null;
  started_at: Date;
  finished_at: Date | null;
  created_at: Date;
};

type ReportScheduleRow = {
  id: string;
  organization_id: string;
  report_id: string;
  name: string;
  format: string;
  frequency: string;
  filters_jsonb: unknown;
  email_to_jsonb: unknown;
  email_delivery_status: string;
  is_enabled: boolean;
  last_run_at: Date | null;
  next_run_at: Date | null;
  created_by: string | null;
  created_at: Date;
  updated_at: Date;
};

reportsRouter.use(requireAuth);

async function getCurrentMembership(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { organization: true, user: { select: { id: true, email: true } } }
  });
}

async function requireMembership(req: AuthedRequest, res: any) {
  if (req.jwt!.type !== 'user') {
    res.status(403).json({ error: 'Machine tokens are not allowed' });
    return null;
  }
  const membership = await getCurrentMembership(req.jwt!.sub);
  if (!membership) {
    res.status(404).json({ error: 'No organization', needsOnboarding: true });
    return null;
  }
  return membership;
}

function sendReportError(res: any, error: unknown) {
  if (error instanceof ReportValidationError) {
    return res.status(error.statusCode).json({ error: error.message });
  }
  const message = error instanceof Error ? error.message : String(error);
  return res.status(500).json({ error: message || 'Report request failed' });
}

function filterInputFromBody(body: any) {
  return {
    from: body?.filters?.from ?? body?.from,
    to: body?.filters?.to ?? body?.to,
    customerId: body?.filters?.customerId ?? body?.customerId,
    siteId: body?.filters?.siteId ?? body?.siteId,
    limit: body?.filters?.limit ?? body?.limit,
    offlineMinutes: body?.filters?.offlineMinutes ?? body?.offlineMinutes
  };
}

function filterInputFromStored(value: unknown) {
  if (!value) return {};
  if (typeof value === 'string') {
    try {
      return JSON.parse(value);
    } catch {
      return {};
    }
  }
  if (typeof value === 'object') return value as Record<string, unknown>;
  return {};
}

function jsonParam(value: unknown): string {
  return JSON.stringify(value ?? {});
}

function readEmailTo(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.filter((item): item is string => typeof item === 'string' && item.trim().length > 0).map((item) => item.trim());
  }
  if (typeof value === 'string') {
    return value
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean);
  }
  return [];
}

function parseBoundedPositiveInt(value: unknown, fallback: number, max: number, field: string): number {
  const raw = Array.isArray(value) ? value[0] : value;
  if (raw == null || raw === '') return fallback;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new ReportValidationError(`${field} must be a positive integer`);
  }
  return Math.min(parsed, max);
}

function mapRun(row: ReportRunRow) {
  return {
    id: row.id,
    organizationId: row.organization_id,
    reportId: row.report_id,
    format: row.format,
    filters: filterInputFromStored(row.filters_jsonb),
    status: row.status,
    rowCount: row.row_count,
    generatedBy: row.generated_by,
    deliveryStatus: row.delivery_status,
    errorMessage: row.error_message,
    startedAt: row.started_at.toISOString(),
    finishedAt: row.finished_at?.toISOString() ?? null,
    createdAt: row.created_at.toISOString()
  };
}

function mapSchedule(row: ReportScheduleRow) {
  return {
    id: row.id,
    organizationId: row.organization_id,
    reportId: row.report_id,
    name: row.name,
    format: row.format,
    frequency: row.frequency,
    filters: filterInputFromStored(row.filters_jsonb),
    emailTo: readEmailTo(row.email_to_jsonb),
    emailDeliveryStatus: row.email_delivery_status,
    isEnabled: row.is_enabled,
    lastRunAt: row.last_run_at?.toISOString() ?? null,
    nextRunAt: row.next_run_at?.toISOString() ?? null,
    createdBy: row.created_by,
    createdAt: row.created_at.toISOString(),
    updatedAt: row.updated_at.toISOString()
  };
}

function csvResponse(res: any, reportId: string, csv: string) {
  res.setHeader('Content-Type', 'text/csv; charset=utf-8');
  res.setHeader('Content-Disposition', `attachment; filename="${reportFilename(reportId as any, 'csv')}"`);
  return res.send(csv);
}

async function generateForRequest(reportId: unknown, organizationId: string, rawFilters: Record<string, unknown>) {
  const parsedReportId = parseReportId(reportId);
  const definition = REPORT_DEFINITION_BY_ID.get(parsedReportId)!;
  const filters = normalizeReportFilters(organizationId, rawFilters);
  const rows = await generateReportRows(reportRepository, parsedReportId, filters);
  return { reportId: parsedReportId, definition, filters, rows };
}

async function insertRun(args: {
  organizationId: string;
  reportId: string;
  format: ReportFormat;
  filters: ReportFilters;
  rowCount: number;
  generatedBy: string;
  deliveryStatus: string;
}) {
  const id = randomUUID();
  const filtersJson = jsonParam(reportFiltersToJson(args.filters));
  const rows = await prisma.$queryRaw<ReportRunRow[]>(
    Prisma.sql`
      INSERT INTO public.rmm_report_run
        (
          id, organization_id, report_id, format, filters_jsonb, status, row_count,
          generated_by, delivery_status, started_at, finished_at, created_at
        )
      VALUES
        (
          ${id}, ${args.organizationId}, ${args.reportId}, ${args.format}, ${filtersJson}::jsonb,
          'succeeded', ${args.rowCount}, ${args.generatedBy}, ${args.deliveryStatus}, NOW(), NOW(), NOW()
        )
      RETURNING *
    `
  );
  return mapRun(rows[0]);
}

async function loadRun(organizationId: string, id: string) {
  const rows = await prisma.$queryRaw<ReportRunRow[]>(
    Prisma.sql`
      SELECT *
      FROM public.rmm_report_run
      WHERE organization_id = ${organizationId}
        AND id = ${id}
      LIMIT 1
    `
  );
  return rows[0] ?? null;
}

reportsRouter.get('/definitions', async (_req: AuthedRequest, res) => {
  return res.json({ items: REPORT_DEFINITIONS });
});

reportsRouter.get('/runs', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const reportId = req.query.reportId ? parseReportId(req.query.reportId) : null;
    const limit = parseBoundedPositiveInt(req.query.limit, 50, 200, 'limit');
    const conditions = [Prisma.sql`organization_id = ${membership.organizationId}`];
    if (reportId) conditions.push(Prisma.sql`report_id = ${reportId}`);
    const rows = await prisma.$queryRaw<ReportRunRow[]>(
      Prisma.sql`
        SELECT *
        FROM public.rmm_report_run
        WHERE ${Prisma.join(conditions, ' AND ')}
        ORDER BY created_at DESC
        LIMIT ${limit}
      `
    );
    return res.json({ items: rows.map(mapRun) });
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.post('/runs', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const format = parseReportFormat(req.body?.format, 'json');
    const generated = await generateForRequest(
      req.body?.reportId,
      membership.organizationId,
      filterInputFromBody(req.body)
    );
    const run = await insertRun({
      organizationId: membership.organizationId,
      reportId: generated.reportId,
      format,
      filters: generated.filters,
      rowCount: generated.rows.length,
      generatedBy: req.jwt!.sub,
      deliveryStatus: format === 'pdf' ? 'pdf_generation_stubbed' : 'ready'
    });
    return res.status(201).json({
      run,
      definition: generated.definition,
      previewRows: generated.rows.slice(0, 25),
      downloadUrl: format === 'csv' ? `/rmm/reports/runs/${run.id}/export.csv` : null,
      pdfStubbed: format === 'pdf'
    });
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.get('/runs/:id/export.csv', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const runRow = await loadRun(membership.organizationId, req.params.id);
    if (!runRow) return res.status(404).json({ error: 'Report run not found' });
    const generated = await generateForRequest(
      runRow.report_id,
      membership.organizationId,
      filterInputFromStored(runRow.filters_jsonb)
    );
    return csvResponse(res, generated.reportId, rowsToCsv(generated.rows, generated.definition.columns));
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.get('/runs/:id', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const runRow = await loadRun(membership.organizationId, req.params.id);
    if (!runRow) return res.status(404).json({ error: 'Report run not found' });
    const generated = await generateForRequest(
      runRow.report_id,
      membership.organizationId,
      filterInputFromStored(runRow.filters_jsonb)
    );
    return res.json({
      run: mapRun(runRow),
      definition: generated.definition,
      filters: reportFiltersToJson(generated.filters),
      items: generated.rows
    });
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.get('/schedules', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const rows = await prisma.$queryRaw<ReportScheduleRow[]>(
    Prisma.sql`
      SELECT *
      FROM public.rmm_report_schedule
      WHERE organization_id = ${membership.organizationId}
      ORDER BY created_at DESC
    `
  );
  return res.json({ items: rows.map(mapSchedule) });
});

reportsRouter.post('/schedules', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const reportId = parseReportId(req.body?.reportId);
    const format = parseReportFormat(req.body?.format, 'csv');
    const frequency = parseReportFrequency(req.body?.frequency);
    const filters = normalizeReportFilters(membership.organizationId, filterInputFromBody(req.body));
    const name = typeof req.body?.name === 'string' && req.body.name.trim()
      ? req.body.name.trim()
      : REPORT_DEFINITION_BY_ID.get(reportId)!.name;
    const emailTo = readEmailTo(req.body?.emailTo);
    const id = randomUUID();
    const filtersJson = jsonParam(reportFiltersToJson(filters));
    const emailJson = jsonParam(emailTo);
    const nextRunAt = nextReportRunAt(frequency);

    const rows = await prisma.$queryRaw<ReportScheduleRow[]>(
      Prisma.sql`
        INSERT INTO public.rmm_report_schedule
          (
            id, organization_id, report_id, name, format, frequency, filters_jsonb,
            email_to_jsonb, email_delivery_status, is_enabled, next_run_at, created_by,
            created_at, updated_at
          )
        VALUES
          (
            ${id}, ${membership.organizationId}, ${reportId}, ${name}, ${format}, ${frequency}, ${filtersJson}::jsonb,
            ${emailJson}::jsonb, 'stubbed', true, ${nextRunAt}, ${req.jwt!.sub}, NOW(), NOW()
          )
        RETURNING *
      `
    );
    return res.status(201).json(mapSchedule(rows[0]));
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.patch('/schedules/:id', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const existingRows = await prisma.$queryRaw<ReportScheduleRow[]>(
      Prisma.sql`
        SELECT *
        FROM public.rmm_report_schedule
        WHERE organization_id = ${membership.organizationId}
          AND id = ${req.params.id}
        LIMIT 1
      `
    );
    const existing = existingRows[0];
    if (!existing) return res.status(404).json({ error: 'Report schedule not found' });

    const reportId = req.body?.reportId == null ? parseReportId(existing.report_id) : parseReportId(req.body.reportId);
    const format = req.body?.format == null ? parseReportFormat(existing.format) : parseReportFormat(req.body.format);
    const frequency = req.body?.frequency == null
      ? parseReportFrequency(existing.frequency)
      : parseReportFrequency(req.body.frequency);
    const existingFilters = filterInputFromStored(existing.filters_jsonb);
    const mergedFilters = {
      ...existingFilters,
      ...Object.fromEntries(
        Object.entries(filterInputFromBody(req.body)).filter(([, value]) => value !== undefined)
      )
    };
    const filters = normalizeReportFilters(membership.organizationId, mergedFilters);
    const name = typeof req.body?.name === 'string' && req.body.name.trim() ? req.body.name.trim() : existing.name;
    const emailTo = req.body?.emailTo === undefined ? readEmailTo(existing.email_to_jsonb) : readEmailTo(req.body.emailTo);
    const isEnabled = typeof req.body?.isEnabled === 'boolean' ? req.body.isEnabled : existing.is_enabled;
    const nextRunAt = isEnabled ? nextReportRunAt(frequency) : null;

    const rows = await prisma.$queryRaw<ReportScheduleRow[]>(
      Prisma.sql`
        UPDATE public.rmm_report_schedule
        SET
          report_id = ${reportId},
          name = ${name},
          format = ${format},
          frequency = ${frequency},
          filters_jsonb = ${jsonParam(reportFiltersToJson(filters))}::jsonb,
          email_to_jsonb = ${jsonParam(emailTo)}::jsonb,
          is_enabled = ${isEnabled},
          next_run_at = ${nextRunAt},
          updated_at = NOW()
        WHERE organization_id = ${membership.organizationId}
          AND id = ${req.params.id}
        RETURNING *
      `
    );
    return res.json(mapSchedule(rows[0]));
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.delete('/schedules/:id', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  const deleted = await prisma.$executeRaw(
    Prisma.sql`
      DELETE FROM public.rmm_report_schedule
      WHERE organization_id = ${membership.organizationId}
        AND id = ${req.params.id}
    `
  );
  if (deleted === 0) return res.status(404).json({ error: 'Report schedule not found' });
  return res.json({ deleted: true });
});

reportsRouter.get('/:reportId/export.csv', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const generated = await generateForRequest(req.params.reportId, membership.organizationId, req.query);
    return csvResponse(res, generated.reportId, rowsToCsv(generated.rows, generated.definition.columns));
  } catch (error) {
    return sendReportError(res, error);
  }
});

reportsRouter.get('/:reportId', async (req: AuthedRequest, res) => {
  const membership = await requireMembership(req, res);
  if (!membership) return;

  try {
    const generated = await generateForRequest(req.params.reportId, membership.organizationId, req.query);
    return res.json({
      definition: generated.definition,
      filters: reportFiltersToJson(generated.filters),
      items: generated.rows
    });
  } catch (error) {
    return sendReportError(res, error);
  }
});
