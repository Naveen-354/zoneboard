// Settings > Developer: scoped API tokens. A new token is shown once; the server keeps only its hash.
import { $, el, api } from "./util.js";

const JSON_HEADERS = { "Content-Type": "application/json" };
let wired = false;

function say(text, kind) {
  const m = $("#devMsg");
  m.textContent = text;
  m.className = "set-msg" + (kind ? " " + kind : "");
}

export async function load() {
  if (!wired) {
    wired = true;
    $("#devNew").addEventListener("submit", async (e) => {
      e.preventDefault();
      const f = e.target.elements;
      const scopes = ["read", "research", "trade"].filter((s) => f[s].checked);
      try {
        const t = await api("/api/dev/tokens", { method: "POST", headers: JSON_HEADERS, body: JSON.stringify({ name: f.name.value.trim(), scopes }) });
        say(`Token (copy it now, it is not shown again): ${t.token}`, "ok");
        e.target.reset();
        load();
      } catch (err) {
        say(err.message, "err");
      }
    });
  }
  let d;
  try {
    d = await api("/api/dev/tokens");
  } catch (err) {
    say(err.message, "err");
    return;
  }
  $("#devBody").replaceChildren(
    ...d.tokens.map((t) => {
      const tr = el("tr");
      const td = el("td");
      if (!t.revoked) {
        const b = el("button", null, "Revoke");
        b.type = "button";
        b.addEventListener("click", async () => {
          await api(`/api/dev/tokens/${t.id}`, { method: "DELETE" });
          load();
        });
        td.append(b);
      } else td.textContent = "revoked";
      tr.append(el("td", null, t.name), el("td", null, t.scopes.join(", ")), el("td", null, t.hint), el("td", null, new Date(t.created_utc).toLocaleString()), el("td", null, t.last_used_utc ? new Date(t.last_used_utc).toLocaleString() : "never"), td);
      return tr;
    }),
  );
}
