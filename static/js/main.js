// ZoneBoard entry point: boots the UI, switches markets, wires controls.
import { $, $$, el, api, setLocale } from "./util.js";
import { Board } from "./board.js";
import { PriceChart } from "./chart.js";
import * as view from "./view.js";
import * as journal from "./journal.js";
import { BinanceSource } from "./feed.js";
import { createCrypto, info as cryptoInfo } from "./markets/crypto.js";
import { createIndia, info as indiaInfo } from "./markets/india.js";

// ---------------------------------------------------------------- preferences
const KEY = "zoneboard.v1";
const prefs = { market: "crypto", view: "board", sym: {}, tf: {}, boardTf: {}, zones: true, levels: true };
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
let config = { demo: false, crypto: { symbols: [] } };
const demo = () => config.demo || new URLSearchParams(location.search).has("demo");

function onSearchEnter(text, firstMatch) {
  if (firstMatch) return select(firstMatch);
  if (market.id === "crypto" && /^[A-Z0-9]{2,20}$/.test(text)) select(text.endsWith("USDT") ? text : text + "USDT");
}

function select(sym) {
  if (!market) return;
  prefs.sym[market.id] = sym;
  save();
  journal.prefill(sym);
  return market.open(sym, market.chartTf);
}

// ---------------------------------------------------------------- connection pill
function onStatus(s) {
  const c = $("#conn");
  const t = $("#connText");
  const set = (cls, text) => {
    c.className = "conn " + cls;
    t.textContent = text;
  };
  if (s.state === "live") set(s.demo ? "demo" : "live", s.demo ? `Demo · simulated · ${s.rate}/s` : `Live · ${s.rate} msg/s`);
  else if (s.state === "delayed") set("warn", `Delayed 15 min · updated ${(s.updated || "").slice(11, 16)}`);
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
        view.banner(market.note);
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

// ---------------------------------------------------------------- market switching
async function switchMarket(id) {
  market?.stop();
  market = null;
  prefs.market = id;
  save();
  for (const b of $$("#market button")) b.classList.toggle("on", b.dataset.m === id);

  const info = id === "crypto" ? cryptoInfo : indiaInfo;
  setLocale(info.locale);
  journal.setMarket(info);
  view.banner(info.note);
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
    const src = demo() ? new (await import("./sim.js")).SimSource() : new BinanceSource(); // the simulator is only fetched in demo mode
    m = createCrypto({ ...opts, src, symbols: config.crypto.symbols });
  } else {
    m = createIndia(opts);
  }
  market = m;
  renderTimeframes();

  const started = m.start(); // crypto: starts streaming at once; India: resolves to the strongest setup
  const sym = prefs.sym[id] || (id === "crypto" ? config.crypto.symbols[0] : await started);
  if (market === m && sym) select(sym);
  if (market === m && prefs.view === "journal") journal.load();
}

$("#market").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b && b.dataset.m !== market?.id) switchMarket(b.dataset.m);
});

// ---------------------------------------------------------------- views
function showView(v) {
  prefs.view = v;
  save();
  $("#boardView").hidden = v !== "board";
  $("#journalView").hidden = v !== "journal";
  for (const b of $$("#tabs button")) b.classList.toggle("on", b.dataset.v === v);
  if (v === "journal") {
    journal.prefill(market?.symbol);
    journal.load();
  }
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
  showView(prefs.view === "journal" ? "journal" : "board");
  await switchMarket(prefs.market === "india" ? "india" : "crypto");
})();
