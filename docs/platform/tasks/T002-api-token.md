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
