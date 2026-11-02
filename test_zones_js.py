"""Checks that the browser zone engine (static/js/zones.js) agrees with zones.py on the same bars.

The live crypto view recomputes zones in JavaScript on every candle, while the backtest and the NSE scanner
use zones.py. If the two ever drift, the chart would show something the backtest never tested.
Skipped when Node.js is not installed.
"""
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

os.environ["DEMO"] = "1"

import pytest

import data
import zones

HERE = Path(__file__).parent
NODE = shutil.which("node")

pytestmark = pytest.mark.skipif(NODE is None, reason="node is not installed")


def _frames():
    out = {}
    for sym in ("AAA", "BBB", "CCC", "DDD"):
        for interval in ("1d", "1h", "15m"):
            out[f"{sym}-{interval}"] = data._synthetic(sym, interval)
    return out


def _js(frames_json):
    p = HERE / ".parity_frames.json"
    p.write_text(json.dumps(frames_json))
    try:
        res = subprocess.run([NODE, str(HERE / "zones_parity.mjs"), str(p)], capture_output=True, text=True, timeout=60, cwd=HERE)
    finally:
        p.unlink(missing_ok=True)
    assert res.returncode == 0, res.stderr
    return json.loads(res.stdout)


def _round_zone(z):
    return {**z, "top": round(z["top"], 2), "bottom": round(z["bottom"], 2)}


def _round_level(x):
    return {**x, "price": round(x["price"], 2)}


def test_js_engine_matches_python():
    frames = _frames()
    payload = {}
    for name, df in frames.items():
        payload[name] = {
            "t": [int(x) for x in data.epoch_seconds(df.index)],
            "o": df["Open"].tolist(),
            "h": df["High"].tolist(),
            "l": df["Low"].tolist(),
            "c": df["Close"].tolist(),
            "v": df["Volume"].tolist(),
        }
    js = _js(payload)

    n_zones = n_levels = 0
    for name, df in frames.items():
        ts = data.epoch_seconds(df.index)
        obs = zones.order_blocks(df)
        liq = zones.liquidity_levels(df)
        py_zones, py_levels = zones.chart_overlays(df, obs, liq, ts)
        py_sum = zones.summarize(df, obs, liq)

        got = js[name]
        js_zones = [_round_zone({k: v for k, v in z.items() if k != "forming"}) for z in got["zones"]]
        js_levels = [_round_level(x) for x in got["levels"]]
        assert js_zones == py_zones, name
        assert js_levels == py_levels, name

        s = got["summary"]
        assert s["bias"] == py_sum["bias"] and s["label"] == py_sum["label"], name
        for k in ("price", "score", "change_pct", "rvol", "atr", "support", "resistance", "liquidity"):
            if py_sum[k] is None:
                assert s[k] is None, (name, k)
            else:
                assert s[k] == pytest.approx(py_sum[k], abs=0.011), (name, k)
        assert (s["ob"] is None) == (py_sum["ob"] is None), name
        if py_sum["ob"]:
            assert s["ob"]["type"] == py_sum["ob"]["type"], name
            assert s["ob"]["top"] == pytest.approx(py_sum["ob"]["top"], abs=0.011), name
        n_zones += len(py_zones)
        n_levels += len(py_levels)

    # the comparison must not pass vacuously
    assert n_zones > 10 and n_levels > 30


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
