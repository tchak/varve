CREATE TABLE "revisions" (
    "revision" TEXT NOT NULL,
    "schema" BYTEA NOT NULL,
    PRIMARY KEY ("revision")
);
CREATE TABLE "surfaces" (
    "hash" TEXT NOT NULL,
    "revision" TEXT NOT NULL,
    "body" BYTEA NOT NULL,
    PRIMARY KEY ("hash")
);
CREATE TABLE "publications" (
    "lineage" TEXT NOT NULL,
    "index" BIGINT NOT NULL,
    "revision" TEXT NOT NULL,
    "body" BYTEA NOT NULL,
    PRIMARY KEY ("lineage", "index")
);
-- Hand-written (the platform-core db.rs rule: the generator derives
-- no foreign keys). Kernel rows never delete — hidden never deletes
-- (DESIGN §2.4) — so both constraints keep the default NO ACTION;
-- they exist to make "an event names an object that is not there"
-- and "a surface names a revision that is not there" impossible at
-- the storage layer, which the loaders would otherwise only catch at
-- first read.
ALTER TABLE "publications"
    ADD CONSTRAINT "fk_publications_revision"
    FOREIGN KEY ("revision") REFERENCES "revisions" ("revision");
ALTER TABLE "surfaces"
    ADD CONSTRAINT "fk_surfaces_revision"
    FOREIGN KEY ("revision") REFERENCES "revisions" ("revision");
