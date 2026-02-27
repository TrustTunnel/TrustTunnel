# Security

- **Scope:** TLS/auth hardening and operational risk notes.
- **Applies to:** Classic (active), Modified (planned contract)
- **Last updated:** 2026-02-27

## 1. TLS requirements

- Use valid certificate chain for every `hostname`/`allowed_sni` in `hosts.toml`.
- Protect private keys with strict file permissions and read-only mounts.
- Rotate certificates and keys regularly; update `hosts.toml` references atomically.
- Prefer TLS passthrough through ingress/LB to keep endpoint-controlled certificate logic.

## 2. Authentication hardening

Classic:
- issue high-entropy static passwords;
- rotate credentials in LK and synchronize `credentials.toml` updates safely.

JWT (for jwt/mixed mode):
- restrict accepted algorithm to one intended mode (`RS256` or `HS256`);
- enforce `issuer`/`audience` where possible;
- keep low TTL and bounded `leeway_seconds`;
- if `device_id_claim` used, enforce strict mapping with LK device inventory.

## 3. Runtime hardening

Recommended production posture:
- run as non-root user (when host networking model allows it);
- drop Linux capabilities not required;
- set read-only root filesystem and explicit writable dirs only if needed;
- define CPU/memory/pid/file-descriptor limits;
- restrict metrics endpoint to private network or localhost.

## 4. JWT key/secret handling

- For RS256: mount public key file read-only (`public_key_path`).
- For HS256: keep secret in env var referenced by `hmac_secret_env`; do not store literal secret in config.
- Rotate keys/secrets with overlap window and coordinated LK token issuance switch.

## 5. Known limitations / risks in this fork

- Modified mode (IP pool quality selection) is not active in current release.
- Misconfigured SNI/cert mapping causes hard TLS failures before auth-level diagnostics.
- If credentials mode is used without credentials on public bind, deployment is insecure (explicitly blocked/warned by validation/runtime checks).

## 6. Operational constraints for LK integration

- Agent-side LK HTTP client is bounded to **2s connect timeout** and **5s total timeout** per request.
- Constraint applies to all LK operations:
  - `GET /internal/vpn/classic/accounts`
  - `POST /internal/vpn/classic/sync-report`
  - `POST /internal/nodes/heartbeat`
- Sync retry policy remains exponential (`1s`, `2s`, `4s`) with 3 retries to avoid breaching the update objective (`<=30s`) during transient errors while still converging quickly when LK is healthy.

