/**
 * 用无头 Chrome 渲染设计画廊：
 *  · 捕获控制台错误与请求失败
 *  · 逐屏导出 PNG 到 docs/design/shots/
 *
 * 用法：node docs/design/render.mjs [--shots]
 */

import { chromium } from "playwright-core";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join, resolve } from "node:path";
import { mkdirSync, existsSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const gallery = process.env.GALLERY_URL || pathToFileURL(join(here, "gallery.html")).href;
const shotDir = join(here, "shots");
const takeShots = process.argv.includes("--shots");

const CHROME = [
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
].find((p) => existsSync(p));

if (!CHROME) {
  console.error("未找到 Chrome/Edge 可执行文件");
  process.exit(1);
}

const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const page = await browser.newPage({ viewport: { width: 1680, height: 1000 }, deviceScaleFactor: 2 });

const errors = [];
page.on("console", (m) => {
  if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
});
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("requestfailed", (r) => errors.push(`requestfailed: ${r.url()} ${r.failure()?.errorText}`));

await page.goto(gallery, { waitUntil: "networkidle" });
await page.waitForTimeout(700);

const counts = await page.evaluate(() => ({
  groups: document.querySelectorAll(".group").length,
  screens: document.querySelectorAll(".screen").length,
  frames: document.querySelectorAll(".screen-frame").length,
  emptyFrames: [...document.querySelectorAll(".screen-frame")].filter((f) => f.children.length === 0).length,
  height: document.body.scrollHeight,
}));

console.log(`group=${counts.groups} screen=${counts.screens} frame=${counts.frames} empty=${counts.emptyFrames} pageHeight=${counts.height}`);

const titles = await page.evaluate(() =>
  [...document.querySelectorAll(".screen")].map((s) => s.getAttribute("data-title")),
);
console.log("screens:", titles.join(" | "));

if (errors.length) {
  console.error(`\n${errors.length} 个错误:`);
  for (const e of [...new Set(errors)].slice(0, 30)) console.error(" -", e);
} else {
  console.log("无控制台错误");
}

if (takeShots) {
  mkdirSync(shotDir, { recursive: true });
  // 关闭缩放，逐屏按原始尺寸截图
  await page.evaluate(() => {
    document.body.classList.add("export");
    document.documentElement.style.setProperty("--frame-zoom", "1");
  });
  await page.waitForTimeout(300);
  const screens = await page.$$(".screen");
  for (let i = 0; i < screens.length; i++) {
    const s = screens[i];
    const title = (await s.getAttribute("data-title")) || `screen-${i}`;
    const safe = title.replace(/[\\/:*?"<>|\s·]+/g, "-").replace(/^-|-$/g, "");
    await s.screenshot({ path: join(shotDir, `${String(i + 1).padStart(2, "0")}-${safe}.png`) });
  }
  console.log(`已导出 ${screens.length} 张截图到 ${shotDir}`);
}

await browser.close();
process.exit(errors.length ? 1 : 0);
