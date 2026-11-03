# Driving Antigravity through the Fyers build

How this works: **Antigravity builds, Claude guides and reviews.** `AGENTS.md` (standing rules) and `docs/fyers/SPEC.md` (what to build) are the contract. You paste one stage prompt at a time, let the agent finish, then bring the result back to Claude for review *before* starting the next stage.

Why stages: the live-data part is risky (undocumented WebSocket, strict rate limits, a broker token). Small stages with a review between them catch a wrong turn while it is cheap.

## Before you start (you, once)

**1. Make changes reviewable.** The project isn't a git repo yet, so there is no way to see what an agent changed. Take a baseline:

```bash
git init
```

```bash
git add -A
```

```bash
git commit -m "Baseline before Fyers live NSE work"
```

After each stage, commit again. Then `git diff HEAD~1` is exactly what that stage changed.

**2. Make sure Antigravity reads the rules.** Open the folder as a project. `AGENTS.md` is a standard agent-rules file and recent Antigravity versions are reported to load it as a workspace rule, but I couldn't confirm the exact setting from official docs: look in Antigravity's Rules panel and check it is listed. If it isn't, add the contents of `AGENTS.md` as a workspace rule (reported location: `.agents/rules/`). Menu names vary by version.

**3. Use the safer modes.** Prefer the agent's **planning mode** (plan first, you approve) over fast/auto mode, and keep terminal commands on **ask-before-run**. You'll be reviewing commands that touch the network and a broker.

**4. Credentials: you, locally, later.** Don't create the Fyers app yet: stage S2 is the first that needs it. When it does: create an **individual app** in the Fyers API Dashboard with **Market Data permission only**, set its redirect URL to `http://127.0.0.1:5000/api/nse/callback`, copy `.env.example` to `.env`, and type your values in yourself. **Never paste them into Antigravity's chat or into this one.**

**5. Market hours.** Live checks (S5, S6) only work **Mon-Fri 09:15-15:30 IST**. Everything else is tested with fakes.

## How to run each stage

- One stage per Antigravity conversation. Start with the stage prompt below.
- When the agent shows its **implementation plan**, read it. If it widens scope, proposes a dependency, or contradicts the spec, push back before approving.
- Tell it to **stop at the end of the stage** (the prompts do).
- Run the three checks from `AGENTS.md` yourself, then `git commit`.
- Come back to Claude with: the stage name, `git diff --stat`, the test output, any `FINDINGS.md` changes, and anything the agent said it wasn't sure about.

---

## S0: Read, plan, resolve the unknowns (no code)

```text
Read AGENTS.md and docs/fyers/SPEC.md completely. This is stage S0. Do NOT write or change any
source code yet.

Produce:
1. A short implementation plan for the whole feature, split into the stages in docs/fyers/STAGES.md,
   naming the files you expect to touch or create in each.
2. For every item in SPEC section 3 (U1..U9): how you will confirm it, and what you already know.
3. Draft docs/fyers/FINDINGS.md with one entry per UNVERIFIED item (status: open).
4. A list of questions or concerns about the spec, especially anything you think is wrong.

For U1, U3, U4 and U8 the ground truth is the official `fyers-apiv3` Python SDK source. Before
downloading anything, ask me. If I agree: `pip download fyers-apiv3 --no-deps` into a NEW EMPTY
folder outside the repo, unzip it there, read the source only, and never run anything from it.
Quote file names and line ranges in FINDINGS.md.

Stop when done and wait for my review.
```

**Done when:** a plan, a FINDINGS draft with sourced answers for U1/U3/U4/U8, and questions. **No source files changed** (`git status` shows only `docs/`).

## S1: Config, symbols, status skeleton (no credentials needed)

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S1: only these things.
- Read FYERS_* settings from .env in the Rust server (never log them). Missing settings means
  "not configured", never an error.
- Add GET /api/nse/status returning at least configured/connected/marketOpen/session/symbols/
  unmapped, never containing secrets. Add india.symbols to GET /api/config.
