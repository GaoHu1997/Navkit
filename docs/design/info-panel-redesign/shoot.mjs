/**
 * 渲染「信息面板 · 对象头顶部重设计」对比板：
 *  · 整板截图 2×（previews/00-对比板.png）
 *  · 逐个方案单独截图 2×（previews/01..05）
 *
 * 用法：node docs/design/info-panel-redesign/shoot.mjs
 * 依赖：docs/design/serve.mjs 已在 127.0.0.1:5211 运行（或设置 BASE_URL）。
 */

import { chromium } from "playwright-core";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { mkdirSync, existsSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "previews");
mkdirSync(out, { recursive: true });

const URL_ = process.env.PAGE_URL || pathToFileURL(join(here, "index.html")).href;

const CHROME = [
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
].find((p) => existsSync(p));

const browser = await chromium.launch({
  executablePath: CHROME,
  headless: true,
  args: ["--no-sandbox", "--disable-gpu", "--allow-file-access-from-files", "--disable-features=Translate"],
});
const page = await browser.newPage({ viewport: { width: 1180, height: 1200 }, deviceScaleFactor: 2 });

const errors = [];
page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
page.on("pageerror", (e) => errors.push(e.message));

await page.goto(URL_, { waitUntil: "domcontentloaded", timeout: 45000 });
await page.waitForSelector(".cell", { timeout: 15000 });
// 等图片（品牌 logo）解码完成，避免截到空白瓦片
await page.evaluate(() =>
  Promise.all([...document.images].map((img) => (img.complete ? 0 : img.decode().catch(() => 0)))),
);
await page.waitForTimeout(300);

const names = ["00-对比板", "01-现状", "02-方案A", "03-方案B", "04-方案C", "05-方案A-列信息"];

const cells = await page.$$(".cell");
await page.screenshot({ path: join(out, `${names[0]}.png`), fullPage: true });
for (let i = 0; i < cells.length; i++) {
  await cells[i].screenshot({ path: join(out, `${names[i + 1]}.png`) });
}

const size = await page.evaluate(() => ({ w: document.body.scrollWidth, h: document.body.scrollHeight }));
console.log(`板面 ${size.w}×${size.h}，导出 ${cells.length + 1} 张到 ${out}`);
if (errors.length) {
  console.log("控制台错误：\n" + errors.join("\n"));
  process.exitCode = 1;
}
await browser.close();
