import express from 'express';
import rateLimit from 'express-rate-limit';

const jsonErrorMessage = 'Too many requests, please try again later.';

function limitHandler(message: string) {
  return (_req: express.Request, res: express.Response) => {
    res.status(429).json({ error: message });
  };
}

export const authLoginRateLimit = rateLimit({
  windowMs: 10 * 60 * 1000,
  max: 20,
  standardHeaders: true,
  legacyHeaders: false,
  skipSuccessfulRequests: true,
  handler: limitHandler('Too many login attempts. Please try again in 10 minutes.')
});

export const authRegisterRateLimit = rateLimit({
  windowMs: 60 * 60 * 1000,
  max: 20,
  standardHeaders: true,
  legacyHeaders: false,
  skipSuccessfulRequests: true,
  handler: limitHandler('Too many registration attempts. Please try again in 1 hour.')
});

export const authMachineTokenRateLimit = rateLimit({
  windowMs: 15 * 60 * 1000,
  max: 80,
  standardHeaders: true,
  legacyHeaders: false,
  handler: limitHandler('Too many token minting requests. Please try again later.')
});

export const authServiceTokenRateLimit = rateLimit({
  windowMs: 15 * 60 * 1000,
  max: 240,
  standardHeaders: true,
  legacyHeaders: false,
  handler: limitHandler('Too many service token requests. Please try again later.')
});

export const authGeneralRateLimit = rateLimit({
  windowMs: 15 * 60 * 1000,
  max: 400,
  standardHeaders: true,
  legacyHeaders: false,
  handler: limitHandler(jsonErrorMessage)
});

export const authJsonParser = express.json({ limit: '32kb' });
export const standardJsonParser = express.json({ limit: '128kb' });
export const installersJsonParser = express.json({ limit: '128kb' });
export const rmmJsonParser = express.json({ limit: '12mb' });
export const rmmTelemetryJsonParser = express.json({ limit: '12mb' });

const commandCenterAiRunnerArtifactJsonParser = express.json({
  limit: process.env.COMMAND_CENTER_AI_RUNNER_ARTIFACT_JSON_LIMIT || '12mb'
});

function isCommandCenterAiRunnerArtifactCallback(req: express.Request): boolean {
  if (req.method !== 'POST') return false;
  const path = (req.originalUrl || req.url).split('?')[0] || '';
  return /^\/command-center\/internal\/ai-runner\/jobs\/[^/]+\/artifacts$/.test(path);
}

function getCommandCenterServiceKey(): string {
  return (process.env.TALOS_AI_RUNNER_SERVICE_KEY || process.env.SERVICE_KEY || '').trim();
}

export function commandCenterJsonParser(
  req: express.Request,
  res: express.Response,
  next: express.NextFunction
) {
  if (!isCommandCenterAiRunnerArtifactCallback(req)) {
    return standardJsonParser(req, res, next);
  }

  const expectedServiceKey = getCommandCenterServiceKey();
  if (!expectedServiceKey) {
    return res.status(503).json({ error: 'SERVICE_KEY is not configured' });
  }
  if ((req.get('x-service-key') || '').trim() !== expectedServiceKey) {
    return res.status(401).json({ error: 'unauthorized' });
  }

  return commandCenterAiRunnerArtifactJsonParser(req, res, next);
}
