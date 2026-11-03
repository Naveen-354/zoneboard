// Live crypto data straight from Binance's public market-data endpoints. No API key, no account.
// Docs: "Market Data Only URLs" (data-api.binance.vision, data-stream.binance.vision). Public data only.
//
// A "source" is anything with: klines(sym, tf, limit), subscribe(stream, cb) -> unsubscribe,
// onStatus(cb), onReconnect(cb). BinanceSource is the real one; sim.js has an offline look-alike.

export const REST = "https://data-api.binance.vision";
export const WS_URL = "wss://data-stream.binance.vision/stream";

/** Candle length in seconds. Binance aligns these to UTC (4h at 00/04/08..., 1d at 00:00 UTC). */
export const TF = { "1m": 60, "5m": 300, "15m": 900, "1h": 3600, "4h": 14400, "1d": 86400 };

export const stream = {
  kline: (sym, tf) => `${sym.toLowerCase()}@kline_${tf}`,
  trade: (sym) => `${sym.toLowerCase()}@aggTrade`,
  ticker: (sym) => `${sym.toLowerCase()}@miniTicker`,
};

const MAX_BARS = 1500;

/** Decimals needed to print a price string exactly, ignoring trailing zeros ("0.00001200" -> 5). */
function decimals(s) {
  const i = s.indexOf(".");
  if (i < 0) return 0;
  let e = s.length;
  while (e > i + 1 && s[e - 1] === "0") e--;
  return e - i - 1;
}

/** Binance kline rows -> frame (see zones.js) plus the price precision seen in the closes. */
export function parseKlines(rows) {
  const f = { t: [], o: [], h: [], l: [], c: [], v: [], lastOpen: true };
  let dp = 0;
  for (const r of rows) {
    f.t.push(r[0] / 1000);
    f.o.push(+r[1]);
    f.h.push(+r[2]);
    f.l.push(+r[3]);
    f.c.push(+r[4]);
    f.v.push(+r[5]);
    const d = decimals(r[4]);
    if (d > dp) dp = d;
  }
  return { frame: f, dp: Math.min(8, Math.max(2, dp)) };
}

/** Kline stream payload -> candle. */
export function candleFromKline(d) {
  const k = d.k;
  return { time: k.t / 1000, o: +k.o, h: +k.h, l: +k.l, c: +k.c, v: +k.v, closed: k.x };
}

function trim(f) {
  if (f.t.length <= MAX_BARS + 100) return;
  const cut = f.t.length - MAX_BARS;
  for (const k of ["t", "o", "h", "l", "c", "v"]) f[k].splice(0, cut);
}

/** Apply an authoritative candle from the kline stream. Returns true if the frame changed. */
export function applyCandle(f, cd) {
  const n = f.t.length - 1;
  let i = -1;
  if (cd.time === f.t[n]) i = n;
  else if (cd.time > f.t[n]) {
    f.t.push(cd.time);
    f.o.push(cd.o);
    f.h.push(cd.h);
    f.l.push(cd.l);
    f.c.push(cd.c);
    f.v.push(cd.v);
    f.lastOpen = !cd.closed;
    trim(f);
    return true;
  } else {
    // late close message for the bar before the one a tick already started
    for (let k = n - 1; k >= Math.max(0, n - 3); k--) if (f.t[k] === cd.time) i = k;
    if (i < 0) return false;
  }
  f.o[i] = cd.o;
  f.h[i] = cd.h;
  f.l[i] = cd.l;
  f.c[i] = cd.c;
  f.v[i] = cd.v;
  if (i === n) f.lastOpen = !cd.closed;
  return true;
}

/** Apply one trade to the forming candle. Returns "new", "upd" or null (late tick for an old bar). */
export function applyTick(f, price, tMs, tfSec) {
  const b = Math.floor(tMs / 1000 / tfSec) * tfSec;
  const n = f.t.length - 1;
  if (b === f.t[n]) {
    if (price > f.h[n]) f.h[n] = price;
    if (price < f.l[n]) f.l[n] = price;
    f.c[n] = price;
    return "upd";
  }
  if (b > f.t[n]) {
    f.t.push(b);
    f.o.push(price);
    f.h.push(price);
    f.l.push(price);
    f.c.push(price);
    f.v.push(0);
    f.lastOpen = true;
    trim(f);
    return "new";
  }
  return null;
}

export class BinanceSource {
  constructor() {
    this.live = true;
    this.subs = new Map(); // stream -> Set<cb>
    this.ws = null;
    this.state = "connecting";
    this.tries = 0;
    this.everOpen = false;
    this.msgs = 0;
    this.lag = null;
    this.lastMsg = 0;
    this.statusCbs = new Set();
    this.reconnectCbs = new Set();
    this.toSub = new Set();
    this.toUnsub = new Set();
    this.inflight = new Map();
    this.timer = setInterval(() => this._tick(), 1000);
    this._connect();
  }

  onStatus(cb) {
    this.statusCbs.add(cb);
    cb(this._status());
    return () => this.statusCbs.delete(cb);
  }

  onReconnect(cb) {
    this.reconnectCbs.add(cb);
    return () => this.reconnectCbs.delete(cb);
  }

