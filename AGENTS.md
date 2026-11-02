# AGENTS.md: standing rules for any coding agent in this repo

ZoneBoard is a personal learning tool: a scanner, chart, journal and backtester for **live crypto** (Binance public data) and **NSE stocks** (Yahoo, ~15 min delayed, being upgraded to live via Fyers). Read `README.md` first for what it does and how to run it.

## Working agreement

1. **Plan before code.** For any task bigger than a one-line fix, write a short plan (files you will touch, tests you will add, risks) and wait for the owner's go-ahead.
2. **Small steps, each ending green.** After each step, run the checks below. Never leave the repo with a failing suite.
3. **Follow the spec when there is one.** A task that points at a file in `docs/` (for example `docs/fyers/SPEC.md`) is governed by it. If the spec is wrong or something it marks UNVERIFIED turns out different in practice, **stop, say what you found, and ask**. Do not silently deviate.
4. **Ask before anything costs money, needs a credential, or is hard to undo.** The owner has said: *if you need an API key, ask first.* Never invent, guess, or request keys in chat.
5. **Report honestly.** If a test fails or a step is skipped, say so with the output.

## Checks (run all that apply before saying "done")

```bash
cd src-tauri && cargo test --no-default-features     # Rust: unit, HTTP API, Python-parity tests
python -m pytest -q                                   # Python: API, zones, JS-vs-Python zone parity
node tools/live_check.mjs                             # live Binance pipeline (needs internet)
```

- Run **one cargo command at a time** (shared build lock) and don't edit sources while a build runs.
- `cargo test --no-default-features` skips the Tauri desktop shell. A full desktop build is slow (about 7 min debug, 25 min release, cold).
- The toolchain here is `stable-x86_64-pc-windows-gnu` (WinLibs MinGW). There is no MSVC. Don't assume `link.exe`, `cl.exe` or vcpkg.
- Release builds **embed `static/` into the binary** at compile time (`rust-embed`); debug builds read it from disk. After changing `static/`, a release build must be redone.

## Layout and ownership

```
static/                 web UI: vanilla ES modules, no framework, no bundler, no CDN. Served by every backend.
  js/zones.js           zone engine (JS port)        ┐ three implementations of the same rules,
src-tauri/src/zones.rs  zone engine (Rust)           │ kept honest by parity tests:
zones.py                zone engine (Python, reference) ┘ test_zones_js.py, src-tauri/tests/parity.rs
src-tauri/              Rust: library + desktop window (Tauri, optional `desktop` feature) + server + backtest
app.py, data.py, backtest.py   original Python backend, kept as the reference
tools/                  small dev scripts
docs/                   specs and guides
```

- **If you change zone or backtest rules**, change all three engines, regenerate the fixture (`python src-tauri/tests/make_fixture.py`), and keep every parity test passing. Never loosen a parity test to make it pass.
- `static/vendor/` is a vendored third-party build (chart library, with its licence). Don't edit it.
- `journal.db` is the owner's real data. **Never** open it in tests or scripts. Tests use `:memory:` or a temp file.

## Conventions

- Match the surrounding code: comment density, naming, idiom. Comments explain *why*, not what.
- **UI:** build DOM with `textContent` / `createElement`, never `innerHTML` with data. Keep it light and fast: no new framework, no webfont, no extra network round trips on the load path. Rows update in place; paint work is batched to one animation frame.
- **Server:** loopback only (`127.0.0.1` and `[::1]`), no login. Keep the `Host`-header check and the security headers. The Content-Security-Policy only allows `'self'` plus Binance's public data hosts: widen it only for a documented reason.
- **Rust:** `cargo test` must stay free of compiler warnings. Errors that reach the UI are `{"error": "..."}` JSON with a sensible status code.
- Prices: crypto can be sub-cent (`0.00000486`). Never hard-code 2 decimals for crypto.

## Secrets (hard rules)

- Credentials live only in a local, git-ignored `.env` (template: `.env.example`) and token files under the user's data folder. They never go in source, tests, fixtures, logs, error messages, URLs, commit messages, chat, or the browser.
- Redact tokens, app secrets and auth codes in every log line and error string. Add a test that proves it.
- The browser never sees a broker token. Anything that needs one runs in the Rust server.
- Tests and CI must run with **no real credentials and no network**: use local fake servers and recorded fixtures.

## Scope guardrails

- This tool **reads market data**. It does **not place, modify or cancel orders**, and must not request trading permissions from any broker.
- Don't add telemetry, analytics, auto-updaters, or any outbound call to a host not named in the spec.
- It is an educational tool, not investment advice. Keep that in the UI footer.
