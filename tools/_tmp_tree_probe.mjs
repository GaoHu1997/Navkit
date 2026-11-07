import { chromium } from "playwright-core";

const EXE = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const URL = process.env.PROBE_URL || "http://localhost:5183/preview.html";

const browser = await chromium.launch({ executablePath: EXE, headless: true });
const page = await browser.newPage({ viewport: { width: 1400, height: 900 } });
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));

await page.goto(URL, { waitUntil: "load" });
await page.waitForSelector(".tree-node", { timeout: 20000 }).catch(() => {});
await page.waitForTimeout(2500);

const info = await page.evaluate(() => {
  const scroll = document.querySelector(".tree-scroll");
  const rect = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  };
  const rows = [...document.querySelectorAll(".tree-node")].map((row) => {
    const label = row.querySelector(".tree-label");
    const icon = row.querySelector(".tree-icon");
    const arrow = row.querySelector(".tree-arrow");
    const cs = getComputedStyle(row);
    return {
      id: row.dataset.id,
      text: row.textContent,
      row: rect(row),
      icon: rect(icon),
      label: rect(label),
      arrow: rect(arrow),
      display: cs.display,
      width: cs.width,
    };
  });
  return {
    scroll: rect(scroll),
    scrollClientWidth: scroll ? scroll.clientWidth : null,
    scrollScrollWidth: scroll ? scroll.scrollWidth : null,
    appExists: !!window.__APP__,
    rows: rows.slice(0, 12),
  };
});
console.log(JSON.stringify(info, null, 2));
console.log("--- console logs ---");
console.log(logs.slice(-20).join("\n"));
await browser.close();
