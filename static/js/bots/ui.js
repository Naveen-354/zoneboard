// Small DOM and formatting helpers shared by the Bot Studio modules. textContent only, never innerHTML.
import { $, el, api, fmt, fmtAuto, toLocalSeconds } from "../util.js";

export { $, el, api, fmt, fmtAuto };

export const JSON_HEADERS = { "Content-Type": "application/json" };
export const post = (path, body) => api(path, { method: "POST", headers: JSON_HEADERS, body: body === undefined ? undefined : JSON.stringify(body) });
export const put = (path, body) => api(path, { method: "PUT", headers: JSON_HEADERS, body: JSON.stringify(body) });
export const del = (path) => api(path, { method: "DELETE" });

/** "1234567.5" -> "1,234,567.5" without going through a float. */
export function money(s) {
  if (s == null) return "–";
  const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(String(s));
  if (!m) return String(s);
  return m[1] + m[2].replace(/\B(?=(\d{3})+(?!\d))/g, ",") + (m[3] ? "." + m[3].slice(0, 8) : "");
}

/** Decimal string shortened for a tile: 2 places from 1 upward, otherwise up to 6 significant digits. */
export function short(s) {
  const x = Number(s);
  if (!Number.isFinite(x)) return money(s);
  const t = Math.abs(x) >= 1 ? x.toFixed(2) : String(Number(x.toPrecision(6)));
  return money(t);
}

/** Signed money with a colour class. `brief` rounds for tiles; tables keep every decimal. */
export function pnl(node, s, brief = false) {
  const neg = String(s).startsWith("-");
  node.textContent = (neg || String(s) === "0" ? "" : "+") + (brief ? short(s) : money(s));
  node.classList.add(neg ? "dn" : String(s) === "0" ? "flat" : "up");
  return node;
}

export const pct = (x, d = 2) => (x == null || Number.isNaN(x) ? "–" : `${fmt(x, d)}%`);
export const num = (x, d = 2) => (x == null || Number.isNaN(x) ? "–" : fmt(x, d));

export function stat(label, value, cls, hint) {
  const s = el("div", "stat" + (cls ? " " + cls : ""));
  const b = el("b");
  if (value instanceof Node) b.append(value);
  else b.textContent = value;
  s.append(b, el("small", null, label));
  if (hint) s.title = hint;
  return s;
}

export function button(text, cls, onClick, title) {
  const b = el("button", cls || "btn", text);
  b.type = "button";
  if (title) b.title = title;
  if (onClick) b.addEventListener("click", onClick);
  return b;
}

export function field(label, input, hint) {
  const l = el("label", "fld");
  l.append(el("span", null, label), input);
  if (hint) l.append(el("small", null, hint));
  return l;
}

export function input(value, attrs = {}) {
  const i = el("input");
  i.type = attrs.type || "text";
  i.value = value ?? "";
  i.autocomplete = "off";
  for (const [k, v] of Object.entries(attrs)) if (k !== "type") i.setAttribute(k, v);
  return i;
}

export function select(options, value, onChange) {
  const s = el("select");
  for (const o of options) {
    if (o.group) {
      const g = el("optgroup");
      g.label = o.group;
      for (const x of o.items) g.append(option(x));
      s.append(g);
    } else s.append(option(o));
  }
  s.value = value ?? "";
  if (onChange) s.addEventListener("change", () => onChange(s.value));
  return s;
}

function option(o) {
  const op = el("option", null, o.label ?? String(o.value));
  op.value = o.value;
  if (o.disabled) op.disabled = true;
  return op;
}

/** A segmented control. options: [{value, label}] */
export function seg(options, value, onChange, cls = "seg sm") {
  const box = el("div", cls);
  box.setAttribute("role", "tablist");
  const paint = (v) => {
    for (const b of box.children) b.classList.toggle("on", b.dataset.v === v);
  };
  for (const o of options) {
    const b = el("button", null, o.label);
    b.type = "button";
    b.dataset.v = o.value;
    if (o.title) b.title = o.title;
    b.addEventListener("click", () => {
      paint(o.value);
      onChange(o.value);
    });
    box.append(b);
  }
  paint(value);
  return box;
}

export function badge(text, kind) {
  return el("span", "bdg " + (kind || ""), text);
}

export function statusBadge(status) {
  return badge(status.toUpperCase(), { running: "ok", paused: "warn", stopped: "off" }[status] || "");
}

export function avatar(a, name) {
  const d = el("div", "avt");
  d.style.background = /^#[0-9a-f]{6}$/i.test(a?.color || "") ? a.color : "#4f46e5";
  d.textContent = a?.glyph || (name || "?").trim().slice(0, 2).toUpperCase();
  d.setAttribute("aria-hidden", "true");
  return d;
}

export function say(node, text, kind) {
  node.textContent = text || "";
  node.className = "st-msg" + (kind ? " " + kind : "");
}

