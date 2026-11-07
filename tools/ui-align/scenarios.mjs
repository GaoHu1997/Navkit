// 场景页截图: designer / backup / users / query
import { chromium } from "playwright-core";
const browser = await chromium.launch({
  executablePath: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  headless: true,
  args: ["--force-device-scale-factor=2", "--hide-scrollbars"],
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });

async function go(pageParam, name, extra = []) {
  await page.goto(`http://localhost:5183/preview.html${pageParam}`, { waitUntil: "networkidle" });
  await page.waitForTimeout(5200);
  for (const a of extra) {
    if (a.startsWith("click:")) await page.click(a.slice(6), { timeout: 4000 }).catch((e) => console.log("[click-fail]", a, e.message.split("\n")[0]));
    if (a.startsWith("wait:")) await page.waitForTimeout(+a.slice(5));
    if (a.startsWith("rclick:")) await page.click(a.slice(7), { button: "right", timeout: 4000 }).catch((e) => console.log("[rclick-fail]", a));
  }
  await page.screenshot({ path: `docs/ui-align/${name}.png` });
  console.log("saved", name);
}

await go("?page=designer", "v2-designer2");
await go("?page=backup", "v2-backup");
await go("?page=users", "v2-users");
await go("", "v2-query2", ["click:.tab:has-text(\"查询\")", "wait:600"]);
await browser.close();
