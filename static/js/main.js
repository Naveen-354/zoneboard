// ZoneBoard entry point: boots the UI, switches markets, wires controls.
import { $, $$, el, api, setLocale } from "./util.js";
import { Board } from "./board.js";
import { PriceChart } from "./chart.js";
import * as view from "./view.js";
import * as journal from "./journal.js";
import { initTrading } from "./trading.js";
import { BinanceSource } from "./feed.js";
import { createCrypto, info as cryptoInfo } from "./markets/crypto.js";
import { createIndia, info as indiaInfo } from "./markets/india.js";

// ---------------------------------------------------------------- preferences
const KEY = "zoneboard.v1";
const prefs = { market: "crypto", mode: "demo", view: "board", sym: {}, tf: {}, boardTf: {}, zones: true, levels: true };
try {
  Object.assign(prefs, JSON.parse(localStorage.getItem(KEY) || "{}"));
} catch {
  /* private mode or bad JSON: start fresh */
}
const save = () => {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    /* storage unavailable */
  }
};

// ---------------------------------------------------------------- components
const narrow = matchMedia("(max-width: 900px)");
const board = new Board({
  onSelect: (sym) => {
    select(sym);
    if (narrow.matches) $(".view").scrollIntoView({ behavior: "smooth", block: "start" }); // phone: the chart sits below the list
  },
  onSearchEnter,
});
const chart = new PriceChart($("#chart"), $("#legend"));
let market = null;
let config = { sample: false, crypto: { symbols: [] } };
let feedState = "connecting"; // the market's connection state, for the order ticket
// The ACCOUNT (top right). Charts always show real prices: the account only decides what an order is.
//   "demo" = practice account: simulated orders at the real price on screen, logged in a separate journal.
//   "live" = real orders. No broker is connected, so this account can't trade yet.
let mode = "demo";
// Made-up prices instead of real ones: for development without internet only (SAMPLE_DATA=1 or ?sample).
// Not part of the Live/Demo toggle, and ordering is switched off while it's on.
let sample = false;

const trading = initTrading({
  ctx: () => ({ market, mode, sample, sym: market?.symbol ?? null, feedState }),
  openSymbol: (sym) => select(sym),
});

function onSearchEnter(text, firstMatch) {
  if (firstMatch) return select(firstMatch);
  if (market.id === "crypto" && /^[A-Z0-9]{2,20}$/.test(text)) select(text.endsWith("USDT") ? text : text + "USDT");
}

function select(sym) {
  if (!market) return;
  prefs.sym[market.id] = sym;
  save();
  const opened = market.open(sym, market.chartTf);
  trading.render();
  return opened;
}

// ---------------------------------------------------------------- connection pill
function onStatus(s) {
  const c = $("#conn");
  const t = $("#connText");
  const set = (cls, text) => {
    c.className = "conn " + cls;
    t.textContent = text;
  };
  feedState = s.state;
  if (s.state === "live") set(s.sample ? "sample" : "live", s.sample ? `Sample data · made up · ${s.rate}/s` : `Live · ${s.rate} msg/s`);
  else if (s.state === "delayed") s.sample ? set("sample", "Sample data · made-up NSE prices") : set("warn", `Delayed 15 min · updated ${(s.updated || "").slice(11, 16)}`);
  else if (s.state === "connecting") set("warn", "Connecting…");
  else if (s.state === "reconnecting") set("warn", "Reconnecting…");
  else if (s.state === "stale") set("warn", "No data, reconnecting…");
  else set("bad", "Offline");
}

function onError(text) {
  view.banner(text, {
    err: true,
    action: {
      label: "Retry",
      run: () => {
        view.banner(noteFor(market));
        market.reload();
      },
    },
  });
}

// ---------------------------------------------------------------- timeframe controls
function renderTimeframes() {
  const box = $("#tf");
  box.replaceChildren();
  for (const tf of market.chartTfs) {
    const b = el("button", tf === market.chartTf ? "on" : "", tf.toUpperCase());
    b.type = "button";
    b.dataset.tf = tf;
    box.append(b);
  }
  const sel = $("#boardTf");
  sel.replaceChildren(...market.boardTfs.map((tf) => Object.assign(el("option"), { value: tf, textContent: tf.toUpperCase() })));
  sel.value = market.boardTf;
  $("#boardTfWrap").hidden = market.boardTfs.length < 2;
}

function setChartTf(tf) {
  if (!market || !market.chartTfs.includes(tf)) return;
  prefs.tf[market.id] = tf;
  save();
  for (const b of $$("#tf button")) b.classList.toggle("on", b.dataset.tf === tf);
  market.setChartTf(tf);
}

$("#tf").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b) setChartTf(b.dataset.tf);
});

$("#boardTf").addEventListener("change", (e) => {
  prefs.boardTf[market.id] = e.target.value;
  save();
  market.setBoardTf(e.target.value);
});

