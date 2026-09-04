import jwt, { SignOptions } from 'jsonwebtoken';
import { env } from './env';

interface JwtPayload {
  sub: string;       // userId or machineId
  type: 'user' | 'machine';
}

export function signUserToken(userId: string) {
  return jwt.sign({ sub: userId, type: 'user' } as JwtPayload, env.jwtSecret, {
    expiresIn: env.tokenTtl
  } as SignOptions);
}

export function signMachineToken(agentId: string) {
  return jwt.sign({ sub: agentId, type: 'machine' } as JwtPayload, env.jwtSecret, {
    expiresIn: env.machineTtl
  } as SignOptions);
}

export function verifyToken(token: string): JwtPayload {
  return jwt.verify(token, env.jwtSecret) as JwtPayload;
}
