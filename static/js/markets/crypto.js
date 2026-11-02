// Crypto market: Binance live data -> zones.js -> board + chart. Runs entirely in the browser, no server hop.
//
// Streams used (all public, no key):
//   <sym>@miniTicker       board prices + 24h change, ~1/s per symbol
//   <sym>@kline_<boardTf>  board bars (score is recomputed from these), ~1 every 2s per symbol
//   <sym>@aggTrade         every trade for the open chart symbol (tick-by-tick candle)
//   <sym>@kline_<chartTf>  authoritative candle + volume for the open chart symbol
import { stream, candleFromKline, applyCandle, applyTick, TF } from "../feed.js";
import { analyze } from "../zones.js";
import { frame as nextFrame, throttle } from "../util.js";
import * as view from "../view.js";

const BOARD_BARS = 300;
const CHART_BARS = 500;

export const info = {
  id: "crypto",
  label: "Crypto",
  locale: "en-US",
  currency: "USDT",
  chartTfs: ["1m", "5m", "15m", "1h", "4h", "1d"],
  boardTfs: ["15m", "1h", "4h", "1d"],
  defaultChartTf: "15m",
  defaultBoardTf: "1h",
  note: null,
};

export const splitSymbol = (sym) => (sym.endsWith("USDT") ? { sym, base: sym.slice(0, -4), quote: "USDT" } : { sym, base: sym, quote: "" });

