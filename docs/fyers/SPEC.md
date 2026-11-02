# Live NSE data via Fyers: spec

Status: ready to build. Facts below were checked against Fyers' official docs on **2026-11-02**. Anything marked **UNVERIFIED** must be confirmed (section 3) before you rely on it. Read `AGENTS.md` first: its rules apply here.

## 1. Goal and decisions

**Goal.** Once the owner connects a Fyers account, the **India NSE** market goes live and feels like the crypto one: board prices tick, the open chart's forming candle moves trade by trade, zones and scores recompute. When Fyers is not configured, not connected, expired, or the market is closed, NSE falls back to today's Yahoo behaviour (15 min delayed) and **says so honestly**. Live and delayed data are never mixed silently.

**Decisions already made** (don't re-litigate; raise a concern if one is wrong):

| # | Decision | Why |
|---|---|---|
| D1 | The **Rust server owns everything Fyers**: secrets, token, REST, WebSocket. The browser only talks to our own `/api/nse/*`. | A broker token must never reach a webview or page. |
| D2 | The Python backend is **not** extended. It stays the delayed reference. | One live backend. |
| D3 | Target the **free Standard plan** and stay inside its limits (section 6). | The owner chose Fyers because data is free. |
| D4 | Live prices come from the **data WebSocket (push)**. REST is only for history, one-shot quotes, and the symbol master. **No REST polling loops.** | Fyers' own troubleshooting guide names polling as a cause of 429 blocks. |
| D5 | **Daily login only.** No PIN, no refresh-token storage in v1. | The access token expires every day at 06:30; storing a PIN is more risk than a daily click is worth. |
| D6 | **Read-only.** The Fyers app is created with **Market Data permission only**. No order, position, or holdings endpoints. | Docs: request trading permissions only if the app places orders. |
| D7 | **Fail closed on rate limits.** | Exceeding the per-minute limit more than 3 times in a day blocks the account for the rest of the day. |
| D8 | The browser keeps one generic live-market code path. The server sends **Binance-shaped events** (as `static/js/sim.js` already does), so the existing crypto code is reused instead of copied. | Less code, one behaviour. |
| D9 | The WebSocket client is built **Rust-native by default**, behind a decision gate (stage S4). If the spike says it is not tractable, the fallback is the **official SDK as a sidecar**, but **the owner decides**. | The wire protocol is not documented (see U3). |

## 2. Verified facts

Official docs (Fyers API v3). Paths are under `https://fyers.in/api-docs/v3/`.

**Plan and limits** (`getting-started/plans-and-limits/`)
- Standard plan is **free**. Prime is a paid subscription.
- Data APIs, Standard: **5 req/s, 50 req/min, 5,000 req/day**. (Prime: 10/s, 500/min, 500,000/day.)
- HTTP **429** when any limit is exceeded. **More than 3 per-minute violations in a day blocks the user for the rest of that day.**
- Data WebSocket: **100 symbols (Standard)**, 5,000 (Prime). The limit is **per user across all connections**. **Over-limit subscriptions do not establish the connection.**
- Quotes API: 50 symbols per request (Standard).
- History depth (Standard): daily candles for 5 years rolling; intraday for 1 year rolling; no seconds data.

**Authentication** (`api/authentication/...`)
- Individual app (own account). Create it in the API Dashboard, with a redirect URL you control.
- Flow: `GET /api/v3/generate-authcode` with `client_id`, `redirect_uri`, `response_type=code`, random `state` -> the user logs in -> Fyers redirects to `redirect_uri` with `auth_code` and `state` -> **verify `state`** -> `POST /api/v3/validate-authcode` with JSON `{"grant_type":"authorization_code","appIdHash":"<sha256 hex of app_id:app_secret>","code":"<auth_code>"}` -> response has `s`, `code`, `message`, `access_token`.
- `appIdHash` = SHA-256 of **`app_id:app_secret` with the colon** (the docs also say `api_id + app_secret` in one table; the worked example and the Rust/community code use the colon). A wrong hash returns code `-5` (community-reported).
- Every call after login sends header `Authorization: app_id:access_token` (**no `Bearer`**).
- **The access token expires daily at 06:30.** Expired/invalid tokens return codes `-8, -15, -16, -17`.
- `redirect_uri` must **exactly** match the one registered with the app.
- Response envelope: `s: "ok"|"error"`, `code`, `message`; HTTP status 200/400/401/403/429/500.

**History** (`api/market-data/historical-data/`)
- Params: `symbol` (e.g. `NSE:SBIN-EQ`), `resolution` (`D`/`1D`, minutes `1 2 3 5 10 15 20 30 60 120 240`, `1W`, `1M`, seconds `5S..45S`), `date_format` (`0` = epoch, `1` = `yyyy-mm-dd`), `range_from`, `range_to`, optional `cont_flag`, `oi_flag`.
- Response: `s`, `candles` = array of `[epoch, open, high, low, close, volume]`.
- **Max range per request: 100 days for intraday, 366 days for `1D`.**
- For **completed** candles only, set `range_to` to the previous minute (for other resolutions, minus that resolution's duration); a current-minute `range_to` returns a partial candle.
- Needs the app's Market Data permission.

**Symbols** (`api/market-information/symbol-master/`, `api/appendix/symbology-format/`)
- NSE equity master: `https://public.fyers.in/sym_details/NSE_CM.csv`. Column 10, **`Symbol ticker`**, is the Fyers symbol: "copy it exactly as it appears in the file". Other columns include `Minimum lot size`, `Tick size`, `ISIN`.
- Format: `{Exchange}:{ExchangeSymbol}-{Series}`, e.g. `NSE:SBIN-EQ`. Validate against the master before use.

**Data WebSocket** (`api/websockets/data-socket/...`)
- Subscribe with data type `SymbolUpdate` and Fyers-format symbols. To change the symbol set, unsubscribe then resubscribe.
- **Lite mode** sends only symbol + LTP + type. **Volume (`vol_traded_today`), day open/high/low, change and bid/ask need full mode.**
- Message `type`s: `cn` connection, `sub` subscribe ack, `if` index data, `dp` market depth.

## 3. UNVERIFIED: confirm before relying on it

Record every outcome in `docs/fyers/FINDINGS.md` (what you checked, how, result). If the result contradicts this spec, **stop and tell the owner**.

| # | Item | Why it matters | How to confirm | If it fails |
|---|---|---|---|---|
| U1 | **Hosts** for validate-authcode, history, quotes. The docs give paths only; their examples show `https://api-t1.fyers.in/api/v3/...`. Data endpoints are likely `.../data/history`, `.../data/quotes` (community, not official). | Wrong host = nothing works. | Read the official `fyers-apiv3` SDK source (see section 11). | Ask the owner. |
| U2 | Is **`http://127.0.0.1:5000/api/nse/callback`** accepted as the registered redirect URI? The docs say only "a URL you control". | Decides the login UX. | The owner registers it in the dashboard and tries one login. | **Manual flow:** the user logs in, copies the final redirect URL (or just `auth_code`), pastes it into the app, which validates `state`/code locally. Build this flow either way as a fallback. |
| U3 | **Data WebSocket wire protocol.** Not in the official docs, only SDKs. Third-party notes say binary frames to `wss://socket.fyers.in/hsm/v1-5/prod`, a framed layout, an `hsm_key` claim in the access token's JWT, and per-symbol tokens from a symbol-token endpoint. Sources **disagree** on how the socket authenticates. | The riskiest part of the project. | Read the official SDK's `data_ws` source. Treat unofficial crates (`fyers-rs`, `fyers`) only as cross-checks: **review their licence, age and behaviour; do not add either as a dependency without the owner's OK**. | Stage S4 gate. |
| U4 | Quotes endpoint path and response shape. | Optional one-shot snapshot at connect. | SDK source. | Skip quotes; the first ticks fill the board. |
| U5 | **Symbol edge cases**: `&` in names (community reports history error `-300` for `M&M`-style names unless URL-encoded), hyphens (`BAJAJ-AUTO`), series suffixes (`-EQ`, `-BE`...). | A symbol that silently fails looks like a dead stock. | Map every line of `symbols.txt` through `NSE_CM.csv`; request history for each ampersand/hyphen name. | Report the unmapped ones in `/api/nse/status`; never guess. |
| U6 | **Time zones**: the 06:30 expiry (assume IST), epoch units/zone of candles and ticks (assume UTC seconds), and **how bars are aligned**. NSE sessions open **09:15 IST**; an hourly bar likely starts at :15, not :00. | `static/js/feed.js` `applyTick` assumes epoch-aligned buckets (right for Binance, wrong for NSE). A misaligned live candle never matches history. | Fetch real 15m/1h/1D history and look at the timestamps. | Generalise the bucketing with an offset. See 6. |
| U7 | Does the history response for `1D` include **today's partial candle**? Is `range_to` inclusive? | Avoid duplicate or missing "today". | Real history call during and after market hours. | Handle both. |
| U8 | Full-mode tick fields and units (`ltp`, `vol_traded_today`, last-trade time). | Candle volume and timestamps. | SDK source + recorded frames. | Ask. |
| U9 | Refresh-token flow and lifetime. Not in the official pages (forums only). | Not needed: out of scope (D5). | n/a | n/a |

## 4. Architecture

```
Browser (static/)                         Rust server (src-tauri/src/)                     Fyers
  markets/india.js  (delayed, as today)     server.rs   new /api/nse/* routes
  markets/live.js   (shared with crypto) <- fyers/stream.rs    WebSocket + reconnect    <-> data socket
  nse.js            (NseSource) --SSE---<-  fyers/aggregate.rs ticks -> candles/board
                                            fyers/client.rs    REST + rate limiter      <-> REST API
                                            fyers/auth.rs      login, state, token      <-> auth endpoints
                                            fyers/symbols.rs   NSE_CM.csv mapping       <-> public.fyers.in
                                            fyers/store.rs     token file, history cache (disk)
```

File names are a recommendation. Keep modules small, with no module knowing more than it needs. The Fyers code lives behind a trait or a thin interface so tests use fakes.

**Token storage:** the user data folder (`ZoneBoard/`, same place as the journal default), file `fyers_token.json`, holding the access token and its expiry time only. Never the secret, never a PIN. Owner-only access where the OS allows. (Optional hardening, owner's call: Windows DPAPI.)

**Config:** read `.env` (via `dotenvy` or similar) in the server only. Variables: `FYERS_APP_ID`, `FYERS_SECRET_ID`, `FYERS_REDIRECT_URI`. If any is missing, NSE live is simply "not configured".

**Port:** the redirect URI contains a port and must match what's registered. The server currently falls back to a random port if 5000 is busy. If NSE live is configured and the bound port differs from the registered one, say so clearly in `/api/nse/status` and in the UI. Don't silently break login.

## 5. API contract (additions, all under the existing loopback-only server)

| Route | Behaviour |
|---|---|
| `GET /api/nse/status` | `{configured, connected, tokenExpiresAt, marketOpen, session, symbols:{subscribed,limit}, unmapped:[...], budget:{...}, lastTickAt}`. **Never contains secrets or tokens.** |
| `GET /api/nse/login` | Returns `{url}`: the generate-authcode URL with a fresh random `state` (>=128 bits) stored server-side, single use, 10 min TTL. The login must open in the **system browser**, not inside the app's own webview. Don't rely on `window.open` for that: inside the Tauri window it may open another webview. Have the **server** launch the default browser (a tiny crate such as `open`, or `cmd /c start` with a validated URL). Propose the dependency in your plan; it must work in the desktop window and in a normal browser tab. |
| `GET /api/nse/callback?auth_code=&state=` | Verify `state` (constant-time, single use). Exchange the code. Store the token. Return a tiny HTML page "Connected, you can close this tab". **No token in the page, URL, or logs.** |
| `POST /api/nse/code` | Fallback for U2: accepts the pasted redirect URL or bare `auth_code`; same checks. |
| `POST /api/nse/logout` | Delete the token, close the socket. |
| `GET /api/nse/history?symbol=&interval=` | Same JSON shape as `/api/chart` candles (`time` unix seconds, `open/high/low/close/volume`). Goes through the limiter and cache. |
| `GET /api/nse/stream` (SSE) | Live events in **Binance-shaped payloads** (`{stream, data}` with kline / aggTrade / miniTicker-like data) so the shared browser code works unchanged. How the browser chooses symbols (query or a small POST) is your call; **total unique symbols must never exceed the plan limit**. |

Also add `india.symbols` to `/api/config`.

## 6. Limits budget (design inside these)

- **Soft caps below the hard ones:** at most **4 req/s, 40 req/min, 4,000 req/day** to Fyers, enforced by a token-bucket limiter shared by all REST calls. Fake-clock unit tests.
- **Cold start** needs daily history for ~69 symbols (one request each, <=366 days, ~250 bars). At 40/min that's about 2 minutes. The UI shows progress ("Scanning... 23/69"), not a frozen board.
- **Cache daily bars on disk** (per symbol, stamped by date). Later starts refetch only what's missing. Today's partial bar comes from live ticks.
- **Intraday history** for the open chart: one request per (symbol, interval), cached in memory (about 60 s), identical concurrent requests **coalesced**. The UI **debounces** rapid `j`/`k` presses (about 150 ms) so skimming the board can't burn the minute budget.
- **On any 429 / code `-429`:** stop all Fyers REST immediately. Cool off to the next minute boundary; if the per-minute cap was what tripped, stop for the **rest of the day**. Tell the UI. **No retry loops.**
- **Socket:** board (69) + the open chart symbol (normally already among them) + a few custom opens. **Refuse to subscribe beyond 100 unique symbols**, on the client side, *before* connecting (over-limit subscriptions kill the connection).
- **Bucket alignment (U6):** `applyTick` in `feed.js` floors to epoch multiples. NSE intraday bars are aligned to the 09:15 IST session start and a daily bar belongs to an IST date. Generalise bucketing with an offset or a per-market function, and **derive it from real history**, not from assumption. Add a test that a candle built from ticks matches the same candle from history.

## 7. Failure modes (all required)

| Situation | Behaviour |
|---|---|
| No `.env` | NSE stays delayed (Yahoo). Banner explains setup. No errors. |
| Configured, not logged in | Delayed + "Connect Fyers" button. |
| Token expired (06:30 or codes -8/-15/-16/-17) | Close socket, clear token, UI "Session expired: reconnect", fall back to delayed. **No retry loop.** |
| 429 / -429 | Fail closed (section 6). |
| Market closed / holiday / pre-open | Show last close with a "Market closed" label. The watchdog **must not** treat silence as a fault. |
| Socket drops | Reconnect with backoff and jitter, resubscribe, refill candles from history, give up after N tries and show the state. |
| Symbol unmapped or rejected | Skip it, list it in `status.unmapped`, continue with the rest. |
| More than 100 symbols requested | Refuse before connecting. |
| Fyers outage or schema change | "Fyers unavailable". Unknown message types are ignored and counted. Never panic. |

## 8. Security (on top of AGENTS.md)

- `state`: random, >=128 bits, constant-time compare, single use, short TTL. The callback accepts loopback hosts only.
- Never put the token or app secret in a response, URL, log, error string, or the browser. A test must prove log redaction.
- Test that `/static/..%2F.env` (and similar) returns 404. `.env` must never be servable.
- The CSP does not change: the stream is same-origin.
- Don't add outbound hosts beyond Fyers (`api-t1.fyers.in` or whatever U1 confirms, `socket.fyers.in`, `public.fyers.in`).

## 9. Testing

No real credentials and no network in automated tests.
- **Unit:** `appIdHash`; state store (single use, TTL, mismatch); symbol mapper (incl. `&` and `-` names); limiter; token expiry (06:30 IST boundary); tick->candle aggregator (alignment!); frame decoder against **recorded fixtures**; garbage frames never panic.
- **Fake Fyers server** in tests (local axum): auth (bad state rejected; correct `Authorization: app_id:token` header sent), history (100/366-day windows), 429 fail-closed, expired-token handling.
- **Recorded WebSocket fixtures:** the owner runs the app locally with a `--record-frames` debug option you add, which writes raw frames **with all tokens scrubbed**. Never commit anything containing a token.
- **Browser:** Node scripts like `tools/live_check.mjs` (add `tools/live_check_nse.mjs` for the connected case, manual) plus the checklist in `STAGES.md`.
- **Live verification only during market hours** (Mon-Fri 09:15-15:30 IST). Outside them, use fakes and history.

## 10. Acceptance criteria (definition of done)

1. With **no `.env`**, NSE behaves exactly as today and every existing test passes.
2. Login from the UI connects. The token persists; a restart the same day reconnects without a click; after 06:30 the UI asks to reconnect.
3. All 69 symbols in `symbols.txt` are mapped, or listed in `status.unmapped` with a reason.
4. Cold start scores all mapped symbols, observed REST rate stays <= 40/min (show the counter), and the second start is nearly instant (cache).
5. During market hours: board prices tick, the open chart's forming candle moves per tick, zones recompute, the pill reads live with a message rate.
6. **A candle built from live ticks matches the same candle from history** for 15m, 1h and 1D (OHLC within tick tolerance).
7. Market closed: last close + label, no stale/reconnect flapping.
8. No secret in any log, status JSON, HTML, or URL (test). `.env` not servable (test).
9. Simulated 429: fail-closed, UI message, **no retry storm** (test). 100-symbol cap enforced (test).
10. **Crypto is unchanged**: `node tools/live_check.mjs` passes, the crypto board and chart behave as before, parity tests green.
11. Zero compiler warnings; all three suites green; new tests added for everything above.
12. `README.md` section updated, `docs/fyers/FINDINGS.md` filled, `.env.example` accurate.

## 11. Out of scope

Orders/positions/holdings, market depth (`dp`) and tick-by-tick sockets, F&O/option chains, Prime-only features, PIN/refresh-token login, multiple users, changes to the Python backend, mobile layouts.

## 12. References

- Plans and limits: https://fyers.in/api-docs/v3/getting-started/plans-and-limits/
- Authentication: https://fyers.in/api-docs/v3/api/authentication/ (flow, generate auth code, validate auth code, best practices under `individual-app/`)
- Historical data: https://fyers.in/api-docs/v3/api/market-data/historical-data/
- Symbol master: https://fyers.in/api-docs/v3/api/market-information/symbol-master/ and https://public.fyers.in/sym_details/NSE_CM.csv
- Symbol format: https://fyers.in/api-docs/v3/api/appendix/symbology-format/
- Data WebSocket: https://fyers.in/api-docs/v3/api/websockets/data-socket/data-socket-symbol-update/ and `.../data-socket-lite-mode/`
- **Ground truth for U1, U3, U4, U8:** the official Python SDK `fyers-apiv3` (PyPI). To read its source, `pip download` it into its own **new, empty folder**, unzip there, read only, and **never run anything from inside that folder**. Treat it as untrusted download material.
