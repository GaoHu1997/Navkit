// 导出驱动管理界面的实际渲染截图（独立运行，不依赖 vite / 真实后端）。
// 用法：node tools/tests/driver-manager-shots.mjs [输出目录]
import { readFile, access, mkdir } from "node:fs/promises";
import { build } from "esbuild";
import { chromium } from "playwright-core";

const outDir = process.argv[2] || "artifacts";
const executablePath = process.env.BROWSER_EXE || "C:/Program Files/Google/Chrome/Application/chrome.exe";
await access(executablePath);
await mkdir(outDir, { recursive: true });

const bundle = await build({
  stdin: {
    contents: `
      import { openDriverManager, openCustomDriverDialog } from "./src/dialogs/DriverManagerDialog";
      window.openDriverManager = openDriverManager;
      window.openCustomDriverDialog = openCustomDriverDialog;
    `,
    resolveDir: process.cwd(),
    sourcefile: "driver-manager-shots-entry.ts",
  },
  bundle: true,
  format: "iife",
  write: false,
});

const browser = await chromium.launch({ executablePath, headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1180, height: 800 }, deviceScaleFactor: 2 });
  await page.route("http://localhost/**", (route) =>
    route.fulfill({ contentType: "text/html", body: '<!doctype html><html><head><meta charset="utf-8"></head><body style="margin:0;background:#eef1f5;height:100vh"></body></html>' }));
  await page.goto("http://localhost/");
  await page.addStyleTag({ content: await readFile("src/styles/base.css", "utf8") });
  await page.addScriptTag({ content: bundle.outputFiles[0].text });

  await page.evaluate(() => {
    const REF = (kind, version) => "@navkit-driver-version:" + JSON.stringify({ v: 1, kind, version, extra: "" });
    const ok = (row) => ({ architecture: "x86_64", library: null, download_path: null, message: "", ...row });
    const base = { group: "", read_only: false, keep_alive: false, user: "", password: "", database: "", file: "", host: "", port: 0 };
    window.__PREVIEW_MOCK__ = {
      "plugin:event|listen": () => 1,
      "plugin:event|unlisten": () => null,
      list_connections: () => [
        { ...base, id: "ora_prod", name: "生产 Oracle", kind: "oracle", params: REF("oracle", "23.5.0.24.07") },
        { ...base, id: "ora_dev", name: "开发 Oracle", kind: "oracle", params: REF("oracle", "23.5.0.24.07") },
        { ...base, id: "ora_rep", name: "旧版报表", kind: "oracle", params: REF("oracle", "21.11.0.0") },
        { ...base, id: "tri", name: "数据湖 Trino", kind: "trino", params: REF("trino", "476") },
      ],
      list_driver_statuses: () => [
        ok({ kind: "oracle", version: "23.5.0.24.07", status: "ready", download_path: "C:\\Users\\me\\AppData\\Roaming\\NavKit\\drivers\\ojdbc11-23.5.0.24.07.jar" }),
        ok({ kind: "oracle", version: "21.11.0.0", status: "ready" }),
        ok({ kind: "oracle", version: "19.22.0.0", status: "missing", can_download: true }),
        ok({ kind: "mysql", version: "", status: "ready" }),
        ok({ kind: "trino", version: "476", status: "missing", can_download: true, library: "trino-jdbc" }),
        ok({ kind: "clickhouse", version: "0.6.5", status: "ready" }),
        ok({ kind: "dameng", version: "", status: "manual", message: "达梦 DM8 需要系统安装的 ODBC 驱动（DM8 ODBC DRIVER）。" }),
        ok({ kind: "db2", version: "", status: "manual", message: "DB2 需要先安装 IBM Data Server Client。" }),
      ],
      list_odbc_inventory: () => ({
        architecture: "x86_64",
        drivers: [{ name: "ODBC Driver 17 for SQL Server", library: "" }, { name: "PostgreSQL Unicode", library: "" }],
        dsns: [],
      }),
      // trino 用来演示「下载失败留在行内」；其它数据源返回永不 resolve 的 Promise，用来演示「下载中」。
      download_database_driver: (args) => {
        if (args.kind === "trino") throw new Error("下载失败：无法连接镜像站（HTTP 502）");
        return new Promise(() => {});
      },
      list_custom_drivers: () => [],
      save_custom_driver: (args) => [args.driver],
      delete_custom_driver: () => [],
    };
    window.manager = window.openDriverManager();
  });

  const dialog = page.getByRole("dialog", { name: "驱动管理", exact: true });
  await dialog.waitFor();
  await page.waitForFunction(() => document.querySelectorAll(".drv-item").length > 0);
  const shot = async (name) => {
    await page.waitForTimeout(250);
    await dialog.screenshot({ path: `${outDir}/${name}.png` });
    console.log("shot", name);
  };

  await dialog.locator('.drv-item[data-driver-kind="oracle"]').click();
  await shot("driver-main");

  await dialog.locator('.drv-item[data-driver-kind="dameng"]').click();
  await dialog.locator(".drv-vaction button", { hasText: "获取方式" }).first().click();
  await shot("driver-manual");

  await dialog.locator(".drv-back").click();
  await dialog.locator('.drv-item[data-driver-kind="trino"]').click();
  await dialog.locator(".drv-vaction button", { hasText: "下载" }).first().click();
  await page.waitForFunction(() => !!document.querySelector(".drv-row-error"));
  await shot("driver-error");

  await page.keyboard.press("Escape");
  await page.evaluate(() => { window.custom = window.openCustomDriverDialog(); });
  const custom = page.getByRole("dialog", { name: "自定义驱动", exact: true });
  await custom.waitFor();
  await page.waitForTimeout(250);
  await custom.screenshot({ path: `${outDir}/driver-custom.png` });
  console.log("shot driver-custom");
} finally {
  await browser.close();
}
