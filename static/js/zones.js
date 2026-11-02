// JS port of zones.py, so zones and scores recompute in the browser on every live candle.
// Keep the two in sync: test_zones_js.py runs both on the same data and compares the output.
//
// A frame is {t:[], o:[], h:[], l:[], c:[], v:[]}, oldest bar first (t = unix seconds).
// This file has no DOM or network code on purpose, so Node can import it for the parity test.

const clip = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const round = (x, d) => {
  const m = 10 ** d;
  return Math.round(x * m) / m;
};

/** Average true range, Wilder smoothing. NaN until n bars exist (same as pandas min_periods=n). */
export function atr(f, n = 14) {
  const { h, l, c } = f;
  const out = new Array(c.length).fill(NaN);
  const alpha = 1 / n;
  let prev = 0;
  for (let i = 0; i < c.length; i++) {
    let tr = h[i] - l[i];
    if (i > 0) tr = Math.max(tr, Math.abs(h[i] - c[i - 1]), Math.abs(l[i] - c[i - 1]));
    prev = i === 0 ? tr : alpha * tr + (1 - alpha) * prev;
    if (i >= n - 1) out[i] = prev;
  }
  return out;
}

/** Swing highs and lows. A pivot is confirmed `right` bars after it forms. */
export function pivots(f, left = 3, right = 3) {
  const { h, l } = f;
  const highs = [];
  const lows = [];
  for (let i = Math.max(left, 1); i < h.length - right; i++) {
    let mx = -Infinity;
    let mn = Infinity;
    for (let k = i - left; k <= i + right; k++) {
      if (h[k] > mx) mx = h[k];
      if (l[k] < mn) mn = l[k];
    }
    if (h[i] === mx && h[i - 1] !== h[i]) highs.push(i);
    if (l[i] === mn && l[i - 1] !== l[i]) lows.push(i);
  }
  return [highs, lows];
}

/** Swing highs/lows with nearly equal ones merged. A level is "swept" once a later bar trades through it. */
export function liquidityLevels(f, a = atr(f), left = 3, right = 3, tolAtr = 0.15) {
  const [highs, lows] = pivots(f, left, right);
  const out = [];
  for (const [kind, idxs, arr] of [["high", highs, f.h], ["low", lows, f.l]]) {
    const pts = idxs.map((i) => [arr[i], i]).sort((x, y) => x[0] - y[0] || x[1] - y[1]);
    const clusters = [];
    for (const [price, i] of pts) {
      const tol = tolAtr * (Number.isNaN(a[i]) ? price * 0.01 : a[i]);
      const last = clusters[clusters.length - 1];
      if (last && price - last[last.length - 1][0] <= tol) last.push([price, i]);
      else clusters.push([[price, i]]);
    }
    for (const cl of clusters) {
      let level = kind === "high" ? -Infinity : Infinity;
      let firstIdx = Infinity;
      let lastIdx = -1;
      for (const [p, i] of cl) {
        level = kind === "high" ? Math.max(level, p) : Math.min(level, p);
        if (i < firstIdx) firstIdx = i;
        if (i > lastIdx) lastIdx = i;
      }
      let sweptIdx = null;
      for (let k = lastIdx + 1; k < arr.length; k++) {
        if (kind === "high" ? arr[k] > level : arr[k] < level) {
          sweptIdx = k;
          break;
        }
      }
      out.push({ kind, price: level, touches: cl.length, firstIdx, lastIdx, sweptIdx });
    }
  }
  return out;
}

/**
 * Order blocks: the last opposite candle before an impulse of >= atrMult x ATR within `lookahead` bars.
 * `confirmIdx` is the bar where the impulse completes. A zone is mitigated once a bar closes through it.
 */
export function orderBlocks(f, atrMult = 1.5, lookahead = 3, a = atr(f)) {
  const { o, h, l, c } = f;
  const n = c.length;
  const found = new Map(); // "type:j" -> [type, j, i]; a later i overwrites (last opposite candle wins)
  for (let i = 14; i < n - 1; i++) {
    const ai = a[i];
    if (Number.isNaN(ai) || ai <= 0) continue;
    if (c[i] < o[i]) {
      for (let k = 1; k <= lookahead; k++) {
        const j = i + k;
        if (j >= n) break;
        if (c[j] > h[i] && c[j] - c[i] >= atrMult * ai) {
          found.set("bullish:" + j, ["bullish", j, i]);
          break;
        }
      }
    } else if (c[i] > o[i]) {
      for (let k = 1; k <= lookahead; k++) {
        const j = i + k;
        if (j >= n) break;
        if (c[j] < l[i] && c[i] - c[j] >= atrMult * ai) {
          found.set("bearish:" + j, ["bearish", j, i]);
          break;
        }
      }
    }
  }
  const obs = [];
  for (const [type, j, i] of found.values()) {
    const top = h[i];
    const bottom = l[i];
    let tested = null;
    let mitigated = null;
    for (let m = j + 1; m < n; m++) {
      if (type === "bullish") {
        if (tested === null && l[m] <= top) tested = m;
        if (c[m] < bottom) {
          mitigated = m;
          break;
        }
      } else {
        if (tested === null && h[m] >= bottom) tested = m;
        if (c[m] > top) {
          mitigated = m;
          break;
        }
      }
    }
    obs.push({ type, idx: i, confirmIdx: j, top, bottom, testedIdx: tested, mitigatedIdx: mitigated, active: mitigated === null });
  }
  obs.sort((x, y) => x.idx - y.idx);
  return obs;
}

