-- Hand-written: toasty 0.10's generator derives no foreign keys from
-- `#[belongs_to]`, so every relation the models declare is enforced
-- here by hand. Policy: rows that are pure dependents of their parent
-- — credentials (sessions, API tokens) and membership join rows —
-- go with it (ON DELETE CASCADE); containers with contents (teams,
-- procedures under an organization) block the delete (the default
-- NO ACTION): removing a container is an explicit decision, never a
-- side effect. Use-case transactions (P.9 Q10) rely on these to
-- turn a partial write into a rolled-back one.
ALTER TABLE "sessions"
    ADD CONSTRAINT "fk_sessions_account"
    FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "api_tokens"
    ADD CONSTRAINT "fk_api_tokens_account"
    FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "organization_memberships"
    ADD CONSTRAINT "fk_organization_memberships_organization"
    FOREIGN KEY ("organization_id") REFERENCES "organizations" ("id") ON DELETE CASCADE;
ALTER TABLE "organization_memberships"
    ADD CONSTRAINT "fk_organization_memberships_account"
    FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "teams"
    ADD CONSTRAINT "fk_teams_organization"
    FOREIGN KEY ("organization_id") REFERENCES "organizations" ("id");
ALTER TABLE "team_memberships"
    ADD CONSTRAINT "fk_team_memberships_team"
    FOREIGN KEY ("team_id") REFERENCES "teams" ("id") ON DELETE CASCADE;
ALTER TABLE "team_memberships"
    ADD CONSTRAINT "fk_team_memberships_account"
    FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "procedures"
    ADD CONSTRAINT "fk_procedures_organization"
    FOREIGN KEY ("organization_id") REFERENCES "organizations" ("id");