  _status(rate = 0) {
    return { state: this.state, rate, lag: this.lag, sample: false };
  }

  _setState(s) {
    this.state = s;
    for (const cb of this.statusCbs) cb(this._status());
  }

  _connect() {
    let ws;
    try {
      ws = new WebSocket(WS_URL);
    } catch {
      this._retry();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.tries = 0;
      this.lastMsg = performance.now();
      this.toSub.clear();
      this.toUnsub.clear();
      this._send("SUBSCRIBE", [...this.subs.keys()]);
      this.lastSend = performance.now();
      const reconnected = this.everOpen;
      this.everOpen = true;
      this._setState("live");
      if (reconnected) for (const cb of this.reconnectCbs) cb();
    };
    ws.onmessage = (ev) => {
      this.msgs++;
      this.lastMsg = performance.now();
      let m;
      try {
        m = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (!m.stream) return;
      const d = m.data;
      if (d.E) this.lag = Date.now() - d.E;
      const set = this.subs.get(m.stream);
      if (set) for (const cb of set) cb(d);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this._retry();
    };
    ws.onerror = () => {}; // onclose always follows
  }

  _retry() {
    if (this.dead) return;
    this._setState(this.everOpen ? "reconnecting" : "connecting");
    const wait = Math.min(15000, 400 * 2 ** this.tries++) * (0.7 + Math.random() * 0.6);
    setTimeout(() => this._connect(), wait);
  }

  _tick() {
    const rate = this.msgs;
    this.msgs = 0;
    if (this.state === "live" && this.subs.size && performance.now() - this.lastMsg > 10000) {
      this._setState("stale");
      this._reset();
      return;
    }
    for (const cb of this.statusCbs) cb(this._status(rate));
  }

  /** Drop the current socket without waiting for its close handshake (a dead link never finishes one), then reconnect. */
  _reset() {
    const old = this.ws;
    this.ws = null;
    if (old) {
      old.onopen = old.onmessage = old.onclose = old.onerror = null;
      try {
        old.close();
      } catch {
        /* already closed */
      }
    }
    this._retry();
  }

  _send(method, params) {
    if (!params.length || this.ws?.readyState !== WebSocket.OPEN) return;
    for (let i = 0; i < params.length; i += 150) {
      this.ws.send(JSON.stringify({ method, params: params.slice(i, i + 150), id: Date.now() + i }));
    }
  }

  /**
   * Binance drops a socket that sends more than 5 control messages a second. Subscribes arrive in bursts
   * (30 board streams as their history loads), so they are batched and sent at most every 500 ms
   * (at most 2 messages per send: one SUBSCRIBE, one UNSUBSCRIBE).
   */
  _schedule() {
    if (this.flushTimer) return;
    const wait = Math.max(0, (this.lastSend || 0) + 500 - performance.now());
    this.flushTimer = setTimeout(() => {
      this.flushTimer = 0;
      this._flush();
    }, wait);
  }

  _flush() {
    const sub = [...this.toSub];
    const unsub = [...this.toUnsub];
    this.toSub.clear();
    this.toUnsub.clear();
    if (sub.length || unsub.length) this.lastSend = performance.now();
    this._send("SUBSCRIBE", sub);
    this._send("UNSUBSCRIBE", unsub);
  }

  subscribe(name, cb) {
    let set = this.subs.get(name);
    if (!set) {
      set = new Set();
      this.subs.set(name, set);
      this.toUnsub.delete(name);
      this.toSub.add(name);
      this._schedule();
    }
    set.add(cb);
    return () => {
      set.delete(cb);
      if (!set.size && this.subs.get(name) === set) {
        this.subs.delete(name);
        this.toSub.delete(name);
        this.toUnsub.add(name);
        this._schedule();
      }
    };
  }

  async klines(sym, tf, limit = 500) {
    const url = `${REST}/api/v3/klines?symbol=${encodeURIComponent(sym)}&interval=${tf}&limit=${limit}`;
    let p = this.inflight.get(url);
    if (!p) {
      p = this._get(url).finally(() => this.inflight.delete(url));
      this.inflight.set(url, p);
    }
    const rows = await p;
    return parseKlines(rows);
  }

  /** One-shot 24h stats so the board has prices and changes before each symbol's first ticker message. */
  async tickers(symbols) {
    const q = encodeURIComponent(JSON.stringify(symbols));
    const rows = await this._get(`${REST}/api/v3/ticker/24hr?type=MINI&symbols=${q}`);
    return rows.map((r) => ({ sym: r.symbol, last: +r.lastPrice, open: +r.openPrice }));
  }

  async _get(url) {
    let last;
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const r = await fetch(url, { signal: AbortSignal.timeout(8000) });
        if (r.ok) return await r.json();
        const body = await r.json().catch(() => null);
        throw Object.assign(new Error(body?.msg || `Binance returned ${r.status}`), { fatal: r.status >= 400 && r.status < 500 && r.status !== 429 });
      } catch (e) {
        last = e;
        if (e.fatal) break;
        await new Promise((res) => setTimeout(res, 400));
      }
    }
    throw last;
  }

  dispose() {
    this.dead = true;
    clearInterval(this.timer);
    this.ws?.close();
  }
}
