"""ZoneBoard: a personal F&O scanner + chart + journal using free, delayed data.

Run:  python app.py       then open http://127.0.0.1:5000
Demo: DEMO=1 python app.py   (synthetic data, no internet needed)
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
        demo=data.DEMO,
        crypto={"symbols": data.load_crypto_symbols()},
        india={"count": len(data.load_symbols())},
    )

# ---------------------------------------------------------------- scanner
_scan_lock = threading.Lock()
_scan_cache: dict = {"t": 0, "value": None}


def run_scan() -> dict:
    frames = data.daily_all()
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
        "demo": data.DEMO,
        "counts": counts,
        "bullish": bull,
        "bearish": bear,
        "all": [{k: r[k] for k in keep} for r in sorted(rows, key=lambda r: -r["score"])],
    }


@app.get("/api/scan")
def scan():
    with _scan_lock:
        if time.time() - _scan_cache["t"] > 60 or _scan_cache["value"] is None:
            _scan_cache["value"] = run_scan()
            _scan_cache["t"] = time.time()
        return jsonify(_scan_cache["value"])


# ------------------------------------------------------------------ chart
@app.get("/api/chart/<symbol>")
def chart(symbol: str):
    symbol = symbol.upper()
    if not SYMBOL_RE.match(symbol):
        return jsonify(error="invalid symbol"), 400
    interval = request.args.get("interval", "1d")
    if interval not in data.INTERVALS:
        return jsonify(error="interval must be one of " + ", ".join(data.INTERVALS)), 400
    df = data.get_ohlcv(symbol, interval)
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
        market TEXT NOT NULL DEFAULT 'india'
    )"""
)
# Journals made before crypto existed have no market column: those trades were all NSE.
# (The old qty column was declared INTEGER; SQLite still stores fractional values in it as-is.)
if "market" not in {r["name"] for r in _conn.execute("PRAGMA table_info(trades)")}:
    _conn.execute("ALTER TABLE trades ADD COLUMN market TEXT NOT NULL DEFAULT 'india'")
_conn.commit()


def _pnl(r) -> float:
    gross = (r["exit"] - r["entry"]) * r["qty"] * (1 if r["side"] == "long" else -1)
    return gross - r["fees"]


def _journal_payload(market: str) -> dict:
    with _db_lock:
        rows = _conn.execute("SELECT * FROM trades WHERE market = ? ORDER BY date DESC, id DESC", (market,)).fetchall()
    trades = [{**dict(r), "pnl": round(_pnl(r), 2)} for r in rows]
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


@app.get("/api/journal")
def journal_get():
    market = request.args.get("market", "india")
    if market not in MARKETS:
        return jsonify(error="market must be one of " + ", ".join(MARKETS)), 400
    return jsonify(_journal_payload(market))


@app.post("/api/journal")
def journal_add():
    b = request.get_json(silent=True) or {}
    try:
        date = str(b.get("date", ""))
        datetime.strptime(date, "%Y-%m-%d")
        market = str(b.get("market") or "india")
        symbol = str(b.get("symbol", "")).upper().strip()
        side = str(b.get("side", "")).lower()
        qty = float(b.get("qty"))
        entry, exit_ = float(b.get("entry")), float(b.get("exit"))
        fees = float(b.get("fees") or 0)
        notes = str(b.get("notes", ""))[:500]
        if market not in MARKETS:
            raise ValueError("market must be one of " + ", ".join(MARKETS))
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
            "INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market) VALUES (?,?,?,?,?,?,?,?,?)",
            (date, symbol, side, qty, entry, exit_, fees, notes, market),
        )
        _conn.commit()
    return jsonify(ok=True), 201


@app.delete("/api/journal/<int:trade_id>")
def journal_delete(trade_id: int):
    with _db_lock:
        _conn.execute("DELETE FROM trades WHERE id = ?", (trade_id,))
        _conn.commit()
    return jsonify(ok=True)


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
