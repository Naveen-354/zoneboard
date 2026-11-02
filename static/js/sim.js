// Offline stand-in for BinanceSource (random-walk prices, no network). Used with DEMO=1 or ?demo=1,
// for trying the UI without internet. Same interface and the same message shapes as the real feed.
// Prices here are made up and mean nothing.
import { TF } from "./feed.js";

const hash = (s) => {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
};
const rng = (seed) => () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const randn = (r) => Math.sqrt(-2 * Math.log(1 - r())) * Math.cos(2 * Math.PI * r());
const STEP_MS = 100;

export class SimSource {
  constructor() {
    this.live = true;
    this.subs = new Map();
    this.syms = new Map(); // sym -> {px, o24, h24, l24, v24}
    this.cur = new Map(); // "sym|tf" -> forming candle
    this.statusCbs = new Set();
    this.msgs = 0;
    this.n = 0;
    this.r = rng(Date.now());
    this.timer = setInterval(() => this._step(), STEP_MS);
    this.statusTimer = setInterval(() => {
      const st = { state: "live", rate: this.msgs, lag: 0, demo: true };
      this.msgs = 0;
      for (const cb of this.statusCbs) cb(st);
    }, 1000);
  }

  onStatus(cb) {
    this.statusCbs.add(cb);
    cb({ state: "live", rate: 0, lag: 0, demo: true });
    return () => this.statusCbs.delete(cb);
  }

  onReconnect() {
    return () => {};
  }

  _sym(sym) {
    let s = this.syms.get(sym);
    if (!s) {
      const u = (hash(sym) % 1000) / 1000;
      const px = 10 ** (u * 6 - 1.5);
      s = { px, o24: px * (0.97 + this.r() * 0.06), h24: px, l24: px, v24: 1e5 * (1 + this.r()) };
      this.syms.set(sym, s);
    }
    return s;
  }

  _dp(px) {
    return px >= 1000 ? 2 : px >= 10 ? 3 : px >= 1 ? 4 : 5;
  }

  subscribe(name, cb) {
    let set = this.subs.get(name);
    if (!set) this.subs.set(name, (set = new Set()));
    set.add(cb);
    return () => {
      set.delete(cb);
      if (!set.size) this.subs.delete(name);
    };
  }

  async klines(sym, tf, limit = 500) {
    sym = sym.toUpperCase();
    const s = this._sym(sym);
    const sec = TF[tf];
    const r = rng(hash(sym + tf));
    const sigma = 0.0012 * Math.sqrt(sec / 60) ** 0.8;
    const bucket = Math.floor(Date.now() / 1000 / sec) * sec;
    const f = { t: [], o: [], h: [], l: [], c: [], v: [], lastOpen: true };
    let p = s.px;
    let drift = 0;
    const closes = [];
    for (let i = 0; i < limit; i++) {
      drift = 0.92 * drift + 0.08 * sigma * randn(r);
      const jump = r() < 0.05 ? (r() < 0.5 ? -1 : 1) * sigma * (2.5 + 2 * r()) : 0;
      const o = p;
      const c = o * Math.exp(drift + sigma * 0.8 * randn(r) + jump);
      const wick = sigma * 0.5;
      f.t.push(bucket - (limit - 1 - i) * sec);
      f.o.push(o);
      f.c.push(c);
      f.h.push(Math.max(o, c) * (1 + Math.abs(randn(r)) * wick));
      f.l.push(Math.min(o, c) * (1 - Math.abs(randn(r)) * wick));
      f.v.push(Math.exp(randn(r) * 0.5) * (1 + Math.abs(c / o - 1) * 400) * 1000);
      p = c;
    }
    const k = s.px / p; // anchor the last close to the live price
    for (let i = 0; i < limit; i++) {
      f.o[i] *= k;
      f.h[i] *= k;
      f.l[i] *= k;
      f.c[i] *= k;
    }
    const key = sym + "|" + tf;
    const prev = this.cur.get(key);
    const n = limit - 1;
    if (prev && prev.t === f.t[n] * 1000) {
      f.o[n] = prev.o;
      f.h[n] = prev.h;
      f.l[n] = prev.l;
      f.c[n] = prev.c;
      f.v[n] = prev.v;
    } else {
      this.cur.set(key, { t: f.t[n] * 1000, o: f.o[n], h: f.h[n], l: f.l[n], c: f.c[n], v: f.v[n] });
    }
    await new Promise((res) => setTimeout(res, 30));
    return { frame: f, dp: this._dp(s.px) };
  }

