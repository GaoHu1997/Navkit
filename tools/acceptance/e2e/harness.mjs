/**
 * NavKit 验收自动化基座：经 CDP 连接真实 Tauri 窗口（WebView2）。
 *
 * 启动被测程序前设置:
 *   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
 *
 * 提供: connect() / shot() / consoleLog 全程捕获 / 常用选择器助手。
 * 所有用例脚本 import { connect, EVID, log } from "./harness.mjs"。
 */
import fs from "node:fs";
import path from "node:path";
import { chromium } from "playwright-core";

export const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w):/, "$1:")), "../../..");
export const EVID = path.join(ROOT, "docs", "验收证据");
fs.mkdirSync(EVID, { recursive: true });

/** 全程控制台/异常日志（R5: 不允许静默失败）。 */
export const consoleLines = [];

/** 连接 CDP，返回 { browser, ctx, page }。 */
export async function connect(url = "http://127.0.0.1:9222") {
  const browser = await chromium.connectOverCDP(url);
  const ctx = browser.contexts()[0];
  if (!ctx) throw new Error("CDP 已连接但无浏览器上下文");
  let page = ctx.pages().find((p) => /navkit|preview|localhost/.test(p.url())) ?? ctx.pages()[0];
  if (!page) throw new Error("CDP 上下文中无页面");
  page.on("console", (msg) => {
    const line = `[${msg.type()}] ${msg.text()}`;
    consoleLines.push(line);
    if (msg.type() === "error" || msg.type() === "warning") fs.appendFileSync(path.join(EVID, "console.log"), line + "\n");
  });
  page.on("pageerror", (err) => {
    const line = `[PAGEERROR] ${err.message}\n${err.stack ?? ""}`;
    consoleLines.push(line);
    fs.appendFileSync(path.join(EVID, "console.log"), line + "\n");
  });
  return { browser, ctx, page };
}

/** 截图到 docs/验收证据/，返回绝对路径。 */
export async function shot(page, name) {
  const p = path.join(EVID, `${name}.png`);
  await page.screenshot({ path: p });
  return p;
}

export function log(msg) {
  console.log(msg);
  fs.appendFileSync(path.join(EVID, "run.log"), `${new Date().toISOString()} ${msg}\n`);
}

/** 等待 ms。 */
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- 选择器助手
// 应用为自绘 DOM：标题栏/菜单栏/Ribbon/标签条/树/网格/状态栏 均有稳定类名。
export const sel = {
  menuBarItem: (label) => `.menubar-item:text-is("${label}")`,
  ribbonBtn: (label) => `.ribbon-btn:has(.ribbon-label:text-is("${label}"))`,
  tabItem: (title) => `.tab-item:has-text("${title}")`,
  treeRow: (text) => `.tree-row:has-text("${text}")`,
  gridHeaderCell: (text) => `.grid-header-cell:text-is("${text}")`,
  statusbar: ".statusbar",
  toast: ".toast",
  dialog: ".dialog",
};

/** 点击菜单栏项并点击下拉项（如 ["文件","新建连接"]）。 */
export async function menuPath(page, [top, ...rest]) {
  await page.click(sel.menuBarItem(top));
  await sleep(120);
  for (const item of rest) {
    await page.click(`.menu-popup .menu-item:text-is("${item}")`);
    await sleep(120);
  }
}

/** 读取 toast 文本（若有）。 */
export async function toastText(page) {
  const t = await page.locator(".toast").allTextContents().catch(() => []);
  return t.join(" | ");
}

/** 判定元素存在。 */
export const exists = (page, selector) => page.locator(selector).count().then((n) => n > 0);
