"""Writes tests/fixtures/parity.json: bars plus the answers zones.py and backtest.py give for them.

The Rust tests replay these bars and must reproduce the answers, so the Rust engine can't drift from
the Python reference. Regenerate only when the zone rules or the backtest rules deliberately change:

    python src-tauri/tests/make_fixture.py
"""
import json
import os
import sys
from pathlib import Path

os.environ["SAMPLE_DATA"] = "1"
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

import backtest
import data
import zones

RR, COST = 2.0, 0.001
cases = []
for sym in ("AAA", "BBB", "CCC"):
    for interval in ("1d", "15m"):
        df = data._synthetic(sym, interval)
        ts = [int(x) for x in data.epoch_seconds(df.index)]
        a = zones.atr(df)
        obs = zones.order_blocks(df, a=a)
        liq = zones.liquidity_levels(df)
        zone_out, level_out = zones.chart_overlays(df, obs, liq, ts)
        trades = [
            [int(df.index.get_loc(r["date"])), r["side"], r["net_R"], r["gross_R"]]
            for r in backtest.backtest_symbol(df, rr=RR, cost_pct=COST)
        ]
        cases.append(
            {
                "name": f"{sym}-{interval}",
                "t": ts,
                "o": df["Open"].tolist(),
                "h": df["High"].tolist(),
                "l": df["Low"].tolist(),
                "c": df["Close"].tolist(),
                "v": df["Volume"].tolist(),
                "zones": zone_out,
                "levels": level_out,
                "summary": zones.summarize(df, obs, liq),
                "trades": trades,
            }
        )

out = ROOT / "src-tauri" / "tests" / "fixtures" / "parity.json"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps({"rr": RR, "cost_pct": COST, "cases": cases}))
print(f"wrote {out} ({out.stat().st_size // 1024} KB): {len(cases)} cases, "
      f"{sum(len(c['zones']) for c in cases)} zones, {sum(len(c['trades']) for c in cases)} trades")
