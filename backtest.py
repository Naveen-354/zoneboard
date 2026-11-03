"""FROZEN (T009, decision D4): legacy Python backend, kept as the reference for the zone/backtest
rules and the parity fixture (backtest reference). New features go to the Rust app only; change this
file only to keep the parity fixture and existing tests green. See AGENTS.md.

Backtest a simple order-block retest rule on daily data.

Rule (long side; short side is the mirror image):
  * A bullish order block is confirmed on bar `confirm_idx`.
  * From the next bar, wait up to WAIT bars for price to retrace into the zone.
  * Enter at the zone top (or the open if it gaps lower inside the zone).
    Skip the setup if price gaps below the whole zone.
  * Stop at the zone bottom. Target at RR x risk. Exit at the close after MAX_HOLD bars.
  * Stop is checked before target on the same bar (pessimistic).
  * No target is allowed on the entry bar itself.
  * A round-trip cost (brokerage, taxes, slippage) is charged in R terms.

Usage:  python backtest.py [--period 5y] [--rr 2] [--cost 0.1]
        SAMPLE_DATA=1 python backtest.py   # random-walk sample data, should show NO edge
"""
from __future__ import annotations

import argparse

import numpy as np
import pandas as pd

import zones

WAIT = 30
MAX_HOLD = 10


def _simulate_long(o, h, l, c, a, obs, rr, cost_pct):
    """Simulate long trades on the given arrays. Returns list of (entry_idx, net_R, gross_R)."""
    n = len(o)
    trades = []
    for ob in obs:
        top, bot = ob["top"], ob["bottom"]
        start = ob["confirm_idx"] + 1
        entry_idx = None
        for j in range(start, min(n, start + WAIT)):
            if o[j] <= bot:  # gapped through the whole zone: skip
                break
            if l[j] <= top:
                entry_idx = j
                break
            if c[j] < bot:  # invalidated before retest
                break
        if entry_idx is None:
            continue
        j = entry_idx
        entry = min(o[j], top)
        risk = entry - bot
        if risk < 0.2 * a[ob["idx"]] or risk <= 0:
            continue
        stop, target = bot, entry + rr * risk
        exit_price = None
        if l[j] <= stop:  # entered and stopped on the same bar (pessimistic)
            exit_price = stop
        else:
            last = min(n - 1, j + MAX_HOLD)
            for m in range(j + 1, last + 1):
                if o[m] <= stop:
                    exit_price = o[m]
                elif l[m] <= stop:
                    exit_price = stop
                elif o[m] >= target:
                    exit_price = o[m]
                elif h[m] >= target:
                    exit_price = target
                if exit_price is not None:
                    break
            if exit_price is None:
                exit_price = c[last]
        gross = (exit_price - entry) / risk
        net = gross - cost_pct * abs(entry) / risk  # abs(): shorts run on negated prices
        trades.append((entry_idx, float(net), float(gross)))
    return trades


def backtest_symbol(df: pd.DataFrame, rr: float = 2.0, cost_pct: float = 0.001):
    a = zones.atr(df)
    obs = zones.order_blocks(df, a=a)
    o, h, l, c = (df[k].to_numpy() for k in ("Open", "High", "Low", "Close"))
    av = a.to_numpy()
    longs = _simulate_long(o, h, l, c, av, [x for x in obs if x["type"] == "bullish"], rr, cost_pct)
    # Short side: negate prices, so a short becomes a long on the mirrored series.
    mirrored = [
        {**x, "top": -x["bottom"], "bottom": -x["top"]} for x in obs if x["type"] == "bearish"
    ]
    shorts = _simulate_long(-o, -l, -h, -c, av, mirrored, rr, cost_pct)
    rows = []
    for idx, net, gross in longs:
        rows.append({"date": df.index[idx], "side": "long", "net_R": net, "gross_R": gross})
    for idx, net, gross in shorts:
        rows.append({"date": df.index[idx], "side": "short", "net_R": net, "gross_R": gross})
    return rows


def stats(rs):
    rs = np.asarray(rs, dtype=float)
    if len(rs) == 0:
        return {"trades": 0}
    wins, losses = rs[rs > 0], rs[rs <= 0]
    pf = wins.sum() / -losses.sum() if losses.sum() < 0 else float("inf")
    return {
        "trades": int(len(rs)),
        "win_rate_%": round(100 * len(wins) / len(rs), 1),
        "avg_R": round(float(rs.mean()), 3),
        "total_R": round(float(rs.sum()), 1),
        "profit_factor": round(float(pf), 2),
    }


def run(frames: dict[str, pd.DataFrame], rr: float, cost_pct: float):
    all_rows = []
    for sym, df in frames.items():
        for r in backtest_symbol(df, rr, cost_pct):
            all_rows.append({"symbol": sym, **r})
    if not all_rows:
        return pd.DataFrame(), {}
    t = pd.DataFrame(all_rows).sort_values("date").reset_index(drop=True)
    half = len(t) // 2
    report = {
        "ALL (after costs)": stats(t["net_R"]),
        "ALL (before costs)": stats(t["gross_R"]),
        "First half (older)": stats(t["net_R"][:half]),
        "Second half (newer)": stats(t["net_R"][half:]),
        "Longs": stats(t.loc[t.side == "long", "net_R"]),
        "Shorts": stats(t.loc[t.side == "short", "net_R"]),
    }
    return t, report


def main():
    import data

    p = argparse.ArgumentParser()
    p.add_argument("--period", default="5y", help="history to download, e.g. 2y, 5y")
    p.add_argument("--rr", type=float, default=2.0, help="reward:risk target")
    p.add_argument("--cost", type=float, default=0.1, help="round-trip cost in percent of price")
    args = p.parse_args()

    frames = data.daily_all(period=args.period)
    print(f"Loaded {len(frames)} symbols{' (SAMPLE random data)' if data.SAMPLE else ''}")
    trades, report = run(frames, args.rr, args.cost / 100)
    if not report:
        print("No trades found.")
        return
    for name, s in report.items():
        print(f"{name:22s} {s}")
    print(
        "\nHow to read this: avg_R is the average result per trade in units of risk. "
        "If it is not clearly above zero AFTER costs, in BOTH halves, the rule has no usable edge."
    )


if __name__ == "__main__":
    main()
