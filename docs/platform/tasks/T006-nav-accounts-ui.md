# T006: Navigation shell, Accounts and Settings pages

Phase 1 · Depends: T003, T005 · Touches: `static/index.html`, `static/css/app.css`, `static/js/` (new `accounts.js`, `settings.js`, `nav.js`), `main.js`

## Goal
Turn the two-screen page into a terminal with real structure, in the **existing ZoneBoard look** (light theme, current CSS tokens, no framework, `textContent` only, no extra requests on the load path).

## In scope
1. Navigation: Board, Orders (placeholder: "arrives with the execution core"), Accounts, Journal, Settings. Keyboard-accessible, mobile-friendly, remembers the last page.
2. **Accounts page:** the demo account as a card with a visible **VIRTUAL** badge, balances and available funds, edit credits, reset with a confirmation. Cards for Binance and FYERS showing "Not connected" with disabled connect buttons (they come alive in T007 and later). A permanent line: *"This app holds no money. Real balances are fetched from your broker; virtual credits are practice only."*
3. **Settings page:** timezone (from `Intl.supportedValuesOf('timeZone')`), desktop-notification toggle, a read-only **Security** section (vault status, API token active, `Live trading: disarmed`), and an **audit log viewer** (latest 100, newest first).
4. The existing Live/Demo toggle keeps working and now shows which account is active.

## Out of scope
Connecting brokers, order forms, strategy pages.

## Acceptance criteria
- Every page and state works at desktop and phone width; no horizontal scroll at 375 px.
- No `innerHTML` with data; no console errors.
- Verified in a real browser; screenshots of each page in the report. If T010's smoke harness exists, add a scenario per page.
