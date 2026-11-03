// Small shared helpers: DOM, number formatting, throttling.

export const $ = (s, root = document) => root.querySelector(s);
export const $$ = (s, root = document) => [...root.querySelectorAll(s)];

export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

// Number formatters are slow to build and fast to reuse, so cache them per (locale, decimals).
const cfg = { locale: "en-US" };
const cache = new Map();

export function setLocale(locale) {
  cfg.locale = locale;
  cache.clear();
}

export function fmt(n, d = 2) {
  if (n == null || Number.isNaN(n)) return "–";
  let f = cache.get(d);
  if (!f) cache.set(d, (f = new Intl.NumberFormat(cfg.locale, { minimumFractionDigits: d, maximumFractionDigits: d })));
  return f.format(n);
}

/** Prices of any size: at least 2 decimals, up to 8 for sub-dollar coins, trailing zeros trimmed. */
export function fmtAuto(n) {
  if (n == null || Number.isNaN(n)) return "–";
  const max = Math.abs(n) >= 100 ? 2 : Math.abs(n) >= 1 ? 4 : 8;
  const key = "a" + max;
  let f = cache.get(key);
  if (!f) cache.set(key, (f = new Intl.NumberFormat(cfg.locale, { minimumFractionDigits: 2, maximumFractionDigits: max })));
  return f.format(n);
}

/** Quantities: no forced decimals (10 stays 10, 0.0153 stays 0.0153), up to 8 places. */
export function fmtQty(n) {
  if (n == null || Number.isNaN(n)) return "–";
  let f = cache.get("q");
  if (!f) cache.set("q", (f = new Intl.NumberFormat(cfg.locale, { maximumFractionDigits: 8 })));
  return f.format(n);
}

export function fmtCompact(n) {
  if (n == null || Number.isNaN(n)) return "–";
  let f = cache.get("c");
  if (!f) cache.set("c", (f = new Intl.NumberFormat(cfg.locale, { notation: "compact", maximumFractionDigits: 2 })));
  return f.format(n);
}

export const signed = (n, d = 2) => (n > 0 ? "+" : "") + fmt(n, d);

/** fetch() that returns parsed JSON and turns {error} bodies into thrown Errors. */
export async function api(path, opts) {
  const r = await fetch(path, opts);
  let body = null;
  try {
    body = await r.json();
  } catch {
    /* no body */
  }
  if (!r.ok) throw new Error(body?.error || r.statusText);
  return body;
}

/** Unix seconds -> seconds that the chart (which draws in UTC) will show as the viewer's local clock. */
export const toLocalSeconds = (t) => t - new Date(t * 1000).getTimezoneOffset() * 60;

/** Run fn at most once per `ms`, always with the latest call's arguments, and once more at the end. */
export function throttle(fn, ms) {
  let last = 0;
  let timer = 0;
  let args;
  return (...a) => {
    args = a;
    const wait = ms - (performance.now() - last);
    if (wait <= 0) {
      last = performance.now();
      fn(...args);
    } else if (!timer) {
      timer = setTimeout(() => {
        timer = 0;
        last = performance.now();
        fn(...args);
      }, wait);
    }
  };
}

/** Coalesce many calls in one frame into a single callback on the next animation frame. */
export function frame(fn) {
  let queued = false;
  return () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      fn();
    });
  };
}

const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Brief green/red wash on an element when its value moves. */
export function flash(node, up) {
  if (reduced || !node.animate) return;
  node.animate([{ backgroundColor: up ? "var(--up-flash)" : "var(--dn-flash)" }, { backgroundColor: "transparent" }], { duration: 650, easing: "ease-out" });
}
