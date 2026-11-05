// AI Research: drafts, reviews and explanations from your own Anthropic key. The assistant gets read-only
// context and has no tools: it cannot approve, arm, change limits, read credentials or trade. Drafts are
// validated by the same rules as every strategy and become a bot only if you save them in Create Bot.
import { el, api, post, put, del, button, say, input, field, select, badge, whenIso } from "./ui.js";

let ctx = null;

export function init(c) {
  ctx = c;
}

export async function render(host) {
  host.replaceChildren(el("p", "st-about", "Loading AI status…"));
  let st;
  try {
    st = await api("/api/ai/status");
  } catch (e) {
    host.replaceChildren(el("p", "st-msg err", e.message));
    return;
  }
  host.replaceChildren(setup(st, host), draftCard(st), explainCard(st), history());
}

function setup(st, host) {
  const c = el("section", "card st-card");
  const hd = el("div", "st-card-h");
  hd.append(el("h3", null, "AI Research"), badge(st.configured ? (st.enabled ? "ON" : "OFF") : "NOT CONFIGURED", st.configured && st.enabled ? "ok" : "off"));
  const msg = el("p", "st-msg");
  const key = input("", { type: "password", placeholder: st.configured ? `key ${st.key_hint} stored in the OS vault` : "Anthropic API key", "aria-label": "Anthropic API key" });
  const model = input(st.model, { "aria-label": "model id" });
  const g = el("div", "st-grid");
  g.append(field("API key (stored in Windows Credential Manager, never shown again)", key), field("Model", model));
  const bar = el("div", "st-actions");
  bar.append(
    button("Save key", "btn sm", async () => {
      try {
        await put("/api/ai/key", { api_key: key.value.trim() });
        key.value = "";
        render(host);
      } catch (e) {
        say(msg, e.message, "err");
      }
    }),
    button(st.enabled ? "Switch AI off" : "Switch AI on", "btn ghost sm", async () => {
      await post("/api/ai/settings", { enabled: !st.enabled, model: model.value.trim() });
      render(host);
    }),
    button("Save model", "btn ghost sm", async () => {
      try {
        await post("/api/ai/settings", { model: model.value.trim() });
        say(msg, "Saved.", "ok");
      } catch (e) {
        say(msg, e.message, "err");
      }
    }),
  );
  if (st.configured) bar.append(button("Remove key", "btn ghost sm danger-t", async () => (await del("/api/ai/key"), render(host))));
  c.append(
    hd,
    el("p", "st-about", `Uses your own Anthropic account (calls are billed to it). ${st.calls_today} of ${st.daily_budget} calls used today. Context it may see: ${st.context_sources.join("; ")}. It cannot: ${st.cannot.join(", ")}.`),
    g,
    bar,
    msg,
  );
  return c;
}

function draftCard(st) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "Draft a strategy from a description"));
  const ta = el("textarea");
  ta.rows = 3;
  ta.maxLength = 2000;
  ta.placeholder = "For example: buy BTC when RSI recovers from oversold in an uptrend, with a 3% stop and exit on overbought.";
  const msg = el("p", "st-msg");
  const out = el("div");
  const go = button("Draft", "btn sm", async () => {
    say(msg, "Asking the model…");
    go.disabled = true;
    try {
      const r = await post("/api/ai/draft", { objective: ta.value });
      out.replaceChildren(proposalView({ kind: "strategy_draft", valid: r.valid, errors: r.errors, spec: r.spec, model: r.meta.model, output: "" }));
      say(msg, r.valid ? "Valid draft. Backtest it before you trust it." : "The draft did not pass validation (see why below).", r.valid ? "ok" : "err");
    } catch (e) {
      say(msg, e.message, "err");
    } finally {
      go.disabled = !st.configured || !st.enabled;
    }
  });
  go.disabled = !st.configured || !st.enabled;
  const reviewBtn = button("Risk-review the strategy in Create Bot", "btn ghost sm", async () => {
    const d = ctx.currentDraft();
    if (!d) return say(msg, "Open Create Bot first.", "err");
    say(msg, "Reviewing…");
    try {
      const r = await post("/api/ai/review", { spec: d.spec });
      out.replaceChildren(el("pre", "ai-out", r.review));
      say(msg, "Review done.", "ok");
    } catch (e) {
      say(msg, e.message, "err");
    }
  });
  reviewBtn.disabled = go.disabled;
  const bar = el("div", "st-actions");
  bar.append(go, reviewBtn, msg);
  c.append(ta, bar, out);
  return c;
}

function explainCard(st) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "Explain a saved backtest (from its stored numbers only)"));
  const msg = el("p", "st-msg");
  const out = el("div");
  const pick = select([{ value: "", label: "Loading saved backtests…" }], "");
  Promise.all(ctx.bots.map((b) => api(`/api/bots/${b.id}/backtests`).then((d) => d.backtests.map((x) => ({ value: String(x.id), label: `#${x.id} · ${b.name} · ${whenIso(x.created_utc)} · ${x.metrics.trades} trades` })))))
    .then((lists) => {
      const opts = lists.flat();
      pick.replaceChildren();
      for (const o of opts.length ? opts : [{ value: "", label: "No saved backtests yet (save one in Backtesting)" }]) {
        const op = el("option", null, o.label);
        op.value = o.value;
        pick.append(op);
      }
    })
    .catch(() => {});
  const go = button("Explain", "btn sm", async () => {
    if (!pick.value) return;
    say(msg, "Asking the model…");
    try {
      const r = await post(`/api/ai/explain/${pick.value}`);
      out.replaceChildren(
        r.numbers_verified ? badge("EVERY NUMBER MATCHES THE STORED RESULTS", "ok") : badge(`UNVERIFIED NUMBERS: ${r.unverified_numbers.join(", ")}`, "warn"),
        el("pre", "ai-out", r.explanation),
      );
      say(msg, "");
    } catch (e) {
      say(msg, e.message, "err");
    }
  });
  go.disabled = !st.configured || !st.enabled;
  const bar = el("div", "st-actions");
  bar.append(pick, go, msg);
  c.append(bar, out);
  return c;
}

function proposalView(p) {
  const d = el("div", "ai-prop");
  d.append(badge(p.valid ? "VALID" : "INVALID", p.valid ? "ok" : "warn"), el("small", "st-about", ` ${p.kind} · ${p.model || ""}`));
  if (p.errors) d.append(el("p", "st-msg err", p.errors));
  if (p.spec) {
    d.append(el("pre", "ai-out", JSON.stringify(p.spec, null, 2)));
    if (p.valid)
      d.append(
        button("Open in Create Bot", "btn ghost sm", () => {
          ctx.applyToBuilder({ ...ctx.blankDraft(), name: "AI draft", description: "Drafted by the AI assistant; reviewed and saved by me.", spec: p.spec });
        }),
      );
  } else if (p.output) d.append(el("pre", "ai-out", p.output));
  return d;
}

function history() {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "History (every AI call is stored and audited)"));
  const list = el("div", "ai-hist");
  api("/api/ai/proposals")
    .then((d) => {
      if (!d.proposals.length) list.append(el("p", "st-about", "Nothing yet."));
      for (const p of d.proposals.slice(0, 20)) {
        const det = el("details");
        det.append(el("summary", null, `#${p.id} · ${p.kind} · ${whenIso(p.created_utc)} · ${p.valid ? "valid" : "flagged"} · ${p.model} (${p.prompt_version})`), proposalView(p));
        list.append(det);
      }
    })
    .catch((e) => list.append(el("p", "st-msg err", e.message)));
  c.append(list);
  return c;
}
