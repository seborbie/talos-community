/*
  Warnings:

  - Made the column `created_at` on table `command_execution_log` required. This step will fail if there are existing NULL values in that column.
  - Made the column `created_at` on table `command_policies` required. This step will fail if there are existing NULL values in that column.
  - Made the column `updated_at` on table `command_policies` required. This step will fail if there are existing NULL values in that column.

*/
-- DropForeignKey
ALTER TABLE "command_policies" DROP CONSTRAINT "fk_customer";

-- DropForeignKey
ALTER TABLE "command_policies" DROP CONSTRAINT "fk_organization";

-- AlterTable
ALTER TABLE "command_execution_log" ALTER COLUMN "created_at" SET NOT NULL,
ALTER COLUMN "created_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "command_policies" ALTER COLUMN "created_at" SET NOT NULL,
ALTER COLUMN "created_at" SET DATA TYPE TIMESTAMP(3),
ALTER COLUMN "updated_at" SET NOT NULL,
ALTER COLUMN "updated_at" DROP DEFAULT,
ALTER COLUMN "updated_at" SET DATA TYPE TIMESTAMP(3);

-- AlterTable
ALTER TABLE "customers" ALTER COLUMN "updated_at" DROP DEFAULT;

-- AddForeignKey
ALTER TABLE "command_policies" ADD CONSTRAINT "command_policies_organization_id_fkey" FOREIGN KEY ("organization_id") REFERENCES "Organization"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "command_policies" ADD CONSTRAINT "command_policies_customer_id_fkey" FOREIGN KEY ("customer_id") REFERENCES "customers"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- RenameIndex
ALTER INDEX "idx_execution_log_agent" RENAME TO "command_execution_log_agent_id_created_at_idx";

-- RenameIndex
ALTER INDEX "idx_execution_log_org" RENAME TO "command_execution_log_organization_id_created_at_idx";

-- RenameIndex
ALTER INDEX "idx_policies_command" RENAME TO "command_policies_command_name_idx";

-- RenameIndex
ALTER INDEX "idx_policies_customer" RENAME TO "command_policies_customer_id_idx";

-- RenameIndex
ALTER INDEX "idx_policies_org" RENAME TO "command_policies_organization_id_idx";
