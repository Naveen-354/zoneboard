"""Server tests: config, static files, journal per market, host check. Run:  python -m pytest -q"""
import os
import sqlite3
import subprocess
import sys
from pathlib import Path

os.environ["DEMO"] = "1"
os.environ.setdefault("JOURNAL_DB", ":memory:")

import app as app_module

HERE = Path(__file__).parent
client = app_module.app.test_client()


def test_config_lists_crypto_symbols():
    c = client.get("/api/config").get_json()
    assert c["demo"] is True
    syms = c["crypto"]["symbols"]
    assert "BTCUSDT" in syms and len(syms) >= 20 and all(s.endswith("USDT") for s in syms)
    assert c["india"]["count"] > 10


def test_static_files_have_module_safe_mime_types():
    for path, mime in (
        ("/static/js/main.js", "text/javascript"),
        ("/static/js/markets/crypto.js", "text/javascript"),
        ("/static/css/app.css", "text/css"),
        ("/static/vendor/lightweight-charts.5.2.1.js", "text/javascript"),
    ):
        r = client.get(path)
        assert r.status_code == 200, path
        assert r.mimetype == mime, path
    assert "immutable" in client.get("/static/vendor/lightweight-charts.5.2.1.js").headers["Cache-Control"]
    assert client.get("/static/../app.py").status_code == 404
    assert client.get("/static/nope.js").status_code == 404


def test_index_sets_csp_and_nosniff():
    r = client.get("/")
    assert r.status_code == 200
    csp = r.headers["Content-Security-Policy"]
    assert "script-src 'self'" in csp and "wss://data-stream.binance.vision" in csp
    assert r.headers["X-Content-Type-Options"] == "nosniff"


def test_rejects_foreign_host_header():
    assert client.get("/api/config", headers={"Host": "evil.example.com"}).status_code == 403
    assert client.get("/api/config", headers={"Host": "127.0.0.1:5000"}).status_code == 200
    assert client.get("/api/config", headers={"Host": "[::1]:5000"}).status_code == 200


def test_scan_and_chart_shapes_for_the_ui():
    scan = client.get("/api/scan").get_json()
    assert len(scan["all"]) == scan["counts"]["total"]
    assert set(scan["all"][0]) == {"symbol", "price", "change_pct", "score", "label"}
    ch = client.get(f"/api/chart/{scan['all'][0]['symbol']}?interval=1d").get_json()
    assert all("volume" in c for c in ch["candles"])
    # times are true unix seconds now (the browser localises them), not India-shifted
    assert abs(ch["candles"][-1]["time"] - __import__("time").time()) < 3 * 86400


def test_journal_is_kept_per_market_and_accepts_fractional_crypto_qty():
    base = {"date": "2026-11-02", "side": "long", "entry": 100, "exit": 110, "fees": 1}
    r = client.post("/api/journal", json={**base, "market": "crypto", "symbol": "btcusdt", "qty": 0.0153})
    assert r.status_code == 201
    r = client.post("/api/journal", json={**base, "market": "india", "symbol": "ZZTEST", "qty": 10})
    assert r.status_code == 201

    crypto = client.get("/api/journal?market=crypto").get_json()
    india = client.get("/api/journal?market=india").get_json()
    try:
        assert [t["symbol"] for t in crypto["trades"]] == ["BTCUSDT"]
        assert crypto["trades"][0]["qty"] == 0.0153
        assert abs(crypto["stats"]["net_pnl"] - (0.0153 * 10 - 1)) < 0.01  # P&L is reported to the cent
        india_syms = [t["symbol"] for t in india["trades"]]
        assert "ZZTEST" in india_syms and "BTCUSDT" not in india_syms
    finally:  # the journal is shared with the other test modules in this process: remove only our rows
        for t in crypto["trades"] + [t for t in india["trades"] if t["symbol"] == "ZZTEST"]:
            client.delete(f"/api/journal/{t['id']}")


def test_journal_validation():
    ok = {"date": "2026-11-02", "side": "long", "entry": 1, "exit": 2, "symbol": "X", "qty": 1}
    assert client.post("/api/journal", json={**ok, "market": "forex"}).status_code == 400
    assert client.get("/api/journal?market=forex").status_code == 400
    assert client.post("/api/journal", json={**ok, "market": "india", "qty": 1.5}).status_code == 400  # NSE lots are whole
    assert client.post("/api/journal", json={**ok, "market": "crypto", "qty": 0}).status_code == 400
    assert client.post("/api/journal", json={**ok, "market": "crypto", "qty": "nan"}).status_code == 400
    assert client.post("/api/journal", json={**ok, "market": "crypto", "qty": "inf"}).status_code == 400


def test_old_journal_gets_market_column_and_keeps_its_trades(tmp_path):
    db = tmp_path / "old.db"
    con = sqlite3.connect(db)
    con.execute(
        """CREATE TABLE trades (id INTEGER PRIMARY KEY AUTOINCREMENT, date TEXT NOT NULL, symbol TEXT NOT NULL,
        side TEXT NOT NULL, qty INTEGER NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
        fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '')"""
    )
    con.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes) VALUES ('2026-10-01','SBIN','long',10,100,110,5,'old')")
    con.commit()
    con.close()

    code = (
        "import json, app; c = app.app.test_client();"
        "print(json.dumps([c.get('/api/journal?market=india').get_json()['stats']['trades'],"
        "c.get('/api/journal?market=crypto').get_json()['stats']['trades']]))"
    )
    env = {**os.environ, "JOURNAL_DB": str(db), "DEMO": "1"}
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=HERE, env=env, timeout=60)
    assert res.returncode == 0, res.stderr
    assert res.stdout.strip().splitlines()[-1] == "[1, 0]"  # old trade is NSE, crypto journal starts empty