export function table(headers, rows, cls) {
  const t = el("table", cls);
  const tr = el("tr");
  for (const h of headers) {
    const th = el("th", typeof h === "object" ? h.cls : null, typeof h === "object" ? h.label : h);
    tr.append(th);
  }
  const thead = el("thead");
  thead.append(tr);
  const tb = el("tbody");
  for (const r of rows) tb.append(r);
  t.append(thead, tb);
  const w = el("div", "table-wrap");
  w.append(t);
  return w;
}

export function row(cells) {
  const tr = el("tr");
  for (const c of cells) {
    if (c instanceof Node && c.tagName === "TD") tr.append(c);
    else {
      const td = el("td");
      if (c instanceof Node) td.append(c);
      else td.textContent = c ?? "–";
      tr.append(td);
    }
  }
  return tr;
}

export const td = (text, cls) => el("td", cls, text);

export function when(unix) {
  if (!unix) return "–";
  const d = new Date(unix * 1000);
  return d.toLocaleString(undefined, { year: "2-digit", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function whenIso(s) {
  if (!s) return "–";
  const d = new Date(s);
  return Number.isNaN(d.getTime()) ? s : d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

// ---------------------------------------------------------------- charts (vendored Lightweight Charts)
const LW = () => window.LightweightCharts;
const BASE = (h) => ({
  height: h,
  layout: { background: { type: "solid", color: "#ffffff" }, textColor: "#64748b", fontSize: 11, attributionLogo: true },
  grid: { vertLines: { color: "#f1f3f7" }, horzLines: { color: "#f1f3f7" } },
  rightPriceScale: { borderVisible: false },
  timeScale: { borderVisible: false, timeVisible: true, secondsVisible: false },
  crosshair: { mode: 1 },
});

/** Points must be strictly increasing in time for the chart: keep the last value per second. */
function strictly(points) {
  const out = [];
  for (const p of points) {
    if (out.length && out[out.length - 1].time >= p.time) {
      if (out[out.length - 1].time === p.time) out[out.length - 1] = p;
      continue;
    }
    out.push(p);
  }
  return out;
}

/** A line chart of (unix seconds, value) pairs, in its own container. Returns a dispose function. */
export function lineChart(host, points, { height = 220, color = "#4f46e5", baseline = null } = {}) {
  host.replaceChildren();
  if (!LW() || points.length < 2) {
    host.append(el("div", "chart-empty", points.length < 2 ? "Not enough points to draw yet." : "Chart library unavailable."));
    return () => {};
  }
  const chart = LW().createChart(host, { ...BASE(height), autoSize: true });
  const data = strictly(points.map(([t, v]) => ({ time: toLocalSeconds(t), value: v })));
  const s = chart.addSeries(LW().AreaSeries, { lineColor: color, topColor: color + "33", bottomColor: color + "05", lineWidth: 2, priceLineVisible: false });
  s.setData(data);
  if (baseline != null) s.createPriceLine({ price: baseline, color: "#94a3b8", lineWidth: 1, lineStyle: 2, axisLabelVisible: false, title: "start" });
  chart.timeScale().fitContent();
  return () => chart.remove();
}

/** Candles with indicator lines and trade markers. */
export function priceChart(host, candles, overlays, markers, { height = 360 } = {}) {
  host.replaceChildren();
  if (!LW() || candles.length < 2) {
    host.append(el("div", "chart-empty", "No price data to draw."));
    return () => {};
  }
  const chart = LW().createChart(host, { ...BASE(height), autoSize: true });
  const cs = chart.addSeries(LW().CandlestickSeries, { upColor: "#0b9f6e", downColor: "#e5484d", borderVisible: false, wickUpColor: "#0b9f6e", wickDownColor: "#e5484d", priceLineVisible: false });
  cs.setData(strictly(candles.map(([t, o, h, l, c]) => ({ time: toLocalSeconds(t), open: o, high: h, low: l, close: c }))));
  const colors = ["#4f46e5", "#f59e0b", "#0ea5e9", "#a855f7", "#14b8a6", "#ef4444", "#64748b", "#84cc16"];
  overlays.forEach((o, i) => {
    const s = chart.addSeries(LW().LineSeries, { color: colors[i % colors.length], lineWidth: 1, priceLineVisible: false, lastValueVisible: false, crosshairMarkerVisible: false, title: o.id });
    s.setData(strictly(o.points.map(([t, v]) => ({ time: toLocalSeconds(t), value: v }))));
  });
  if (markers.length && LW().createSeriesMarkers) {
    const m = markers.map((x) => ({ ...x, time: toLocalSeconds(x.time) })).sort((a, b) => a.time - b.time);
    LW().createSeriesMarkers(cs, m);
  }
  chart.timeScale().fitContent();
  return () => chart.remove();
}
