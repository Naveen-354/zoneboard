"""Order block / liquidity zone detection and a simple strength score.

These are simplified, rule-based readings of "smart money" chart concepts.
They are a starting point to test, not a proven edge. Use backtest.py before
trusting any of it with real money.

All functions take a pandas DataFrame with columns Open, High, Low, Close, Volume
(oldest bar first) and return plain Python values.
"""
from __future__ import annotations

import numpy as np
import pandas as pd


def atr(df: pd.DataFrame, n: int = 14) -> pd.Series:
    """Average true range (Wilder smoothing)."""
    h, l, c = df["High"], df["Low"], df["Close"]
    pc = c.shift(1)
    tr = pd.concat([h - l, (h - pc).abs(), (l - pc).abs()], axis=1).max(axis=1)
    return tr.ewm(alpha=1 / n, adjust=False, min_periods=n).mean()


def pivots(df: pd.DataFrame, left: int = 3, right: int = 3):
    """Swing highs and lows. A pivot is confirmed `right` bars after it forms."""
    hi = df["High"].to_numpy()
    lo = df["Low"].to_numpy()
    highs, lows = [], []
    for i in range(max(left, 1), len(df) - right):
        if hi[i] == hi[i - left : i + right + 1].max() and hi[i - 1] != hi[i]:
            highs.append(i)
        if lo[i] == lo[i - left : i + right + 1].min() and lo[i - 1] != lo[i]:
            lows.append(i)
    return highs, lows


def liquidity_levels(df: pd.DataFrame, left: int = 3, right: int = 3, tol_atr: float = 0.15):
    """Swing highs/lows, with nearly equal ones merged into one level.

    Stops tend to rest above equal highs and below equal lows, so clusters with
    more touches are treated as stronger liquidity pools. A level is "swept"
    once a later bar trades through it.
    """
    a = atr(df).to_numpy()
    hi = df["High"].to_numpy()
    lo = df["Low"].to_numpy()
    highs, lows = pivots(df, left, right)
    out = []
    for kind, idxs, arr in (("high", highs, hi), ("low", lows, lo)):
        pts = sorted((float(arr[i]), i) for i in idxs)
        clusters: list[list[tuple[float, int]]] = []
        for price, i in pts:
            tol = tol_atr * (a[i] if not np.isnan(a[i]) else price * 0.01)
            if clusters and price - clusters[-1][-1][0] <= tol:
                clusters[-1].append((price, i))
            else:
                clusters.append([(price, i)])
        for cl in clusters:
            prices = [p for p, _ in cl]
            idx = [i for _, i in cl]
            level = max(prices) if kind == "high" else min(prices)
            last = max(idx)
            rest = arr[last + 1 :]
            hit = np.nonzero(rest > level)[0] if kind == "high" else np.nonzero(rest < level)[0]
            out.append(
                {
                    "kind": kind,
                    "price": float(level),
                    "touches": len(cl),
                    "first_idx": int(min(idx)),
                    "last_idx": int(last),
                    "swept_idx": int(last + 1 + hit[0]) if len(hit) else None,
                }
            )
    return out


def order_blocks(df: pd.DataFrame, atr_mult: float = 1.5, lookahead: int = 3, a: pd.Series | None = None):
    """Find order blocks.

    Bullish OB: the last down candle before an up-move that closes above that
    candle's high and gains at least `atr_mult` x ATR within `lookahead` bars.
    Bearish OB is the mirror image. The zone is the full range of that candle.

    `confirm_idx` is the bar on which the impulse completes, so a trading rule
    must not act on the zone before that bar (this keeps backtests honest).
    A zone is mitigated once a bar closes through it.
    """
    a_arr = (atr(df) if a is None else a).to_numpy()
    o = df["Open"].to_numpy()
    h = df["High"].to_numpy()
    l = df["Low"].to_numpy()
    c = df["Close"].to_numpy()
    n = len(df)
    found: dict[tuple[str, int], int] = {}
    for i in range(14, n - 1):
        ai = a_arr[i]
        if np.isnan(ai) or ai <= 0:
            continue
        if c[i] < o[i]:
            for k in range(1, lookahead + 1):
                j = i + k
                if j >= n:
                    break
                if c[j] > h[i] and c[j] - c[i] >= atr_mult * ai:
                    found[("bullish", j)] = i  # later i overwrites: last opposite candle wins
                    break
        elif c[i] > o[i]:
            for k in range(1, lookahead + 1):
                j = i + k
                if j >= n:
                    break
                if c[j] < l[i] and c[i] - c[j] >= atr_mult * ai:
                    found[("bearish", j)] = i
                    break
    obs = []
    for (typ, j), i in found.items():
        top, bot = float(h[i]), float(l[i])
        tested = mitigated = None
        for m in range(j + 1, n):
            if typ == "bullish":
                if tested is None and l[m] <= top:
                    tested = m
                if c[m] < bot:
                    mitigated = m
                    break
            else:
                if tested is None and h[m] >= bot:
                    tested = m
                if c[m] > top:
                    mitigated = m
                    break
        obs.append(
            {
                "type": typ,
                "idx": int(i),
                "confirm_idx": int(j),
                "top": top,
                "bottom": bot,
                "tested_idx": tested,
                "mitigated_idx": mitigated,
                "active": mitigated is None,
            }
        )
    obs.sort(key=lambda x: x["idx"])
    return obs


