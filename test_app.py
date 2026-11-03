"""Server tests: config, static files, journal per market, host check. Run:  python -m pytest -q"""
import os
import sqlite3
import subprocess
import sys
from pathlib import Path

os.environ["SAMPLE_DATA"] = "1"
os.environ.setdefault("JOURNAL_DB", ":memory:")

import app as app_module

HERE = Path(__file__).parent
client = app_module.app.test_client()


def test_config_lists_crypto_symbols():
    c = client.get("/api/config").get_json()
    assert c["sample"] is True  # the test run uses made-up sample data
    syms = c["crypto"]["symbols"]
    assert "BTCUSDT" in syms and len(syms) >= 20 and all(s.endswith("USDT") for s in syms)
    assert c["india"]["count"] > 10


def test_config_reports_the_simulated_fee_rates():
    assert client.get("/api/config").get_json()["fees"] == {"crypto": 0.1, "india": 0.05}


def test_sample_data_flag_per_request_overrides_the_environment():
    # the environment says SAMPLE_DATA=1; a request can still ask for real data (not fetched here: no network in tests)
    with app_module.app.test_request_context("/api/scan"):
        assert app_module.want_sample() is True
    with app_module.app.test_request_context("/api/scan?sample=0"):
        assert app_module.want_sample() is False
    with app_module.app.test_request_context("/api/scan?sample=1"):
        assert app_module.want_sample() is True
    assert client.get("/api/scan?sample=1").get_json()["sample"] is True


def _open(**kw):
    # "demo" is the paper-trading account: real prices, simulated fills
    body = {"market": "crypto", "mode": "demo", "symbol": "ZZPAPERUSDT", "side": "long", "qty": 0.01, "price": 78000, **kw}
    return client.post("/api/positions", json=body)


def _cleanup(market, mode="demo"):
    for p in client.get(f"/api/positions?market={market}&mode={mode}").get_json()["positions"]:
        if p["symbol"].startswith("ZZ"):
            client.post(f"/api/positions/{p['id']}/close", json={"price": p["entry"]})
    for t in client.get(f"/api/journal?market={market}&mode={mode}").get_json()["trades"]:
        if t["symbol"].startswith("ZZ"):
            client.delete(f"/api/journal/{t['id']}")


def test_paper_position_lifecycle_long_and_short():
    try:
        r = _open()
        assert r.status_code == 201
        pos = r.get_json()["position"]
        assert pos["fee_pct"] == 0.1 and pos["entry"] == 78000 and pos["opened_at"].endswith("Z") and pos["mode"] == "demo"
        assert [p["id"] for p in client.get("/api/positions?market=crypto&mode=demo").get_json()["positions"] if p["symbol"] == "ZZPAPERUSDT"] == [pos["id"]]

        r = client.post(f"/api/positions/{pos['id']}/close", json={"price": 79000, "date": "2026-11-02"})
        assert r.status_code == 200
        t = r.get_json()["trade"]
        fees = 0.1 / 100 * 0.01 * (78000 + 79000)  # each side at its own price
        assert abs(t["fees"] - fees) < 1e-9
        assert t["pnl"] == round(0.01 * 1000 - fees, 2) and t["date"] == "2026-11-02" and t["mode"] == "demo"
        assert client.post(f"/api/positions/{pos['id']}/close", json={"price": 79000}).status_code == 404  # already closed

        # the closed trade is in the demo journal; the position is gone
        assert any(x["symbol"] == "ZZPAPERUSDT" for x in client.get("/api/journal?market=crypto&mode=demo").get_json()["trades"])
        assert not [p for p in client.get("/api/positions?market=crypto&mode=demo").get_json()["positions"] if p["symbol"] == "ZZPAPERUSDT"]

        # a short on NSE wins when price falls: (100-90)*10 - fees(0.05% of 100*10 + 90*10)
        pid = _open(market="india", symbol="ZZSHORT", side="short", qty=10, price=100).get_json()["position"]["id"]
        t = client.post(f"/api/positions/{pid}/close", json={"price": 90}).get_json()["trade"]
        assert t["pnl"] == round(100 - 0.05 / 100 * 10 * 190, 2)
    finally:
        _cleanup("crypto")
        _cleanup("india")


