import { Request, Response, NextFunction } from 'express';
import { verifyToken } from '../lib/auth';

export interface AuthedRequest extends Request {
  jwt?: { sub: string; type: 'user' | 'machine' };
}

export function requireAuth(req: AuthedRequest, res: Response, next: NextFunction) {
  const header = req.header('authorization') || '';
  const token = header.startsWith('Bearer ') ? header.slice(7) : null;
  if (!token) return res.status(401).json({ error: 'Missing Bearer token' });

  try {
    req.jwt = verifyToken(token);
    next();
  } catch {
    res.status(401).json({ error: 'Invalid or expired token' });
  }
}
