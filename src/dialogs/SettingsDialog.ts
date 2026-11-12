/** 设置对话框：常规、字体、SQL 编辑器、WebDAV（数据源同步）。左侧选项卡 + 右侧配置。 */

import type { AppSettings } from "../core/types";
import type { DatasourceSync } from "../core/datasourceSync";
import { checkbox, Dialog, field, numberInput, select } from "../ui/Dialog";
import { createDatasourceSyncTab, type DatasourceSyncTab } from "./DatasourceSyncTab";
import { ICON_APP_LOGO, iconSvgByName, svg } from "../ui/icons";
import { accessibleToolDialog } from "./toolDialog";
import "../styles/settings-dialog.css";

const FONT_FAMILIES = [
  "Microsoft YaHei UI",
  "Microsoft YaHei",
  "Segoe UI",
  "Tahoma",
  "SimSun",
  "SimHei",
  "KaiTi",
  "Arial",
  "Consolas",
  "Courier New",
];

const SQL_FONT_FAMILIES = [
  "Consolas",
  "Courier New",
  "Lucida Console",
  "Microsoft YaHei UI",
  "SimSun",
];

export type SettingsTabKey = "general" | "font" | "sql" | "sync";

/** 沿用 field 原语，将标签、说明与控件分别放在两列。 */
function settingsField(
  prefix: string,
  key: string,
  label: string,
  help: string,
  control: HTMLElement,
  unit?: string,
): HTMLElement {
  const input = control.matches("input, select, textarea")
    ? control
    : control.querySelector<HTMLInputElement>("input")!;
  input.id = `${prefix}-${key}`;
  input.dataset.settingsField = key;
  const row = field("", control);
  row.classList.add("settings-field");
  const copy = row.querySelector<HTMLElement>(".field-label")!;
  copy.classList.add("settings-field-copy");
  const name = document.createElement("label");
  name.className = "settings-field-label";
  name.htmlFor = input.id;
  name.textContent = label;
  const hint = document.createElement("p");
  hint.className = "settings-field-help";
  hint.id = `${input.id}-help`;
  hint.textContent = help;
  input.setAttribute("aria-describedby", hint.id);
  copy.append(name, hint);
  const value = row.querySelector<HTMLElement>(".field-control")!;
  value.classList.add("settings-field-value");
  if (unit) {
    const suffix = document.createElement("span");
    suffix.className = "settings-unit";
    suffix.textContent = unit;
    value.appendChild(suffix);
  }
  if (control.classList.contains("checkbox")) {
    const checkboxInput = input as HTMLInputElement;
    const state = control.querySelector<HTMLElement>("span")!;
    state.setAttribute("aria-hidden", "true");
    const update = () => { state.textContent = checkboxInput.checked ? "已启用" : "已关闭"; };
    checkboxInput.addEventListener("change", update);
    update();
  }
  return row;
}

function settingsSection(title: string, ...rows: HTMLElement[]): HTMLElement {
  const section = document.createElement("section");
  section.className = "settings-section";
  const heading = document.createElement("h3");
  heading.className = "settings-section-title";
  heading.textContent = title;
  section.append(heading, ...rows);
  return section;
}

function settingsPanel(title: string, description: string): HTMLElement {
  const panel = document.createElement("section");
  panel.className = "settings-panel";
  const heading = document.createElement("header");
  heading.className = "settings-panel-heading";
  const name = document.createElement("h2");
  name.className = "settings-panel-title";
  name.textContent = title;
  const hint = document.createElement("p");
  hint.className = "settings-panel-description";
  hint.textContent = description;
  heading.append(name, hint);
  panel.appendChild(heading);
  return panel;
}

function settingsPreview(title: string, sample: HTMLElement): HTMLElement {
  const preview = document.createElement("section");
  preview.className = "settings-preview";
  const heading = document.createElement("h3");
  heading.className = "settings-preview-title";
  heading.textContent = title;
  preview.append(heading, sample);
  return preview;
}

