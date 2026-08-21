CREATE TABLE "procedures" (
    "id" UUID NOT NULL,
    "organization_id" UUID NOT NULL,
    "title" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    "updated_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE INDEX "index_procedures_by_organization_id" ON "procedures" ("organization_id");
CREATE TABLE "teams" (
    "id" UUID NOT NULL,
    "organization_id" UUID NOT NULL,
    "name" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    "updated_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE INDEX "index_teams_by_organization_id" ON "teams" ("organization_id");
CREATE TABLE "team_memberships" (
    "team_id" UUID NOT NULL,
    "account_id" UUID NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("team_id", "account_id")
);
CREATE INDEX "index_team_memberships_by_team_id" ON "team_memberships" ("team_id");
CREATE INDEX "index_team_memberships_by_account_id" ON "team_memberships" ("account_id");
CREATE TABLE "organization_memberships" (
    "organization_id" UUID NOT NULL,
    "account_id" UUID NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("organization_id", "account_id")
);
CREATE INDEX "index_organization_memberships_by_organization_id" ON "organization_memberships" ("organization_id");
CREATE INDEX "index_organization_memberships_by_account_id" ON "organization_memberships" ("account_id");
CREATE TABLE "organizations" (
    "id" UUID NOT NULL,
    "slug" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    "updated_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE UNIQUE INDEX "index_organizations_by_slug" ON "organizations" ("slug");