  async tickers(symbols) {
    return symbols.map((sym) => {
      const s = this._sym(sym);
      return { sym, last: s.px, open: s.o24 };
    });
  }

  _emit(name, data) {
    const set = this.subs.get(name);
    if (!set) return;
    this.msgs++;
    for (const cb of set) cb(data);
  }

  _step() {
    const now = Date.now();
    this.n++;
    const bySym = new Map(); // sym -> {trade, ticker, klines:[tf]}
    for (const name of this.subs.keys()) {
      const [s, kind] = name.split("@");
      const sym = s.toUpperCase();
      let e = bySym.get(sym);
      if (!e) bySym.set(sym, (e = { klines: [] }));
      if (kind === "aggTrade") e.trade = true;
      else if (kind === "miniTicker") e.ticker = true;
      else if (kind.startsWith("kline_")) e.klines.push(kind.slice(6));
    }
    const everySecond = this.n % (1000 / STEP_MS) === 0;
    for (const [sym, e] of bySym) {
      const s = this._sym(sym);
      s.px *= Math.exp(0.0002 * randn(this.r) + (this.r() < 0.01 ? 0.0015 * randn(this.r) : 0));
      if (s.px > s.h24) s.h24 = s.px;
      if (s.px < s.l24) s.l24 = s.px;
      const dp = this._dp(s.px);
      const px = s.px.toFixed(dp);
      const lower = sym.toLowerCase();
      if (e.trade && this.r() < 0.7) {
        this._emit(`${lower}@aggTrade`, { e: "aggTrade", E: now, s: sym, p: px, q: (this.r() * 0.5).toFixed(5), T: now, m: this.r() < 0.5 });
      }
      for (const tf of e.klines) {
        const sec = TF[tf];
        const key = sym + "|" + tf;
        const b = Math.floor(now / 1000 / sec) * sec * 1000;
        let c = this.cur.get(key);
        if (!c) this.cur.set(key, (c = { t: b, o: s.px, h: s.px, l: s.px, c: s.px, v: 0 }));
        const msg = (x) => ({ e: "kline", E: now, s: sym, k: { t: c.t, T: c.t + sec * 1000 - 1, s: sym, i: tf, o: c.o.toFixed(dp), c: c.c.toFixed(dp), h: c.h.toFixed(dp), l: c.l.toFixed(dp), v: c.v.toFixed(3), x } });
        if (c.t < b) {
          this._emit(`${lower}@kline_${tf}`, msg(true));
          c = { t: b, o: s.px, h: s.px, l: s.px, c: s.px, v: 0 };
          this.cur.set(key, c);
        }
        c.c = s.px;
        if (s.px > c.h) c.h = s.px;
        if (s.px < c.l) c.l = s.px;
        c.v += this.r() * 20;
        if (everySecond) this._emit(`${lower}@kline_${tf}`, msg(false));
      }
      if (e.ticker && everySecond) {
        s.v24 += this.r() * 50;
        this._emit(`${lower}@miniTicker`, { e: "24hrMiniTicker", E: now, s: sym, c: px, o: s.o24.toFixed(dp), h: s.h24.toFixed(dp), l: s.l24.toFixed(dp), v: s.v24.toFixed(2), q: (s.v24 * s.px).toFixed(2) });
      }
    }
  }

  dispose() {
    clearInterval(this.timer);
    clearInterval(this.statusTimer);
  }
}
