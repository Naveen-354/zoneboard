// Browser smoke test: boots the real server on sample data and drives the real UI in headless Edge/Chrome.
//   node tools/ui_smoke.mjs
// Needs: Node 22+ (global fetch and WebSocket, no npm packages), a built server
//   (cd src-tauri && cargo build --no-default-features --bin zoneboard-server) and Edge/Chrome/Chromium.
// No internet and no real data: SAMPLE_DATA=1, a throwaway journal and a throwaway data folder, all removed at the end.
// Set CHROME=/path/to/browser to pick a browser; ZB_SERVER=/path/to/zoneboard-server to pick a server binary.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const win = process.platform === "win32";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- locating things
function findServer() {
  const exe = win ? "zoneboard-server.exe" : "zoneboard-server";
  const cands = [process.env.ZB_SERVER, join(root, "src-tauri", "target", "debug", exe), join(root, "src-tauri", "target", "release", exe)];
  return cands.find((p) => p && existsSync(p));
}

function findBrowser() {
  const cands = [process.env.CHROME];
  if (win) {
    for (const base of [process.env["ProgramFiles(x86)"], process.env.ProgramFiles, process.env.LOCALAPPDATA]) {
      if (!base) continue;
      cands.push(join(base, "Microsoft", "Edge", "Application", "msedge.exe"), join(base, "Google", "Chrome", "Application", "chrome.exe"));
    }
  } else {
    cands.push("/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/microsoft-edge", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    try {
      for (const d of readdirSync("/opt/pw-browsers")) if (/^chromium-\d+$/.test(d)) cands.push(join("/opt/pw-browsers", d, "chrome-linux", "chrome"));
    } catch {}
    cands.push("/opt/pw-browsers/chromium");
  }
  return cands.find((p) => p && existsSync(p));
}

const freePort = () =>
  new Promise((res, rej) => {
    const s = createServer();
    s.once("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => res(port));
    });
  });

// ---------------------------------------------------------------- DevTools protocol (minimal)
class Page {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.errors = []; // console.error calls, uncaught exceptions, failed loads: boot must have none
    ws.addEventListener("message", (m) => this.#onMessage(JSON.parse(m.data)));
    ws.addEventListener("close", () => {
      for (const p of this.pending.values()) p.rej(new Error("browser connection closed"));
    });
  }
  #onMessage(m) {
    if (m.id) {
      const p = this.pending.get(m.id);
      this.pending.delete(m.id);
      if (p) m.error ? p.rej(new Error(m.error.message)) : p.res(m.result);
    } else if (m.method === "Runtime.exceptionThrown") {
      const d = m.params.exceptionDetails;
      this.errors.push(`exception: ${d.exception?.description || d.text}`);
    } else if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") {
      this.errors.push("console.error: " + m.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
    } else if (m.method === "Log.entryAdded" && m.params.entry.level === "error") {
      this.errors.push(`log: ${m.params.entry.text} ${m.params.entry.url || ""}`);
    }
  }
  send(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((res, rej) => this.pending.set(id, { res, rej }));
  }
  /** Evaluate an expression in the page (awaits promises) and return its JSON value. */
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  }
  /** Poll until the expression is truthy; throw with `what` if it never is. */
  async waitFor(expr, what, ms = 15000) {
    const end = Date.now() + ms;
    let last;
    while (Date.now() < end) {
      try {
        last = await this.eval(expr);
        if (last) return last;
      } catch (e) {
        last = e.message;
      }
      await sleep(100);
    }
    throw new Error(`timed out waiting for ${what}` + (last ? ` (last: ${JSON.stringify(last)})` : ""));
  }
  async load(url) {
    this.errors.length = 0;
    await this.send("Page.navigate", { url });
    await this.waitFor("document.readyState === 'complete'", "page load");
  }
  async viewport(width, height, mobile = false) {
    await this.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile });
  }
}

// ---------------------------------------------------------------- scenarios: add new ones to the list
const assert = (cond, msg) => {
  if (!cond) throw new Error(msg);
};
const SEL = (s) => JSON.stringify(s);

/** Wait for the board to be live: rows with prices, a chart canvas, the sample pill. */
async function waitBooted(p) {
  await p.waitFor(`document.querySelectorAll('#boardBody .row').length > 0`, "board rows");
  await p.waitFor(`[...document.querySelectorAll('#boardBody .row .px')].some(e => /\\d/.test(e.textContent))`, "prices on the board");
  await p.waitFor(`document.querySelector('#chart canvas') !== null`, "chart canvas");
}

