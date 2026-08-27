CREATE TABLE "record_entries" (
    "record" TEXT NOT NULL,
    "seq" BIGINT NOT NULL,
    "envelope" BYTEA NOT NULL,
    "content" BYTEA NOT NULL,
    PRIMARY KEY ("record", "seq")
);
