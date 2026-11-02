// Candlestick chart (TradingView Lightweight Charts v5, vendored) with order-block zones and liquidity
// levels drawn by a series primitive, so they are painted in the chart's own render pass, in sync
// with the candles. No polling, no DOM overlay.
import { fmt, fmtCompact, toLocalSeconds } from "./util.js";

const LW = window.LightweightCharts;

const C = {
  up: "#0ea271",
  dn: "#e5484d",
  text: "#64748b",
  grid: "#f1f3f7",
  cross: "#94a3b8",
  liq: "#c27803",
};
const FONT = 'ui-sans-serif, system-ui, -apple-system, "Segoe UI Variable Text", "Segoe UI", Roboto, sans-serif';

class Overlay {
  constructor() {
    this.zones = [];
    this.levels = [];
    this.dp = 2;
    this.on = { zones: true, levels: true };
    this._views = [this._view("bottom", false), this._view("top", true)];
  }

  _view(z, labels) {
    return { zOrder: () => z, renderer: () => ({ draw: (target) => this._draw(target, labels) }) };
  }

  attached({ chart, series, requestUpdate }) {
    this.chart = chart;
    this.series = series;
    this.refresh = requestUpdate;
  }

  detached() {
    this.chart = this.series = this.refresh = null;
  }

  paneViews() {
    return this._views;
  }

  updateAllViews() {}

  set(zones, levels, dp) {
    this.zones = zones;
    this.levels = levels;
    this.dp = dp;
    this.refresh?.();
  }

  show(on) {
    this.on = on;
    this.refresh?.();
  }

  _draw(target, labels) {
    if (!this.chart) return;
    target.useMediaCoordinateSpace(({ context: ctx, mediaSize }) => {
      const ts = this.chart.timeScale();
      const w = mediaSize.width;
      const y = (p) => this.series.priceToCoordinate(p);
      const x0 = (t) => Math.max(0, ts.timeToCoordinate(toLocalSeconds(t)) ?? 0);
      ctx.font = `600 10.5px ${FONT}`;
      ctx.textBaseline = "middle";

      // Labels are drawn nearest-to-price first and skipped when they would land on one already drawn.
      const placed = [];
      const claim = (b) => {
        if (placed.some((p) => b.x < p.x + p.w && b.x + b.w > p.x && b.y < p.y + p.h && b.y + b.h > p.y)) return false;
        placed.push(b);
        return true;
      };

      if (this.on.zones) {
        for (const z of this.zones) {
          const y1 = y(z.top);
          const y2 = y(z.bottom);
          const x = x0(z.start);
          if (y1 == null || y2 == null || x >= w) continue;
          const top = Math.min(y1, y2);
          const h = Math.max(2, Math.abs(y2 - y1));
          const bull = z.type === "bullish";
          const rgb = bull ? "14,162,113" : "229,72,77";
          if (!labels) {
            ctx.fillStyle = `rgba(${rgb},${z.forming ? 0.07 : 0.13})`;
            ctx.fillRect(x, top, w - x, h);
            ctx.strokeStyle = `rgba(${rgb},0.55)`;
            ctx.lineWidth = 1;
            ctx.setLineDash(z.forming ? [4, 3] : []);
            ctx.strokeRect(x + 0.5, top + 0.5, w - x - 1, h - 1);
            ctx.setLineDash([]);
          } else {
            const text = (bull ? "Bull OB" : "Bear OB") + (z.forming ? " · forming" : z.tested ? " · tested" : "");
            const ty = h < 16 ? top - 8 : top + 9; // thin zones get their label just above
            if (!claim({ x: x + 4, y: ty - 7, w: ctx.measureText(text).width + 4, h: 14 })) continue;
            ctx.fillStyle = `rgb(${rgb})`;
            ctx.textAlign = "left";
            ctx.fillText(text, x + 6, ty);
          }
        }
      }

      if (this.on.levels) {
        for (const lv of this.levels) {
          const yy = y(lv.price);
          const x = x0(lv.start);
          if (yy == null || x >= w) continue;
          if (!labels) {
            ctx.strokeStyle = C.liq;
            ctx.globalAlpha = 0.75;
            ctx.lineWidth = 1;
            ctx.setLineDash([5, 4]);
            ctx.beginPath();
            ctx.moveTo(x, Math.round(yy) + 0.5);
            ctx.lineTo(w, Math.round(yy) + 0.5);
            ctx.stroke();
            ctx.setLineDash([]);
            ctx.globalAlpha = 1;
          } else {
            const text = `${lv.kind === "high" ? "Liq ▲" : "Liq ▼"} ${fmt(lv.price, this.dp)}${lv.touches > 1 ? " ×" + lv.touches : ""}`;
            const tw = ctx.measureText(text).width + 10;
            if (!claim({ x: w - tw - 6, y: yy - 9, w: tw, h: 18 })) continue;
            ctx.fillStyle = "rgba(255,255,255,0.92)";
            ctx.strokeStyle = "rgba(194,120,3,0.35)";
            ctx.beginPath();
            ctx.roundRect(w - tw - 6, yy - 9, tw, 18, 5);
            ctx.fill();
            ctx.stroke();
            ctx.fillStyle = C.liq;
            ctx.textAlign = "left";
            ctx.fillText(text, w - tw - 1, yy + 0.5);
          }
        }
      }
    });
  }
}

