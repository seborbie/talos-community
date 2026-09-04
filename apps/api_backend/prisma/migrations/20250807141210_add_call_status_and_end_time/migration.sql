-- AlterTable
ALTER TABLE "Call" ADD COLUMN     "endedAt" TIMESTAMP(3),
ADD COLUMN     "status" TEXT NOT NULL DEFAULT 'active';
