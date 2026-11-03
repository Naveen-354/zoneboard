# ZoneBoard

A personal scanner, chart, journal and backtester for **live crypto** and **delayed NSE stocks**, for learning how markets move.

- **Board:** ranks the watchlist into the strongest bullish and bearish setups, labels weak ones "choppy". Prices tick live.
- **Chart:** candles, volume, order block zones, swing-high/low liquidity levels, support and resistance. The forming candle moves trade by trade.
- **Paper trading:** an order ticket on the chart screen. Buy or sell at the price on screen, watch the position's P&L move, close it. **Simulated: no broker, no real money.**
- **Journal:** every closed paper trade is logged automatically, per market, with net P&L, win rate, profit factor and fees paid.
- **Live / Demo account toggle** (top right): **Demo** is the practice account (simulated orders on real prices). **Live** is the account for real orders, which isn't connected to any broker yet.
- **Backtest:** test the order block rule on years of history before trusting it.

| Market | Data | Speed |
|---|---|---|
| Crypto (Binance spot) | Binance public market data. **No account, no API key.** | **Live**, every trade |
| India NSE | Yahoo Finance (unofficial) | **~15 min delayed**, refreshed each minute |

Live NSE prices need a broker feed (Fyers or Zerodha Kite). That is **not connected yet**: see [Live NSE data](#live-nse-data).

## Run it

### Desktop app (Rust + Tauri)

```bash
cd src-tauri
cargo run --release          # first build is slow (about 25 min cold), later ones are incremental
```

One native window, no browser, no Python. The release build puts `zoneboard.exe` (11.8 MB, the web UI is embedded in it) in `src-tauri/target/release/`. **Keep `WebView2Loader.dll` in the same folder as the exe**: without it Windows can't start the window. (The `windows-gnu` toolchain loads it dynamically; the MSVC toolchain would link it in.)

Needs the Rust toolchain and the WebView2 runtime (already part of Windows 11). Built and tested here with the `stable-x86_64-pc-windows-gnu` toolchain. You'll see one harmless linker warning (`multiple non-default manifests`): the icon, manifest and version info are still embedded.

### In a browser tab

Without the desktop shell (no Tauri, no webview):

```bash
cd src-tauri
cargo run --release --no-default-features --bin zoneboard-server     # then open http://127.0.0.1:5000
```

**Legacy: the Python server.** It is frozen (decision D4, task T009): the Rust app is the product, and the Python code is kept only as the reference for the zone and backtest rules and for the parity fixture. It gets no new features, and it does **not** support Rust-only features (accounts, API token, vault, audit and settings), so parts of the UI may not work against it. It still serves the UI and the older `/api`, and still runs:

```bash
python -m venv .venv
.venv\Scripts\activate            # Linux/macOS: source .venv/bin/activate
pip install -r requirements.txt
python app.py                     # http://127.0.0.1:5000
```

### Live and Demo accounts

The toggle in the top right is about the **account**, not the data: **the charts always show real prices** in both.

| | Demo (default) | Live |
|---|---|---|
| Prices | real | real |
| Orders | **simulated**: filled at the price on screen, no broker, no real money | **none yet**: placing real orders needs a connected broker, which this app doesn't have |
| Journal | your paper trades | trades made through a connected broker (none) |

Switching is instant and keeps both accounts' positions and journals separate. `?demo` or `?live` in the URL picks the account; otherwise the app remembers your last choice (Demo the first time).

### Developing without internet (sample data)

Only for development: `SAMPLE_DATA=1` (PowerShell: `$env:SAMPLE_DATA = "1"`) on the server, or `?sample` in the URL, swaps every price for made-up random ones (`?sample=0` forces real data). It is not part of the Live/Demo toggle, it's labelled everywhere, and **ordering is switched off** so made-up prices can never reach a journal.

## Using it

| Key | Does |
|---|---|
| `j` / `k` or `↓` / `↑` | next / previous symbol on the board |
| `1`-`6` | chart timeframe (1m 5m 15m 1h 4h 1d for crypto) |
| `/` | search; **Enter** opens any Binance pair, even one not on the board (type `PEPE` or `LDOUSDT`) |

- **Setups / All:** *Setups* shows the strongest bullish and bearish; *All* lists every symbol with its live price.
- **Board timeframe:** the board's score is computed on the timeframe you pick (crypto: 15m / 1h / 4h / 1d). 24/7 markets have no "today", so a 1h board stays meaningful at any hour.
- **Zones / Levels** buttons toggle the overlays. A zone whose impulse candle is still open is drawn dashed and marked *forming*: it can disappear when the candle closes.
**Paper trading** (chart screen, right of the chart):
1. Pick a symbol, choose **Buy** or **Sell** (a sell opens a short), enter a quantity or tap an amount chip (100 / 500 / 1k USDT, or ₹10k / ₹50k / ₹1L), press the big button. It fills at the price shown.
2. The position appears under the chart with its live P&L (after the simulated fees a close would cost). Click its symbol to jump to its chart.
3. **Close** it: the trade is logged to the **Journal** and the result is shown.
- Fees are simulated: 0.1% per side for crypto and 0.05% for NSE (the same costs the backtest assumes).
- Only the **Demo** account can trade; the Live account's ticket is disabled and says why (the server refuses live orders too). Orders are also refused when they'd be unrealistic: crypto needs a live feed, and **NSE needs the market open** (Mon to Fri, 09:15 to 15:30 IST; its price is 15 minutes old, which the ticket says). Outside NSE hours, practise on crypto: it trades 24/7.
- Open positions are stored by the app, so they survive a reload or restart. NSE quantities must be whole shares.
- There are no stop-loss or limit orders yet: only market orders you close yourself.

- Your symbol, timeframes and toggles are remembered (per address, so `127.0.0.1:5000` and `localhost:5000` keep separate settings).
- The watchlists are `symbols_crypto.txt` and `symbols.txt`. The Rust app reads them from the folder you launch it from (or next to the exe); if there is none it uses the copy built into the exe, so after editing the ones in the project folder either launch from there or rebuild.

## How it stays fast

- The browser talks **straight to Binance** (WebSocket for ticks, REST for history): no server hop on the live path.
- The zone engine runs **in the browser** (`static/js/zones.js`), so zones and scores recompute on each live candle. Whole-chart analysis of 500 bars takes ~0.3 ms; applying a trade tick is microseconds.
- No framework, no bundler, no webfonts, no CDN: 11 small ES modules plus a vendored copy of the chart library (`static/vendor/`). Rows update in place and all painting is batched to one animation frame.
- On a normal connection the board is scored and the chart drawn about **0.6 s** after the page starts loading.
- Data feed health shows top right: `Live · N msg/s`. It reconnects and refills itself if the socket drops or goes silent for 10 s. Subscription changes are batched so Binance's 5-messages-per-second limit is never hit.
- Open it at `http://127.0.0.1:5000` (the Rust server also answers on `localhost`; the Flask dev server is slower on `localhost` under Windows because of the IPv6 fallback).
- To check the live pipeline from this machine without opening the app: `node tools/live_check.mjs` (REST, WebSocket ticks, and zone analysis on real candles).
- To smoke-test the UI in a headless browser (sample data, temp journal, no internet): build the server once (`cd src-tauri && cargo build --no-default-features --bin zoneboard-server`), then `node tools/ui_smoke.mjs`.

## Backtest

```bash
cd src-tauri
cargo run --release --bin zoneboard-backtest                                  # NSE daily, 5 years, 0.1% cost
cargo run --release --bin zoneboard-backtest -- --market crypto --interval 1h # Binance, 5000 candles each, 0.2% cost
cargo run --release --bin zoneboard-backtest -- --market crypto --interval 15m --bars 20000 --rr 2
```

Results are in R (units of risk) before and after costs, split into an older and a newer half. **If the average R is not clearly above zero after costs in both halves, the rule has no usable edge.** On random sample data (`SAMPLE_DATA=1`) it correctly shows a loss; that is the sanity check you want.

(The legacy Python version still works too, but is frozen: `python backtest.py --period 5y --rr 2 --cost 0.1`, NSE only.)

## Tests

```bash
cd src-tauri && cargo test --no-default-features      # Rust: zones, journal, backtest, HTTP API, Python parity
python -m pytest -q                                   # Python: same checks, plus JS-vs-Python zone parity and the UI's market-hours logic
```

Three implementations of the zone rules exist (Python, JavaScript for the live chart, Rust). They are kept honest by tests that replay the same bars through each and compare zones, levels, scores and backtest trades.

## What the zones mean (and don't)

- **Bullish order block:** the last down candle before a strong up-move (at least 1.5 x ATR within 3 bars, closing above that candle's high). The zone is that candle's range. Bearish is the mirror image. A zone is "mitigated" once a bar closes through it.
- **Liquidity:** swing highs and lows (3 bars each side). Nearly equal levels are merged, and a level is dropped once price trades through it.
- **Score (-100 to +100):** latest bar's move in ATR units 40%, trend vs EMA20/EMA50 25%, relative volume 20%, nearness to an active order block 15%. Under 15 in magnitude = choppy.
- This is one simple, rule-based reading of "smart money" ideas. **It has no proven edge.** Real-time data only makes the picture current; it does not create an edge, so the backtest still matters.

## Live NSE data

Real-time Indian stock prices are not available for free without a broker account: reverse-engineering a broker or exchange website would break its terms and put your accounts at risk. The official routes are a broker API:

| Broker | API / data (as of one Jan 2026 article: confirm on the broker's own page) |
|---|---|
| Fyers | trading API free; quotes and market data listed as free; needs an account with KYC |
| Zerodha Kite Connect | order API free for personal use; market data is a separate paid add-on |
| Angel One SmartAPI, Upstox, Dhan | APIs available; data terms to be checked |

**Decision: Fyers** (free Standard plan: 100 live symbols, 5 requests/s, 50/min, 5,000/day for data; the access token expires every day at 06:30, so there is a quick login each morning). It isn't built yet. The build is being done in Google Antigravity, guided by:

- [`AGENTS.md`](AGENTS.md): standing rules for any coding agent in this repo (checks, layout, secrets, scope).
- [`docs/fyers/SPEC.md`](docs/fyers/SPEC.md): what to build, what is verified against Fyers' docs and what is still unverified, limits, failure modes, acceptance criteria.
- [`docs/fyers/STAGES.md`](docs/fyers/STAGES.md): the eight stages with a paste-ready prompt and a review checklist for each.

Your Fyers app id and secret go in a local, git-ignored `.env` (template: `.env.example`), never in the code or in chat. The feature is read-only: market data only, no orders.

## Limits to know

- Yahoo data is unofficial, delayed (~15 min) and can break or have gaps. Its terms limit it to personal, non-commercial use.
- `symbols.txt` is a starting list of liquid stocks, **not** the official NSE F&O list (which changes). Symbols Yahoo doesn't recognise are skipped.
- The server has no login and listens on loopback only (`127.0.0.1` and `[::1]`). It refuses requests with any other `Host` header. Don't change that to expose it to a network.
- The page's Content-Security-Policy only lets it load its own scripts and talk to Binance's public data hosts.
- Binance's public market-data hosts are reachable from most countries but not all; if the badge never turns "Live", check your connection or VPN, or try `SAMPLE_DATA=1` (made-up prices, for development).
- The journal is a SQLite file. The Python app uses `journal.db` next to `app.py`. The Rust app uses `JOURNAL_DB` if set, else a `journal.db` in the folder you launch from, else `%APPDATA%\ZoneBoard\journal.db` (so `cargo run` from `src-tauri` starts a fresh journal there, not the project's `journal.db`: launch from the project folder or set `JOURNAL_DB` to keep using that one). Back it up if it matters. Journals from before crypto support open as-is: their trades are filed under India.
- Crypto quantities can be fractional; NSE quantities must be whole numbers. Trades are kept per market (USDT and rupees never mix) and per account (Demo-account paper trades never mix with Live-account ones). Orders can't be typed into the Journal any more: trades come from the chart screen. (`POST /api/journal` still exists for scripts.)
- Educational tool, not investment advice.

## Layout

```
static/                 the web UI (served by every backend)
  js/zones.js           zone engine (port of zones.py)
  js/feed.js            Binance REST + WebSocket, reconnect, rate-limited subscribes
  js/sim.js             made-up sample-data feed (development only)
  js/chart.js           candlestick chart + zone/liquidity overlay (chart library 5.2.1 vendored in vendor/)
  js/trading.js         paper-trading ticket and open positions (talks to /api/positions)
  js/hours.js           NSE session hours (tested in Node)
  js/board.js, view.js, journal.js, markets/   board, header, journal, per-market wiring
src-tauri/              Rust: library + desktop window + server + backtest
  src/zones.rs, data.rs, journal.rs, backtest.rs, server.rs
  tests/                parity with Python (fixture from tests/make_fixture.py), HTTP API
app.py, data.py, zones.py, backtest.py    the original Python backend (FROZEN legacy; kept as the reference)
tools/live_check.mjs    one-command check that live Binance data works from this machine
```
