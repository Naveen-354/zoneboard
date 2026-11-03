# T007: Binance connection, read side

Phase 1 · Depends: T001, T004, T005 (T002 recommended) · Touches: `exchange/` (new), `server.rs`, `accounts.rs`

## Goal
Connect a Binance account and show its **real** balances and permissions. **No orders in this task.** Start with the Spot Testnet.

## Verify before coding, record in `docs/platform/FINDINGS-binance.md`
Everything below was gathered from secondary sources and must be confirmed against Binance's official developer docs: request signing (HMAC-SHA256, Ed25519), `recvWindow`, clock sync via `/api/v3/time`, `GET /api/v3/account` fields, request-weight headers, base URLs for live and testnet, **that the testnet has no `/sapi/*`**, `GET /sapi/v1/account/apiRestrictions` and its `enableWithdrawals` flag, and the **user-data stream change** (the old `listenKey` endpoints reportedly ended on 2026-02-20; the replacement is on the WebSocket API). If anything contradicts this spec, stop and report.

## In scope
1. Signed REST client with both key types (HMAC for system-generated keys, Ed25519 for self-generated), clock-offset correction, weight tracking from response headers, hard stop and backoff on 429/418 (never retry-loop).
2. `POST /api/accounts` for `kind: binance`, `environment: testnet|live`, credentials in the body (**write-only**, straight to the T004 vault; never stored in SQLite, never echoed). Validate by calling the account endpoint.
3. **Reject any key that can withdraw funds** (live: `enableWithdrawals` true) with: "This key can withdraw funds. Create a key with Reading (and Spot trading if you plan to trade) only, restricted to your IP." Store nothing in that case.
4. `GET /api/accounts/{id}/balances`: real data from Binance, cached at most 10 s. On failure return the error and the time of the last success. **Never fabricate or silently serve stale balances as current.**
5. `POST /api/accounts/{id}/verify` (re-check permissions) and `DELETE /api/accounts/{id}` (disconnect: vault entry deleted, audit entry written). The API shows only a `key_hint` (last 4 characters).
6. **Live environment is gated:** creating a `live` account requires `"confirm_live": true` in the request, writes an audit entry, and is **never exercised by tests or by the agent**. Only the owner tries live, by hand, with a read-only key.

## Out of scope
Orders, user-data stream, market data, deposit/withdrawal history (needs `/sapi` on a live key; separate task after this lands).

## Acceptance criteria
- A local **fake Binance server** in the tests covers: signature checked against the official documentation's example vectors, clock skew, 429 handling, withdrawal-enabled rejection, bad key, balances mapping.
- Tests prove no secret appears in logs, audit detail, error text or any API response.
- The owner can connect a **testnet** account with throwaway keys and see its balances.

## Notes
Pre-approved dependencies: `hmac`, `sha2`, `ed25519-dalek`, `subtle`, `tokio-tungstenite` (not needed yet), `rand`.
