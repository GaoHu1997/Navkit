// 驱动管理界面的独立浏览器验证：不启动 vite、不访问真实后端。
// 覆盖：状态词唯一、被引用版本禁止删除、需手动获取不给下载、失败留在行内、
//       筛选后右栏跟随、自定义驱动单页表单可保存、键盘与焦点、无横向溢出。
import assert from "node:assert/strict";
import { readFile, access } from "node:fs/promises";
import { build } from "esbuild";
import { chromium } from "playwright-core";

const executablePath = process.env.BROWSER_EXE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
await access(executablePath);

const bundle = await build({
  stdin: {
    contents: `
      import { openDriverManager, openCustomDriverDialog } from "./src/dialogs/DriverManagerDialog";
      window.openDriverManager = openDriverManager;
      window.openCustomDriverDialog = openCustomDriverDialog;
    `,
    resolveDir: process.cwd(),
    sourcefile: "driver-manager-browser-entry.ts",
  },
  bundle: true,
  format: "iife",
  write: false,
});

const browser = await chromium.launch({ executablePath, headless: true });
const CHIPS = ["已下载", "未下载", "部分就绪", "需手动获取", "下载中", "下载失败"];

try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));

  await page.route("http://localhost/**", (route) =>
    route.fulfill({ contentType: "text/html", body: '<!doctype html><html><head><meta charset="utf-8"></head><body><button id="trigger">驱动管理</button></body></html>' }));
  await page.goto("http://localhost/");
  await page.addStyleTag({ content: await readFile("src/styles/base.css", "utf8") });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });

  await page.evaluate(() => {
    const REF = (kind, version) => "@navkit-driver-version:" + JSON.stringify({ v: 1, kind, version, extra: "" });
    const ok = (row) => ({ architecture: "x86_64", library: null, download_path: null, message: "", ...row });
    const base = { group: "", read_only: false, keep_alive: false, user: "", password: "", database: "", file: "", host: "", port: 0 };
    window.__CALLS__ = [];
    window.__PREVIEW_MOCK__ = {
      "plugin:event|listen": () => 1,
      "plugin:event|unlisten": () => null,
      list_connections: () => [
        { ...base, id: "ora_prod", name: "生产 Oracle", kind: "oracle", params: REF("oracle", "23.5.0.24.07") },
        { ...base, id: "ora_dev", name: "开发 Oracle", kind: "oracle", params: REF("oracle", "23.5.0.24.07") },
      ],
      list_driver_statuses: () => [
        ok({ kind: "oracle", version: "23.5.0.24.07", status: "ready", download_path: "C:\\drivers\\ojdbc11.jar" }),
        ok({ kind: "oracle", version: "19.22.0.0", status: "missing", can_download: true }),
        ok({ kind: "trino", version: "476", status: "missing", can_download: true, library: "trino-jdbc" }),
        ok({ kind: "dameng", version: "", status: "manual", message: "达梦 DM8 需要系统安装的 ODBC 驱动（DM8 ODBC DRIVER）。" }),
        ok({ kind: "db2", version: "", status: "manual", message: "DB2 需要先安装 IBM Data Server Client。" }),
      ],
      list_odbc_inventory: () => ({
        architecture: "x86_64",
        drivers: [{ name: "ODBC Driver 17 for SQL Server", library: "" }],
        dsns: [],
      }),
      download_database_driver: (args) => {
        window.__CALLS__.push(["download", args.kind, args.version]);
        if (args.kind === "trino") throw new Error("下载失败：无法连接镜像站");
        return ok({ kind: args.kind, version: args.version || "", status: "ready" });
      },
      list_custom_drivers: () => [],
      save_custom_driver: (args) => {
        window.__CALLS__.push(["save", args.driver.name]);
        return [args.driver];
      },
      delete_custom_driver: () => [],
    };
    document.getElementById("trigger").focus();
    window.manager = window.openDriverManager();
  });

  const dialog = page.getByRole("dialog", { name: "驱动管理", exact: true });
  await dialog.waitFor();
  await page.waitForFunction(() => document.querySelectorAll(".drv-item").length > 0);

  // 内置协议不出现在清单里；状态词只能是约定的六个
  assert.equal(await dialog.locator(".drv-item").count(), 4);
  const chips = await dialog.locator(".drv-item .drv-chip").allTextContents();
  assert.ok(chips.every((text) => CHIPS.includes(text)), `unexpected chip: ${chips.join(",")}`);

  // 被连接引用的版本：删除置灰 + 行内写明是谁在用（提示挂在 .drv-vwrap 里、行下方）
  await dialog.locator('.drv-item[data-driver-kind="oracle"]').click();
  const referenced = dialog.locator(".drv-vwrap", { hasText: "23.5.0.24.07" });
  assert.equal(await referenced.locator("button", { hasText: "删除" }).isDisabled(), true);
  assert.match(await referenced.locator(".drv-row-hint").innerText(), /生产 Oracle、开发 Oracle/);

  // 筛选收窄左栏后，右栏必须跟着走，不能还显示左边看不到的数据源
  await dialog.locator('.drv-filter[data-filter="manual"]').click();
  assert.equal(await dialog.locator(".drv-item").count(), 2);
  assert.match(await dialog.locator(".drv-detail-title").innerText(), /DB2|达梦/);
  await dialog.locator('.drv-filter[data-filter="all"]').click();

  // 需手动获取：不给「下载」，改「获取方式」，进去要讲清为什么 + 期望的驱动名
  await dialog.locator('.drv-item[data-driver-kind="dameng"]').click();
  assert.equal(await dialog.locator(".drv-vaction button", { hasText: "下载" }).count(), 0);
  await dialog.locator(".drv-vaction button", { hasText: "获取方式" }).first().click();
  const manual = await dialog.locator(".drv-manual").innerText();
  assert.match(manual, /为什么不能自动下载/);
  assert.match(manual, /DM8 ODBC DRIVER/);
  await dialog.locator(".drv-back").click();

  // 下载失败留在该行，附重试，且不弹第二个对话框
  await dialog.locator('.drv-item[data-driver-kind="trino"]').click();
  await dialog.locator(".drv-vaction button", { hasText: "下载" }).first().click();
  await page.waitForFunction(() => !!document.querySelector(".drv-row-error"));
  assert.match(await dialog.locator(".drv-row-error").innerText(), /镜像站/);
  assert.equal(await dialog.locator(".drv-row-error button", { hasText: "重试" }).count(), 1);
  assert.equal(await page.locator(".dialog").count(), 1);
  assert.deepEqual(await page.evaluate(() => window.__CALLS__.filter((c) => c[0] === "download")), [["download", "trino", "476"]]);

  // 键盘：点过清单项后 Esc 仍要能关闭（重建清单不能把焦点丢给 body）
  await dialog.locator('.drv-item[data-driver-kind="oracle"]').click();
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector("[role=dialog]"));
  assert.equal(await page.locator("#trigger").evaluate((el) => document.activeElement === el), true);

  // 自定义驱动：单页表单，保存走 IPC
  await page.evaluate(() => { window.custom = window.openCustomDriverDialog(); });
  const custom = page.getByRole("dialog", { name: "自定义驱动", exact: true });
  await custom.waitFor();
  assert.equal(await custom.getByLabel("显示名称").inputValue(), "");
  assert.match(await custom.innerText(), /URL 模板/);
  await custom.getByLabel("显示名称").fill("测试 Oracle");
  await custom.getByLabel("驱动目标").fill("C:\\drivers\\ojdbc11.jar");
  await custom.getByRole("button", { name: "保存驱动" }).click();
  await page.waitForFunction(() => window.__CALLS__.some((c) => c[0] === "save"));
  assert.deepEqual(await page.evaluate(() => window.__CALLS__.filter((c) => c[0] === "save")), [["save", "测试 Oracle"]]);

  // 布局：任何面板都不允许横向溢出
  const overflow = await page.evaluate(() =>
    [...document.querySelectorAll(".drv-shell, .drv-toolbar, .drv-list, .drv-detail, .drv-table, .drv-vrow, .drv-custom-form")]
      .filter((el) => el.scrollWidth > el.clientWidth + 1)
      .map((el) => `${el.className} ${el.scrollWidth}>${el.clientWidth}`));
  assert.deepEqual(overflow, []);

  assert.deepEqual(errors, []);
  console.log("PASS: 状态词唯一、被引用版本禁删、筛选联动、需手动不给下载、失败留在行内、Esc 焦点、自定义驱动保存、无横向溢出");
} finally {
  await browser.close();
}
