/**
 * 渲染表设计器调整稿：逐屏导出 PNG 到 docs/design/shots/
 * 用法：node docs/design/render-table-designer.mjs
 */
import { chromium } from "playwright-core";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, mkdirSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const page_url = pathToFileURL(join(here, "table-designer-redesign.html")).href;
const shotDir = join(here, "shots");
const CHROME = [
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
].find((p) => existsSync(p));
if (!CHROME) { console.error("未找到 Chrome/Edge"); process.exit(1); }

const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const page = await browser.newPage({ viewport: { width: 1340, height: 1200 }, deviceScaleFactor: 2 });
const errors = [];
page.on("console", (m) => { if (m.type() === "error") errors.push(`console.error: ${m.text()}`); });
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));

await page.goto(page_url, { waitUntil: "networkidle" });
await page.waitForTimeout(400);

const info = await page.evaluate(() => ({
  bodyW: document.body.scrollWidth,
  bodyH: document.body.scrollHeight,
  overflowX: document.body.scrollWidth > window.innerWidth,
  shots: document.querySelectorAll(".shot").length,
}));
console.log(JSON.stringify(info));
if (info.overflowX) errors.push(`页面横向溢出：${info.bodyW} > 视口宽度`);

mkdirSync(shotDir, { recursive: true });
const names = ["01-fields", "02-indexes", "03-details", "04-sql-preview"];
const shots = await page.$$(".shot");
for (let i = 0; i < shots.length; i++) {
  await shots[i].screenshot({ path: join(shotDir, `${names[i] ?? `shot-${i + 1}`}.png`) });
}
console.log(`已导出 ${shots.length} 张：${names.slice(0, shots.length).join(", ")}`);
if (errors.length) { console.error(errors.join("\n")); }
await browser.close();
process.exit(errors.length ? 1 : 0);
