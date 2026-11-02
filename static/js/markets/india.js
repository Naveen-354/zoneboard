// India NSE: scanner and charts from the server (/api/scan, /api/chart). The data is Yahoo Finance,
// delayed ~15 minutes, so this market polls once a minute instead of streaming.
// Live NSE needs a broker feed (Fyers or Zerodha Kite); it plugs in here once a broker is chosen.
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
  note: "NSE prices come from Yahoo Finance and are delayed about 15 minutes. Live NSE data needs a broker feed (Fyers or Zerodha Kite), which isn't connected yet.",
};

export function createIndia({ board, chart, onStatus, onError, chartTf: initialChartTf }) {
  let timer = 0;
  let cur = null; // {sym, tf}
  let token = 0;
  let chartTf = initialChartTf || info.defaultChartTf;
  let ready = false;

  async function scan() {
    try {
      const d = await api("/api/scan");
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
      onStatus({ state: "delayed", rate: 0, updated: d.updated, demo: d.demo });
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
      d = await api(`/api/chart/${encodeURIComponent(sym)}?interval=${tf}`);
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
    chart.load(f, 2, quiet);
    chart.setOverlay(d.zones, d.levels, 2);
    view.chips(d.summary, 2);
    view.setPrice(d.summary.price, 2);
    view.setChange(d.summary.change_pct, tf === "1d" ? "day" : "bar");
    view.msg("");
  }

  function schedule() {
    clearInterval(timer);
    timer = setInterval(() => {
      if (document.hidden) return;
      scan();
      if (cur) load(cur.sym, cur.tf, true);
    }, REFRESH_MS);
  }

  return {
    ...info,
    live: false,
    get chartTf() { return chartTf; },
    get boardTf() { return "1d"; },
    get symbol() { return cur?.sym ?? null; },
    /** Resolves to a good first symbol to open (the strongest setup), or null. */
    async start() {
      board.setItems([]);
      schedule();
      const d = await scan();
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
      token++;
    },
  };
}
