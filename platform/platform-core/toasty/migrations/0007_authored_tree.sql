-- The authored tree replaces the schema as the draft's single source
-- (design/platform.md P.4, settled 2026-08-24): the stored value is
-- platform JSON carrying sections, notes and audiences, not the
-- kernel's wire-canonical schema bytes. Drop + add rather than
-- rename, deliberately: old bytes are wire-schema format the tree
-- decoder refuses, so pre-publish dev drafts reset to "none" instead
-- of surfacing as corruption.
ALTER TABLE "procedures" DROP COLUMN "revision_draft_schema";
ALTER TABLE "procedures" ADD COLUMN "revision_draft_tree" BYTEA;
