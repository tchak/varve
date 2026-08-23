-- Revision drafts (design/platform.md P.4): a nullable embedded
-- object deferred out of the catalog SELECT. `revision_draft` is
-- toasty's presence column for the Option<embed> (NULL = no draft);
-- the `#[version]` counter starts at 1, which the generator does not
-- default — hand-added so existing rows survive the ALTER.
ALTER TABLE "procedures" ADD COLUMN "revision_draft_base" TEXT;
ALTER TABLE "procedures" ADD COLUMN "version" BIGINT NOT NULL DEFAULT 1;
ALTER TABLE "procedures" ADD COLUMN "revision_draft" BOOLEAN;
ALTER TABLE "procedures" ADD COLUMN "revision_draft_schema" BYTEA;
