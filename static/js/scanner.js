// Market Scanner: configurable scans over the crypto or NSE symbol list. Every result shows the formula, the
// source and timestamp, and whether that symbol's data is fresh. A hit can open a backtest; never an order.
import { $, el, api, fmtAuto } from "./util.js";

const JSON_HEADERS = { "Content-Type": "application/json" };
let cat = null;
let onResearch = () => {};

export function init(opts) {
  onResearch = opts.onResearch || onResearch;
}

function field(label, node) {
  const l = el("label", "fld");
  l.append(el("span", null, label), node);
  return l;
}

export async function load() {
  const host = $("#scanBody");
  if (!cat) {
    try {
      cat = await api("/api/scanner");
    } catch (e) {
      host.replaceChildren(el("p", "st-msg err", e.message));
      return;
    }
  }
  const form = el("section", "card st-card");
  const tpl = el("select");
  for (const t of cat.templates) {
    const o = el("option", null, t.name);
    o.value = t.id;
    tpl.append(o);
  }
  const market = el("select");
  for (const [v, l] of [
    ["crypto", "Crypto (Binance)"],
    ["india", "NSE (Yahoo, delayed)"],
  ]) {
    const o = el("option", null, l);
    o.value = v;
    market.append(o);
  }
  const iv = el("select");
  const params = el("div", "st-grid");
  const formula = el("p", "st-about");
  const fill = () => {
    const t = cat.templates.find((x) => x.id === tpl.value);
    const ivs = market.value === "crypto" ? ["5m", "15m", "1h", "4h", "1d"] : ["15m", "1h", "1d"];
    iv.replaceChildren(...ivs.map((v) => Object.assign(el("option", null, v), { value: v })));
    iv.value = t.id === "gap" ? "1d" : market.value === "crypto" ? "1h" : "1d";
    params.replaceChildren(
      ...t.params.map((p) => {
        const i = el("input");
        i.type = "number";
        i.step = "any";
        i.value = p.default;
        i.name = p.name;
        return field(p.name, i);
      }),
    );
    formula.textContent = `Formula: ${t.formula}. Markets: ${t.markets.join(", ")}.`;
  };
  tpl.addEventListener("change", fill);
  market.addEventListener("change", fill);
  fill();
  const msg = el("p", "st-msg");
  const out = el("div");
  const run = el("button", "btn", "Run scan");
  run.type = "button";
  run.addEventListener("click", async () => {
    const p = {};
    for (const i of params.querySelectorAll("input")) p[i.name] = Number(i.value);
    msg.textContent = "Scanning the universe (closed candles only)…";
    msg.className = "st-msg";
    run.disabled = true;
    try {
      const r = await api("/api/scanner/run", { method: "POST", headers: JSON_HEADERS, body: JSON.stringify({ template: tpl.value, market: market.value, interval: iv.value, params: p }) });
      msg.textContent = "";
      out.replaceChildren(results(r));
    } catch (e) {
      msg.textContent = e.message;
      msg.className = "st-msg err";
    } finally {
      run.disabled = false;
    }
  });
  const g = el("div", "st-grid");
  g.append(field("Scan", tpl), field("Market", market), field("Timeframe", iv));
  const bar = el("div", "st-actions");
  bar.append(run, msg);
  form.append(el("h3", null, "Market Scanner"), g, params, formula, bar);
  const uns = el("section", "card st-card");
  uns.append(el("h4", null, "Not available (and why)"));
  const ul = el("ul", "gate-list");
  for (const u of cat.unsupported) {
    const li = el("li", "no");
    li.append(el("i", null, "✕"), el("b", null, u.feature), el("small", null, u.why));
    ul.append(li);
  }
  uns.append(ul);
  host.replaceChildren(form, out, uns);
}

function results(r) {
  const c = el("section", "card st-card");
  c.append(
    el("h4", null, `${r.name}: ${r.results.length} of ${r.scanned} symbols`),
    el("p", "st-about", `${r.source} · ${r.interval} · scanned ${new Date(r.scanned_utc).toLocaleString()} · ${r.note}${r.stale_symbols.length ? ` · stale data: ${r.stale_symbols.join(", ")}` : ""}${r.failed.length ? ` · ${r.failed.length} symbol(s) failed to load` : ""}`),
  );
  const t = el("table");
  const hr = el("tr");
  for (const h of ["Symbol", "Result", "Score", "Close", "Bar", "Data", ""]) hr.append(el("th", null, h));
  const th = el("thead");
  th.append(hr);
  const tb = el("tbody");
  for (const x of r.results.slice(0, 100)) {
    const tr = el("tr");
    const go = el("button", "btn ghost sm", "Backtest");
    go.type = "button";
    go.title = "Open a research experiment on this symbol in Bot Studio";
    go.addEventListener("click", () => onResearch({ market: r.market, symbol: x.symbol, interval: r.interval }));
    const td = el("td");
    td.append(go);
    tr.append(el("td", null, x.symbol), el("td", "notes", x.detail), el("td", "num", String(x.score)), el("td", "num", fmtAuto(x.close)), el("td", null, new Date(x.bar_time * 1000).toLocaleString()), el("td", x.fresh ? "up" : "dn", x.fresh ? "fresh" : "STALE"), td);
    tb.append(tr);
  }
  t.append(th, tb);
  const w = el("div", "table-wrap");
  w.append(t);
  c.append(r.results.length ? w : el("p", "st-about", "No symbol matched."));
  return c;
}
