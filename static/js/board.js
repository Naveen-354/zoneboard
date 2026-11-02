// The left-hand board: ranked setups (or every symbol) with live prices.
// Rows are created once and updated in place, so a price tick costs two text writes and no layout work.
import { $, $$, el, fmt, signed, flash, frame, throttle } from "./util.js";

const TOP = 15;

export class Board {
  constructor({ onSelect, onSearchEnter }) {
    this.items = new Map();
    this.mode = "setups";
    this.filter = "";
    this.sel = null;
    this.dirty = new Set();
    this.root = $("#board");
    this.lists = { bull: $("#bullList"), bear: $("#bearList"), all: $("#allList"), setups: $("#setups") };
    this.paint = frame(() => this._paint());
    this.rank = throttle(() => this._layout(), 1200); // re-sorting every tick would make rows jump around

    $("#boardBody").addEventListener("click", (e) => {
      const row = e.target.closest(".row");
      if (row) onSelect(row.dataset.sym);
    });
    $("#boardMode").addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (b) this.setMode(b.dataset.b);
    });
    const search = $("#search");
    search.addEventListener("input", () => {
      this.filter = search.value.trim().toUpperCase();
      this._layout();
    });
    search.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && this.filter) onSearchEnter(this.filter, this._visible()[0]?.sym);
      if (e.key === "Escape") {
        search.value = "";
        this.filter = "";
        search.blur();
        this._layout();
      }
    });
  }

  /** Replace the symbol list. `list` is [{sym, base, quote}]. */
  setItems(list) {
    this.items.clear();
    for (const s of list) {
      const item = { sym: s.sym, base: s.base, quote: s.quote || "", price: null, dp: 2, chg24: null, score: null, label: null };
      item.row = this._makeRow(item);
      this.items.set(s.sym, item);
    }
    this.sel = null;
    this._layout();
  }

  item(sym) {
    return this.items.get(sym);
  }

  /** Mark an item as changed after mutating its fields. Cheap to call on every tick. `rerank` if score/label changed. */
  touch(item, rerank = false) {
    this.dirty.add(item);
    this.paint();
    if (rerank) this.rank();
  }

  setMode(mode) {
    this.mode = mode;
    for (const b of $$("#boardMode button")) b.classList.toggle("on", b.dataset.b === mode);
    this._layout();
  }

  setSelected(sym) {
    this.items.get(this.sel)?.row.el.classList.remove("sel");
    this.sel = sym;
    const it = this.items.get(sym);
    if (it) {
      it.row.el.classList.add("sel");
      if (it.row.el.isConnected) it.row.el.scrollIntoView({ block: "nearest" });
    }
  }

  /** Move the selection up/down through the visible rows (keyboard j/k). */
  step(delta) {
    // only rows in the list that is showing: hidden lists keep their detached-from-view rows attached
    const rows = $$(".row", this.lists.all.hidden ? this.lists.setups : this.lists.all);
    if (!rows.length) return null;
    const i = rows.findIndex((r) => r.dataset.sym === this.sel);
    const next = rows[Math.min(rows.length - 1, Math.max(0, i + delta))];
    return next?.dataset.sym ?? null;
  }

  setCounts(c) {
    $("#cBull").textContent = c ? c.bull : "–";
    $("#cBear").textContent = c ? c.bear : "–";
    $("#cChop").textContent = c ? c.chop : "–";
  }

  /** Force a re-rank now (after a batch of score updates). */
  relayout() {
    this._layout();
  }

  _makeRow(item) {
    const b = el("button", "row");
    b.type = "button";
    b.dataset.sym = item.sym;
    const rk = el("span", "rk");
    const nm = el("span", "nm");
    nm.append(el("b", null, item.base), el("small", null, item.quote));
    const px = el("span", "px", "–");
    const chg = el("span", "chg", "");
    const sc = el("span", "sc");
    const bar = el("i");
    sc.append(bar);
    b.append(rk, nm, px, chg, sc);
    return { el: b, rk, px, chg, bar, shown: {} };
  }

  _paint() {
    for (const it of this.dirty) {
      const r = it.row;
      const s = r.shown;
      if (it.price != null) {
        const t = fmt(it.price, it.dp);
        if (t !== s.px) {
          if (s.price != null && it.price !== s.price) flash(r.px, it.price > s.price);
          r.px.textContent = t;
          s.px = t;
          s.price = it.price;
        }
      }
      if (it.chg24 != null) {
        const t = signed(it.chg24) + "%";
        if (t !== s.chg) {
          r.chg.textContent = t;
          s.chg = t;
          r.chg.className = "chg " + (it.chg24 >= 0 ? "up" : "dn");
        }
      }
      if (it.score != null) {
        const w = Math.min(100, Math.abs(it.score)).toFixed(0) + "%";
        if (w !== s.w || (it.score >= 0) !== s.pos) {
          r.bar.style.width = w;
          r.bar.className = it.score >= 0 ? "up" : "dn";
          s.w = w;
          s.pos = it.score >= 0;
        }
      }
    }
    this.dirty.clear();
  }

  _visible() {
    const all = [...this.items.values()];
    const f = this.filter;
    if (!f && this.mode === "setups") return all;
    const list = f ? all.filter((i) => i.sym.includes(f)) : all;
    return list.sort((a, b) => (b.score ?? -Infinity) - (a.score ?? -Infinity) || (a.sym < b.sym ? -1 : 1));
  }

  _layout() {
    const all = [...this.items.values()];
    const scored = all.filter((i) => i.label);
    this.setCounts(scored.length ? { bull: scored.filter((i) => i.label === "bullish").length, bear: scored.filter((i) => i.label === "bearish").length, chop: scored.filter((i) => i.label === "choppy").length } : null);

    const flat = !!this.filter || this.mode === "all";
    this.lists.setups.hidden = flat;
    this.lists.all.hidden = !flat;
    this.root.classList.toggle("no-rank", flat);

    if (flat) {
      this._place(this.lists.all, this._visible(), false, "Nothing matches. Press Enter to try this symbol.");
      return;
    }
    const bull = all.filter((i) => i.label === "bullish").sort((a, b) => b.score - a.score).slice(0, TOP);
    const bear = all.filter((i) => i.label === "bearish").sort((a, b) => a.score - b.score).slice(0, TOP);
    const wait = scored.length ? "No strong setups right now" : "Scanning…";
    this._place(this.lists.bull, bull, true, wait);
    this._place(this.lists.bear, bear, true, wait);
  }

  _place(container, items, ranked, emptyText) {
    if (!items.length) {
      container.replaceChildren(el("div", "empty", emptyText));
      return;
    }
    const want = items.map((i) => i.row.el);
    const same = container.children.length === want.length && want.every((e, k) => container.children[k] === e);
    if (!same) container.replaceChildren(...want);
    if (ranked) items.forEach((it, k) => (it.row.rk.textContent = String(k + 1).padStart(2, "0")));
  }
}
