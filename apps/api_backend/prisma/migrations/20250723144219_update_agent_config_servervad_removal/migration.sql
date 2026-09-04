/*
  Warnings:

  - You are about to drop the column `temperature` on the `Agent` table. All the data in the column will be lost.
  - You are about to drop the column `turnDetectionType` on the `Agent` table. All the data in the column will be lost.

*/
-- AlterTable
ALTER TABLE "Agent" DROP COLUMN "temperature",
DROP COLUMN "turnDetectionType";
