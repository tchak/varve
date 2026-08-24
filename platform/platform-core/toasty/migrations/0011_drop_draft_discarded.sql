-- Hand-written: toasty's generator refuses enum-variant removal, and
-- Postgres has no DROP VALUE — the type is recreated without
-- `draft_discarded` (P.4, amended 2026-08-24: discarding the working
-- buffer is authoring workflow, the same altitude as the autosaves
-- the log deliberately omits; the trail records lifecycle facts).
-- Existing rows of that kind exist only in dev/test databases —
-- nothing is deployed — and go with the value.
DELETE FROM "procedure_events" WHERE "kind" = 'draft_discarded';
CREATE TYPE "procedure_event_kind_next" AS ENUM ('created', 'published', 'closed', 'reopened');
ALTER TABLE "procedure_events"
    ALTER COLUMN "kind" TYPE procedure_event_kind_next
    USING ("kind"::text::procedure_event_kind_next);
DROP TYPE "procedure_event_kind";
ALTER TYPE "procedure_event_kind_next" RENAME TO "procedure_event_kind";
