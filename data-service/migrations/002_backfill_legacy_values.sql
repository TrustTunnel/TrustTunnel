UPDATE classic_state_accounts
SET
    legacy_username = COALESCE(legacy_username, lk_username),
    legacy_password = COALESCE(legacy_password, lk_password),
    updated_at = NOW()
WHERE
    (legacy_username IS NULL OR legacy_password IS NULL)
    AND lk_username IS NOT NULL
    AND lk_password IS NOT NULL;
