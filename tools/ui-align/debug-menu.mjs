// 调试4:谁移除了 popup
import { chromium } from "playwright-core";
const browser = await chromium.launch({
  executablePath: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));
await page.goto("http://localhost:5183/preview.html", { waitUntil: "networkidle" });
await page.waitForTimeout(1200);
await page.evaluate(() => {
  const origRemove = Element.prototype.remove;
  window.__log = [];
  Element.prototype.remove = function () {
    if (this.classList && this.classList.contains("menu-popup")) {
      window.__log.push(new Error("remove").stack || "nostack");
    }
    origRemove.call(this);
  };
});
await page.click(".menubar-item >> text=\"查看\"");
await page.waitForTimeout(400);
const log = await page.evaluate(() => window.__log);
for (const l of log) console.log(l.split("\n").slice(0, 7).join("\n"), "\n----");
await browser.close();