export class PriceChart {
  /** @param el chart container, @param legend element that shows O/H/L/C under the cursor */
  constructor(el, legend) {
    this.legend = legend;
    this.dp = 2;
    this.hover = null;
    this.chart = LW.createChart(el, {
      autoSize: true,
      layout: { background: { type: "solid", color: "#ffffff" }, textColor: C.text, fontSize: 11, fontFamily: FONT, attributionLogo: true },
      grid: { vertLines: { color: C.grid }, horzLines: { color: C.grid } },
      rightPriceScale: { borderVisible: false, scaleMargins: { top: 0.08, bottom: 0.2 } },
      timeScale: { borderVisible: false, timeVisible: true, secondsVisible: false, rightOffset: 8, barSpacing: 9, minBarSpacing: 2 },
      crosshair: {
        mode: LW.CrosshairMode.Normal,
        vertLine: { color: C.cross, labelBackgroundColor: "#0f172a", style: LW.LineStyle.Dashed },
        horzLine: { color: C.cross, labelBackgroundColor: "#0f172a", style: LW.LineStyle.Dashed },
      },
    });
    this.candles = this.chart.addSeries(LW.CandlestickSeries, {
      upColor: C.up,
      downColor: C.dn,
      borderVisible: false,
      wickUpColor: C.up,
      wickDownColor: C.dn,
    });
    this.volume = this.chart.addSeries(LW.HistogramSeries, {
      priceFormat: { type: "volume" },
      priceScaleId: "vol",
      lastValueVisible: false,
      priceLineVisible: false,
    });
    this.chart.priceScale("vol").applyOptions({ scaleMargins: { top: 0.84, bottom: 0 } });
    this.overlay = new Overlay();
    this.candles.attachPrimitive(this.overlay);
    this.chart.subscribeCrosshairMove((p) => {
      const c = p.time ? p.seriesData.get(this.candles) : null;
      this.hover = c ? { ...c, v: p.seriesData.get(this.volume)?.value } : null;
      this._legend(this.hover || this.last);
    });
  }

  _row(f, i) {
    return { time: toLocalSeconds(f.t[i]), open: f.o[i], high: f.h[i], low: f.l[i], close: f.c[i], v: f.v[i] };
  }

  _vol(r) {
    return { time: r.time, value: r.v, color: r.close >= r.open ? "rgba(14,162,113,0.28)" : "rgba(229,72,77,0.28)" };
  }

  _legend(r) {
    if (!r || !this.legend) return;
    const up = r.close >= r.open;
    const d = (x) => fmt(x, this.dp);
    const pct = r.open ? ((r.close / r.open - 1) * 100).toFixed(2) : "0.00";
    this.legend.replaceChildren();
    const add = (k, v, cls) => {
      const s = document.createElement("span");
      s.className = "lg " + (cls || "");
      const b = document.createElement("b");
      b.textContent = k;
      s.append(b, v);
      this.legend.append(s);
    };
    add("O", d(r.open));
    add("H", d(r.high));
    add("L", d(r.low));
    add("C", d(r.close), up ? "up" : "dn");
    add("", `${up ? "+" : ""}${pct}%`, up ? "up" : "dn");
    if (r.v != null) add("Vol", fmtCompact(r.v));
  }

  /** Replace all data. `keepView` leaves the scroll/zoom alone (used when refilling after a reconnect). */
  load(f, dp, keepView = false) {
    this.dp = dp;
    const n = f.t.length;
    const rows = new Array(n);
    const vols = new Array(n);
    for (let i = 0; i < n; i++) {
      const r = this._row(f, i);
      rows[i] = r;
      vols[i] = this._vol(r);
    }
    this.candles.applyOptions({ priceFormat: { type: "price", precision: dp, minMove: 1 / 10 ** dp } });
    this.candles.setData(rows);
    this.volume.setData(vols);
    this.last = rows[n - 1];
    if (!keepView) this.chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 110), to: n + 8 });
    this._legend(this.hover || this.last);
  }

  /** Hot path: redraw only the newest candle. */
  tick(f) {
    const i = f.t.length - 1;
    const r = this._row(f, i);
    try {
      this.candles.update(r);
      this.volume.update(this._vol(r));
    } catch {
      this.load(f, this.dp, true); // series rejected the update (time went backwards): resync
      return;
    }
    this.last = r;
    if (!this.hover) this._legend(r);
  }

  setOverlay(zones, levels, dp) {
    this.overlay.set(zones, levels, dp);
  }

  showOverlay(on) {
    this.overlay.show(on);
  }

  clear() {
    this.candles.setData([]);
    this.volume.setData([]);
    this.overlay.set([], [], this.dp);
    this.last = this.hover = null;
    this.legend?.replaceChildren();
  }

  resetView(n) {
    this.chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 110), to: n + 8 });
  }
}
