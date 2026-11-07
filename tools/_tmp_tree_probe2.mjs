import { chromium } from "playwright-core";

const EXE = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const URL = process.env.PROBE_URL || "http://localhost:5183/preview.html";

async function run() {
  const browser = await chromium.launch({ executablePath: EXE, headless: true });
  const results = [];
  for (const where of ["label", "icon", "blank"]) {
    const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
    await page.goto(URL, { waitUntil: "load" });
    await page.waitForSelector(".tree-node", { timeout: 20000 });
    await page.waitForTimeout(2600);
    const row = page.locator('.tree-node', { hasText: "开发环境" }).first();
    const box = await row.boundingBox();
    const before = await page.evaluate(() => {
      const r = [...document.querySelectorAll(".tree-node")].find((n) => n.textContent.includes("开发环境"));
      return { expanded: r?.getAttribute("aria-expanded"), count: document.querySelectorAll(".tree-node").length };
    });
    let tx, ty;
    if (where === "label") { tx = box.x + 60; ty = box.y + box.height / 2; }
    if (where === "icon") { tx = box.x + 34; ty = box.y + box.height / 2; }
    if (where === "blank") { tx = box.x + box.width - 30; ty = box.y + box.height / 2; }
    await page.mouse.dblclick(tx, ty);
    await page.waitForTimeout(600);
    const after = await page.evaluate(() => {
      const r = [...document.querySelectorAll(".tree-node")].find((n) => n.textContent.includes("开发环境"));
      return { expanded: r?.getAttribute("aria-expanded"), count: document.querySelectorAll(".tree-node").length };
    });
    results.push({ where, box, tx, ty, before, after });
    await page.close();
  }
  console.log(JSON.stringify(results, null, 2));
  await browser.close();
}
run();
