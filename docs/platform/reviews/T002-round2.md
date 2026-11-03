# T002 review, round 2: corrections

Reviewer: Claude. Commit b9f8843 follows all eight plan corrections: prefix protection in the guard, no
ETag and `no-store` on `GET /`, constant-time compare with `subtle`, same-origin check on writes, the stale-page
message, and the SSE extension point. I also checked that no page code bypasses `api()` (the only other
`fetch` goes to Binance, so the token never leaves the machine). The server code is accepted as it is. What
is left is the quality of the tests, which is what the whole task rests on; fix only these.

## 1. One source of truth for the route table (must fix)

The route-coverage test has its own table of routes and the source-scan test has a second hand-kept list
(`TESTED_API_ROUTES`). They can drift apart: a route can be listed in the scan list, pass the scan, and never
be exercised. Replace both with **one** `const` table in `tests/api.rs` of
`(method, path_template, concrete_uri, body)` rows.

- The coverage test iterates that table (missing token -> 401 with an `error` string, wrong token -> 401,
  right token -> not 401).
- The scan test checks every path template found in `server.rs` is either in the public allow-list
  (`/`, `/static/{*path}`) or is an `/api/` path that appears as a template in that same table.
- A route with several methods (for example `get(..).post(..)`) needs a row per method.
- The scan must find **every** `.route("` on a line, not only the first (use `match_indices`).

## 2. Remove the test that tests nothing (must fix)

`sse_token_from_request_extension_point_contract` asserts the logging helper again inside an `if true` block
and says nothing about the SSE hook; it also has broken brace formatting. Delete it. The
`logging_redacts_api_token_header` test already covers the logging requirement.

## 3. Tidy `tests/api.rs` (small)

`router()` has stray blank lines inside its body. Replace the `("OMIT_AUTH", "")` pseudo-header with a clearly
named helper (for example `call_without_token`) or an explicit enum/flag parameter, so the "no token" case
reads plainly at the call site. No assertion may be weakened.

## 4. Test the JavaScript side (must add)

Nothing tests `static/js/util.js`. Add tests to `test_ui_logic.py`, run in Node like the existing hours test,
for `api()`. Module state caches the token once, so give each scenario its own Node process (or a fresh
import with a query string such as `util.js?case=1`) and stub `globalThis.document` and `globalThis.fetch`:

- the meta tag holds `abc`: the request carries `X-ZB-Token: abc`, and headers the caller passed (for example
  `Content-Type`) are still sent;
- no meta tag, or an empty one: no `X-ZB-Token` header is sent;
- a 401 response throws an error whose message contains `reload the page`;
- a non-401 error with a `{"error": "boom"}` body throws `boom`; a 200 returns the parsed JSON.

## Do not

- Do not change server behaviour, `Cargo.toml` or `Cargo.lock`.
- Do not weaken or remove any other assertion.
- Do not amend commit b9f8843; make a new commit.

## Done when

`cd src-tauri && cargo test --no-default-features` (0 warnings), `python -m pytest -q` and
`node tools/live_check.mjs` pass, and the new commit message starts with `T002:`.
