-- Hand-edited after generation: the generator emits NOT NULL with no
-- default, which existing case-file rows would refuse. Every row so
-- far predates the record log, so the backfill mints each one a
-- fresh random id — uniqueness is what the column promises; the
-- UUID v7 time-ordering of new mints is a convenience, not a
-- storage invariant.
ALTER TABLE "case_files" ADD COLUMN "record_id" UUID;
UPDATE "case_files" SET "record_id" = gen_random_uuid() WHERE "record_id" IS NULL;
ALTER TABLE "case_files" ALTER COLUMN "record_id" SET NOT NULL;
CREATE UNIQUE INDEX "index_case_files_by_record_id" ON "case_files" ("record_id");
