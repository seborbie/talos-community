import { Request, Response, NextFunction } from 'express';
import { env } from '../lib/env';

export interface RmmServerRequest extends Request {
  rmmServer?: boolean;
}

export function attachRmmServerAuth(req: RmmServerRequest, _res: Response, next: NextFunction) {
  const header = req.header('x-rmm-server-key');
  if (header && env.rmmServerApiKey && header === env.rmmServerApiKey) {
    req.rmmServer = true;
  }
  next();
}

export function requireRmmServer(req: RmmServerRequest, res: Response, next: NextFunction) {
  if (!req.rmmServer) {
    return res.status(401).json({ error: 'Unauthorized' });
  }
  next();
}
