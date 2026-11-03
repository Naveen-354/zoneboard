// India NSE: scanner and charts from the server (/api/scan, /api/chart). The data is Yahoo Finance,
// delayed ~15 minutes, so this market polls once a minute instead of streaming.
// Live NSE needs a broker feed (Fyers, chosen but not built yet: see docs/fyers/); it plugs in here.
//
// Sample data (development only) asks the server for made-up bars (?sample=1) and animates them with small
// random-walk ticks here in the browser, so the board and chart move without internet.
import { api } from "../util.js";
import * as view from "../view.js";

const REFRESH_MS = 60000;

export const info = {
  id: "india",
  label: "India NSE",
  locale: "en-IN",
  currency: "₹",
  chartTfs: ["1d", "1h", "15m"],
  boardTfs: ["1d"],
  defaultChartTf: "1d",
  defaultBoardTf: "1d",
  note: "NSE prices come from Yahoo Finance and are delayed about 15 minutes. Live NSE data needs a broker feed (Fyers), which isn't connected yet.",
};

const gauss = () => Math.sqrt(-2 * Math.log(1 - Math.random())) * Math.cos(2 * Math.PI * Math.random());

export function createIndia({ board, chart, onStatus, onError, chartTf: initialChartTf, sample = false }) {
  let timer = 0;
  let sampleTimer = 0;
  let cur = null; // {sym, tf}
  let frame = null; // the open chart's bars, so sample ticks can move the forming candle
  let token = 0;
  let chartTf = initialChartTf || info.defaultChartTf;
  let ready = false;
  const qs = `sample=${sample ? 1 : 0}`;

  async function scan() {
    try {
      const d = await api(`/api/scan?${qs}`);
      if (!ready) {
        board.setItems(d.all.map((r) => ({ sym: r.symbol, base: r.symbol, quote: "" })));
        ready = true;
        if (cur) board.setSelected(cur.sym);
      }
      for (const r of d.all) {
        const it = board.item(r.symbol);
        if (!it) continue;
        it.price = r.price;
        it.dp = 2;
        it.chg24 = r.change_pct;
        it.score = r.score;
        it.label = r.label;
        board.touch(it);
      }
      board.relayout();
      onStatus({ state: "delayed", rate: 0, updated: d.updated, sample: d.sample });
      return d;
    } catch (e) {
      onError(`NSE scan failed: ${e.message}`);
      onStatus({ state: "error", rate: 0 });
      return null;
    }
  }

  async function load(sym, tf, quiet) {
    const t = ++token;
    if (!quiet) {
      view.setSymbol(sym);
      view.chips(null);
      view.msg("Loading…", "load");
      chart.clear();
      board.setSelected(sym);
    }
    let d;
    try {
      d = await api(`/api/chart/${encodeURIComponent(sym)}?interval=${tf}&${qs}`);
    } catch (e) {
      if (t === token && !quiet) view.msg(e.message, "err");
      return;
    }
    if (t !== token) return;
    const f = { t: [], o: [], h: [], l: [], c: [], v: [] };
    for (const c of d.candles) {
      f.t.push(c.time);
      f.o.push(c.open);
      f.h.push(c.high);
      f.l.push(c.low);
      f.c.push(c.close);
      f.v.push(c.volume ?? 0);
    }
    frame = f;
    chart.load(f, 2, quiet);
    chart.setOverlay(d.zones, d.levels, 2);
    view.chips(d.summary, 2);
    view.setPrice(d.summary.price, 2);
    view.setChange(d.summary.change_pct, tf === "1d" ? "day" : "bar");
    view.msg("");
  }

  function schedule() {
    clearInterval(timer);
    if (sample) return; // made-up data never changes on the server: nothing to refresh
    timer = setInterval(() => {
      if (document.hidden) return;
      scan();
      if (cur) load(cur.sym, cur.tf, true);
    }, REFRESH_MS);
  }

  /** Sample data: nudge every price a little each second, and move the open chart's forming candle with it. */
  function startSampleTicks() {
    clearInterval(sampleTimer);
    sampleTimer = setInterval(() => {
      for (const it of board.items.values()) {
        if (it.price == null) continue;
        const next = it.price * Math.exp(0.0004 * gauss());
        if (it.chg24 != null) it.chg24 += (next / it.price - 1) * 100;
        it.price = next;
        board.touch(it);
      }
      const price = cur && frame ? board.item(cur.sym)?.price : null;
      if (price == null) return;
      const n = frame.c.length - 1;
      frame.c[n] = price;
      if (price > frame.h[n]) frame.h[n] = price;
      if (price < frame.l[n]) frame.l[n] = price;
      chart.tick(frame);
      view.setPrice(price, 2);
    }, 1000);
  }

  return {
    ...info,
    live: false,
    get chartTf() { return chartTf; },
    get boardTf() { return "1d"; },
    get symbol() { return cur?.sym ?? null; },
    /** Last known price and its decimals for the order ticket, or null. */
    quote(sym) {
      const it = board.item(sym);
      return it && it.price != null ? { price: it.price, dp: 2 } : null;
    },
    watch() {}, // every NSE symbol is already on the board
    /** Resolves to a good first symbol to open (the strongest setup), or null. */
    async start() {
      board.setItems([]);
      schedule();
      const d = await scan();
      if (sample && d) startSampleTicks();
      return d && (d.bullish[0] || d.bearish[0] || d.all[0])?.symbol;
    },
    open(sym, tf = chartTf) {
      chartTf = tf;
      cur = { sym, tf };
      return load(sym, tf, false);
    },
    setChartTf(tf) {
      chartTf = tf;
      if (cur) return this.open(cur.sym, tf);
    },
    setBoardTf() {},
    reload() {
      scan();
      if (cur) load(cur.sym, cur.tf, false);
    },
    stop() {
      clearInterval(timer);
      clearInterval(sampleTimer);
      token++;
    },
  };
}
