import express from 'express';
import cors from 'cors';
import dotenv from 'dotenv';

// Load environment variables
dotenv.config();

import { log } from './lib/logger';
import { env } from './lib/env';
import { authRouter } from './routes/auth.routes';
import { orgsRouter } from './routes/orgs.routes';
import { customersRouter } from './routes/customers.routes';
import { sitesRouter } from './routes/sites.routes';
import { policiesRouter } from './routes/policies.routes';
import { reportsRouter } from './routes/reports.routes';
import { rmmRouter } from './routes/rmm.routes';
import { rmmTelemetryRouter } from './routes/rmmTelemetry.routes';
import { installersRouter } from './routes/installers.routes';
import { updatesRouter } from './routes/updates.routes';
import { auditRouter } from './routes/audit.routes';
import { patchesRouter } from './routes/patches.routes';
import { featureUpgradesRouter } from './routes/featureUpgrades.routes';
import { commandCenterRouter } from './routes/commandCenter.routes';
import { secureNotesRouter } from './routes/secureNotes.routes';
import { startAiRunnerLeaseReconciler } from './lib/commandCenterAiRunner';
import {
  authJsonParser,
  commandCenterJsonParser,
  installersJsonParser,
  rmmJsonParser,
  rmmTelemetryJsonParser,
  standardJsonParser
} from './middleware/security';
 
const app = express();
app.disable('x-powered-by');
app.set('trust proxy', env.apiTrustedProxies);

const localDevOrigins = [
  'http://localhost:3000',
  'http://127.0.0.1:3000'
];

const trustedViewerOrigins = [
  'http://tauri.localhost',
  'https://tauri.localhost',
  'tauri://localhost'
];

function isPrivateIpv4(hostname: string): boolean {
  return (
    /^10\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(hostname) ||
    /^192\.168\.\d{1,3}\.\d{1,3}$/.test(hostname) ||
    /^172\.(1[6-9]|2\d|3[0-1])\.\d{1,3}\.\d{1,3}$/.test(hostname)
  );
}

function isAllowedDevOrigin(origin: string): boolean {
  if (process.env.NODE_ENV === 'production') {
    return false;
  }

  try {
    const { protocol, hostname } = new URL(origin);
    if (protocol !== 'http:' && protocol !== 'https:') {
      return false;
    }
    return (
      hostname === 'localhost' ||
      hostname === '127.0.0.1' ||
      isPrivateIpv4(hostname)
    );
  } catch {
    return false;
  }
}

function parseAllowedOrigins(): string[] {
  const raw = process.env.CORS_ALLOWED_ORIGINS || process.env.FRONTEND_URL || '';
  const origins = new Set(
    raw
      .split(',')
      .map((value) => value.trim())
      .filter((value) => value.length > 0)
  );

  for (const origin of trustedViewerOrigins) {
    origins.add(origin);
  }

  if (process.env.NODE_ENV !== 'production') {
    for (const origin of localDevOrigins) {
      origins.add(origin);
    }
  }

  return [...origins];
}

const allowedOrigins = parseAllowedOrigins();
export function isCorsOriginAllowed(origin: string | undefined): boolean {
  if (!origin) {
    return true;
  }
  return allowedOrigins.length === 0 || allowedOrigins.includes(origin) || isAllowedDevOrigin(origin);
}

app.use(
  cors({
    origin(origin, callback) {
      if (isCorsOriginAllowed(origin)) {
        return callback(null, true);
      }
      return callback(new Error(`Origin '${origin}' is not allowed by CORS`));
    },
    methods: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS', 'HEAD'],
    allowedHeaders: ['Authorization', 'Content-Type', 'X-Requested-With'],
    exposedHeaders: [
      'Content-Disposition',
      'X-Installer-Filename',
      'X-Talos-Manifest-Signature',
      'X-Talos-Manifest-Key-Id'
    ],
    credentials: false,
    maxAge: 86400
  })
);
app.use((req, res, next) => {
  res.setHeader('X-Content-Type-Options', 'nosniff');
  res.setHeader('Referrer-Policy', 'no-referrer');
  res.setHeader('X-Frame-Options', 'DENY');
  res.setHeader('Cross-Origin-Resource-Policy', 'cross-origin');
  res.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
  next();
});

const enableRequestLogging = process.env.NODE_ENV !== 'production';
if (enableRequestLogging) {
  // Request logging middleware (dev-only)
  app.use((req, res, next) => {
    const start = Date.now();

    log.info(`${req.method} ${req.path}`);

    res.on('finish', () => {
      const duration = Date.now() - start;
      log.info(`${req.method} ${req.path}`, { status: res.statusCode, duration_ms: duration });
    });

    next();
  });
}

app.use('/auth', authJsonParser, authRouter);
app.use('/orgs', standardJsonParser, orgsRouter);
app.use('/customers', standardJsonParser, customersRouter);
app.use('/sites', standardJsonParser, sitesRouter);
app.use('/policies', standardJsonParser, policiesRouter);
app.use('/rmm/installers', installersJsonParser, installersRouter);
app.use('/rmm/updates', updatesRouter);
app.use('/rmm/patches', standardJsonParser, patchesRouter);
app.use('/rmm/feature-upgrades', standardJsonParser, featureUpgradesRouter);
app.use('/rmm/reports', standardJsonParser, reportsRouter);
app.use('/rmm', rmmJsonParser, rmmRouter);
app.use('/audit', standardJsonParser, auditRouter);
app.use('/rmm/telemetry', rmmTelemetryJsonParser, rmmTelemetryRouter);
app.use('/command-center', commandCenterJsonParser, commandCenterRouter);
app.use('/secure-notes', standardJsonParser, secureNotesRouter);

app.get('/', (_, res) => res.send('API up'));

// Global error handling middleware (must be last)
app.use((err: any, req: express.Request, res: express.Response, next: express.NextFunction) => {
  log.error('request error', { error: err?.message ?? String(err) });
  
  // Handle different error types
  if (err.name === 'PrismaClientKnownRequestError') {
    // Prisma-specific errors
    if (err.code === 'P2002') {
      return res.status(409).json({ error: 'Duplicate entry' });
    }
    if (err.code === 'P2025') {
      return res.status(404).json({ error: 'Record not found' });
    }
  }
  
  if (err.name === 'ValidationError') {
    return res.status(400).json({ error: 'Validation error', details: err.message });
  }

  if (err.type === 'entity.too.large' || err.status === 413) {
    return res.status(413).json({ error: 'Request body too large' });
  }
  
  // Default error response
  res.status(err.status || 500).json({
    error: err.message || 'Internal server error',
    ...(process.env.NODE_ENV === 'development' && { stack: err.stack })
  });
});

const PORT = parseInt(process.env.API_PORT || process.env.PORT || '3001', 10);
const HOST = process.env.API_BIND_HOST?.trim() || '127.0.0.1';
export function startServer(port = PORT, host = HOST) {
  startAiRunnerLeaseReconciler();
  return app.listen(port, host, () => log.info(`listening on ${host}:${port}`));
}

if (require.main === module) {
  startServer();
}

export { app };

// Handle unhandled promise rejections
process.on('unhandledRejection', (reason, promise) => {
  log.error('unhandled rejection', { reason: String(reason), promise: String(promise) });
});

// Handle uncaught exceptions (last resort)
process.on('uncaughtException', (error) => {
  log.error('uncaught exception', { error: error?.message ?? String(error) });
});
