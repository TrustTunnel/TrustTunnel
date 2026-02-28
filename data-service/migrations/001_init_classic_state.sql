CREATE TABLE IF NOT EXISTS classic_state_accounts (
    id BIGSERIAL PRIMARY KEY,
    lk_username TEXT NOT NULL,
    lk_password TEXT NOT NULL,
    legacy_username TEXT,
    legacy_password TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
