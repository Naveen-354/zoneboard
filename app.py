"""FROZEN (T009, decision D4): legacy Python backend, kept as the reference for the zone/backtest
rules and the parity fixture (Python server). New features go to the Rust app only; change this
file only to keep the parity fixture and existing tests green. See AGENTS.md.

ZoneBoard: a personal F&O scanner + chart + journal using free, delayed data.

Run:  python app.py       then open http://127.0.0.1:5000
Sample data: SAMPLE_DATA=1 python app.py   (made-up prices for development; no internet needed)
"""
from __future__ import annotations

import os
import re
import sqlite3
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import numpy as np
from flask import Flask, jsonify, request, send_from_directory

import data
import zones

HERE = Path(__file__).parent
app = Flask(__name__, static_folder=None)

SYMBOL_RE = re.compile(r"^[A-Z0-9&\-]{1,20}$")
IST = timezone(timedelta(hours=5, minutes=30))
MARKETS = ("crypto", "india")
# The account. "demo" = paper trading on real prices (the only account that can place orders here).
# "live" = real orders, which this app does not place: no broker is connected.
MODES = ("live", "demo")
# Simulated fee per side, percent of notional. Round trip 0.2% / 0.1%: the same costs the backtest assumes.
FEE_PCT = {"crypto": 0.1, "india": 0.05}
LOCAL_HOSTS = {"127.0.0.1", "localhost", "[::1]"}

