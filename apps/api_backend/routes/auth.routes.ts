import { Router } from 'express';
import bcrypt from 'bcrypt';
import { prisma } from '../lib/prisma';
import { requireAuth, AuthedRequest } from '../middleware/auth';
import { signMachineToken, signUserToken } from '../lib/auth';
import { env } from '../lib/env';
import { isKnownPublicExampleCredential } from '../lib/environmentPolicy';
import { createLogger } from '../lib/logger';
import { auditRequest, tryWriteAuditEvent } from '../lib/audit';
import {
  getCommunityRegistrationStatus,
  parseRegistrationInput,
  registerFirstCommunityUser,
  RegistrationClosedError,
  RegistrationInputError
} from '../lib/communityRegistration';
import {
  authGeneralRateLimit,
  authLoginRateLimit,
  authMachineTokenRateLimit,
  authRegisterRateLimit,
  authServiceTokenRateLimit
} from '../middleware/security';

const log = createLogger('api_backend::auth');

export const authRouter = Router();
authRouter.use(authGeneralRateLimit);

async function getUserMembershipForAudit(userId: string) {
  return prisma.organizationMember.findFirst({
    where: { userId },
    include: { user: { select: { id: true, email: true } } }
  });
}

authRouter.get('/registration-status', async (_req, res) => {
  try {
    return res.json(await getCommunityRegistrationStatus());
  } catch (error) {
    log.error('registration status failed', { error: String(error) });
    return res.status(500).json({ error: 'Internal server error' });
  }
});

authRouter.post('/register', authRegisterRateLimit, async (req, res) => {
  try {
    const input = parseRegistrationInput(req.body);
    const user = await registerFirstCommunityUser(input);

    // Generate token
    const token = signUserToken(user.id);

    await tryWriteAuditEvent(auditRequest(req, {
      actorType: 'user',
      userId: user.id,
      userEmail: user.email,
      actionType: 'auth.register',
      targetType: 'user',
      targetId: user.id,
      targetName: user.email,
      result: 'success'
    }));

    res.status(201).json({
      token,
      user: {
        id: user.id,
        email: user.email,
        createdAt: user.createdAt
      }
    });
  } catch (error) {
    if (error instanceof RegistrationInputError) {
      return res.status(400).json({ error: error.message });
    }
    if (error instanceof RegistrationClosedError) {
      return res.status(403).json({
        error: 'Registration is closed. Ask a Talos administrator to provision your account.'
      });
    }
    log.error('registration failed', { error: String(error) });
    return res.status(500).json({ error: 'Internal server error' });
  }
});