function emaLast(c, span) {
  const k = 2 / (span + 1);
  let e = c[0];
  for (let i = 1; i < c.length; i++) e = k * c[i] + (1 - k) * e;
  return e;
}

/** Score from -100 (strongest bearish) to +100 (strongest bullish). Same weights as zones.strength. */
export function strength(f, obs, a, dp = 2) {
  const { c, v } = f;
  const n = c.length;
  const price = c[n - 1];
  const atrv = a[n - 1];
  const ret = n > 1 ? c[n - 1] / c[n - 2] - 1 : 0;
  const atrPct = price ? atrv / price : 0;
  const mom = atrPct > 0 ? clip(ret / atrPct / 2, -1, 1) : 0;

  const ema20 = emaLast(c, 20);
  const ema50 = emaLast(c, 50);
  const trend = 0.5 * Math.sign(price - ema20) + 0.5 * Math.sign(ema20 - ema50);

  let base = 0;
  if (n > 21) {
    for (let i = n - 21; i < n - 1; i++) base += v[i];
    base /= 20;
  }
  const rvol = base > 0 ? v[n - 1] / base : 1;
  const volTerm = Math.sign(mom) * clip((rvol - 1) / 2, 0, 1);

  let obTerm = 0;
  for (const ob of obs) {
    if (!ob.active) continue;
    if (ob.type === "bullish" && ob.bottom <= price && price - ob.top <= 1.5 * atrv) obTerm = 1;
    else if (ob.type === "bearish" && ob.top >= price && ob.bottom - price <= 1.5 * atrv) obTerm = obTerm === 0 ? -1 : 0;
  }

  const score = 100 * (0.4 * mom + 0.25 * trend + 0.2 * volTerm + 0.15 * obTerm);
  const label = Math.abs(score) < 15 ? "choppy" : score > 0 ? "bullish" : "bearish";
  return { score: round(score, 1), label, change_pct: round(ret * 100, 2), rvol: round(rvol, 2), atr: round(atrv, dp) };
}

/** Current picture for one market: score, nearest zone, support/resistance, liquidity pool. `dp` = price decimals. */
export function summarize(f, obs, liq, a, dp = 2) {
  const rp = (x) => (x === null ? null : round(x, dp));
  const price = f.c[f.c.length - 1];
  const st = strength(f, obs, a, dp);
  const bias = st.label !== "choppy" ? st.label : st.score >= 0 ? "bullish" : "bearish";

  const openLows = liq.filter((x) => x.kind === "low" && x.sweptIdx === null && x.price < price);
  const openHighs = liq.filter((x) => x.kind === "high" && x.sweptIdx === null && x.price > price);
  const support = openLows.length ? Math.max(...openLows.map((x) => x.price)) : null;
  const resistance = openHighs.length ? Math.min(...openHighs.map((x) => x.price)) : null;

  const poolSrc = bias === "bullish" ? openLows : openHighs;
  let pool = null;
  if (poolSrc.length) {
    pool = [...poolSrc].sort((x, y) => y.touches - x.touches || Math.abs(x.price - price) - Math.abs(y.price - price))[0].price;
  }

  let obPick = null;
  if (bias === "bullish") {
    const cand = obs.filter((o) => o.active && o.type === "bullish" && o.bottom <= price);
    if (cand.length) obPick = cand.reduce((b, o) => (Math.abs(price - o.top) < Math.abs(price - b.top) ? o : b));
  } else {
    const cand = obs.filter((o) => o.active && o.type === "bearish" && o.top >= price);
    if (cand.length) obPick = cand.reduce((b, o) => (Math.abs(o.bottom - price) < Math.abs(b.bottom - price) ? o : b));
  }

  return {
    price: round(price, dp),
    bias,
    ...st,
    support: rp(support),
    resistance: rp(resistance),
    liquidity: rp(pool),
    ob: obPick && { top: rp(obPick.top), bottom: rp(obPick.bottom), type: obPick.type },
  };
}

/**
 * Everything the chart needs, in the same shape /api/chart returns for NSE:
 * {summary, zones:[{type,top,bottom,start,tested}], levels:[{kind,price,touches,start}]}.
 * Zone and level prices are exact; only the summary is rounded, to `dp` decimals (crypto can trade at 0.00001).
 * `start` is a unix time. `forming` marks a zone whose impulse bar is still open, so it can vanish at the close.
 */
export function analyze(f, { lastBarOpen = false, dp = 2 } = {}) {
  const n = f.c.length;
  const a = atr(f);
  const obs = orderBlocks(f, 1.5, 3, a);
  const liq = liquidityLevels(f, a);
  const price = f.c[n - 1];

  const active = obs.filter((o) => o.active);
  const dist = (o) => Math.min(Math.abs(price - o.top), Math.abs(price - o.bottom));
  active.sort((x, y) => dist(x) - dist(y));
  const zones = active.slice(0, 6).map((o) => ({
    type: o.type,
    top: o.top,
    bottom: o.bottom,
    start: f.t[o.idx],
    tested: o.testedIdx !== null,
    forming: lastBarOpen && o.confirmIdx >= n - 1,
  }));

  const open = liq.filter((x) => x.sweptIdx === null);
  const above = open.filter((x) => x.price > price).sort((x, y) => x.price - y.price).slice(0, 4);
  const below = open.filter((x) => x.price <= price).sort((x, y) => y.price - x.price).slice(0, 4);
  const levels = [...above, ...below].map((x) => ({ kind: x.kind, price: x.price, touches: x.touches, start: f.t[x.firstIdx] }));

  return { summary: n >= 40 ? summarize(f, obs, liq, a, dp) : null, zones, levels };
}
