# T002: API token and CSRF protection

Phase 0 · Depends: T001 · Touches: `server.rs`, `static/js/util.js`, `static/index.html`, `tests/api.rs`

## Goal
Stop web pages, other origins and casual local callers from using the local API. Today any request that reaches the loopback port is accepted.

## Threat model (be honest about it in the code comments)
Protects against: other websites (CSRF, DNS rebinding), browser extensions on other origins. Does **not** protect against malware already running as this Windows user, which could read the page or the keychain anyway. Say so.

## In scope
1. At startup generate a 256-bit random token (OS randomness, hex). Different every launch.
2. **Every `/api/*` route** requires header `X-ZB-Token` matching it (constant-time compare, `subtle`). Missing or wrong: `401 {"error": ...}`. Static files stay public.
3. `GET /` serves the HTML with the token inserted into a `<meta name="zb-token">` tag at serve time. `static/js/util.js` `api()` reads it once and sends the header on every call. Keep `index.html` valid when opened without the placeholder.
4. State-changing methods (`POST`, `PUT`, `PATCH`, `DELETE`) also require: `Origin` header absent or equal to the server's own origin (anything else: `403`), and the existing JSON content-type rule.
5. The token never appears in logs, errors or any response other than the HTML meta tag. Add a test.
6. Design so a future SSE endpoint can authenticate differently (EventSource cannot set headers): leave a clearly marked extension point, but do **not** build SSE now.

## Out of scope
Users, passwords, sessions, SSE, changing any route's behaviour.

## Acceptance criteria
- A table-driven test enumerates **every route registered on the router** and proves: no token -> 401, wrong token -> 401, right token -> normal behaviour. A new route that forgets protection must fail this test.
- Cross-origin `Origin` on a state-changing request -> 403, even with the right token.
- Token differs between two server instances; compare is constant-time (use `subtle`, assert by code review note).
- The desktop window and the browser tab both still work (the page gets its token from the HTML).
- Existing tests updated to send the token via one shared helper; no test weakened.

## Notes
Pre-approved dependencies: `subtle`, `rand`.

## Plan review: corrections the implementation must follow (Claude, 2026-11-03)
Antigravity's plan was reviewed and approved **with these changes**. Where it differs from the plan, this section wins.

1. **No ETag and no 304 on `GET /`.** The plan derived the ETag from the token, which puts the token in a response header and breaks rule 5 ("only in the HTML meta tag"). Serve `GET /` with `Cache-Control: no-store` instead. Reason: a cached copy of the page would carry a dead token after the next restart (the desktop WebView keeps a persistent cache), and every API call would then fail. Static assets stay cacheable (they contain no token). Test: response headers on `GET /` contain no token and include `no-store`.
2. **Route coverage must be impossible to forget.** Protect by path prefix (`/api` and `/api/*`) in the middleware, never per handler. Tests: (a) a table of every current API route and method: no token -> 401, wrong token -> 401, right token -> normal behaviour; (b) a path that matches no route (`/api/__unregistered__`) is also 401 without a token, which proves the protection is by prefix; (c) a source-scan test (`include_str!("../src/server.rs")`) that extracts every `.route("...")` path and asserts each one either starts with `/api/` and is in the table, or is in an explicit public allow-list (`/`, `/static/{*path}`). A new route outside those rules must fail this test.
3. **A stale page must say so.** On a `401`, `api()` in `util.js` throws a clear message ("ZoneBoard was restarted or this page is stale: reload the page"), not a bare "unauthorized". The token changes on every launch, so an open tab breaks after a restart.
4. **Decisions on the plan's questions:** removing `<link rel="preload" href="/api/config">` is approved (a preload cannot carry the header; note the small load-time cost in your report). `AppState::api_token()` is approved as a read-only accessor for tests; mark it `#[doc(hidden)]` and make sure no `Debug`, `Serialize` or log call can print it.
5. **Dependencies:** `rand = "0.8"` (already in `Cargo.lock`; `rngs::OsRng` + `RngCore::fill_bytes`) and `subtle = "2"`. Nothing else. Do not move to `rand` 0.9 or 0.10 (their API differs).
6. **SSE extension point:** one small function, for example `token_from_request(&Request) -> Option<&str>`, that returns the `X-ZB-Token` header today, with a comment saying where a future query-parameter variant for `EventSource` would go. Build nothing else.
7. **Logging proof:** add a test that `logging::is_secret_field("X-ZB-Token")` is true, and make sure no code path logs header values.
8. **Verification split:** you run the automated checks. Claude does the browser checks (token in the meta tag, header on requests, 401 on a bare fetch, 403 on a foreign Origin); do not try to drive a browser.
