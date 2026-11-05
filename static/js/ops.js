// Operations: infrastructure monitor, portfolio risk, attribution and drift, strategy versions, accounts
// orchestrator, smart orders (paper), ledger and workflows. Loaded on first visit.
import { $, el, api } from "./util.js";
import { button, field, input, select, seg, say, post, put, badge } from "./bots/ui.js";
import { rows, obj, card } from "./render.js";

const TABS = [
  { value: "monitor", label: "Monitor" },
  { value: "risk", label: "Risk" },
  { value: "attribution", label: "Attribution & drift" },
  { value: "versions", label: "Versions" },
  { value: "accounts", label: "Accounts" },
  { value: "smart", label: "Smart orders" },
  { value: "ledger", label: "Ledger" },
  { value: "workflows", label: "Workflows" },
];
let tab = "monitor";
let wired = false;

const host = () => $("#opsBody");
const fail = (e) => host().replaceChildren(el("p", "st-msg err", e.message));
const msg = () => el("p", "st-msg");

export function wire() {
  if (wired) return;
  wired = true;
  $("#opsTabs").replaceWith(Object.assign(seg(TABS, tab, (v) => ((tab = v), load()), "seg st-tabs"), { id: "opsTabs" }));
}

export async function load() {
  wire();
  host().replaceChildren(el("p", "st-about", "Loading…"));
  try {
    await PAGES[tab]();
  } catch (e) {
    fail(e);
  }
}