- Map every line of symbols.txt to a Fyers symbol using the NSE_CM.csv master (SPEC 3, U5). Commit a
  SMALL trimmed slice of that CSV as a test fixture. Report unmapped names and the reason.
- Market session logic (open/closed/pre-open, Mon-Fri 09:15-15:30 IST) as a pure function with tests.
No auth, no network calls to Fyers, no frontend changes. Add tests for everything. Run all three
checks. Stop and wait for review.
```

**Done when:** all 69 symbols are mapped or listed with a reason; NSE behaves exactly as before with no `.env`; tests green; no warnings.

## S2: Authentication

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S2: authentication only.
Implement SPEC section 5 routes login, callback, code, logout, plus the token store and expiry
(06:30 IST). Use the hosts/behaviour recorded in FINDINGS.md (U1); if they are still open, stop and
ask me.
Requirements: state is >=128-bit random, constant-time compared, single use, 10-minute TTL;
appIdHash is SHA-256 of "app_id:app_secret"; header is "Authorization: app_id:access_token";
tokens/secrets/auth codes never appear in logs, errors, URLs, status JSON or HTML (add a test that
proves redaction). Build the manual paste-the-redirect-URL fallback too (U2).
Test against a LOCAL FAKE Fyers server only: no real credentials, no network. Add tests for state
mismatch, replay, expiry, wrong hash error handling, and codes -8/-15/-16/-17.
No history, no WebSocket, no frontend. Run all checks. Stop and wait for review.
```

**Then you:** copy `.env.example` to `.env`, fill it in, run the app and do **one real login** to settle U2 (does Fyers accept the `127.0.0.1` redirect?). Ask the agent to record the result in `FINDINGS.md`.

**Done when:** fake-server tests pass, the redaction test exists and passes, and your one real login connects (or the manual fallback does, with U2 recorded).