const scenarios = [
  ["boot: no errors, board populated, chart drawn, sample pill", async (p, url) => {
    await p.load(url);
    await waitBooted(p);
    await p.waitFor(`/^Sample data/.test(document.querySelector('#connText').textContent)`, "pill to read 'Sample data'");
    const rows = await p.eval(`document.querySelectorAll('#boardBody .row').length`);
    assert(rows >= 5, `expected at least 5 board rows, saw ${rows}`);
    const size = await p.eval(`(() => { const c = document.querySelector('#chart canvas'); return [c.width, c.height]; })()`);
    assert(size[0] > 100 && size[1] > 100, `chart canvas is ${size.join("x")}`);
    // Give late errors (failed fetches, a rejected promise) a moment to land before judging.
    await sleep(800);
    assert(p.errors.length === 0, `console errors:\n    ${p.errors.join("\n    ")}`);
  }],

  ["account toggle: instant, ticket disabled with the right message", async (p, url) => {
    await p.load(url);
    await waitBooted(p);
    // The ticket says "Pick a symbol" until one is chosen, which would hide the message under test.
    await p.eval(`document.querySelector('#boardBody .row').click()`);
    await p.waitFor(`document.querySelector('#tkSym').textContent !== '–'`, "a symbol on the ticket");
    const state = () => p.eval(`({
      live: document.querySelector('#mode [data-mode=live]').classList.contains('on'),
      demo: document.querySelector('#mode [data-mode=demo]').classList.contains('on'),
      go: document.querySelector('#tkGo').disabled,
      goText: document.querySelector('#tkGo').textContent,
      msg: document.querySelector('#tkMsg').textContent,
      qty: document.querySelector('#tkQty').disabled,
    })`);
    // Sample data switches ordering off in both accounts, with the sample message.
    let s = await state();
    assert(s.demo && !s.live, "Demo should be selected on first load");
    assert(s.go && s.qty, "ticket should be disabled on sample data (Demo)");
    assert(/Sample data/.test(s.msg), `Demo ticket message should mention sample data, got: ${s.msg}`);

    // The switch must be synchronous: read the DOM in the same task as the click.
    const sync = await p.eval(`(() => { document.querySelector('#mode [data-mode=live]').click();
      return { live: document.querySelector('#mode [data-mode=live]').classList.contains('on'), demo: document.querySelector('#mode [data-mode=demo]').classList.contains('on') }; })()`);
    assert(sync.live && !sync.demo, "toggle to Live did not apply instantly");
    await p.waitFor(`document.querySelector('#tkGo').disabled`, "ticket disabled in Live");
    s = await state();
    assert(s.go && s.qty, "ticket must be disabled in Live");
    // Sample data outranks the account in the ticket's gate, so Live shows the sample message here. The
    // Live-only wording ("isn't connected") needs real prices, which a no-network test does not have.
    assert(/Sample data/.test(s.msg), `Live ticket message unexpected: ${s.msg}`);
    assert(s.goText === "Ordering is off", `Live button text unexpected: ${s.goText}`);

    await p.eval(`document.querySelector('#mode [data-mode=demo]').click()`);
    s = await state();
    assert(s.demo && !s.live, "toggle back to Demo did not apply");
    assert(s.go, "ticket must stay disabled on sample data in Demo");
  }],

  ["navigation: Board <-> Journal", async (p, url) => {
    await p.load(url);
    await waitBooted(p);
    const shown = (id) => p.eval(`!document.querySelector(${SEL(id)}).hidden`);
    assert((await shown("#boardView")) && !(await shown("#journalView")), "Board should show first");
    await p.eval(`document.querySelector('#tabs [data-v=journal]').click()`);
    await p.waitFor(`!document.querySelector('#journalView').hidden && document.querySelector('#boardView').hidden`, "Journal view");
    assert(await p.eval(`document.querySelector('#tabs [data-v=journal]').classList.contains('on')`), "Journal tab not highlighted");
    await p.waitFor(`document.querySelector('#jstats').children.length > 0 || !document.querySelector('#jempty').hidden`, "journal content (stats or empty note)");
    await p.eval(`document.querySelector('#tabs [data-v=board]').click()`);
    await p.waitFor(`!document.querySelector('#boardView').hidden && document.querySelector('#journalView').hidden`, "Board view again");
    assert(p.errors.length === 0, `console errors:\n    ${p.errors.join("\n    ")}`);
  }],

  ["pages: Orders, Accounts and Settings render, labelled and without errors", async (p, url) => {
    await p.load(url);
    await waitBooted(p);
    const go = async (v) => {
      await p.eval(`document.querySelector('#tabs [data-v=${v}]').click()`);
      await p.waitFor(`!document.querySelector('#${v}View').hidden && document.querySelectorAll('main:not([hidden])').length === 1`, `${v} page alone on screen`);
      assert(await p.eval(`document.querySelector('#tabs [data-v=${v}]').getAttribute('aria-current') === 'page'`), `${v} tab not marked current`);
    };
    // the header says which account is active, and that it is virtual
    await p.waitFor(`/VIRTUAL/.test(document.querySelector('#acctChip').textContent)`, "VIRTUAL account chip");

    await go("orders");
    assert(await p.eval(`/execution core/.test(document.querySelector('#ordersView').textContent)`), "Orders placeholder text missing");

    await go("accounts");
    await p.waitFor(`document.querySelectorAll('#acctList .acct').length >= 1`, "an account card");
    const a = await p.eval(`(() => { const c = document.querySelector('#acctList .acct'); return { badge: c.querySelector('.vbadge')?.textContent, text: c.textContent, note: document.querySelector('#acctNote').textContent,
      brokers: [...document.querySelectorAll('.broker')].map(b => [b.textContent, b.querySelector('button').disabled]) }; })()`);
    assert(a.badge === "VIRTUAL", `account card badge: ${a.badge}`);
    assert(/10,000/.test(a.text) && /1,000,000/.test(a.text), "default virtual credits not shown");
    assert(/holds no money/.test(a.note), "the 'holds no money' line is missing");
    assert(a.brokers.length === 2 && a.brokers.every(([t]) => /NOT CONNECTED|NOT BUILT/i.test(t)), "broker cards must say they are not connected");
    assert(a.brokers[1][1] === true && /FYERS/.test(a.brokers[1][0]), "FYERS connect stays disabled until its connector exists");
    assert(await p.eval(`document.querySelector('#bnForm [name=api_secret]').type === 'password'`), "the secret field must be a password field");
    // edit credits end to end: the server's validation message and a successful save both reach the page
    await p.eval(`document.querySelector('#acctList .acct-actions button').click()`);
    await p.eval(`(() => { const f = document.querySelector('#acctList .acct-edit'); f.elements.USDT.value = '0.123456789'; f.requestSubmit(); })()`);
    await p.waitFor(`/decimal places/.test(document.querySelector('#acctList .acct-msg').textContent)`, "server validation message");
    p.errors.length = 0; // the browser logs the deliberate 400 above as a console error; anything after it is a real error
    await p.eval(`(() => { const f = document.querySelector('#acctList .acct-edit'); f.elements.USDT.value = '12500'; f.requestSubmit(); })()`);
    await p.waitFor(`document.querySelector('#acctList .acct').textContent.includes('12,500')`, "saved credits");

    await go("settings");
    await p.waitFor(`document.querySelectorAll('#setTz option').length > 5 && document.querySelectorAll('#auditBody tr').length >= 2`, "timezone list and audit rows");
    const st = await p.eval(`({ sec: document.querySelector('#setSecurity').textContent, audit: document.querySelector('#auditBody').textContent })`);
    assert(/Live trading\s*disarmed/.test(st.sec) && /API token\s*active/.test(st.sec), `security section: ${st.sec}`);
    assert(/account_created/.test(st.audit) && /account_updated/.test(st.audit), "audit viewer should show the account events");
    assert(p.errors.length === 0, `console errors:\n    ${p.errors.join("\n    ")}`);
  }],

  ["Bot Studio: catalog, create, backtest on sample data, live refused", async (p, url) => {
    await p.load(url);
    await waitBooted(p);
    await p.eval(`document.querySelector('#tabs [data-v=bots]').click()`);
    await p.waitFor(`document.querySelectorAll('.cat-card').length === 7`, "seven catalog bots");
    const kinds = await p.eval(`[...document.querySelectorAll('.cat-card .bdg')].map(b => b.textContent).join(',')`);
    assert(/OPEN SOURCE/.test(kinds) && /COMMERCIAL/.test(kinds), `open source vs commercial badges: ${kinds}`);
    assert(await p.eval(`[...document.querySelectorAll('.cat-note')].every(n => /Not integrated/.test(n.textContent))`), "every catalog card must say it is not integrated");
    // add the first catalog bot, create it
    await p.eval(`document.querySelector('.cat-card .btn').click()`);
    await p.waitFor(`/not an integration/.test(document.querySelector('.st-build-head')?.textContent || '')`, "builder says the bot is not an integration");
    await p.eval(`document.querySelector('.st-actions .btn').click()`);
    await p.waitFor(`/Created/.test(document.querySelector('.st-actions .st-msg').textContent)`, "bot created");
    await p.eval(`document.querySelector('#stTabs [data-t=mine]').click()`);
    await p.waitFor(`document.querySelectorAll('.bot-card').length === 1 && /VIRTUAL/.test(document.querySelector('.bot-card').textContent)`, "bot card labelled VIRTUAL");
    // backtest with the saved report
    await p.eval(`[...document.querySelectorAll('.bot-card button')].find(b => b.textContent === 'Backtest').click()`);
    await p.waitFor(`[...document.querySelectorAll('.st-actions .btn')].some(b => b.textContent === 'Run backtest')`, "backtest form");
    await p.eval(`[...document.querySelectorAll('.st-actions .btn')].find(b => b.textContent === 'Run backtest').click()`);
    await p.waitFor(`document.querySelector('.bt-out .stats') && /SAMPLE DATA/.test(document.querySelector('.bt-out').textContent)`, "backtest metrics labelled SAMPLE", 30000);
    // live readiness: refused, with the reasons
    await p.eval(`document.querySelector('#stTabs [data-t=perf]').click()`);
    await p.waitFor(`document.querySelectorAll('.gate-list li').length >= 6`, "live-readiness checklist");
    assert(await p.eval(`/Live order route built/.test(document.querySelector('.gate-list').textContent) && document.querySelector('.gate-list li.ok') !== null`), "checklist shows the saved backtest and the missing live route");
    assert(p.errors.length === 0, `console errors:\n    ${p.errors.join("\n    ")}`);
  }],

  ["mobile 375px: no horizontal scroll", async (p, url) => {
    await p.viewport(375, 812, true);
    try {
      await p.load(url);
      await waitBooted(p);
      for (const view of ["board", "orders", "bots", "accounts", "journal", "settings"]) {
        await p.eval(`document.querySelector('#tabs [data-v=${view}]').click()`);
        await sleep(300);
        const m = await p.eval(`({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth, bw: document.body.scrollWidth })`);
        assert(m.sw <= m.cw && m.bw <= m.cw, `horizontal overflow on ${view} at 375px: scrollWidth ${m.sw}, body ${m.bw}, client ${m.cw}`);
      }
    } finally {
      await p.send("Emulation.clearDeviceMetricsOverride");
    }
  }],
];