const PAGES = {
  async monitor() {
    const [m, n] = await Promise.all([api("/api/monitor"), api("/api/notifications")]);
    const overall = el("p", null);
    overall.append("Overall: ", badge(m.overall.toUpperCase(), m.overall === "ok" ? "ok" : m.overall === "warning" ? "warn" : "off"), ` · evaluated ${m.evaluated_utc}`);
    const read = button("Mark all read", "btn ghost sm", async () => (await post("/api/notifications/read", {}), load()));
    host().replaceChildren(
      card("Health", overall, rows(m.components, 8, ["kind", "name", "status", "detail", "last_ok_utc"])),
      card("API latency and errors", obj(m.metrics)),
      card("Notifications", read, rows(n.notifications, 8, ["ts_utc", "source", "title", "detail", "count", "read"])),
      card("Incidents", rows(m.incidents, 9, ["severity", "component", "title", "count", "first_utc", "last_utc", "status"])),
      card("Thresholds", obj(m.thresholds), el("p", "st-about", "Change thresholds with PUT /api/monitor/thresholds (Developer Hub).")),
    );
  },

  async risk() {
    const v = await api("/api/risk/portfolio");
    const cur = select([{ value: "USDT" }, { value: "INR" }], "USDT");
    const [exp, loss, dd] = [input("", { placeholder: "none" }), input("", { placeholder: "none" }), input("", { placeholder: "none" })];
    const m = msg();
    const save = button("Save portfolio limits", "btn", async () => {
      try {
        await put("/api/risk/portfolio", { [cur.value]: { max_exposure: exp.value || null, max_daily_loss: loss.value || null, max_drawdown_pct: dd.value || null } });
        say(m, "Saved. Every new order on every path is checked against these.", "ok");
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const form = el("div", "st-form");
    form.append(field("Currency", cur), field("Max exposure (all accounts)", exp), field("Max daily loss (all accounts)", loss), field("Max drawdown % (demo)", dd), save, m);
    const bl = await api("/api/econ/events");
    const on = el("input");
    on.type = "checkbox";
    on.checked = !!bl.blackout.enabled;
    const [before, after] = [input(bl.blackout.minutes_before, { type: "number", min: 0, max: 720 }), input(bl.blackout.minutes_after, { type: "number", min: 0, max: 720 })];
    const m2 = msg();
    const bform = el("div", "st-form");
    bform.append(
      field("Pause new entries around high-importance events", on),
      field("Minutes before", before),
      field("Minutes after", after),
      button("Save blackout", "btn", async () => {
        try {
          await put("/api/econ/blackout", { enabled: on.checked, minutes_before: Number(before.value), minutes_after: Number(after.value) });
          say(m2, "Saved.", "ok");
        } catch (e) {
          say(m2, e.message, "err");
        }
      }),
      m2,
    );
    host().replaceChildren(card("Portfolio risk", obj(v)), card("Portfolio limits", form), card("Economic-event blackout", bform, el("p", "st-about", "Events come from your imports in Research Lab > News & calendar.")));
  },

  async attribution() {
    const dim = select(["bot", "strategy_version", "symbol", "account", "exit_reason", "side", "month"].map((v) => ({ value: v })), "bot");
    const out = el("div");
    const paint = async () => {
      try {
        out.replaceChildren(obj(await api(`/api/attribution?dimension=${dim.value}`)));
      } catch (e) {
        out.replaceChildren(el("p", "st-msg err", e.message));
      }
    };
    dim.addEventListener("change", paint);
    const bots = (await api("/api/bots")).bots;
    const drift = el("div");
    for (const b of bots.slice(0, 20)) {
      try {
        const d = await api(`/api/drift/${b.id}`);
        drift.append(el("h5", null, `${b.name} · ${d.status}`), obj(d));
      } catch (e) {
        drift.append(el("p", "st-msg err", `${b.name}: ${e.message}`));
      }
    }
    if (!bots.length) drift.append(el("p", "st-about", "No bots yet."));
    host().replaceChildren(card("Performance attribution", field("Group by", dim), out), card("Strategy drift (live/paper vs backtest)", drift));
    paint();
  },

  async versions() {
    const list = (await api("/api/strategies")).strategies || [];
    const out = el("div");
    const vid = input("", { type: "number", placeholder: "version id" });
    const other = input("", { type: "number", placeholder: "compare with id" });
    const m = msg();
    const show = async () => {
      try {
        const d = await api(`/api/strategy-versions/${vid.value}`);
        const kids = [obj(d)];
        if (other.value) kids.push(el("h5", null, "Diff"), obj(await api(`/api/strategy-versions/${vid.value}/diff/${other.value}`)));
        out.replaceChildren(...kids);
        say(m, "");
      } catch (e) {
        say(m, e.message, "err");
      }
    };
    const act = (path, body, ok) =>
      button(ok, "btn ghost sm", async () => {
        try {
          await post(`/api/strategy-versions/${vid.value}/${path}`, body());
          say(m, `${ok}: done`, "ok");
          show();
        } catch (e) {
          say(m, e.message, "err");
        }
      });
    const tag = select(["experimental", "validated", "paper-approved", "production-approved"].map((v) => ({ value: v })), "experimental");
    const f = el("div", "st-form");
    f.append(field("Version", vid), field("Compare", other), button("Show", "btn", show), field("Tag", tag), act("tags", () => ({ tag: tag.value }), "Add tag"), act("rollback", () => ({}), "Roll back to this version"), m);
    host().replaceChildren(card("Strategies", rows(list, 8)), card("Version detail, diff, tags and rollback", f, out));
  },

  async accounts() {
    const o = await api("/api/orchestrator");
    const list = el("div");
    for (const a of o.accounts) {
      const acc = a.account;
      const c = el("div", "card st-card");
      const name = acc.kind === "demo" ? `${acc.label} (VIRTUAL)` : `${acc.label} (${acc.environment === "live" ? "LIVE, real" : "TESTNET"})`;
      const m = msg();
      const toggle = button(acc.status === "disabled" ? "Enable" : "Disable", "btn ghost sm", async () => {
        try {
          await post(`/api/accounts/${encodeURIComponent(acc.id)}/status`, { status: acc.status === "disabled" ? "active" : "disabled" });
          load();
        } catch (e) {
          say(m, e.message, "err");
        }
      });
      const pause = button("Pause its bots", "btn ghost sm", async () => {
        try {
          const r = await post(`/api/orchestrator/${encodeURIComponent(acc.id)}/pause-bots`, {});
          say(m, `Paused ${r.paused.length} bot(s).`, "ok");
        } catch (e) {
          say(m, e.message, "err");
        }
      });
      c.append(el("h4", null, name), el("p", null, `Status: ${acc.status} · open positions ${a.open_positions} · working smart orders ${a.working_smart_orders} · bots ${JSON.stringify(a.bots)}`), rows(a.limits, 6), toggle, pause, m);
      list.append(c);
    }
    host().replaceChildren(el("p", "no-money", o.notice || "This app holds no money."), el("p", "st-about", "A disabled account takes no new orders on any path (manual, bots, smart orders, workflows); open positions can still be closed."), list);
  },

  async smart() {
    const [sym, side, qty, kids, every, price, limit] = [input("BTCUSDT"), select([{ value: "long" }, { value: "short" }], "long"), input("0.01", { type: "number", step: "any" }), input("5", { type: "number", min: 2, max: 50 }), input("60", { type: "number", min: 5 }), input("", { type: "number", step: "any", placeholder: "price on screen" }), input("", { type: "number", step: "any", placeholder: "optional" })];
    const algo = select([{ value: "twap", label: "TWAP (spread over time)" }, { value: "slice", label: "Slices (one per tick)" }], "twap");
    const m = msg();
    const f = el("div", "st-form");
    f.append(
      field("Symbol", sym),
      field("Side", side),
      field("Total quantity", qty),
      field("Algorithm", algo),
      field("Children", kids),
      field("Seconds between children", every),
      field("Arrival price", price),
      field("Limit price", limit, "children are skipped when the price is worse"),
      button("Start on the demo account", "btn", async () => {
        try {
          await post("/api/smart-orders", { symbol: sym.value, side: side.value, total_qty: Number(qty.value), algo: algo.value, children: Number(kids.value), interval_s: Number(every.value), price: Number(price.value), limit_price: limit.value ? Number(limit.value) : null });
          load();
        } catch (e) {
          say(m, e.message, "err");
        }
      }),
      m,
    );
    const l = await api("/api/smart-orders");
    const list = el("div");
    for (const p of l.orders) {
      const c = el("div", "card st-card");
      const acts = [];
      if (["working", "paused"].includes(p.status)) acts.push(button("Cancel", "btn ghost sm", async () => (await post(`/api/smart-orders/${p.id}/cancel`, {}), load())));
      if (p.status === "paused") acts.push(button("Resume", "btn ghost sm", async () => (await post(`/api/smart-orders/${p.id}/resume`, {}), load())));
      c.append(el("h4", null, `#${p.id} ${p.algo.toUpperCase()} ${p.side} ${p.total_qty} ${p.symbol} · ${p.status} · ${p.label}`), obj(p.metrics), rows(p.children, 8, ["seq", "due_utc", "qty", "status", "price", "reason"]), ...acts);
      list.append(c);
    }
    host().replaceChildren(card("New smart order (paper)", el("p", "st-about", "Each child is a paper fill at the latest price and passes the same risk checks as a manual order. A refused child stops the order."), f), list);
  },

  async ledger() {
    const basis = select([{ value: "calendar", label: "Calendar year" }, { value: "india_fy", label: "Indian FY (Apr–Mar)" }], "calendar");
    const rep = el("div");
    const paint = async () => rep.replaceChildren(obj(await api(`/api/ledger/report?basis=${basis.value}`)));
    basis.addEventListener("change", paint);
    const l = await api("/api/ledger");
    const m = msg();
    const [acct, cur, amt, memo] = [input("demo-1"), select([{ value: "USDT" }, { value: "INR" }], "USDT"), input("", { placeholder: "e.g. -1.25" }), input("", { placeholder: "why (required)" })];
    const adj = el("div", "st-form");
    adj.append(
      field("Account", acct),
      field("Currency", cur),
      field("Amount", amt),
      field("Memo", memo),
      button("Add adjustment", "btn", async () => {
        try {
          await post("/api/ledger/adjustments", { account_id: acct.value, currency: cur.value, amount: amt.value, memo: memo.value });
          load();
        } catch (e) {
          say(m, e.message, "err");
        }
      }),
      m,
    );
    const rid = input("", { type: "number", placeholder: "entry id" });
    const rmemo = input("", { placeholder: "why" });
    const rev = el("div", "st-form");
    rev.append(
      field("Reverse entry", rid),
      field("Memo", rmemo),
      button("Reverse", "btn ghost", async () => {
        try {
          await post(`/api/ledger/${rid.value}/reverse`, { memo: rmemo.value });
          load();
        } catch (e) {
          say(m, e.message, "err");
        }
      }),
    );
    const csv = button("Download ledger (CSV)", "btn ghost sm", async () => {
      const r = await fetch("/api/ledger/export", { headers: { "X-ZB-Token": document.querySelector('meta[name="zb-token"]').content } });
      const url = URL.createObjectURL(await r.blob());
      const a = el("a");
      a.href = url;
      a.download = "zoneboard-ledger.csv";
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
    const st = el("textarea");
    st.rows = 4;
    st.placeholder = "date,symbol,side,qty,price,fee,ref";
    const recOut = el("div");
    const rec = button("Reconcile statement", "btn ghost", async () => {
      try {
        recOut.replaceChildren(obj(await post("/api/ledger/reconcile", { account_id: acct.value, csv: st.value })));
      } catch (e) {
        recOut.replaceChildren(el("p", "st-msg err", e.message));
      }
    });
    host().replaceChildren(
      card("Yearly totals", field("Basis", basis), rep, el("p", "st-about", "Record keeping for you and your accountant. Not tax advice.")),
      card("Entries (append-only; corrections are reversals)", csv, rows(l.entries.slice().reverse(), 10, ["id", "ts_utc", "account_id", "kind", "currency", "amount", "ref_type", "ref_id", "reverses_id", "memo"])),
      card("Adjust or reverse", adj, rev),
      card("Broker statement reconciliation", st, rec, recOut),
    );
    paint();
  },

  async workflows() {
    const l = await api("/api/workflows");
    const name = input("Drift alert");
    const graph = el("textarea");
    graph.rows = 10;
    graph.value = JSON.stringify(
      {
        trigger: { event: "StrategyDriftDetected" },
        nodes: [
          { id: "c", type: "condition", params: { field: "payload.status", op: "eq", value: "degraded" } },
          { id: "n", type: "notify", params: { title: "A strategy drifted" } },
          { id: "p", type: "pause_bot", params: { bot_id: "from_event" } },
        ],
        edges: [["trigger", "c"], ["c", "n"], ["c", "p"]],
      },
      null,
      2,
    );
    const m = msg();
    const f = el("div", "st-form wide");
    f.append(
      field("Name", name),
      field("Graph (JSON)", graph, `Nodes: ${l.node_types.join(", ")}. Orders, limits, live arming and credentials are never available here.`),
      button("Create (validated, not enabled)", "btn", async () => {
        try {
          const w = await post("/api/workflows", { name: name.value, graph: JSON.parse(graph.value) });
          say(m, w.problems.length ? `Saved as draft: ${w.problems.join("; ")}` : "Saved and validated. Dry-run it, then enable it.", w.problems.length ? "err" : "ok");
          load();
        } catch (e) {
          say(m, e.message, "err");
        }
      }),
      m,
    );
    const list = el("div");
    for (const w of l.workflows) {
      const c = el("div", "card st-card");
      const out = el("div");
      const evType = w.graph?.trigger?.event || "";
      const dry = button("Dry run", "btn ghost sm", async () => {
        try {
          const payload = prompt("Event payload (JSON) for the dry run", '{"status":"degraded","bot_id":1}');
          if (payload == null) return;
          out.replaceChildren(obj(await post(`/api/workflows/${w.id}/dryrun`, { event: { type: evType, payload: JSON.parse(payload) } })));
        } catch (e) {
          out.replaceChildren(el("p", "st-msg err", e.message));
        }
      });
      const st = (s) => button({ enabled: "Enable", disabled: "Disable", archived: "Archive" }[s], "btn ghost sm", async () => {
        try {
          await post(`/api/workflows/${w.id}/status`, { status: s });
          load();
        } catch (e) {
          out.replaceChildren(el("p", "st-msg err", e.message));
        }
      });
      c.append(el("h4", null, `${w.name} · v${w.version} · ${w.status}`), el("p", "st-about", `On ${evType}${w.problems.length ? " · problems: " + w.problems.join("; ") : ""}`), dry, w.status === "enabled" ? st("disabled") : st("enabled"), st("archived"), out);
      list.append(c);
    }
    host().replaceChildren(card("New workflow", f), list);
  },
};
