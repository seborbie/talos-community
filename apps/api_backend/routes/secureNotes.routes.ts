import { Router } from "express";
import { env } from "../lib/env";
import {
  checkSecureNoteForUser,
  isSecretHandle,
  isSecureNoteCode,
  resolveGeneratedSecretForRunner,
  revealSecureNoteForUser,
} from "../lib/secureNotes";
import { requireAuth, type AuthedRequest } from "../middleware/auth";

export const secureNotesRouter = Router();

function secureNoteStatus(status: string): number {
  switch (status) {
    case "available":
    case "revealed":
      return 200;
    case "unauthorized":
      return 403;
    case "expired":
    case "viewed":
    case "not_found":
      return 404;
    default:
      return 500;
  }
}

function requireServiceKey(req: any, res: any): boolean {
  const expected = (env.aiRunnerServiceKey || env.serviceKey || "").trim();
  if (!expected) {
    res.status(503).json({ error: "Service key is not configured" });
    return false;
  }
  const provided = String(req.header("x-service-key") || "").trim();
  if (provided !== expected) {
    res.status(401).json({ error: "Invalid service key" });
    return false;
  }
  return true;
}

secureNotesRouter.get("/:code/check", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }
  const code = String(req.params.code || "").trim().toLowerCase();
  if (!isSecureNoteCode(code)) {
    return res.status(400).json({ status: "invalid", error: "Invalid secure note code" });
  }
  try {
    const result = await checkSecureNoteForUser(code, req.jwt.sub);
    return res.status(secureNoteStatus(result.status)).json(result);
  } catch (error) {
    return res.status(500).json({ status: "error", error: error instanceof Error ? error.message : String(error) });
  }
});

secureNotesRouter.post("/:code/reveal", requireAuth, async (req: AuthedRequest, res) => {
  if (req.jwt?.type !== "user") {
    return res.status(403).json({ error: "Machine tokens are not allowed" });
  }
  const code = String(req.params.code || "").trim().toLowerCase();
  if (!isSecureNoteCode(code)) {
    return res.status(400).json({ status: "invalid", error: "Invalid secure note code" });
  }
  try {
    const result = await revealSecureNoteForUser(code, req.jwt.sub);
    return res.status(secureNoteStatus(result.status)).json(result);
  } catch (error) {
    return res.status(500).json({ status: "error", error: error instanceof Error ? error.message : String(error) });
  }
});

secureNotesRouter.post("/internal/runner-secrets/:handle/reveal", async (req, res) => {
  if (!requireServiceKey(req, res)) {
    return;
  }
  const secretHandle = String(req.params.handle || "").trim();
  if (!isSecretHandle(secretHandle)) {
    return res.status(400).json({ error: "Invalid secret handle" });
  }
  const jobId = typeof req.body?.jobId === "string" ? req.body.jobId.trim() : "";
  const runnerId = typeof req.body?.runnerId === "string" ? req.body.runnerId.trim() : null;
  const leaseId = typeof req.body?.leaseId === "string" ? req.body.leaseId.trim() : null;
  if (!jobId || !leaseId) {
    return res.status(400).json({ error: "jobId and leaseId are required" });
  }
  try {
    const result = await resolveGeneratedSecretForRunner({ jobId, runnerId, leaseId, secretHandle });
    return res.json(result);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes("lease")) return res.status(409).json({ error: message });
    if (message.includes("not found")) return res.status(404).json({ error: message });
    return res.status(400).json({ error: message });
  }
});
