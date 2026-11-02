// The chart panel's header, stat chips and status message. Plain DOM writes, kept in one place.
import { $, el, fmt, signed, flash } from "./util.js";

const px = $("#px");
let lastPx = null;
let lastText = "";

export function setSymbol(sym, name = "") {
  $("#sym").textContent = sym;
  $("#symName").textContent = name;
  lastPx = null;
  lastText = "";
  px.textContent = "";
  $("#pxChg").textContent = "";
}

export function setPrice(price, dp) {
  const t = fmt(price, dp);
  if (t === lastText) return;
  if (lastPx != null) flash(px, price > lastPx);
  px.textContent = t;
  lastText = t;
  lastPx = price;
}

export function setChange(pct, label = "24h") {
  const c = $("#pxChg");
  if (pct == null) {
    c.textContent = "";
    return;
  }
  c.textContent = `${signed(pct)}% ${label}`;
  c.className = pct >= 0 ? "up" : "dn";
}

function chip(label, value, cls) {
  const c = el("div", "chip" + (cls ? " " + cls : ""));
  c.append(el("small", null, label), el("b", null, value));
  return c;
}

export function chips(s, dp) {
  const box = $("#chips");
  if (!s) {
    box.replaceChildren();
    return;
  }
  const bias = s.label === "choppy" ? `choppy, leaning ${s.bias}` : s.label;
  box.replaceChildren(
    chip("Bias", `${bias} · ${fmt(s.score, 0)}`, s.label === "bullish" ? "up" : s.label === "bearish" ? "dn" : ""),
    chip("Resistance", fmt(s.resistance, dp)),
    chip(s.bias === "bullish" ? "Bull OB" : "Bear OB", s.ob ? `${fmt(s.ob.bottom, dp)} – ${fmt(s.ob.top, dp)}` : "none nearby"),
    chip("Support", fmt(s.support, dp)),
    chip("Liquidity", fmt(s.liquidity, dp)),
    chip("Rel. volume", fmt(s.rvol) + "×"),
  );
}

/** Message over the chart. kind: "" (info), "load" (spinner), "err". Empty text hides it. */
export function msg(text, kind = "") {
  const m = $("#msg");
  m.className = "msg" + (kind === "err" ? " err" : "");
  m.replaceChildren();
  if (!text) return;
  if (kind === "load") m.append(el("span", "spin"));
  m.append(document.createTextNode(text));
}

/** Banner under the header. Pass null to hide. */
export function banner(text, { err = false, action = null } = {}) {
  const b = $("#banner");
  if (!text) {
    b.hidden = true;
    return;
  }
  b.hidden = false;
  b.className = "banner" + (err ? " err" : "");
  b.replaceChildren(el("span", null, text));
  if (action) {
    const btn = el("button", null, action.label);
    btn.type = "button";
    btn.addEventListener("click", action.run);
    b.append(btn);
  }
}
