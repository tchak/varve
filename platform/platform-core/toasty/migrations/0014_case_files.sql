CREATE TYPE "case_file_state_value" AS ENUM ('draft');
CREATE TYPE "case_file_event_kind" AS ENUM ('created');
CREATE TABLE "case_file_events" (
    "id" UUID NOT NULL,
    "case_file_id" UUID NOT NULL,
    "actor_account_id" UUID,
    "kind" case_file_event_kind NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE INDEX "index_case_file_events_by_case_file_id" ON "case_file_events" ("case_file_id");
CREATE TABLE "case_files" (
    "id" UUID NOT NULL,
    "procedure_id" UUID NOT NULL,
    "state" case_file_state_value NOT NULL,
    "state_since" TIMESTAMPTZ(6),
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    "updated_at" TIMESTAMPTZ(6) NOT NULL,
    "version" BIGINT NOT NULL,
    PRIMARY KEY ("id")
);
CREATE INDEX "index_case_files_by_procedure_id" ON "case_files" ("procedure_id");
CREATE INDEX "index_case_files_by_state" ON "case_files" ("state");
CREATE TABLE "case_file_participants" (
    "case_file_id" UUID NOT NULL,
    "account_id" UUID NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("case_file_id", "account_id")
);
CREATE INDEX "index_case_file_participants_by_account_id" ON "case_file_participants" ("account_id");
-- Hand-written (db.rs: the generator derives no foreign keys), on
-- 0005's policy. A procedure with case files blocks its delete (the
-- default NO ACTION): deleting a procedure only exists for
-- never-published drafts (G.2 rule 8), which cannot have case files.
-- Participant join rows and events are pure dependents and go with
-- the case file — the P.4 point of the separate events table:
-- "erase the case file" is a clean cascade (DESIGN §2.10).
-- `actor_account_id` stays unconstrained (attribution, not a
-- declared relation — the 0008 argument).
ALTER TABLE "case_files"
    ADD CONSTRAINT "fk_case_files_procedure"
    FOREIGN KEY ("procedure_id") REFERENCES "procedures" ("id");
ALTER TABLE "case_file_participants"
    ADD CONSTRAINT "fk_case_file_participants_case_file"
    FOREIGN KEY ("case_file_id") REFERENCES "case_files" ("id") ON DELETE CASCADE;
ALTER TABLE "case_file_participants"
    ADD CONSTRAINT "fk_case_file_participants_account"
    FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "case_file_events"
    ADD CONSTRAINT "fk_case_file_events_case_file"
    FOREIGN KEY ("case_file_id") REFERENCES "case_files" ("id") ON DELETE CASCADE;