## S3: REST client, rate limiter, history, cache

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S3: REST only.
Implement the shared token-bucket rate limiter (4/s, 40/min, 4000/day soft caps, fake-clock tests),
the history client with the 100/366-day windows and the "completed candles" range_to rule, an
in-memory cache with request coalescing, the on-disk daily-bar cache, and GET /api/nse/history.
On any 429 / -429 fail closed exactly as SPEC section 6 says. Expose a request counter in
/api/nse/status.budget.
Add a small debug tool that prints real history timestamps for one symbol at 15m, 1h and 1D so we can
settle U6/U7 (alignment, today's partial candle, range_to inclusivity). Do NOT assume: record what
the real data shows in FINDINGS.md.
Test with the local fake server. No WebSocket, no frontend. Run all checks. Stop for review.
```

**Then you:** run the timestamp tool once with your real token and let the agent record U6/U7.

**Done when:** limiter and 429 fail-closed are tested; a cold-start fetch of all symbols stays at or under 40/min with the counter proving it; U6/U7 are answered from real data.

## S4: WebSocket spike and decision gate (no production code)

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S4: a spike and a written decision. Do NOT
add the WebSocket client to the app.

Using the official fyers-apiv3 SDK source (already in FINDINGS from S0) answer: what is the exact
connection and authentication sequence; what do frames look like (framing, request types, field
layout, how symbols map to socket topics, ping/keepalive); full mode vs lite mode fields; what
changed between SDK versions. Then write docs/fyers/WS-DECISION.md comparing:
 A) a Rust-native client (estimate size, the decoder's testability from recorded frames, risk of the
    protocol changing, whether an unofficial Rust crate is worth using or only worth cross-checking);
 B) the official SDK run as a sidecar process (what it needs installed, how the server would supervise
    it, how the token would be handed over, impact on the single-exe goal).
Give a recommendation, the cost of being wrong, and a test plan for either choice. Stop and wait for
the owner's decision.
```

**This is a gate.** Bring `WS-DECISION.md` to Claude and decide together. My default is Rust-native, because the whole project is moving that way, **unless** the decoder looks fragile or large; then the sidecar. It is your call.

**Done when:** the memo is evidence-based (file/line references to the SDK source), not opinion.

## S5: WebSocket, aggregation, SSE stream (per the S4 decision)

```text
Read AGENTS.md, docs/fyers/SPEC.md and docs/fyers/WS-DECISION.md. This is stage S5. Implement the
approach the owner chose.
- Socket client with reconnect (backoff + jitter), resubscribe, refill from history, and the hard
  100-unique-symbol cap enforced BEFORE connecting.
- A --record-frames debug option that writes raw frames with all tokens scrubbed, for fixtures. Never
  commit a recording containing a token.
- Tick -> candle/board aggregation using the bar alignment settled in FINDINGS (U6): NSE intraday
  bars align to 09:15 IST, not to epoch multiples. Prove it with a test that a candle built from
  ticks matches the same candle from history.
- GET /api/nse/stream (SSE) emitting Binance-shaped events as in SPEC section 5.
- Market-closed behaviour from SPEC section 7: silence is not a fault.
Tests: decoder against recorded fixtures, garbage frames never panic, aggregator alignment,
reconnect, cap enforcement. No frontend changes. Run all checks. Stop for review.
```

**Then you:** during market hours, run once with `--record-frames` so the agent can add real fixtures (check them for tokens before committing).

**Done when:** fixtures decode, aggregator alignment is proven against history, the cap and reconnect are tested.

## S6: Frontend

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S6: browser changes only.
- Generalise static/js/markets/crypto.js into a shared live-market module used by BOTH crypto and NSE;
  crypto behaviour must be byte-for-byte equivalent in outcome (node tools/live_check.mjs and a manual
  check must still pass).
- Add static/js/nse.js (NseSource) implementing the same source interface over /api/nse/history and
  the /api/nse/stream SSE. Generalise tick bucketing in static/js/feed.js to support the NSE alignment
  without changing crypto behaviour.
- India market: use live when /api/nse/status says connected, else the existing delayed module. Clear
  pill states: Live NSE / Delayed 15 min / Market closed / Session expired. A "Connect Fyers" button
  (opens the login URL in the system browser; paste fallback if needed) and a setup hint when not
  configured. Debounce chart opens (~150 ms) for NSE.
- The paper-trading ticket (static/js/trading.js) relies on each market module's quote(sym) and
  watch(symbols), and gates NSE orders on static/js/hours.js. Live NSE must feed quote() and keep that
  gate working: don't break them. The Live/Demo toggle is the ACCOUNT (Demo = paper orders on real prices, Live = real orders, none connected); it must keep working for NSE, and live NSE prices go through the same real-data path.
- Keep the UI light and fast: no framework, no extra requests on the load path, textContent only.
Verify in a real browser (screenshots of each state). Run all checks. Stop for review.
```

**Done when:** crypto unchanged, each pill state reachable and correct, no console errors, load path unchanged.

## S7: Hardening and documentation

```text
Read AGENTS.md and docs/fyers/SPEC.md. This is stage S7. Walk SPEC sections 7, 8 and 10 line by line
and add a test for every row that isn't already covered: failure-mode matrix, redaction, /static
cannot serve .env, 429 fail-closed, symbol cap. Update README.md (setup, daily login, limits, what
"Live NSE" requires), finish docs/fyers/FINDINGS.md, and make .env.example accurate. Run all three
checks plus a release build. Then write a short walkthrough of what was built and what was NOT
verified. Stop for review.
```

---

## What Claude will check at each review

1. **Scope:** only the stage's items changed; no new dependency or outbound host that wasn't agreed.
2. **Spec fidelity:** anything UNVERIFIED was confirmed and recorded, not assumed.
3. **Secrets:** grep the diff and fixtures for tokens, app ids, secrets; read the redaction test.
4. **Tests that can fail:** I'll mutate the code under a new test and confirm it goes red.
5. **Limits:** the REST counter, the 100-symbol cap, and fail-closed behaviour actually enforced.
6. **No regressions:** crypto and the Python/Rust parity suites untouched and green.
7. **Honest reporting:** what the agent says it verified matches what its logs show.
