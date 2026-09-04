CREATE TABLE "public"."rmm_emergency_update_state" (
    "singleton_key" TEXT NOT NULL,
    "active" BOOLEAN NOT NULL DEFAULT false,
    "reason" TEXT NOT NULL DEFAULT 'emergency',
    "requested_by" TEXT,
    "triggered_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "expires_at" TIMESTAMPTZ(3),
    "updated_at" TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "rmm_emergency_update_state_pkey" PRIMARY KEY ("singleton_key")
);
