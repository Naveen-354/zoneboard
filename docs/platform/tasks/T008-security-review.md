# T008: Independent adversarial security review

Phase 1 · Depends: T002, T004, T007 merged (or their branches) · Touches: nothing in code, a report only

## Goal
A second, independent set of eyes on the security-critical code, before any order path is built on top of it. Antigravity reviews code Claude specified and Antigravity (a different run) wrote.

## Scope
Review the API token and CSRF layer, the vault, the Binance client and the accounts/audit code. Act as an attacker with: (a) a web page the user visits, (b) a local process running as the same user, (c) a malicious or broken Binance response.

Look for, with a concrete reproduction idea for each finding:
- calling any API route without the token, or with it from another origin;
- a secret (API key, secret, token, signature, PEM) reaching a log line, audit row, error message, status JSON, HTML, URL or panic text;
- route handlers that skip the token check (compare the route list with the protected list);
- CSRF, DNS rebinding, request smuggling around the Host/Origin checks;
- SQL built from strings; path traversal in static serving; unbounded request sizes or lists;
- key-permission bypass (a fake or hostile response claiming `enableWithdrawals: false`);
- floats used for money; panics (`unwrap`/`expect`) reachable from external input;
- secrets kept in memory longer than necessary; derive macros that make them printable.

## Output
`docs/platform/reviews/T008-security.md`: findings ranked critical / high / medium / low, each with file and line, how it could be exploited, and a suggested fix. For each critical or high finding, also add a **failing test** (marked `#[ignore]` with a comment naming the finding) so the fix is provable. **Do not fix anything in this task.**

## Acceptance criteria
- Every route and every place a secret is handled is listed in the report as reviewed.
- Findings are specific, not generic advice. "No issues found" is acceptable only with the review notes showing what was checked.