# The page loads its own scripts only, and may talk to Binance's public data hosts and nobody else.
CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; "
    "connect-src 'self' https://data-api.binance.vision wss://data-stream.binance.vision; "
    "object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
)
MIME = {".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".json": "application/json"}


@app.before_request
def only_local_hosts():
    # Stops DNS-rebinding: a web page can't reach this no-login app through a hostname it controls.
    host = request.host
    if not host.endswith("]"):  # drop the :port, but keep a bare IPv6 literal like [::1] intact
        host = host.rsplit(":", 1)[0]
    if host not in LOCAL_HOSTS:
        return jsonify(error="this app only answers on localhost"), 403


@app.after_request
def security_headers(resp):
    resp.headers["X-Content-Type-Options"] = "nosniff"
    resp.headers["Referrer-Policy"] = "no-referrer"
    if resp.mimetype == "text/html":
        resp.headers["Content-Security-Policy"] = CSP
    return resp


@app.get("/api/config")
def config():
    return jsonify(
        sample=data.SAMPLE,
        crypto={"symbols": data.load_crypto_symbols()},
        india={"count": len(data.load_symbols())},
        fees=FEE_PCT,
    )


def want_sample() -> bool:
    """?sample=1 / ?sample=0 on a request; without it, the SAMPLE_DATA=1 environment decides."""
    v = request.args.get("sample")
    return data.SAMPLE if v is None else v in ("1", "true")


# ---------------------------------------------------------------- scanner
_scan_lock = threading.Lock()
_scan_cache: dict = {}  # sample flag -> {"t": time, "value": scan}


def run_scan(sample: bool) -> dict:
    frames = data.daily_all(sample=sample)
    rows = []
    for sym, df in frames.items():
        try:
            s = zones.summarize(df)
        except Exception:
            continue
        rows.append({"symbol": sym, **s})
    bull = sorted((r for r in rows if r["label"] == "bullish"), key=lambda r: -r["score"])[:20]
    bear = sorted((r for r in rows if r["label"] == "bearish"), key=lambda r: r["score"])[:20]
    counts = {
        "total": len(rows),
        "bullish": sum(r["label"] == "bullish" for r in rows),
        "bearish": sum(r["label"] == "bearish" for r in rows),
        "choppy": sum(r["label"] == "choppy" for r in rows),
        "advancing": sum(r["change_pct"] > 0 for r in rows),
        "declining": sum(r["change_pct"] < 0 for r in rows),
    }
    keep = ("symbol", "price", "change_pct", "score", "label")
    return {
        "updated": datetime.now(IST).strftime("%Y-%m-%d %H:%M:%S IST"),
        "sample": sample,
        "counts": counts,
        "bullish": bull,
        "bearish": bear,
        "all": [{k: r[k] for k in keep} for r in sorted(rows, key=lambda r: -r["score"])],
    }


@app.get("/api/scan")
def scan():
    sample = want_sample()
    with _scan_lock:
        hit = _scan_cache.get(sample)
        if hit is None or time.time() - hit["t"] > 60:
            hit = _scan_cache[sample] = {"t": time.time(), "value": run_scan(sample)}
        return jsonify(hit["value"])


# ------------------------------------------------------------------ chart
@app.get("/api/chart/<symbol>")
def chart(symbol: str):
    symbol = symbol.upper()
    if not SYMBOL_RE.match(symbol):
        return jsonify(error="invalid symbol"), 400
    interval = request.args.get("interval", "1d")
    if interval not in data.INTERVALS:
        return jsonify(error="interval must be one of " + ", ".join(data.INTERVALS)), 400
    df = data.get_ohlcv(symbol, interval, sample=want_sample())
    if df is None or len(df) < 40:
        return jsonify(error=f"no data for {symbol} (check the symbol or your connection)"), 404

    ts = data.epoch_seconds(df.index)
    a = zones.atr(df)
    obs = zones.order_blocks(df, a=a)
    liq = zones.liquidity_levels(df)
    zone_out, level_out = zones.chart_overlays(df, obs, liq, ts)

    shown = df.iloc[-300:]
    shown_ts = ts[-300:]
    candles = [
        {
            "time": int(t),
            "open": round(float(r.Open), 2),
            "high": round(float(r.High), 2),
            "low": round(float(r.Low), 2),
            "close": round(float(r.Close), 2),
            "volume": float(r.Volume),
        }
        for t, r in zip(shown_ts, shown.itertuples())
    ]
    return jsonify(
        symbol=symbol,
        interval=interval,
        candles=candles,
        zones=zone_out,
        levels=level_out,
        summary=zones.summarize(df, obs, liq),
    )


# ---------------------------------------------------------------- journal
DB_PATH = os.environ.get("JOURNAL_DB", str(HERE / "journal.db"))
_db_lock = threading.Lock()
_conn = sqlite3.connect(DB_PATH, check_same_thread=False)
_conn.row_factory = sqlite3.Row
_conn.execute(
    """CREATE TABLE IF NOT EXISTS trades (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
        qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
        fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
        market TEXT NOT NULL DEFAULT 'india',
        mode TEXT NOT NULL DEFAULT 'live'
    )"""
)
# Older journals: no market column (those trades were all NSE), no mode column (all live prices).
# (The old qty column was declared INTEGER; SQLite still stores fractional values in it as-is.)
_cols = {r["name"] for r in _conn.execute("PRAGMA table_info(trades)")}
if "market" not in _cols:
    _conn.execute("ALTER TABLE trades ADD COLUMN market TEXT NOT NULL DEFAULT 'india'")
if "mode" not in _cols:
    _conn.execute("ALTER TABLE trades ADD COLUMN mode TEXT NOT NULL DEFAULT 'live'")
# Open paper positions. Closing one writes a row to `trades` and deletes it here.
_conn.execute(
    """CREATE TABLE IF NOT EXISTS positions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        market TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'live',
        symbol TEXT NOT NULL, side TEXT NOT NULL,
        qty REAL NOT NULL, entry REAL NOT NULL, fee_pct REAL NOT NULL, opened_at TEXT NOT NULL
    )"""
)
# An earlier build recorded the order ticket's paper trades under "live" (meaning real prices). They were never
# real orders, so they belong to the demo account. Journal entries typed in by hand (any other note) stay live.
_conn.execute("UPDATE trades SET mode = 'demo' WHERE mode = 'live' AND notes = 'Paper trade'")
_conn.execute("UPDATE positions SET mode = 'demo' WHERE mode = 'live'")
_conn.commit()


def _pnl(r) -> float:
    gross = (r["exit"] - r["entry"]) * r["qty"] * (1 if r["side"] == "long" else -1)
    return gross - r["fees"]


def _trade_dict(r) -> dict:
    return {**dict(r), "pnl": round(_pnl(r), 2)}


def _journal_payload(market: str, mode: str) -> dict:
    with _db_lock:
        rows = _conn.execute("SELECT * FROM trades WHERE market = ? AND mode = ? ORDER BY date DESC, id DESC", (market, mode)).fetchall()
    trades = [_trade_dict(r) for r in rows]
    pnls = np.array([t["pnl"] for t in trades], dtype=float)
    wins, losses = pnls[pnls > 0], pnls[pnls <= 0]
    stats = {
        "trades": len(trades),
        "net_pnl": round(float(pnls.sum()), 2) if len(trades) else 0.0,
        "fees": round(float(sum(t["fees"] for t in trades)), 2),
        "win_rate": round(100 * len(wins) / len(trades), 1) if trades else 0.0,
        "avg_win": round(float(wins.mean()), 2) if len(wins) else 0.0,
        "avg_loss": round(float(losses.mean()), 2) if len(losses) else 0.0,
        "profit_factor": round(float(wins.sum() / -losses.sum()), 2) if len(losses) and losses.sum() < 0 else None,
    }
    return {"trades": trades, "stats": stats}


def _check_market_mode(market: str, mode: str) -> None:
    if market not in MARKETS:
        raise ValueError("market must be one of " + ", ".join(MARKETS))
    if mode not in MODES:
        raise ValueError("mode must be one of " + ", ".join(MODES))


@app.get("/api/journal")
def journal_get():
    market, mode = request.args.get("market", "india"), request.args.get("mode", "live")
    try:
        _check_market_mode(market, mode)
    except ValueError as e:
        return jsonify(error=str(e)), 400
    return jsonify(_journal_payload(market, mode))


@app.post("/api/journal")
def journal_add():
    b = request.get_json(silent=True) or {}
    try:
        date = str(b.get("date", ""))
        datetime.strptime(date, "%Y-%m-%d")
        market = str(b.get("market") or "india")
        mode = str(b.get("mode") or "live")
        symbol = str(b.get("symbol", "")).upper().strip()
        side = str(b.get("side", "")).lower()
        qty = float(b.get("qty"))
        entry, exit_ = float(b.get("entry")), float(b.get("exit"))
        fees = float(b.get("fees") or 0)
        notes = str(b.get("notes", ""))[:500]
        _check_market_mode(market, mode)
        if not SYMBOL_RE.match(symbol) or side not in ("long", "short"):
            raise ValueError("symbol or side invalid")
        if not all(np.isfinite([qty, entry, exit_, fees])):
            raise ValueError("numbers must be finite")
        if qty <= 0 or entry <= 0 or exit_ <= 0 or fees < 0:
            raise ValueError("qty, prices must be positive; fees cannot be negative")
        if market == "india" and qty != int(qty):
            raise ValueError("NSE quantity must be a whole number")
    except (ValueError, TypeError) as e:
        return jsonify(error=f"invalid trade: {e}"), 400
    with _db_lock:
        _conn.execute(
            "INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (date, symbol, side, qty, entry, exit_, fees, notes, market, mode),
        )
        _conn.commit()
    return jsonify(ok=True), 201


@app.delete("/api/journal/<int:trade_id>")
def journal_delete(trade_id: int):
    with _db_lock:
        _conn.execute("DELETE FROM trades WHERE id = ?", (trade_id,))
        _conn.commit()
    return jsonify(ok=True)


# -------------------------------------------------------- paper positions
# Orders are simulated: a market order fills at the price the page sends (the real price on screen).
# Closing a position records it in the journal. No broker is involved and no real money moves, so only the
# demo account can open positions: a "live" order would claim a real trade that never happened.
def _positive(v, name: str) -> float:
    x = float(v)
    if not np.isfinite(x) or x <= 0:
        raise ValueError(f"{name} must be a positive number")
    return x


@app.get("/api/positions")
def positions_get():
    market, mode = request.args.get("market", "india"), request.args.get("mode", "demo")
    try:
        _check_market_mode(market, mode)
    except ValueError as e:
        return jsonify(error=str(e)), 400
    with _db_lock:
        rows = _conn.execute("SELECT * FROM positions WHERE market = ? AND mode = ? ORDER BY id DESC", (market, mode)).fetchall()
    return jsonify(positions=[dict(r) for r in rows])


@app.post("/api/positions")
def positions_open():
    b = request.get_json(silent=True) or {}
    try:
        market, mode = str(b.get("market") or "india"), str(b.get("mode") or "demo")
        _check_market_mode(market, mode)
        if mode != "demo":
            raise ValueError("live orders aren't supported: no broker is connected, so only the demo account can trade")
        symbol = str(b.get("symbol", "")).upper().strip()
        side = str(b.get("side", "")).lower()
        if not SYMBOL_RE.match(symbol) or side not in ("long", "short"):
            raise ValueError("symbol or side invalid")
        qty, price = _positive(b.get("qty"), "qty"), _positive(b.get("price"), "price")
        if market == "india" and qty != int(qty):
            raise ValueError("NSE quantity must be a whole number")
    except (ValueError, TypeError) as e:
        return jsonify(error=f"invalid order: {e}"), 400
    opened = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    with _db_lock:
        cur = _conn.execute(
            "INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES (?,?,?,?,?,?,?,?)",
            (market, mode, symbol, side, qty, price, FEE_PCT[market], opened),
        )
        _conn.commit()
        row = _conn.execute("SELECT * FROM positions WHERE id = ?", (cur.lastrowid,)).fetchone()
    return jsonify(ok=True, position=dict(row)), 201


@app.post("/api/positions/<int:pid>/close")
def positions_close(pid: int):
    b = request.get_json(silent=True) or {}
    try:
        price = _positive(b.get("price"), "price")
        date = b.get("date")
        if date is not None:
            datetime.strptime(str(date), "%Y-%m-%d")
    except (ValueError, TypeError) as e:
        return jsonify(error=f"invalid close: {e}"), 400
    with _db_lock:
        pos = _conn.execute("SELECT * FROM positions WHERE id = ?", (pid,)).fetchone()
        if pos is None:
            return jsonify(error="no such open position"), 404
        day = str(date) if date else datetime.now(IST if pos["market"] == "india" else timezone.utc).strftime("%Y-%m-%d")
        fees = pos["fee_pct"] / 100 * pos["qty"] * (pos["entry"] + price)  # both sides, at their own prices
        try:
            cur = _conn.execute(
                "INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES (?,?,?,?,?,?,?,?,?,?)",
                (day, pos["symbol"], pos["side"], pos["qty"], pos["entry"], price, fees, "Paper trade", pos["market"], pos["mode"]),
            )
            _conn.execute("DELETE FROM positions WHERE id = ?", (pid,))
            _conn.commit()
        except sqlite3.Error:
            _conn.rollback()
            raise
        trade = _conn.execute("SELECT * FROM trades WHERE id = ?", (cur.lastrowid,)).fetchone()
    return jsonify(ok=True, trade=_trade_dict(trade))


# ----------------------------------------------------------------- static
@app.get("/")
def index():
    return send_from_directory(HERE / "static", "index.html")


@app.get("/static/<path:filename>")
def static_files(filename: str):
    # mimetype is set explicitly: Windows can map .js to text/plain, which browsers refuse for ES modules.
    resp = send_from_directory(HERE / "static", filename, mimetype=MIME.get(Path(filename).suffix.lower()))
    if filename.startswith("vendor/"):  # versioned file names, safe to cache for a year
        resp.cache_control.max_age = 31536000
        resp.cache_control.immutable = True
    return resp


if __name__ == "__main__":
    # 127.0.0.1 only: this app has no login, so it must not be exposed to your network.
    app.run(host="127.0.0.1", port=int(os.environ.get("PORT", "5000")), debug=False)
