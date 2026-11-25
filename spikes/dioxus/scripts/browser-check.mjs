// Drive headless Chrome over CDP (no deps; Node >= 22) to check hydration,
// client-side Facet navigation, and the login -> admin flow with JS on.
// Usage: PASSCODE=... node scripts/browser-check.mjs [base-url]
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const BASE = process.argv[2] ?? "http://127.0.0.1:8080";
const PASSCODE = process.env.PASSCODE;
const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const profile = mkdtempSync(join(tmpdir(), "spike-chrome-"));
const chrome = spawn(CHROME, [
  "--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`,
  "--no-first-run", "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let targets;
for (let i = 0; i < 50; i++) {
  try { targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json(); break; }
  catch { await sleep(200); }
}
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r, { once: true }));

let id = 0;
const pending = new Map();
const problems = [];
const bytes = {};
ws.addEventListener("message", (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  if (msg.method === "Runtime.exceptionThrown") problems.push(msg.params.exceptionDetails.text);
  if (msg.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(msg.params.type))
    problems.push(msg.params.args.map((a) => a.value ?? a.description).join(" "));
  if (msg.method === "Network.responseReceived") bytes[msg.params.requestId] = { url: msg.params.response.url };
  if (msg.method === "Network.loadingFinished" && bytes[msg.params.requestId])
    bytes[msg.params.requestId].wire = msg.params.encodedDataLength;
});
const send = (method, params = {}) => new Promise((r) => {
  const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params }));
});
const evaluate = async (expr) =>
  (await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value;
const waitFor = async (expr, ms = 15000) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) { if (await evaluate(expr)) return Date.now() - t0; await sleep(50); }
  throw new Error(`timed out waiting for: ${expr}`);
};
const text = (sel) => evaluate(`document.querySelector(${JSON.stringify(sel)})?.textContent ?? null`);

await send("Runtime.enable");
await send("Network.enable");
await send("Network.setCacheDisabled", { cacheDisabled: true });

try {
  console.log("== Hydration");
  const t0 = Date.now();
  await send("Page.navigate", { url: `${BASE}/` });
  await waitFor(`document.readyState === "complete"`);
  await waitFor(`document.getElementById("js-state")?.textContent === "JS: hydrated"`);
  console.log(`hydrated ${Date.now() - t0} ms after navigate; #js-state="${await text("#js-state")}"; ${await text("#count")}`);
  for (const b of Object.values(bytes)) if (/\.(wasm|js)$/.test(b.url))
    console.log(`  transferred ${b.wire} B  ${b.url.replace(BASE, "")}`);

  console.log("== Client-side Facet change (no page reload)");
  await evaluate(`window.__noReload = 1`);
  await evaluate(`(() => { const s = document.querySelector('select[name=year]'); s.value = '2024';
    s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  const ms = await waitFor(`document.getElementById("count")?.textContent === "3 Tests"`);
  console.log(`after selecting 2024: "${await text("#count")}" in ${ms} ms; url=${await evaluate("location.pathname + location.search")}; same JS context=${await evaluate("window.__noReload === 1")}`);
  await evaluate(`(() => { const s = document.querySelector('select[name=level]'); s.value = 'Geometry';
    s.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await waitFor(`document.getElementById("count")?.textContent === "1 Tests"`);
  console.log(`after Geometry: "${await text("#count")}" ${(await text("#results")).trim()}; url=${await evaluate("location.pathname + location.search")}`);

  console.log("== SPA link to /admin while anonymous");
  await evaluate(`[...document.querySelectorAll('nav a')].find(a => a.textContent === 'Admin').click()`);
  await waitFor(`document.querySelector("h1")?.textContent === "Admin"`);
  console.log(`client-side render: "${(await evaluate("document.querySelector('h1').nextElementSibling?.textContent"))}"; url=${await evaluate("location.pathname")}`);

  if (PASSCODE) {
    console.log("== Login with JS on (native form POST, 303 to /admin)");
    await send("Page.navigate", { url: `${BASE}/login` });
    await waitFor(`document.getElementById("js-state")?.textContent === "JS: hydrated"`);
    await evaluate(`document.querySelector('input[name=passcode]').value = ${JSON.stringify(PASSCODE)};
      document.querySelector('form').requestSubmit()`);
    await waitFor(`location.pathname === "/admin" && !!document.getElementById("objects")`);
    await waitFor(`document.getElementById("js-state")?.textContent === "JS: hydrated"`);
    const n = await evaluate(`document.querySelectorAll('#objects li').length`);
    console.log(`on ${await evaluate("location.pathname")}, hydrated, ${n} objects listed; document.cookie="${await evaluate("document.cookie")}" (HttpOnly hides the session)`);
  }
} catch (e) {
  console.log("FAILED:", e.message);
  process.exitCode = 1;
}
console.log(problems.length ? `console errors/warnings:\n  ${problems.join("\n  ")}` : "no console errors or warnings");
ws.close();
chrome.kill();
await sleep(300);
rmSync(profile, { recursive: true, force: true });
