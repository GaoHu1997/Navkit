import { chromium } from "playwright-core";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join, extname, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdirSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".woff2": "font/woff2" };
const server = createServer(async (req, res) => {
  try {
    let p = decodeURIComponent((req.url || "/").split("?")[0]);
    if (p === "/") p = "/mockup-table-form.html";
    const fp = join(here, p);
    const data = await readFile(fp);
    res.writeHead(200, { "content-type": MIME[extname(fp)] || "application/octet-stream" });
    res.end(data);
  } catch (e) { res.writeHead(404); res.end("not found"); }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;
const CHROME = ["C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe"].find((p) => existsSync(p));
const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const page = await browser.newPage({ viewport: { width: 1560, height: 1200 }, deviceScaleFactor: 2 });
const errors = [];
page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text()); });
await page.goto("http://127.0.0.1:" + port + "/mockup-table-form.html", { waitUntil: "networkidle" });
try { await page.waitForFunction(() => document.querySelectorAll(".screen-frame").length === 3, undefined, { timeout: 8000 }); } catch (e) { console.log("WAITFAIL errors=" + errors.length); console.log([...new Set(errors)].join("\n")); await browser.close(); server.close(); process.exit(1); }
await page.waitForTimeout(400);
const outDir = join(here, "shots");
mkdirSync(outDir, { recursive: true });
const names = ["表数据-表单视图-01-网格视图", "表数据-表单视图-02-表单视图", "表数据-表单视图-03-页脚细节"];
const frames = await page.$$(".screen-frame");
for (let i = 0; i < frames.length; i++) {
  await frames[i].screenshot({ path: join(outDir, names[i] + ".png") });
}
console.log("frames=" + frames.length + " errors=" + errors.length);
if (errors.length) console.log([...new Set(errors)].slice(0, 20).join("\n"));
await browser.close();
server.close();
