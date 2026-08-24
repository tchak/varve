CREATE TYPE "procedure_state_value" AS ENUM ('draft', 'published', 'closed');
CREATE TYPE "procedure_event_kind" AS ENUM ('created', 'published', 'closed', 'reopened', 'draft_discarded');
-- Hand-edited after generation: the generator emits NOT NULL with no
-- default, which existing catalog rows would refuse; every procedure
-- so far is unpublished, so `draft` is the truth, not a guess.
ALTER TABLE "procedures" ADD COLUMN "state" procedure_state_value NOT NULL DEFAULT 'draft';
ALTER TABLE "procedures" ADD COLUMN "state_since" TIMESTAMPTZ(6);
CREATE INDEX "index_procedures_by_state" ON "procedures" ("state");
CREATE TABLE "procedure_events" (
    "id" UUID NOT NULL,
    "procedure_id" UUID NOT NULL,
    "actor_account_id" UUID,
    "kind" procedure_event_kind NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE INDEX "index_procedure_events_by_procedure_id" ON "procedure_events" ("procedure_id");
-- Hand-written (db.rs: the generator derives no foreign keys).
-- Events are pure dependents of their procedure and go with it —
-- deleting a procedure only exists for never-published drafts (G.2
-- rule 8), whose audit trail ends with them. `actor_account_id` is
-- deliberately unconstrained: it is attribution, not a declared
-- relation, and account-deletion semantics are not this table's to
-- decide.
ALTER TABLE "procedure_events"
    ADD CONSTRAINT "fk_procedure_events_procedure"
    FOREIGN KEY ("procedure_id") REFERENCES "procedures" ("id") ON DELETE CASCADE;