authRouter.post('/login', authLoginRateLimit, async (req, res) => {
  const { email, password } = req.body;

  try {
    // Find user
    const user = await prisma.user.findUnique({
      where: { email }
    });

    if (!user) {
      await tryWriteAuditEvent(auditRequest(req, {
        actorType: 'unknown',
        userEmail: typeof email === 'string' ? email : null,
        actionType: 'auth.login',
        targetType: 'user',
        targetName: typeof email === 'string' ? email : null,
        result: 'failure',
        statusCode: 401,
        errorMessage: 'Invalid credentials'
      }));
      return res.status(401).json({ error: 'Invalid credentials' });
    }

    // Check password
    const isValid = await bcrypt.compare(password, user.password);
    if (!isValid) {
      const membership = await getUserMembershipForAudit(user.id);
      await tryWriteAuditEvent(auditRequest(req, {
        organizationId: membership?.organizationId ?? null,
        actorType: 'user',
        userId: user.id,
        userEmail: user.email,
        actionType: 'auth.login',
        targetType: 'user',
        targetId: user.id,
        targetName: user.email,
        result: 'failure',
        statusCode: 401,
        errorMessage: 'Invalid credentials'
      }));
      return res.status(401).json({ error: 'Invalid credentials' });
    }

    // Generate token
    const token = signUserToken(user.id);

    const membership = await getUserMembershipForAudit(user.id);
    await tryWriteAuditEvent(auditRequest(req, {
      organizationId: membership?.organizationId ?? null,
      actorType: 'user',
      userId: user.id,
      userEmail: user.email,
      actionType: 'auth.login',
      targetType: 'user',
      targetId: user.id,
      targetName: user.email,
      result: 'success'
    }));

    res.json({
      token,
      user: {
        id: user.id,
        email: user.email,
        createdAt: user.createdAt
      }
    });
  } catch (error) {
    log.error('login failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});

authRouter.post('/machine-token', authMachineTokenRateLimit, requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt!.type !== 'user') {
    return res.status(403).json({ error: 'Only users can generate machine tokens' });
  }

  try {
    const token = signMachineToken(req.jwt!.sub);

    const membership = await getUserMembershipForAudit(req.jwt!.sub);
    await tryWriteAuditEvent(auditRequest(req, {
      organizationId: membership?.organizationId ?? null,
      actorType: 'user',
      userId: req.jwt!.sub,
      userEmail: membership?.user.email ?? null,
      actionType: 'auth.machine_token.issue',
      targetType: 'user',
      targetId: req.jwt!.sub,
      result: 'success'
    }));

    res.json({ token });
  } catch (error) {
    log.error('machine token failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});

// Service-to-service minting: AGENT_BACKEND uses SERVICE_KEY to mint a machine token scoped to an agent
authRouter.post('/service/machine-token', authServiceTokenRateLimit, async (req, res) => {
  const serviceKey = env.serviceKey?.trim();
  if (!serviceKey || isKnownPublicExampleCredential(serviceKey)) {
    return res.status(503).json({ error: 'Service minting not configured' });
  }
  const presented = req.header('x-service-key') || '';
  if (presented !== serviceKey) {
    return res.status(401).json({ error: 'Unauthorized' });
  }

  const { agentId } = req.body || {};
  if (!agentId || typeof agentId !== 'string') {
    return res.status(400).json({ error: 'agentId is required' });
  }

  try {
    const token = signMachineToken(agentId);
    return res.json({ token });
  } catch (e) {
    log.error('service machine-token failed', { error: String(e) });
    return res.status(500).json({ error: 'Internal server error' });
  }
});

/**
 * GET /auth/context - get current user org context
 */
  authRouter.get('/context', requireAuth, async (req: AuthedRequest, res) => {
    if (req.jwt!.type !== 'user') {
      return res.status(403).json({ error: 'Machine tokens cannot access context' });
    }
  
    try {
      const membership = await prisma.organizationMember.findFirst({
        where: { userId: req.jwt!.sub },
        select: {
          organizationId: true,
          role: true,
          user: {
            select: {
              email: true
            }
          }
        }
      });

    if (!membership) {
      return res.status(404).json({ error: 'No organization membership found' });
    }

      res.json({
        userId: req.jwt!.sub,
        organizationId: membership.organizationId,
        role: membership.role,
        email: membership.user.email
      });
  } catch (error) {
    log.error('context failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});

/**
 * GET /auth/profile - get current user profile
 */
authRouter.get('/profile', requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt!.type !== 'user') {
    return res.status(403).json({ error: 'Machine tokens cannot access profile' });
  }

  try {
    const user = await prisma.user.findUnique({
      where: { id: req.jwt!.sub },
      select: {
        id: true,
        email: true,
        createdAt: true
      }
    });

    if (!user) {
      return res.status(404).json({ error: 'User not found' });
    }

    res.json({
      user: {
        id: user.id,
        email: user.email,
        createdAt: user.createdAt
      }
    });
  } catch (error) {
    log.error('profile failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});

/**
 * DELETE /auth/account - delete user account
 */
authRouter.delete('/account', requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt!.type !== 'user') {
    return res.status(403).json({ error: 'Machine tokens cannot delete accounts' });
  }

  try {
    const userId = req.jwt!.sub;

    // Remove user from any org memberships first (orgs own memberships)
    await prisma.organizationMember.deleteMany({ where: { userId } });

    // Delete user
    await prisma.user.delete({
      where: { id: userId }
    });

    res.status(204).end();
  } catch (error) {
    log.error('account deletion failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});

/**
 * PATCH /auth/profile - update user profile
 */
authRouter.patch('/profile', requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt!.type !== 'user') {
    return res.status(403).json({ error: 'Machine tokens cannot update profile' });
  }

  try {
    const userId = req.jwt!.sub;
    const { email, currentPassword, newPassword } = req.body;

    // Get current user
    const user = await prisma.user.findUnique({
      where: { id: userId }
    });

    if (!user) {
      return res.status(404).json({ error: 'User not found' });
    }

    // Verify current password
    const isValid = await bcrypt.compare(currentPassword, user.password);
    if (!isValid) {
      return res.status(400).json({ error: 'Current password is incorrect' });
    }

    // Prepare update data
    const updateData: any = {};
    
    if (email && email !== user.email) {
      // Check if email is already taken
      const existingUser = await prisma.user.findUnique({
        where: { email }
      });
      if (existingUser) {
        return res.status(400).json({ error: 'Email already in use' });
      }
      updateData.email = email;
    }

    if (newPassword) {
      updateData.password = await bcrypt.hash(newPassword, 10);
    }

    // Update user
    const updatedUser = await prisma.user.update({
      where: { id: userId },
      data: updateData,
      select: {
        id: true,
        email: true,
        createdAt: true
      }
    });

    const membership = await getUserMembershipForAudit(userId);
    await tryWriteAuditEvent(auditRequest(req, {
      organizationId: membership?.organizationId ?? null,
      actorType: 'user',
      userId,
      userEmail: updatedUser.email,
      actionType: 'user.profile.update',
      targetType: 'user',
      targetId: userId,
      targetName: updatedUser.email,
      result: 'success',
      metadata: {
        emailChanged: Boolean(email && email !== user.email),
        passwordChanged: Boolean(newPassword)
      }
    }));

    res.json({
      user: updatedUser
    });
  } catch (error) {
    log.error('profile update failed', { error: String(error) });
    res.status(500).json({ error: 'Internal server error' });
  }
});
