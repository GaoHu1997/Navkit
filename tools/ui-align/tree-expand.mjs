// 通过 App 命令 API 打开数据库(绕开自动演示脚本的菜单事件干扰)
import { chromium } from "playwright-core";
const browser = await chromium.launch({
  executablePath: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  headless: true,
  args: ["--force-device-scale-factor=2", "--hide-scrollbars"],
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
await page.goto("http://localhost:5183/preview.html", { waitUntil: "networkidle" });
await page.waitForTimeout(7000);

const state = await page.evaluate(() => {
  const app = window.__APP__;
  // 选中 dbswitch 库节点后执行「打开数据库」命令
  const node = app.tree.find ? [...document.querySelectorAll('.tree-node')].find((n) => (n.dataset.id || '').endsWith('/db/dbswitch')) : null;
  const id = node?.dataset.id;
  if (!id) return { ok: false, reason: "node not found", ids: [...document.querySelectorAll(".tree-node")].slice(0, 6).map((n) => n.dataset.id) };
  node.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  app.tree.select(id);
  return app.runCommand("db-open").then(() => ({ ok: true }));
});
await page.waitForTimeout(1500);
await page.evaluate(() => {
  const app = window.__APP__;
  const el = [...document.querySelectorAll('.tree-node')].find((n) => (n.dataset.id || '').endsWith('/db/dbswitch/grp-table') || (n.dataset.id || '').endsWith('/db/dbswitch'));
  // 若分组未展开,通过树 API 展开
  const n = app.tree.find(el?.dataset.id ?? "");
  if (n && !n.expanded && n.expandable) app.tree.toggle(n.id);
});
await page.waitForTimeout(600);
await page.screenshot({ path: "docs/ui-align/v2-tree-pict4.png" });
await page.screenshot({ path: "docs/ui-align/v2-tree-pict4-crop.png", clip: { x: 0, y: 130, width: 370, height: 520 } });
await browser.close();
console.log(JSON.stringify(state));
