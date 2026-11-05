# Binance findings (T007)

Recorded 2026-11-04 while building `src-tauri/src/exchange/binance.rs`. The build sandbox could not reach
binance.com (the egress proxy refuses it), so items are marked by how they were checked.

| Item | Status | How |
|---|---|---|
| Signature: hex HMAC-SHA256 of the full query string with the secret key | **VERIFIED** | The official documentation's example (secret `NhqP…Tj0j`, `symbol=LTCBTC…timestamp=1499827319559`) reproduces the published signature `c8db5682…6b71` exactly; it is a unit test (`signature_matches_the_official_documentation_example`). |
| `timestamp` + `recvWindow` (5000 ms here), `X-MBX-APIKEY` header, `-1021` = outside recvWindow | From the official docs, not re-fetched | Implemented; the fake server enforces it; clock skew of 3 s is tested. |
| `GET /api/v3/time` → `serverTime` (ms) | From the official docs, not re-fetched | Used for the clock offset. |
| `GET /api/v3/account` fields (`canTrade`, `accountType`, `permissions`, `balances[asset, free, locked]`, `omitZeroBalances`) | From the official docs, not re-fetched | Balances are passed through as Binance's own decimal strings. |
| `X-MBX-USED-WEIGHT-1M` header | From the official docs, not re-fetched | Tracked and shown. |
| 429 rate limit, 418 IP ban, `Retry-After` | From the official docs, not re-fetched | Hard pause per account; no retry loop (tested: the second call does not reach the exchange). |
| Live `https://api.binance.com`, testnet `https://testnet.binance.vision`, no `/sapi` on the testnet | From the official docs, not re-fetched | Withdrawal check runs on live only. |
| `GET /sapi/v1/account/apiRestrictions` → `enableWithdrawals` | From the official docs, not re-fetched | A key with withdrawals enabled is refused and nothing is stored (tested). |
| `POST /api/v3/order` (`newClientOrderId`, `newOrderRespType=FULL`), `GET /api/v3/order` by `origClientOrderId`, `-2013` = no such order, `exchangeInfo` `LOT_SIZE` / `NOTIONAL` filters | From the official docs, not re-fetched | Used by the testnet order path (Phase 3); tested only against the fake server. |
| User-data stream changes | **NOT CHECKED** | Not used: fills are read from the order reply and by order lookup. |
| Ed25519 keys | **NOT SUPPORTED** | HMAC (system-generated) keys only. |

**Owner, before relying on this:** connect a Spot Testnet account with throwaway keys and confirm balances
appear; that is the first contact with the real API.