/** `src` is a BinanceSource, or a SimSource in demo mode (see feed.js / sim.js). */
export function createCrypto({ src, symbols, board, chart, onStatus, onError, chartTf: initialChartTf, boardTf: initialBoardTf }) {
  const offStatus = src.onStatus(onStatus);

  const subs = []; // permanent (tickers)
  let boardSubs = [];
  let boardTf = initialBoardTf || info.defaultBoardTf;
  let boardToken = 0;
  const bars = new Map(); // sym -> {frame, dp}
  const stale = new Set(); // board symbols whose bars changed since their last score

  let cur = null; // open chart: {sym, tf, frame, dp, subs[]}
  let chartToken = 0;
  let chartTf = initialChartTf || info.defaultChartTf;

  // ------------------------------------------------------------------ board
  function score(sym) {
    const b = bars.get(sym);
    const it = board.item(sym);
    if (!b || !it || b.frame.t.length < 40) return;
    const s = analyze(b.frame, { dp: b.dp }).summary;
    if (!s) return;
    it.score = s.score;
    it.label = s.label;
    it.dp = b.dp;
    if (it.price == null) it.price = s.price;
    board.touch(it, true);
  }

  const flushScores = throttle(() => {
    for (const sym of stale) score(sym);
    stale.clear();
  }, 1000);

  async function loadBoard() {
    const token = ++boardToken;
    for (const off of boardSubs) off();
    boardSubs = [];
    bars.clear();
    stale.clear();
    for (const it of board.items.values()) it.score = it.label = null;
    board.relayout();

    let failed = 0;
    let lastErr = "";
    await Promise.all(
      symbols.map(async (sym) => {
        try {
          const res = await src.klines(sym, boardTf, BOARD_BARS);
          if (token !== boardToken) return;
          bars.set(sym, res);
          score(sym);
          boardSubs.push(
            src.subscribe(stream.kline(sym, boardTf), (d) => {
              const b = bars.get(sym);
              if (!b || !applyCandle(b.frame, candleFromKline(d))) return;
              stale.add(sym);
              flushScores();
            }),
          );
        } catch (e) {
          failed++;
          lastErr = e.message;
        }
      }),
    );
    if (token !== boardToken) return;
    board.relayout();
    if (failed === symbols.length) onError(`Can't load Binance market data (${lastErr}).`);
    else if (failed) console.warn(`${failed} board symbols failed to load: ${lastErr}`);
  }

  /** Best effort: quiet pairs may not send their first ticker message for a while. The stream takes over after. */
  function seedTickers() {
    src
      .tickers(symbols)
      .then((rows) => {
        for (const r of rows) {
          const it = board.item(r.sym);
          if (!it) continue;
          if (it.price == null) {
            it.price = r.last;
            it.dp = r.last >= 1000 ? 2 : r.last >= 10 ? 3 : r.last >= 1 ? 4 : 8; // refined when the bars load
          }
          if (it.chg24 == null) it.chg24 = (r.last / r.open - 1) * 100;
          board.touch(it);
        }
      })
      .catch(() => {});
  }

  function start() {
    board.setItems(symbols.map(splitSymbol));
    for (const sym of symbols) {
      subs.push(
        src.subscribe(stream.ticker(sym), (d) => {
          const it = board.item(sym);
          if (!it) return;
          const c = +d.c;
          if (!(cur && cur.sym === sym)) it.price = c; // the open symbol's price comes from trades, which are fresher
          it.chg24 = (c / +d.o - 1) * 100;
          board.touch(it);
        }),
      );
    }
    subs.push(src.onReconnect(() => { loadBoard(); if (cur) refill(); }));
    seedTickers();
    return loadBoard();
  }

  // ------------------------------------------------------------------ chart
  const paint = nextFrame(() => {
    if (!cur) return;
    const f = cur.frame;
    chart.tick(f);
    const px = f.c[f.c.length - 1];
    view.setPrice(px, cur.dp);
    const it = board.item(cur.sym);
    if (it) {
      it.price = px;
      it.dp = cur.dp;
      board.touch(it);
    }
  });

  const recompute = throttle(() => {
    if (!cur) return;
    const a = analyze(cur.frame, { lastBarOpen: cur.frame.lastOpen, dp: cur.dp });
    chart.setOverlay(a.zones, a.levels, cur.dp);
    view.chips(a.summary, cur.dp);
  }, 500);

  function closeChart() {
    if (cur) for (const off of cur.subs) off();
    cur = null;
  }

  async function open(sym, tf = chartTf) {
    closeChart();
    const token = ++chartToken;
    chartTf = tf;
    const { base } = splitSymbol(sym);
    view.setSymbol(base, sym.endsWith("USDT") ? "/ USDT" : "");
    view.chips(null);
    view.msg("Loading…", "load");
    chart.clear();
    board.setSelected(sym);
    let res;
    try {
      res = await src.klines(sym, tf, CHART_BARS);
    } catch (e) {
      if (token === chartToken) {
        // Binance's error replies carry no CORS headers, so a bad symbol reaches us as a bare "Failed to fetch".
        // If the live socket is up the network is fine, so the symbol is the likely problem.
        const badPair = e.message.includes("Invalid symbol") || (e instanceof TypeError && src.state === "live");
        view.msg(badPair ? `${sym} isn't a Binance spot pair. Try a symbol like BTCUSDT.` : `Couldn't load ${sym}: ${e.message}`, "err");
      }
      return;
    }
    if (token !== chartToken) return;
    const c = (cur = { sym, tf, frame: res.frame, dp: res.dp, subs: [] });
    chart.load(c.frame, c.dp);
    view.msg("");
    view.setPrice(c.frame.c.at(-1), c.dp);
    const it = board.item(sym);
    if (it?.chg24 != null) view.setChange(it.chg24);
    recompute();

    const sec = TF[tf];
    c.subs.push(
      src.subscribe(stream.trade(sym), (d) => {
        if (cur !== c) return;
        const r = applyTick(c.frame, +d.p, d.T, sec);
        if (!r) return;
        paint();
        recompute();
      }),
      src.subscribe(stream.kline(sym, tf), (d) => {
        if (cur !== c || !applyCandle(c.frame, candleFromKline(d))) return;
        paint();
        recompute();
      }),
      src.subscribe(stream.ticker(sym), (d) => view.setChange((+d.c / +d.o - 1) * 100)),
    );
  }

  /** After a reconnect: ticks were missed, so reload the history but keep the user's zoom and scroll. */
  async function refill() {
    const c = cur;
    if (!c) return;
    try {
      const res = await src.klines(c.sym, c.tf, CHART_BARS);
      if (cur !== c) return;
      c.frame = res.frame;
      c.dp = res.dp;
      chart.load(c.frame, c.dp, true);
      recompute();
    } catch {
      /* the next kline message will repair the last bar */
    }
  }

  return {
    ...info,
    src,
    get chartTf() { return chartTf; },
    get boardTf() { return boardTf; },
    get symbol() { return cur?.sym ?? null; },
    start,
    open,
    setChartTf(tf) {
      if (cur) return open(cur.sym, tf);
      chartTf = tf;
    },
    setBoardTf(tf) {
      boardTf = tf;
      return loadBoard();
    },
    reload() {
      loadBoard();
      if (cur) open(cur.sym, cur.tf);
    },
    stop() {
      closeChart();
      for (const off of [...subs, ...boardSubs]) off();
      subs.length = 0;
      boardSubs = [];
      boardToken++;
      chartToken++;
      offStatus();
      src.dispose();
    },
  };
}
