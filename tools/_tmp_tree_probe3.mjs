import { chromium } from "playwright-core";
const EXE = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const URL = "http://localhost:5183/preview.html";

async function boxOf(loc) {
  for (let i = 0; i < 20; i++) {
    const b = await loc.boundingBox();
    if (b) return b;
    await new Promise((r) => setTimeout(r, 200));
  }
  return null;
}

async function one(browser, nodeText, where) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  await page.goto(URL, { waitUntil: "load" });
  await page.waitForSelector(".tree-node", { timeout: 20000 });
  await page.waitForTimeout(4200);
  const row = page.locator(".tree-node", { hasText: nodeText }).first();
  await row.waitFor({ state: "visible", timeout: 15000 });
  const rbox = await boxOf(row);
  const icon = await boxOf(row.locator(".tree-icon").first());
  const label = await boxOf(row.locator(".tree-label").first());
  const grab = () => page.evaluate((t) => {
    const r = [...document.querySelectorAll(".tree-node")].find((n) => n.textContent.includes(t));
    return { id: r?.dataset.id, expanded: r?.getAttribute("aria-expanded"), count: document.querySelectorAll(".tree-node").length, tabs: [...document.querySelectorAll("[class*=tab]")].map(x=>x.textContent.trim()).filter(Boolean).slice(0,8) };
  }, nodeText);
  const before = await grab();
  let tx, ty;
  if (where === "label") { tx = label.x + label.width / 2; ty = label.y + label.height / 2; }
  else if (where === "icon") { tx = icon.x + icon.width / 2; ty = icon.y + icon.height / 2; }
  else { tx = rbox.x + rbox.width - 12; ty = rbox.y + rbox.height / 2; }
  await page.mouse.dblclick(tx, ty);
  await page.waitForTimeout(1200);
  const after = await grab();
  await page.close();
  return { nodeText, where, dbl: { tx, ty }, rbox, before, after };
}

const browser = await chromium.launch({ executablePath: EXE, headless: true });
const out = [];
for (const where of ["label", "icon", "blank"]) out.push(await one(browser, "password_manager", where));
for (const where of ["label", "icon", "blank"]) out.push(await one(browser, "localhost", where));
console.log(JSON.stringify(out, null, 2));
await browser.close();