for (const [id, key] of [["tZones", "zones"], ["tLevels", "levels"]]) {
  const b = $("#" + id);
  b.classList.toggle("on", prefs[key]);
  b.addEventListener("click", () => {
    prefs[key] = !prefs[key];
    b.classList.toggle("on", prefs[key]);
    chart.showOverlay({ zones: prefs.zones, levels: prefs.levels });
    save();
  });
}
chart.showOverlay({ zones: prefs.zones, levels: prefs.levels });

// ---------------------------------------------------------------- market and account switching
/** The banner under the header: what to know about this market's data (or that the data is made up). */
const noteFor = (m) => (sample ? "Sample data is on: every price here is made up. For development only, so ordering is switched off." : m.note);

async function switchMarket(id) {
  market?.stop();
  market = null;
  prefs.market = id;
  save();
  for (const b of $$("#market button")) b.classList.toggle("on", b.dataset.m === id);

  const info = id === "crypto" ? cryptoInfo : indiaInfo;
  setLocale(info.locale);
  journal.setContext(info, mode);
  view.banner(noteFor(info));
  trading.reset(); // the previous market's positions are not this market's
  board.setItems([]);
  chart.clear();
  view.setSymbol("Pick a symbol");
  view.chips(null);
  view.msg("");
  onStatus({ state: "connecting" });

  const saved = (list, key) => (list.includes(prefs[key][id]) ? prefs[key][id] : undefined);
  const opts = { board, chart, onStatus, onError, chartTf: saved(info.chartTfs, "tf"), boardTf: saved(info.boardTfs, "boardTf") };
  let m;
  if (id === "crypto") {
    const src = sample ? new (await import("./sim.js")).SimSource() : new BinanceSource(); // the simulator is only fetched for sample data
    m = createCrypto({ ...opts, src, symbols: config.crypto.symbols });
  } else {
    m = createIndia({ ...opts, sample });
  }
  market = m;
  renderTimeframes();
  trading.refresh();

  const started = m.start(); // crypto: starts streaming at once; India: resolves to the strongest setup
  const sym = prefs.sym[id] || (id === "crypto" ? config.crypto.symbols[0] : await started);
  if (market === m && sym) select(sym);
  if (market === m && prefs.view === "journal") journal.load();
}

$("#market").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b && b.dataset.m !== market?.id) switchMarket(b.dataset.m);
});

/** Live <-> Demo account (top right). */
function applyMode(next) {
  mode = next;
  document.body.classList.toggle("is-demo", mode === "demo");
  for (const b of $$("#mode button")) b.classList.toggle("on", b.dataset.mode === mode);
}

// Both accounts see the same real prices, so switching restarts nothing: only the ticket, the open positions and the
// journal change over.
$("#mode").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (!b || b.dataset.mode === mode) return;
  applyMode(b.dataset.mode);
  prefs.mode = mode;
  save();
  if (!market) return;
  journal.setContext(market, mode);
  trading.reset();
  trading.refresh();
  if (prefs.view === "journal") journal.load();
});

$("#goChart").addEventListener("click", () => showView("board"));

// ---------------------------------------------------------------- views
function showView(v) {
  prefs.view = v;
  save();
  $("#boardView").hidden = v !== "board";
  $("#journalView").hidden = v !== "journal";
  for (const b of $$("#tabs button")) b.classList.toggle("on", b.dataset.v === v);
  if (v === "journal") journal.load();
  else trading.render();
}
$("#tabs").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b) showView(b.dataset.v);
});

// ---------------------------------------------------------------- keyboard
addEventListener("keydown", (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey || !market) return;
  if (/^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName) || prefs.view !== "board") return;
  if (e.key === "/") {
    e.preventDefault();
    $("#search").focus();
  } else if (e.key === "j" || e.key === "ArrowDown" || e.key === "k" || e.key === "ArrowUp") {
    e.preventDefault();
    const next = board.step(e.key === "j" || e.key === "ArrowDown" ? 1 : -1);
    if (next) select(next);
  } else if (/^[1-9]$/.test(e.key)) {
    setChartTf(market.chartTfs[+e.key - 1]);
  }
});

// ---------------------------------------------------------------- boot
(async () => {
  try {
    config = await api("/api/config");
  } catch (e) {
    onError(`Can't reach the ZoneBoard server: ${e.message}`);
    return;
  }
  trading.setFees(config.fees);
  const params = new URLSearchParams(location.search);
  // Account: ?live or ?demo in the URL wins, otherwise your last choice, otherwise the practice (demo) account.
  applyMode(params.has("live") ? "live" : params.has("demo") ? "demo" : prefs.mode === "live" ? "live" : "demo");
  // Made-up data (development): SAMPLE_DATA=1 on the server or ?sample in the URL; ?sample=0 forces real data.
  sample = params.get("sample") === "0" ? false : config.sample || params.has("sample");
  showView(prefs.view === "journal" ? "journal" : "board");
  await switchMarket(prefs.market === "india" ? "india" : "crypto");
})();
