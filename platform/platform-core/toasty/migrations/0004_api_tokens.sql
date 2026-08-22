CREATE TABLE "api_tokens" (
    "id" UUID NOT NULL,
    "token_hash" TEXT NOT NULL,
    "account_id" UUID NOT NULL,
    "name" TEXT NOT NULL,
    "prefix" TEXT NOT NULL,
    "created_at" TIMESTAMPTZ(6) NOT NULL,
    "expires_at" TIMESTAMPTZ(6) NOT NULL,
    PRIMARY KEY ("id")
);
CREATE UNIQUE INDEX "index_api_tokens_by_token_hash" ON "api_tokens" ("token_hash");
CREATE INDEX "index_api_tokens_by_account_id" ON "api_tokens" ("account_id");