// ---------------------------------------------------------------- run
const procs = [];
const dirs = [];
let browserOutput = "";
let serverOutput = "";

function killAll() {
  for (const pr of procs) {
    try {
      if (win && pr.pid) spawn("taskkill", ["/pid", String(pr.pid), "/T", "/F"], { stdio: "ignore" });
      else pr.kill("SIGKILL");
    } catch {}
  }
}
async function cleanup() {
  killAll();
  await sleep(win ? 800 : 200); // let Windows release the file locks before deleting
  for (const d of dirs) {
    for (let i = 0; i < 5; i++) {
      try {
        rmSync(d, { recursive: true, force: true });
        break;
      } catch {
        await sleep(300);
      }
    }
  }
}

async function main() {
  const server = findServer();
  if (!server) {
    console.error("ui_smoke: the server is not built. Build it first (this script builds nothing):\n  cd src-tauri && cargo build --no-default-features --bin zoneboard-server\nOr point ZB_SERVER at a zoneboard-server binary.");
    return 2;
  }
  const browser = findBrowser();
  if (!browser) {
    console.error("ui_smoke: no Edge/Chrome/Chromium found. Install one or set CHROME=/path/to/browser.");
    return 2;
  }

  const tmp = mkdtempSync(join(tmpdir(), "zb-smoke-"));
  dirs.push(tmp);
  const dataDir = join(tmp, "data");
  const profile = join(tmp, "profile");

  const port = await freePort();
  const sp = spawn(server, [], {
    cwd: tmp,
    env: { ...process.env, SAMPLE_DATA: "1", PORT: String(port), ZONEBOARD_DATA: dataDir, JOURNAL_DB: join(tmp, "journal.db") },
    stdio: ["ignore", "pipe", "pipe"],
  });
  procs.push(sp);
  sp.stdout.on("data", (d) => (serverOutput += d));
  sp.stderr.on("data", (d) => (serverOutput += d));
  let serverExit = null;
  sp.on("exit", (c) => (serverExit = c));
  const url = `http://127.0.0.1:${port}/`;
  for (let i = 0; i < 100; i++) {
    if (serverExit !== null) throw new Error(`server exited early (code ${serverExit}):\n${serverOutput}`);
    try {
      if ((await fetch(url)).ok) break;
    } catch {}
    await sleep(100);
    if (i === 99) throw new Error("server did not start in 10 s:\n" + serverOutput);
  }
  // The API wants X-ZB-Token, which the server puts in the page's <meta name="zb-token">. The browser reads it itself,
  // so this only proves the harness got a served page that carries one.
  const html = await (await fetch(url)).text();
  const token = /<meta name="zb-token" content="([^"]+)"/.exec(html)?.[1];
  if (!token) throw new Error('the served page has no <meta name="zb-token"> content');
  const probe = await fetch(url + "api/positions", { headers: { "X-ZB-Token": token } });
  if (probe.status === 401 || probe.status === 403) throw new Error(`API rejected the token from the page (HTTP ${probe.status})`);

  const bp = spawn(browser, [
    "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-sandbox", "--disable-gpu",
    "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--window-size=1280,900", "about:blank",
  ], { stdio: ["ignore", "pipe", "pipe"] });
  procs.push(bp);
  bp.stdout.on("data", (d) => (browserOutput += d));
  bp.stderr.on("data", (d) => (browserOutput += d));

  // Chrome writes the chosen debugging port into <profile>/DevToolsActivePort.
  let dtPort = null;
  for (let i = 0; i < 150 && !dtPort; i++) {
    try {
      dtPort = readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0].trim();
    } catch {
      await sleep(100);
    }
  }
  if (!dtPort) throw new Error("browser did not open a debugging port:\n" + browserOutput.slice(-800));
  const targets = await (await fetch(`http://127.0.0.1:${dtPort}/json/list`)).json();
  const wsUrl = targets.find((t) => t.type === "page")?.webSocketDebuggerUrl;
  if (!wsUrl) throw new Error("browser has no page target");
  const ws = new WebSocket(wsUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res, { once: true });
    ws.addEventListener("error", () => rej(new Error("could not connect to the browser")), { once: true });
  });
  const page = new Page(ws);
  await page.send("Runtime.enable");
  await page.send("Log.enable");
  await page.send("Page.enable");
  await page.viewport(1280, 900);

  let failed = 0;
  for (const [name, fn] of scenarios) {
    try {
      // Fresh storage per scenario so a toggle in one never leaks into the next.
      await page.send("Page.navigate", { url: "about:blank" });
      await page.send("Network.enable");
      await page.send("Network.clearBrowserCache");
      await page.send("Network.clearBrowserCookies");
      await page.eval("0");
      await page.send("Storage.clearDataForOrigin", { origin: `http://127.0.0.1:${port}`, storageTypes: "all" });
      await fn(page, url);
      console.log(`PASS  ${name}`);
    } catch (e) {
      failed++;
      console.log(`FAIL  ${name}\n    ${String(e.message).replace(/\n/g, "\n    ")}`);
    }
  }
  console.log(failed ? `\n${failed} of ${scenarios.length} scenarios failed.` : `\nAll ${scenarios.length} scenarios passed.`);
  try {
    ws.close();
  } catch {}
  return failed ? 1 : 0;
}

let code = 1;
try {
  code = await main();
} catch (e) {
  console.error("ui_smoke: " + e.message);
  code = 1;
} finally {
  await cleanup();
}
process.exit(code);
