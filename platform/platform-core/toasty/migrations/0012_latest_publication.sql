-- §2.13 decision 9: the lineage head is a publication content
-- address (revision + surface set + parents), not a revision id —
-- two surface-only publications from one revision must stay
-- distinct as fork anchors.
ALTER TABLE "procedures" RENAME COLUMN "latest_revision" TO "latest_publication";
