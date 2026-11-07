/** 设置对话框：常规、字体、SQL 编辑器、WebDAV（数据源同步）。左侧选项卡 + 右侧配置。 */

import type { AppSettings } from "../core/types";
import type { DatasourceSync } from "../core/datasourceSync";
import { checkbox, Dialog, field, numberInput, select } from "../ui/Dialog";
import { createDatasourceSyncTab, type DatasourceSyncTab } from "./DatasourceSyncTab";
import { ICON_APP_LOGO, iconSvgByName, svg } from "../ui/icons";

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

export function openSettingsDialog(
  current: AppSettings,
  onSave: (s: AppSettings) => void,
  datasourceSync?: DatasourceSync,
  initialTab: SettingsTabKey = "general",
): Dialog {
  const fontInput = select(
    FONT_FAMILIES.map((f) => ({ value: f, label: f })),
    current.font_family,
    180,
  );
  const fontSize = numberInput(current.font_size, 60);
  const gridFontSize = numberInput(current.grid_font_size, 60);
  const rowsPerPage = numberInput(current.rows_per_page, 90);
  const timeout = numberInput(current.query_timeout_secs, 90);
  const sqlFont = select(
    SQL_FONT_FAMILIES.map((f) => ({ value: f, label: f })),
    current.sql_font_family,
    180,
  );
  const sqlFontSize = numberInput(current.sql_font_size, 60);
  const maxRecent = numberInput(current.max_recent, 90);
  const autoComplete = checkbox("启用自动补全", current.auto_complete);
  const confirmClose = checkbox("关闭前确认", current.confirm_on_close);

  let syncTab: DatasourceSyncTab | null = null;

  const dlg = new Dialog({
    title: "设置",
    width: 640,
    render: (body) => {
      const tabs = document.createElement("div");
      tabs.className = "tabs";
      const header = document.createElement("div");
      header.className = "tabs-header";
      header.setAttribute("role", "tablist");
      const tabsBody = document.createElement("div");
      tabsBody.className = "tabs-body";

      const pages = new Map<SettingsTabKey, { tab: HTMLElement; panel: HTMLElement }>();
      const addTab = (key: SettingsTabKey, label: string, panel: HTMLElement) => {
        const tab = document.createElement("div");
        tab.className = "tabs-tab";
        tab.setAttribute("role", "tab");
        tab.textContent = label;
        tab.addEventListener("click", () => switchTo(key));
        panel.style.display = "none";
        header.appendChild(tab);
        tabsBody.appendChild(panel);
        pages.set(key, { tab, panel });
      };
      const switchTo = (key: SettingsTabKey) => {
        const page = pages.get(key);
        if (!page) return;
        for (const { tab, panel } of pages.values()) {
          tab.classList.remove("active");
          panel.style.display = "none";
        }
        page.tab.classList.add("active");
        page.panel.style.display = "";
      };

      // 常规面板
      const generalPanel = document.createElement("div");
      generalPanel.appendChild(field("每页行数:", rowsPerPage));
      generalPanel.appendChild(field("查询超时（秒）:", timeout));
      generalPanel.appendChild(field("最近文件数:", maxRecent));
      const switches = document.createElement("div");
      switches.className = "check-stack";
      switches.appendChild(autoComplete);
      switches.appendChild(confirmClose);
      generalPanel.appendChild(field("", switches));

      // 字体面板
      const fontPanel = document.createElement("div");
      fontPanel.appendChild(field("界面字体:", fontInput));
      fontPanel.appendChild(field("界面字号:", fontSize));
      fontPanel.appendChild(field("网格字号:", gridFontSize));

      // SQL 面板
      const sqlPanel = document.createElement("div");
      sqlPanel.appendChild(field("编辑器字体:", sqlFont));
      sqlPanel.appendChild(field("编辑器字号:", sqlFontSize));

      // 数据源同步面板（内容在对话框构造完成后注入）
      const syncPanel = document.createElement("div");
      syncPanel.className = "settings-sync-panel";

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
          if (syncTab) await syncTab.saveIfNeeded();
          onSave({
            ...current,
            font_family: fontInput.value,
            font_size: Number(fontSize.value) || 12,
            grid_font_size: Number(gridFontSize.value) || 12,
            rows_per_page: Number(rowsPerPage.value) || 1000,
            query_timeout_secs: Number(timeout.value) || 30,
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
  dlg.onClose(() => syncTab?.dispose());
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
