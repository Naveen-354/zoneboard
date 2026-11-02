"""Tests for the zone logic and API. Run:  python -m pytest -q   (or python test_zones.py)"""
import os

os.environ["DEMO"] = "1"
os.environ.setdefault("JOURNAL_DB", ":memory:")

import numpy as np
import pandas as pd

import backtest
import zones


def make_df(rows):
    idx = pd.bdate_range("2025-01-01", periods=len(rows))
    return pd.DataFrame(rows, columns=["Open", "High", "Low", "Close", "Volume"], index=idx)


def flat_then(rows_tail, n=30):
    """30 quiet bars (range ~2) followed by custom bars."""
    base = [[100, 101, 99, 100.5, 1000]] * n
    return make_df(base + rows_tail)


def test_bullish_order_block_found_and_bounds():
    # bearish candle 100->98 (range 97.5..100.2), then a strong up bar closing 106
    df = flat_then([[100, 100.2, 97.5, 98, 1000], [98, 106.5, 97.8, 106, 3000], [106, 107, 105, 106.5, 1000]])
    obs = zones.order_blocks(df)
    bull = [o for o in obs if o["type"] == "bullish"]
    assert len(bull) == 1
    ob = bull[0]
    assert ob["idx"] == 30 and ob["confirm_idx"] == 31
    assert ob["top"] == 100.2 and ob["bottom"] == 97.5
    assert ob["active"]


def test_order_block_mitigated_when_close_breaks_it():
    df = flat_then(
        [
            [100, 100.2, 97.5, 98, 1000],
            [98, 106.5, 97.8, 106, 3000],
            [106, 107, 96, 96.5, 2000],  # closes below the zone bottom (97.5)
        ]
    )
    ob = [o for o in zones.order_blocks(df) if o["type"] == "bullish"][0]
    assert not ob["active"] and ob["mitigated_idx"] == 32


def test_bearish_order_block_is_mirror():
    df = flat_then([[100, 102.5, 99.8, 102, 1000], [102, 102.2, 94, 94.5, 3000], [94.5, 95, 93.5, 94, 1000]])
    bear = [o for o in zones.order_blocks(df) if o["type"] == "bearish"]
    assert len(bear) == 1 and bear[0]["top"] == 102.5 and bear[0]["bottom"] == 99.8


def test_no_order_block_without_displacement():
    df = flat_then([[100, 100.5, 99.5, 99.8, 1000], [99.8, 100.6, 99.6, 100.4, 1000]])
    assert [o for o in zones.order_blocks(df) if o["idx"] >= 30] == []


def test_liquidity_equal_highs_cluster_and_sweep():
    rows = []
    price = 100.0
    # two equal highs near 110 with dips in between, then a sweep above
    path = [100, 104, 108, 110, 108, 104, 102, 105, 109, 110, 108, 104, 101, 103, 106, 108, 111]
    for p in path:
        rows.append([p - 0.3, p + 0.2, p - 0.6, p, 1000])
    df = make_df(rows)
    liq = [x for x in zones.liquidity_levels(df, left=2, right=2) if x["kind"] == "high"]
    cluster = max(liq, key=lambda x: x["touches"])
    assert cluster["touches"] == 2
    assert cluster["swept_idx"] is not None  # final bar trades above ~110.2


def test_summarize_shape():
    from data import _synthetic

    s = zones.summarize(_synthetic("TEST", "1d"))
    assert s["bias"] in ("bullish", "bearish")
    assert -100 <= s["score"] <= 100
    assert s["label"] in ("bullish", "bearish", "choppy")


def test_backtest_no_lookahead_and_costs_reduce_result():
    from data import _synthetic

    df = _synthetic("ABC", "1d")
    rows = backtest.backtest_symbol(df, rr=2.0, cost_pct=0.001)
    for r in rows:
        assert r["net_R"] <= r["gross_R"]
    ob_confirms = {o["confirm_idx"] for o in zones.order_blocks(df)}
    # every entry happens strictly after some confirmation bar
    assert all(df.index.get_loc(r["date"]) > min(ob_confirms) for r in rows)


def test_backtest_hand_example_hits_target():
    # bullish OB zone 97.5..100.2 confirmed at bar 31; bar 32 retraces to 100 then rallies
    tail = [
        [100, 100.2, 97.5, 98, 1000],
        [98, 106.5, 97.8, 106, 3000],
        [106, 106.2, 100.0, 105, 1000],  # entry at 100.2 (zone top), no stop (low 100.0 > 97.5)
        [105, 112, 104, 111, 1000],  # target = 100.2 + 2*2.7 = 105.6 -> hit
    ]
    df = flat_then(tail)
    rows = [r for r in backtest.backtest_symbol(df, rr=2.0, cost_pct=0.0) if r["side"] == "long"]
    assert len(rows) == 1 and abs(rows[0]["gross_R"] - 2.0) < 1e-9


def test_api_endpoints():
    import app as app_module

    client = app_module.app.test_client()
    scan = client.get("/api/scan").get_json()
    assert scan["demo"] is True and scan["counts"]["total"] > 10
    assert all(r["score"] >= 15 for r in scan["bullish"])
    sym = scan["bullish"][0]["symbol"] if scan["bullish"] else "SBIN"
    for interval in ("1d", "1h", "15m"):
        ch = client.get(f"/api/chart/{sym}?interval={interval}").get_json()
        assert len(ch["candles"]) > 40 and "summary" in ch
        assert all(z["top"] > z["bottom"] for z in ch["zones"])
    assert client.get("/api/chart/..%2Fetc?interval=1d").status_code in (400, 404)
    assert client.get("/api/chart/SBIN?interval=bad").status_code == 400

    r = client.post("/api/journal", json={"date": "2026-10-30", "symbol": "sbin", "side": "long", "qty": 10, "entry": 100, "exit": 110, "fees": 20, "notes": "<b>x</b>"})
    assert r.status_code == 201
    r = client.post("/api/journal", json={"date": "2026-10-30", "symbol": "INFY", "side": "short", "qty": 5, "entry": 100, "exit": 110, "fees": 10})
    assert r.status_code == 201
    j = client.get("/api/journal").get_json()
    assert j["stats"]["trades"] == 2
    assert abs(j["stats"]["net_pnl"] - (100 - 20 + (-50) - 10)) < 1e-6
    bad = client.post("/api/journal", json={"date": "nope", "symbol": "X", "side": "long", "qty": 1, "entry": 1, "exit": 1})
    assert bad.status_code == 400
    assert client.delete(f"/api/journal/{j['trades'][0]['id']}").status_code == 200


if __name__ == "__main__":
    import sys

    import pytest

    sys.exit(pytest.main([__file__, "-q"]))
