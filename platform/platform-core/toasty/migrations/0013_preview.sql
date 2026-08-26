-- G.12 (the fillable preview): the scratch value bag beside the
-- draft — its own column, so filling never forks the virtual draft.
ALTER TABLE "procedures" ADD COLUMN "preview" BYTEA;
