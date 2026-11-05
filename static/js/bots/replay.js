// Market Replay: step through history candle by candle and watch a strategy act. The server runs the strategy
// once with the same engine as backtests and bots; the engine reads closed candles only (tested for every
// template), so revealing the result bar by bar shows exactly what the strategy knew at each moment. Playback
// speed changes nothing but how fast it is shown.
import { el, post, field, input, select, button, say, money, fmtAuto, when } from "./ui.js";
import { toLocalSeconds } from "../util.js";

let ctx = null;
let st = null; // { r, i, timer, chart, series, markers }

export function init(c) {
  ctx = c;
}

function stop() {
  if (st?.timer) clearInterval(st.timer);
  if (st) st.timer = 0;
}

export function render(host) {
  stop();
  st?.chart?.remove();
  st = null;
  const c = el("section", "card st-card");
  const hd = el("div", "st-card-h");
  hd.append(el("h3", null, "Market Replay"), el("span", "jsub", "Replay history candle by candle with a strategy running on it"));
  if (!ctx.bots.length) {
    c.append(hd, el("p", "st-about", "Create a bot first: replay runs a bot's strategy."));
    host.replaceChildren(c);
    return;
  }
  const pick = select(ctx.bots.map((b) => ({ value: String(b.id), label: `${b.name} · ${b.symbol} ${b.interval}` })), String(ctx.bots[0].id));
  const bars = input(400, { type: "number", min: 100, max: 2000, step: 50 });
  const g = el("div", "st-grid");
  g.append(field("Bot (strategy, symbol, timeframe)", pick), field("Candles of history", bars));
  const msg = el("p", "st-msg");
  const stage = el("div", "replay");
  const load = button("Load replay", "btn", async () => {
    say(msg, "Running the strategy over history…");
    try {
      const r = await post("/api/backtest", { bot_id: Number(pick.value), bars: Number(bars.value) });
      say(msg, r.data.sample ? "SAMPLE DATA: made-up prices." : r.data.source, r.data.sample ? "" : "ok");
      setup(stage, r);
    } catch (e) {
      say(msg, e.message, "err");
    }
  });
  const bar = el("div", "st-actions");
  bar.append(load, msg);
  c.append(hd, g, bar, stage);
  host.replaceChildren(c);
}

function setup(stage, r) {
  stop();
  st?.chart?.remove();
  const LW = window.LightweightCharts;
  const box = el("div", "chart-box tall");
  const info = el("div", "replay-info");
  const ctl = el("div", "st-actions");
  const pos = input(0, { type: "range", min: 0, max: r.candles.length - 1, step: 1, class: "replay-pos", "aria-label": "replay position" });
  const speed = select(
    [
      { value: "1000", label: "1 candle / s" },
      { value: "250", label: "4 / s" },
      { value: "80", label: "12 / s" },
      { value: "20", label: "50 / s" },
    ],
    "250",
  );
  const play = button("▶ Play", "btn sm", () => (st.timer ? pause() : start()));
  const pause = () => {
    stop();
    play.textContent = "▶ Play";
  };
  const start = () => {
    stop();
    play.textContent = "❚❚ Pause";
    st.timer = setInterval(() => (st.i >= r.candles.length - 1 ? pause() : show(st.i + 1)), Number(speed.value));
  };
  speed.addEventListener("change", () => st.timer && start());
  pos.addEventListener("input", () => show(Number(pos.value)));
  ctl.append(button("⏮", "btn ghost sm", () => show(0), "Back to the start"), button("◀ Step", "btn ghost sm", () => show(Math.max(0, st.i - 1))), play, button("Step ▶", "btn ghost sm", () => show(Math.min(r.candles.length - 1, st.i + 1))), speed, pos);
  stage.replaceChildren(ctl, box, info);
  const chart = LW.createChart(box, { autoSize: true, height: 360, layout: { background: { type: "solid", color: "#fff" }, textColor: "#64748b", fontSize: 11, attributionLogo: true }, grid: { vertLines: { color: "#f1f3f7" }, horzLines: { color: "#f1f3f7" } }, timeScale: { timeVisible: true, borderVisible: false }, rightPriceScale: { borderVisible: false } });
  const series = chart.addSeries(LW.CandlestickSeries, { upColor: "#0b9f6e", downColor: "#e5484d", borderVisible: false, wickUpColor: "#0b9f6e", wickDownColor: "#e5484d" });
  const markers = LW.createSeriesMarkers ? LW.createSeriesMarkers(series, []) : null;
  const eq = new Map(r.equity.map(([t, v]) => [t, v]));
  st = { r, i: 0, timer: 0, chart, series, markers };
  const show = (i) => {
    st.i = i;
    pos.value = String(i);
    const t = r.candles[i][0];
    series.setData(r.candles.slice(0, i + 1).map(([tt, o, h, l, c]) => ({ time: toLocalSeconds(tt), open: o, high: h, low: l, close: c })));
    const m = [];
    for (const x of r.trades.filter((x) => x.leg === 0)) {
      if (x.entry_time <= t) m.push({ time: toLocalSeconds(x.entry_time), position: x.side === "long" ? "belowBar" : "aboveBar", color: x.side === "long" ? "#0b9f6e" : "#e5484d", shape: x.side === "long" ? "arrowUp" : "arrowDown", text: x.side === "long" ? "B" : "S" });
      if (x.exit_time <= t) m.push({ time: toLocalSeconds(x.exit_time), position: "inBar", color: String(x.pnl).startsWith("-") ? "#e5484d" : "#0b9f6e", shape: "circle", text: "" });
    }
    markers?.setMarkers(m.sort((a, b) => a.time - b.time));
    const closed = r.trades.filter((x) => x.exit_time <= t);
    const open = r.trades.filter((x) => x.entry_time <= t && x.exit_time > t);
    const net = closed.reduce((a, x) => a + Number(x.pnl), 0);
    info.replaceChildren(
      el("b", null, when(t)),
      el("span", null, ` · candle ${i + 1} of ${r.candles.length} · close ${fmtAuto(r.candles[i][4])}`),
      el("span", null, ` · equity ${eq.has(t) ? money(eq.get(t).toFixed(2)) : "–"} · ${closed.length} closed trade(s), net ${fmtAuto(net)} · ${open.length} open`),
    );
  };
  show(Math.min(r.candles.length - 1, 120));
}

export const stopReplay = stop;
