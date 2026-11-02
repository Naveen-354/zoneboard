// Is Binance's public data reachable from here, and does the live pipeline work end to end?
//   node tools/live_check.mjs
// Checks REST history, the WebSocket (ticks + candles), and the zone engine on the real candles.
// Needs Node 22+ (global WebSocket and fetch). No API key involved.
import { BinanceSource, stream, candleFromKline, applyCandle, applyTick, TF } from "../static/js/feed.js";
import { analyze } from "../static/js/zones.js";

let failed = false;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  " + detail : ""}`);
  if (!ok) failed = true;
};

const src = new BinanceSource();
try {
  const t0 = performance.now();
  const { frame, dp } = await src.klines("BTCUSDT", "1m", 500);
  check("REST: 500 BTCUSDT 1m candles", frame.t.length === 500, `(${Math.round(performance.now() - t0)} ms, last close ${frame.c.at(-1)})`);

  const a = analyze(frame, { lastBarOpen: true, dp });
  check("zone engine on live candles", !!a.summary, a.summary ? `(${a.summary.label}, score ${a.summary.score}, ${a.zones.length} zones, ${a.levels.length} levels)` : "");

  let ticks = 0;
  let klines = 0;
  src.subscribe(stream.trade("BTCUSDT"), (d) => {
    ticks++;
    applyTick(frame, +d.p, d.T, TF["1m"]);
  });
  src.subscribe(stream.kline("BTCUSDT", "1m"), (d) => {
    klines++;
    applyCandle(frame, candleFromKline(d));
  });
  await new Promise((r) => setTimeout(r, 8000));
  check("WebSocket: trades arriving", ticks > 0, `(${ticks} in 8 s)`);
  check("WebSocket: candle updates arriving", klines > 0, `(${klines} in 8 s)`);
} catch (e) {
  check("Binance reachable", false, `(${e.message}). Check your connection or VPN; the app still runs with DEMO=1.`);
}
src.dispose();
console.log(failed ? "\nSomething is not working: see FAIL above." : "\nAll good: live crypto data works from this machine.");
process.exit(failed ? 1 : 0);