def strength(df: pd.DataFrame, obs: list[dict], a: pd.Series) -> dict:
    """Score from -100 (strongest bearish) to +100 (strongest bullish).

    Blend of: today's move in ATR units (40%), trend vs EMA20/EMA50 (25%),
    relative volume confirming the move (20%), and being near an active
    order block in the move's direction (15%). Free data has no open interest,
    so OI is not used. Scores under 15 in magnitude are labelled "choppy".
    """
    c = df["Close"]
    price = float(c.iloc[-1])
    atrv = float(a.iloc[-1])
    ret = float(c.iloc[-1] / c.iloc[-2] - 1) if len(c) > 1 else 0.0
    atr_pct = atrv / price if price else 0.0
    mom = float(np.clip(ret / atr_pct / 2, -1, 1)) if atr_pct > 0 else 0.0

    ema20 = float(c.ewm(span=20, adjust=False).mean().iloc[-1])
    ema50 = float(c.ewm(span=50, adjust=False).mean().iloc[-1])
    trend = 0.5 * np.sign(price - ema20) + 0.5 * np.sign(ema20 - ema50)

    vol = df["Volume"]
    base = float(vol.iloc[-21:-1].mean()) if len(vol) > 21 else 0.0
    rvol = float(vol.iloc[-1] / base) if base > 0 else 1.0
    vol_term = float(np.sign(mom) * np.clip((rvol - 1) / 2, 0, 1))

    ob_term = 0.0
    for ob in obs:
        if not ob["active"]:
            continue
        if ob["type"] == "bullish" and ob["bottom"] <= price and price - ob["top"] <= 1.5 * atrv:
            ob_term = 1.0
        elif ob["type"] == "bearish" and ob["top"] >= price and ob["bottom"] - price <= 1.5 * atrv:
            ob_term = -1.0 if ob_term == 0 else 0.0

    score = 100 * (0.40 * mom + 0.25 * trend + 0.20 * vol_term + 0.15 * ob_term)
    label = "choppy" if abs(score) < 15 else ("bullish" if score > 0 else "bearish")
    return {
        "score": round(float(score), 1),
        "label": label,
        "change_pct": round(ret * 100, 2),
        "rvol": round(rvol, 2),
        "atr": round(atrv, 2),
    }


def chart_overlays(df: pd.DataFrame, obs: list[dict], liq: list[dict], ts) -> tuple[list[dict], list[dict]]:
    """What the chart draws: the 6 nearest active zones, and the 4 open liquidity levels above and below price.

    `ts` is the bar times (unix seconds), same length as df. static/js/zones.js `analyze` mirrors this.
    """
    price = float(df["Close"].iloc[-1])
    active = [o for o in obs if o["active"]]
    active.sort(key=lambda o: min(abs(price - o["top"]), abs(price - o["bottom"])))
    zone_out = [
        {
            "type": o["type"],
            "top": round(o["top"], 2),
            "bottom": round(o["bottom"], 2),
            "start": int(ts[o["idx"]]),
            "tested": o["tested_idx"] is not None,
        }
        for o in active[:6]
    ]
    open_lv = [x for x in liq if x["swept_idx"] is None]
    above = sorted((x for x in open_lv if x["price"] > price), key=lambda x: x["price"])[:4]
    below = sorted((x for x in open_lv if x["price"] <= price), key=lambda x: -x["price"])[:4]
    level_out = [
        {"kind": x["kind"], "price": round(x["price"], 2), "touches": x["touches"], "start": int(ts[x["first_idx"]])}
        for x in above + below
    ]
    return zone_out, level_out


def summarize(df: pd.DataFrame, obs: list[dict] | None = None, liq: list[dict] | None = None) -> dict:
    """Current picture for one stock: score, nearest zone, support/resistance, liquidity."""
    a = atr(df)
    obs = order_blocks(df, a=a) if obs is None else obs
    liq = liquidity_levels(df) if liq is None else liq
    price = float(df["Close"].iloc[-1])
    st = strength(df, obs, a)
    bias = st["label"] if st["label"] != "choppy" else ("bullish" if st["score"] >= 0 else "bearish")

    open_lows = [x for x in liq if x["kind"] == "low" and x["swept_idx"] is None and x["price"] < price]
    open_highs = [x for x in liq if x["kind"] == "high" and x["swept_idx"] is None and x["price"] > price]
    support = max((x["price"] for x in open_lows), default=None)
    resistance = min((x["price"] for x in open_highs), default=None)

    pool_src = open_lows if bias == "bullish" else open_highs
    pool = None
    if pool_src:
        pool = sorted(pool_src, key=lambda x: (-x["touches"], abs(x["price"] - price)))[0]["price"]

    ob_pick = None
    if bias == "bullish":
        cand = [o for o in obs if o["active"] and o["type"] == "bullish" and o["bottom"] <= price]
        if cand:
            ob_pick = min(cand, key=lambda o: abs(price - o["top"]))
    else:
        cand = [o for o in obs if o["active"] and o["type"] == "bearish" and o["top"] >= price]
        if cand:
            ob_pick = min(cand, key=lambda o: abs(o["bottom"] - price))

    return {
        "price": round(price, 2),
        "bias": bias,
        **st,
        "support": None if support is None else round(support, 2),
        "resistance": None if resistance is None else round(resistance, 2),
        "liquidity": None if pool is None else round(pool, 2),
        "ob": None if ob_pick is None else {"top": round(ob_pick["top"], 2), "bottom": round(ob_pick["bottom"], 2), "type": ob_pick["type"]},
    }