def test_live_account_cannot_place_orders_and_demo_trades_stay_out_of_it():
    # no broker is connected, so a "live" order would claim a real trade that never happened
    r = _open(mode="live", symbol="ZZLIVEUSDT")
    assert r.status_code == 400 and "live orders" in r.get_json()["error"]
    assert not [p for p in client.get("/api/positions?market=crypto&mode=live").get_json()["positions"] if p["symbol"] == "ZZLIVEUSDT"]
    try:
        pid = _open(symbol="ZZDEMOUSDT").get_json()["position"]["id"]
        client.post(f"/api/positions/{pid}/close", json={"price": 78100})
        assert not [t for t in client.get("/api/journal?market=crypto&mode=live").get_json()["trades"] if t["symbol"] == "ZZDEMOUSDT"]
        assert [t for t in client.get("/api/journal?market=crypto&mode=demo").get_json()["trades"] if t["symbol"] == "ZZDEMOUSDT"]
    finally:
        _cleanup("crypto")


def test_order_validation():
    for bad in (
        {"price": 0}, {"price": -5}, {"price": "nan"}, {"price": "inf"}, {"qty": 0}, {"qty": "abc"},
        {"side": "buy"}, {"market": "forex"}, {"mode": "paper"}, {"symbol": "../etc"}, {"symbol": ""},
        {"market": "india", "qty": 1.5},  # NSE lots are whole
    ):
        assert _open(**bad).status_code == 400, bad
    assert client.get("/api/positions?market=forex").status_code == 400
    assert client.get("/api/positions?market=crypto&mode=paper").status_code == 400
    assert client.post("/api/positions/999999/close", json={"price": 1}).status_code == 404
    pid = _open().get_json()["position"]["id"]
    try:
        assert client.post(f"/api/positions/{pid}/close", json={"price": 0}).status_code == 400
        assert client.post(f"/api/positions/{pid}/close", json={"price": 5, "date": "nope"}).status_code == 400
        assert client.post(f"/api/positions/{pid}/close", json={}).status_code == 400
    finally:
        _cleanup("crypto")


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
        "c.get('/api/journal?market=crypto').get_json()['stats']['trades'],"
        "c.get('/api/journal?market=india&mode=demo').get_json()['stats']['trades'],"
        "c.get('/api/positions?market=india').get_json()['positions']]))"
    )
    env = {**os.environ, "JOURNAL_DB": str(db), "SAMPLE_DATA": "1"}
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=HERE, env=env, timeout=60)
    assert res.returncode == 0, res.stderr
    # old trade is NSE at live prices: visible there, absent from crypto and from demo; no open positions
    assert res.stdout.strip().splitlines()[-1] == "[1, 0, 0, []]"


def test_earlier_paper_trades_recorded_as_live_move_to_the_demo_account(tmp_path):
    # the previous build filed the ticket's paper trades under "live" (meaning real prices); they were never real orders
    db = tmp_path / "prev.db"
    con = sqlite3.connect(db)
    con.executescript(
        """CREATE TABLE trades (id INTEGER PRIMARY KEY AUTOINCREMENT, date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
        qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL, fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
        market TEXT NOT NULL DEFAULT 'india', mode TEXT NOT NULL DEFAULT 'live');
        CREATE TABLE positions (id INTEGER PRIMARY KEY AUTOINCREMENT, market TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'live',
        symbol TEXT NOT NULL, side TEXT NOT NULL, qty REAL NOT NULL, entry REAL NOT NULL, fee_pct REAL NOT NULL, opened_at TEXT NOT NULL);
        INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES
          ('2026-11-02','BTCUSDT','long',0.01,78000,79000,1.5,'Paper trade','crypto','live'),
          ('2026-10-01','SBIN','long',10,100,110,5,'bought via my broker','india','live');
        INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES ('crypto','live','ETHUSDT','long',1,2000,0.1,'2026-11-02T10:00:00Z');"""
    )
    con.commit()
    con.close()
    code = (
        "import json, app; c = app.app.test_client();"
        "j = lambda m, md: [t['symbol'] for t in c.get(f'/api/journal?market={m}&mode={md}').get_json()['trades']];"
        "p = lambda md: [x['symbol'] for x in c.get(f'/api/positions?market=crypto&mode={md}').get_json()['positions']];"
        "print(json.dumps([j('crypto','live'), j('crypto','demo'), j('india','live'), p('live'), p('demo')]))"
    )
    env = {**os.environ, "JOURNAL_DB": str(db), "SAMPLE_DATA": "1"}
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, cwd=HERE, env=env, timeout=60)
    assert res.returncode == 0, res.stderr
    # the paper trade and the open paper position are demo now; the hand-entered broker trade stays live
    assert res.stdout.strip().splitlines()[-1] == '[[], ["BTCUSDT"], ["SBIN"], [], ["ETHUSDT"]]'
