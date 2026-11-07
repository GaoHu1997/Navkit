/**
 * UI 对齐验证截图脚本。
 * 用法: node tools/ui-align/shot.mjs <name> [actions...]
 * 动作(可选, 依次执行):
 *   wait:<ms>                  等待
 *   click:<selector>           点击
 *   dblclick:<selector>        双击
 *   type:<selector>|<text>     填入文字
 *   press:<key>                按键
 *   shot:<name>                截图(附加)
 *   view:<w>x<h>               改视口
 * 结果输出到 docs/ui-align/<name>.png
 */
import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright-core";

const ROOT = path.resolve(new URL("..", import.meta.url).pathname.replace(/^\/(\w):/, "$1:"), "..");
const OUT = path.join(ROOT, "docs", "ui-align");
fs.mkdirSync(OUT, { recursive: true });

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const EXE = fs.existsSync(CHROME) ? CHROME : EDGE;

const name = process.argv[2] || "baseline";
const actions = process.argv.slice(3);

const browser = await chromium.launch({
  executablePath: EXE,
  headless: true,
  args: ["--force-device-scale-factor=2", "--hide-scrollbars"],
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
const errors = [];
page.on("console", (m) => {
  if (m.type() === "error") errors.push(`[console] ${m.text()}`);
});
page.on("pageerror", (e) => errors.push(`[pageerror] ${e.message}`));

await page.goto("http://localhost:5183/preview.html", { waitUntil: "networkidle" });
await page.waitForSelector(".titlebar, .app, #app *", { timeout: 15000 }).catch(() => {});
await page.waitForTimeout(1200);

for (const a of actions) {
  if (a.startsWith("wait:")) await page.waitForTimeout(+a.slice(5));
  else if (a.startsWith("click:")) await page.click(a.slice(6), { timeout: 5000 }).catch((e) => errors.push(`[click-fail] ${a}: ${e.message.split("\n")[0]}`));
  else if (a.startsWith("dblclick:")) await page.dblclick(a.slice(9), { timeout: 5000 }).catch((e) => errors.push(`[dblclick-fail] ${a}: ${e.message.split("\n")[0]}`));
  else if (a.startsWith("rclick:")) await page.click(a.slice(7), { button: "right", timeout: 5000 }).catch((e) => errors.push(`[rclick-fail] ${a}`));
  else if (a.startsWith("type:")) {
    const [sel, ...rest] = a.slice(5).split("|");
    await page.fill(sel, rest.join("|"), { timeout: 5000 }).catch((e) => errors.push(`[type-fail] ${a}`));
  } else if (a.startsWith("press:")) await page.keyboard.press(a.slice(6));
  else if (a.startsWith("view:")) {
    const [w, h] = a.slice(5).split("x").map(Number);
    await page.setViewportSize({ width: w, height: h });
  } else if (a.startsWith("shot:")) {
    await page.screenshot({ path: path.join(OUT, `${a.slice(5)}.png`) });
  }
}

await page.screenshot({ path: path.join(OUT, `${name}.png`) });
await browser.close();
console.log(`saved: docs/ui-align/${name}.png`);
if (errors.length) {
  console.log("ERRORS:");
  for (const e of errors.slice(0, 20)) console.log("  " + e);
  process.exitCode = 2;
}