export function openSettingsDialog(
  current: AppSettings,
  onSave: (s: AppSettings) => void | Promise<void>,
  datasourceSync?: DatasourceSync,
  initialTab: SettingsTabKey = "general",
): Dialog {
  const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const prefix = `settings-${crypto.randomUUID()}`;
  const fontInput = select(
    FONT_FAMILIES.map((f) => ({ value: f, label: f })),
    current.font_family,
  );
  const fontSize = numberInput(current.font_size);
  const gridFontSize = numberInput(current.grid_font_size);
  const rowsPerPage = numberInput(current.rows_per_page);
  const timeout = numberInput(current.query_timeout_secs);
  timeout.min = "0"; timeout.max = "86400"; timeout.step = "1";
  const connectTimeout = numberInput(current.connect_timeout_secs ?? 30);
  const acquireTimeout = numberInput(current.pool_acquire_timeout_secs ?? 30);
  const busyTimeout = numberInput(current.sqlite_busy_timeout_secs ?? 10);
  for (const field of [connectTimeout, acquireTimeout, busyTimeout]) { field.min = field === busyTimeout ? "0" : "1"; field.max = "86400"; field.step = "1"; }
  const sqlFont = select(
    SQL_FONT_FAMILIES.map((f) => ({ value: f, label: f })),
    current.sql_font_family,
  );
  const sqlFontSize = numberInput(current.sql_font_size);
  const maxRecent = numberInput(current.max_recent);
  const autoComplete = checkbox("", current.auto_complete);
  const confirmClose = checkbox("", current.confirm_on_close);

  let syncTab: DatasourceSyncTab | null = null;
  let activeTab: HTMLButtonElement | null = null;

  const dlg = new Dialog({
    title: "设置",
    width: 840,
    render: (body) => {
      const tabs = document.createElement("div");
      tabs.className = "tabs settings-layout";
      const header = document.createElement("div");
      header.className = "tabs-header settings-sidebar";
      header.setAttribute("role", "tablist");
      header.setAttribute("aria-label", "设置分类");
      header.setAttribute("aria-orientation", "vertical");
      const tabsBody = document.createElement("div");
      tabsBody.className = "tabs-body settings-content";

      const pages = new Map<SettingsTabKey, { tab: HTMLButtonElement; panel: HTMLElement }>();
      const addTab = (key: SettingsTabKey, label: string, panel: HTMLElement) => {
        const tab = document.createElement("button");
        tab.type = "button";
        tab.className = "tabs-tab settings-tab";
        tab.id = `${prefix}-tab-${key}`;
        tab.dataset.settingsTab = key;
        tab.setAttribute("role", "tab");
        tab.setAttribute("aria-controls", `${prefix}-panel-${key}`);
        tab.textContent = label;
        tab.addEventListener("click", () => switchTo(key));
        tab.addEventListener("keydown", (event) => {
          const keys = [...pages.keys()];
          let index = keys.indexOf(key);
          if (event.key === "ArrowDown" || event.key === "ArrowRight") index = (index + 1) % keys.length;
          else if (event.key === "ArrowUp" || event.key === "ArrowLeft") index = (index - 1 + keys.length) % keys.length;
          else if (event.key === "Home") index = 0;
          else if (event.key === "End") index = keys.length - 1;
          else return;
          event.preventDefault();
          switchTo(keys[index]);
          pages.get(keys[index])?.tab.focus();
        });
        panel.id = `${prefix}-panel-${key}`;
        panel.dataset.settingsPanel = key;
        panel.setAttribute("role", "tabpanel");
        panel.setAttribute("aria-labelledby", tab.id);
        panel.tabIndex = 0;
        panel.hidden = true;
        header.appendChild(tab);
        tabsBody.appendChild(panel);
        pages.set(key, { tab, panel });
      };
      const switchTo = (key: SettingsTabKey) => {
        const page = pages.get(key);
        if (!page) return;
        for (const { tab, panel } of pages.values()) {
          const selected = tab === page.tab;
          tab.classList.toggle("active", selected);
          tab.setAttribute("aria-selected", String(selected));
          tab.tabIndex = selected ? 0 : -1;
          panel.hidden = !selected;
        }
        tabsBody.scrollTop = 0;
        activeTab = page.tab;
      };

      const row = (key: string, label: string, help: string, control: HTMLElement, unit?: string) =>
        settingsField(prefix, key, label, help, control, unit);
      const generalPanel = settingsPanel("常规设置", "设置查询、结果显示与日常操作的默认行为。");
      generalPanel.append(
        settingsSection("查询与结果",
          row("rows_per_page", "每页行数", "结果网格每页显示的默认行数。", rowsPerPage, "行"),
          row("query_timeout_secs", "查询超时", "0 禁用应用执行预算；覆盖整个批次及结果读取。支持范围由驱动能力决定，服务器自身限制仍保留。", timeout, "秒"),
          row("connect_timeout_secs", "连接超时", "连接握手预算，独立于查询执行。", connectTimeout, "秒"),
          row("pool_acquire_timeout_secs", "连接池等待", "等待空闲物理连接的预算。", acquireTimeout, "秒"),
          row("sqlite_busy_timeout_secs", "SQLite 锁等待", "等待数据库锁的预算；0 不等待。", busyTimeout, "秒")),
        settingsSection("使用偏好",
          row("max_recent", "最近文件数", "最近文件列表保留的数量。", maxRecent, "个"),
          row("auto_complete", "启用自动补全", "在编辑 SQL 时提供补全建议。", autoComplete),
          row("confirm_on_close", "关闭前确认", "保留关闭时的确认提示。", confirmClose)),
      );

      const fontPanel = settingsPanel("字体", "统一界面与结果网格的文字显示。");
      fontPanel.append(
        settingsSection("界面显示",
          row("font_family", "界面字体", "菜单、对话框与工具栏使用的字体。", fontInput),
          row("font_size", "界面字号", "界面文字的默认大小。", fontSize, "px")),
        settingsSection("结果网格",
          row("grid_font_size", "网格字号", "结果网格文字的默认大小。", gridFontSize, "px")),
      );
      const interfaceSample = document.createElement("p");
      interfaceSample.className = "settings-font-sample";
      interfaceSample.dataset.settingsPreview = "interface";
      interfaceSample.textContent = "连接 · 查询结果 · 用户管理 · NavKit 0123456789";
      fontPanel.appendChild(settingsPreview("界面字体预览 · 只读示例", interfaceSample));
      const gridSample = document.createElement("table");
      gridSample.className = "settings-font-grid";
      gridSample.dataset.settingsPreview = "grid";
      gridSample.setAttribute("aria-label", "网格字号预览");
      const gridHead = document.createElement("thead");
      const gridHeadRow = document.createElement("tr");
      for (const label of ["id", "username", "created_at"]) {
        const cell = document.createElement("th");
        cell.scope = "col";
        cell.textContent = label;
        gridHeadRow.appendChild(cell);
      }
      gridHead.appendChild(gridHeadRow);
      const gridBody = document.createElement("tbody");
      const gridRow = document.createElement("tr");
      for (const text of ["1001", "NavKit 用户", "2026-11-08 12:41:09"]) {
        const cell = document.createElement("td");
        cell.textContent = text;
        gridRow.appendChild(cell);
      }
      gridBody.appendChild(gridRow);
      gridSample.append(gridHead, gridBody);
      const gridWrap = document.createElement("div");
      gridWrap.className = "settings-grid-preview";
      gridWrap.appendChild(gridSample);
      fontPanel.appendChild(gridWrap);

      const sqlPanel = settingsPanel("SQL 编辑器", "设置编辑器字体与字号。");
      sqlPanel.appendChild(settingsSection("编辑器显示",
        row("sql_font_family", "编辑器字体", "SQL 正文使用的字体。", sqlFont),
        row("sql_font_size", "编辑器字号", "SQL 正文文字的默认大小。", sqlFontSize, "px")));
      const sqlSample = document.createElement("pre");
      sqlSample.className = "settings-sql-sample";
      sqlSample.dataset.settingsPreview = "sql";
      sqlSample.innerHTML = '<span class="settings-sql-keyword">SELECT</span> id, username, created_at\n'
        + '<span class="settings-sql-keyword">FROM</span> t_user\n'
        + '<span class="settings-sql-keyword">WHERE</span> enabled = 1\n'
        + '<span class="settings-sql-keyword">ORDER BY</span> created_at <span class="settings-sql-keyword">DESC</span>\n'
        + '<span class="settings-sql-keyword">LIMIT</span> 1000;';
      sqlPanel.appendChild(settingsPreview("SQL 字体预览 · 只读示例", sqlSample));
      const updatePreview = () => {
        interfaceSample.style.fontFamily = fontInput.value;
        interfaceSample.style.fontSize = `${Number(fontSize.value) || 12}px`;
        gridSample.style.fontFamily = fontInput.value;
        gridSample.style.fontSize = `${Number(gridFontSize.value) || 12}px`;
        sqlSample.style.fontFamily = sqlFont.value;
        sqlSample.style.fontSize = `${Number(sqlFontSize.value) || 13}px`;
      };
      for (const control of [fontInput, fontSize, gridFontSize, sqlFont, sqlFontSize]) {
        control.addEventListener("input", updatePreview);
        control.addEventListener("change", updatePreview);
      }
      updatePreview();

      // 数据源同步面板（内容在对话框构造完成后注入）
      const syncPanel = settingsPanel("WebDAV", "配置数据源同步连接、安全信息与频率。");
      syncPanel.classList.add("settings-sync-panel");

      addTab("general", "常规", generalPanel);
      addTab("font", "字体", fontPanel);
      addTab("sql", "SQL 编辑器", sqlPanel);
      if (datasourceSync) addTab("sync", "WebDAV", syncPanel);

      tabs.appendChild(header);
      tabs.appendChild(tabsBody);
      body.appendChild(tabs);
      switchTo(pages.has(initialTab) ? initialTab : "general");
    },
    buttons: [
      {
        label: "确定",
        primary: true,
        onClick: async () => {
          // 连接信息先于设置保存：失败时留在对话框内，由 Dialog 统一提示。
          for (const field of [timeout, connectTimeout, acquireTimeout, busyTimeout]) {
            const value = Number(field.value);
            const valid = field.value.trim() !== "" && Number.isInteger(value) && value >= Number(field.min) && value <= 86400;
            field.setAttribute("aria-invalid", String(!valid));
            if (!valid) { field.focus(); throw new Error("超时字段必须填写范围内的整数秒（查询/SQLite 锁等待允许 0）"); }
          }
          if (syncTab) await syncTab.saveIfNeeded();
          await onSave({
            ...current,
            font_family: fontInput.value,
            font_size: Number(fontSize.value) || 12,
            grid_font_size: Number(gridFontSize.value) || 12,
            rows_per_page: Number(rowsPerPage.value) || 1000,
            query_timeout_secs: Number(timeout.value),
            connect_timeout_secs: Number(connectTimeout.value),
            pool_acquire_timeout_secs: Number(acquireTimeout.value),
            sqlite_busy_timeout_secs: Number(busyTimeout.value),
            theme: current.theme,
            max_recent: Number(maxRecent.value) || 10,
            auto_complete: (autoComplete.querySelector("input") as HTMLInputElement).checked,
            confirm_on_close: (confirmClose.querySelector("input") as HTMLInputElement).checked,
            show_system_tray: current.show_system_tray,
            sql_font_family: sqlFont.value,
            sql_font_size: Number(sqlFontSize.value) || 13,
          });
        },
      },
      { label: "取消" },
    ],
  });

  dlg.el.classList.add("settings-dialog");
  if (datasourceSync) {
    syncTab = createDatasourceSyncTab(datasourceSync);
    dlg.body.querySelector<HTMLElement>(".settings-sync-panel")?.appendChild(syncTab.el);
  }
  accessibleToolDialog(dlg, "设置", () => syncTab?.dispose(), returnFocus);
  // 滚动由右侧内容区承担，保留辅助原语的 Escape、焦点约束与归还行为。
  dlg.body.style.overflow = "hidden";
  const showError = dlg.showError.bind(dlg);
  dlg.showError = (message) => {
    showError(message);
    const error = dlg.body.querySelector<HTMLElement>(".dialog-error");
    if (!error) return;
    error.id = `${prefix}-error`;
    error.setAttribute("role", "alert");
    for (const input of dlg.body.querySelectorAll<HTMLElement>('[aria-invalid="true"]')) {
      const descriptions = (input.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
      input.setAttribute("aria-describedby", [...new Set([...descriptions, error.id])].join(" "));
    }
  };
  queueMicrotask(() => { if (dlg.el.isConnected) activeTab?.focus(); });
  return dlg;
}

/** 关于对话框。 */
export function openAboutDialog(info: Record<string, string>): Dialog {
  return new Dialog({
    title: "关于 NavKit",
    width: 380,
    render: (body) => {
      const wrap = document.createElement("div");
      wrap.style.textAlign = "center";
      const img = document.createElement("div");
      img.style.lineHeight = "0";
      img.innerHTML = svg(ICON_APP_LOGO, 48);
      wrap.appendChild(img);

      const title = document.createElement("div");
      title.style.fontSize = "14px";
      title.style.fontWeight = "bold";
      title.style.margin = "8px 0 4px";
      title.textContent = info.product ?? "NavKit";
      wrap.appendChild(title);

      const ver = document.createElement("div");
      ver.style.marginBottom = "10px";
      ver.textContent = `版本 ${info.version ?? ""}`;
      wrap.appendChild(ver);

      const table = document.createElement("table");
      table.className = "mini-table";
      table.style.textAlign = "left";
      const rows: [string, string][] = [
        ["版权所有", info.copyright ?? ""],
        ["发行商", info.publisher ?? ""],
        ["网站", info.url ?? ""],
        ["支持", info.support ?? ""],
        ["数据目录", info.dataDir ?? ""],
      ];
      for (const [k, v] of rows) {
        const tr = document.createElement("tr");
        const th = document.createElement("th");
        th.style.width = "90px";
        th.textContent = k;
        const td = document.createElement("td");
        td.textContent = v;
        tr.appendChild(th);
        tr.appendChild(td);
        table.appendChild(tr);
      }
      wrap.appendChild(table);
      body.appendChild(wrap);
    },
    buttons: [{ label: "确定", primary: true }],
  });
}

/** 服务器信息对话框。 */
export function openServerInfoDialog(
  info: {
    product: string;
    version: string;
    host: string;
    port: number;
    charset: string;
    collation: string;
    uptime: string | null;
    extra: [string, string][];
  },
): Dialog {
  return new Dialog({
    title: "服务器信息",
    width: 460,
    render: (body) => {
      const table = document.createElement("table");
      table.className = "mini-table";
      const rows: [string, string][] = [
        ["产品", info.product],
        ["版本", info.version],
        ["主机", info.host],
        ["端口", String(info.port)],
        ["字符集", info.charset],
        ["排序规则", info.collation],
      ];
      if (info.uptime) rows.push(["运行时间", info.uptime]);
      for (const [k, v] of info.extra) {
        if (rows.some((r) => r[0] === k)) continue;
        rows.push([k, v]);
      }
      for (const [k, v] of rows) {
        const tr = document.createElement("tr");
        const th = document.createElement("th");
        th.style.width = "140px";
        th.textContent = k;
        const td = document.createElement("td");
        td.textContent = v;
        td.style.wordBreak = "break-all";
        tr.appendChild(th);
        tr.appendChild(td);
        table.appendChild(tr);
      }
      body.appendChild(table);
    },
    buttons: [{ label: "确定", primary: true }],
  });
}

/** 收藏夹管理对话框。 */
export function openFavoritesDialog(
  favorites: { id: string; name: string; sql: string }[],
  onDelete: (id: string) => void,
  onOpen: (id: string) => void,
): Dialog {
  const list = document.createElement("div");
  list.style.maxHeight = "280px";
  list.style.overflow = "auto";
  list.style.border = "1px solid var(--line)";
  list.style.background = "#fff";

  function refresh(): void {
    list.innerHTML = "";
    if (!favorites.length) {
      const empty = document.createElement("div");
      empty.style.padding = "16px";
      empty.style.color = "var(--fg-muted)";
      empty.textContent = "暂无收藏";
      list.appendChild(empty);
      return;
    }
    for (const f of favorites) {
      const row = document.createElement("div");
      row.style.display = "flex";
      row.style.alignItems = "center";
      row.style.borderBottom = "1px solid #eee";
      row.style.padding = "3px 6px";
      row.style.cursor = "default";
      row.onmouseenter = () => (row.style.background = "var(--bg-hover)");
      row.onmouseleave = () => (row.style.background = "");
      row.ondblclick = () => onOpen(f.id);

      const icon = document.createElement("span");
      icon.style.lineHeight = "0";
      icon.style.display = "inline-flex";
      icon.style.marginRight = "6px";
      icon.innerHTML = iconSvgByName("object_star");

      const name = document.createElement("div");
      name.style.flex = "1";
      name.textContent = f.name;

      const del = document.createElement("button");
      del.className = "btn";
      del.style.minWidth = "0";
      del.style.height = "20px";
      del.style.padding = "0 8px";
      del.textContent = "删除";
      del.addEventListener("click", () => {
        onDelete(f.id);
      });

      row.appendChild(icon);
      row.appendChild(name);
      row.appendChild(del);
      list.appendChild(row);
    }
  }
  refresh();

  return new Dialog({
    title: "管理收藏夹",
    width: 460,
    render: (body) => {
      body.appendChild(list);
    },
    buttons: [{ label: "关闭", primary: true }],
  });
}
