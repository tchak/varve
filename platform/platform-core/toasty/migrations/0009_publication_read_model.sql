ALTER TABLE "procedures" ADD COLUMN "latest_revision" TEXT;
ALTER TABLE "procedure_events" ADD COLUMN "facts" BYTEA;
