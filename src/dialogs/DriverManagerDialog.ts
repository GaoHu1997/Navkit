/**
 * 驱动管理 —— 一页回答一个问题：「这个数据源现在能不能连？不能连的话怎么补上？」
 *
 * 结构（见 docs/design/shots/37-驱动管理-主界面.png）：
 *   顶部工具条：搜索 + 状态筛选（带计数）+ 当前架构 + 重新检测
 *   左侧清单：按分类分组的数据源，行尾是它当前的状态
 *   右侧详情：选中数据源的版本表，分「已下载 / 可下载 / 需手动获取」三节，
 *             下载、删除、进度、失败都贴在该版本行上
 *   页脚：左侧「添加自定义驱动…」（次级入口），右侧「关闭」
 *
 * 三条硬规则：
 *   1. 状态词只有 已下载 / 未下载 / 部分就绪 / 需手动获取 / 下载中 / 下载失败；
 *   2. 操作贴着对象，不设全局提示行，也不让用户点了才看见错误；
 *   3. 后端没有的能力（直链、下载进度百分比）不假装有。
 */
import { open as openFile } from "@tauri-apps/plugin-dialog";
import {
  api,
  driverApi,
  parseVersionSelection,
  type DatabaseDriverStatus,
  type DriverDefinition,
  type DriverProtocol,
  type DriverSource,
  type OdbcInventory,
} from "../core/api";
import { ensureDriverStatuses, refreshDriverStatuses, subscribeDriverStatuses } from "../core/driverStatus";
import { DB_CATEGORIES, kindIcon, kindLabel, kindMeta } from "../core/dbcatalog";
import type { ConnectionConfig } from "../core/types";
import { Dialog, numberInput, passwordInput, textInput } from "../ui/Dialog";
import { connectionField, newConnectionId, prepareConnectionDialog } from "./ConnectionDialog";
import {
  ICON_COPY,
  ICON_CROSS,
  ICON_DOWNLOAD,
  ICON_EXTERNAL_LINK,
  ICON_PLUS_LINE,
  ICON_REFRESH,
  ICON_SEARCH_16,
  ICON_SPINNER,
  iconSvgByName,
  svg,
} from "../ui/icons";

const DEFAULT_TEMPLATE = "DRIVER={{driver}};SERVER={{host}};PORT={{port}};DATABASE={{database}};UID={{user}};PWD={{password}}";
const DEFAULT_JDBC_URL = "jdbc:mysql://{{host}}:{{port}}/{{database}}";

/** 数据源级状态：由它下面所有版本聚合而来，和版本级状态用同一套词。 */
type KindState = "ready" | "partial" | "missing" | "manual" | "downloading" | "failed";

const STATE_TEXT: Record<KindState, string> = {
  ready: "已下载",
  partial: "部分就绪",
  missing: "未下载",
  manual: "需手动获取",
  downloading: "下载中",
  failed: "下载失败",
};

const STATE_TONE: Record<KindState, string> = {
  ready: "ok",
  partial: "accent",
  missing: "muted",
  manual: "warn",
  downloading: "accent",
  failed: "danger",
};

const versionKey = (row: { kind: string; version: string }): string => `${row.kind}@${row.version}`;

/** 小工具：建元素。 */
function node<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (cls) element.className = cls;
  if (text) element.textContent = text;
  return element;
}

function iconOf(name: string, size = 13): string {
  return iconSvgByName(name) || svg(ICON_DOWNLOAD, size);
}

/** 状态徽标：全应用统一，一处定义。 */
function stateChip(state: KindState | string, text?: string): HTMLElement {
  const chip = node("span", `drv-chip drv-chip-${STATE_TONE[state as KindState] ?? "muted"}`);
  chip.textContent = text ?? STATE_TEXT[state as KindState] ?? state;
  return chip;
}

// ---------------------------------------------------------------------------
// 兼容导出（「新建连接」等处历史上按分组/标签读取驱动状态）
// ---------------------------------------------------------------------------

export interface ManagedGroup { kind: string; rows: DatabaseDriverStatus[] }

/** 把按版本展开的状态行按数据源分组（内置协议/不支持/jdbcx 不展示）。 */
export function groupManagedStatuses(statuses: DatabaseDriverStatus[]): ManagedGroup[] {
  const groups = new Map<string, DatabaseDriverStatus[]>();
  for (const item of statuses) {
    if (item.status === "builtin" || item.status === "unsupported" || item.kind === "jdbcx") continue;
    const rows = groups.get(item.kind) ?? [];
    rows.push(item);
    groups.set(item.kind, rows);
  }
  return [...groups.entries()].map(([kind, rows]) => ({ kind, rows }));
}

export function managedStatusLabel(status: DatabaseDriverStatus): string {
  // 状态词只此一份：同一个状态在左栏、详情、别处都得叫同一个名字。
  switch (status.status) {
    case "ready": return STATE_TEXT.ready;
    case "missing": return STATE_TEXT.missing;
    case "manual": return STATE_TEXT.manual;
    default: return status.status;
  }
}

/** 版本行上的动作状态（下载中 / 刚失败），由对话框维护。 */
interface RowActivity { busy: "download" | "delete" | ""; error: string }

/** 引用该版本的连接名，用于「谁在用 / 为什么不能删」。 */
interface DriverRefs {
  byVersion: Map<string, string[]>;
  byKind: Map<string, string[]>;
}

function collectRefs(connections: ConnectionConfig[]): DriverRefs {
  const byVersion = new Map<string, string[]>();
  const byKind = new Map<string, string[]>();
  for (const connection of connections) {
    const selection = parseVersionSelection(connection.params ?? "");
    if (!selection) continue;
    const key = versionKey(selection);
    const versionHits = byVersion.get(key) ?? [];
    versionHits.push(connection.name);
    byVersion.set(key, versionHits);
    const kindHits = byKind.get(selection.kind) ?? [];
    kindHits.push(connection.name);
    byKind.set(selection.kind, kindHits);
  }
  return { byVersion, byKind };
}

function aggregateState(
  rows: DatabaseDriverStatus[],
  activity: Map<string, RowActivity>,
): KindState {
  if (!rows.length) return "missing";
  const ready = rows.filter((row) => row.status === "ready");
  const downloadable = rows.filter((row) => row.status !== "ready" && row.can_download);
  const states = rows.map((row) => activity.get(versionKey(row)));
  if (states.some((entry) => entry?.busy === "download")) return "downloading";
  if (ready.length === rows.length) return "ready";
  if (ready.length > 0) return "partial";
  if (downloadable.length === 0) return "manual";
  if (states.some((entry) => entry?.error)) return "failed";
  return "missing";
}

// ---------------------------------------------------------------------------
// 主对话框
// ---------------------------------------------------------------------------

/** 筛选分组的成员关系：一条规则，避免两处判断漂移。 */
function matchesFilter(state: KindState, filter: string): boolean {
  if (filter === "all") return true;
  if (filter === "ready") return state === "ready";
  // 「缺少」= 现在还不能完整用：未下载 / 部分就绪 / 失败 / 下载中
  if (filter === "missing") return state === "missing" || state === "partial" || state === "failed" || state === "downloading";
  if (filter === "manual") return state === "manual";
  return true;
}

/**
 * 打开驱动管理。
 * @param onChanged 自定义驱动发生变化（供「新建连接」刷新自定义驱动列表）
 * @param onRuntimeChanged 托管驱动文件发生变化（供上层做运行期刷新）
 */
export function openDriverManager(
  onChanged?: (drivers: DriverDefinition[]) => void,
  onRuntimeChanged?: () => void,
): Dialog {
  const previousFocus = document.activeElement as HTMLElement | null;
  let closed = false;

  let statuses: DatabaseDriverStatus[] = [];
  let inventory: OdbcInventory = { drivers: [], dsns: [], architecture: "" };
  let connections: ConnectionConfig[] = [];
  let refs: DriverRefs = collectRefs([]);
  let loadError = "";
  let statusLoading = true;

  let selectedKind = "";
  let filter = "all";
  let query = "";
  let focusKey = "";
  let manualView = false;
  const activity = new Map<string, RowActivity>();

  // —— 骨架 ——
  const toolbar = node("div", "drv-toolbar");
  const search = node("input", "drv-search");
  search.type = "search";
  search.placeholder = "搜索数据源";
  search.setAttribute("aria-label", "搜索数据源");
  const searchWrap = node("div", "drv-search-wrap");
  searchWrap.innerHTML = svg(ICON_SEARCH_16, 13);
  searchWrap.appendChild(search);

  const filters = node("div", "drv-filters");
  const filterButtons = new Map<string, HTMLButtonElement>();
  for (const [id, label] of [["all", "全部"], ["ready", "已就绪"], ["missing", "缺少"], ["manual", "需手动"]] as const) {
    const button = node("button", "drv-filter");
    button.type = "button";
    button.dataset.filter = id;
    button.setAttribute("aria-pressed", "false");
    const text = node("span", "drv-filter-label", label);
    const count = node("span", "drv-filter-count");
    button.append(text, count);
    button.addEventListener("click", () => {
      filter = id;
      manualView = false;
      render();
    });
    filterButtons.set(id, button);
    filters.appendChild(button);
  }

  const spacer = node("div", "drv-toolbar-spacer");
  const arch = node("span", "drv-arch");
  const refresh = node("button", "btn drv-refresh");
  refresh.type = "button";
  refresh.innerHTML = `${svg(ICON_REFRESH, 13)}<span>重新检测</span>`;

  toolbar.append(searchWrap, filters, spacer, arch, refresh);

  const list = node("div", "drv-list");
  list.setAttribute("role", "navigation");
  list.setAttribute("aria-label", "数据源驱动清单");
  const detail = node("div", "drv-detail");
  const body = node("div", "drv-body");
  body.append(list, detail);

  const flush = node("div", "drv-flush");
  flush.setAttribute("role", "status");
  flush.setAttribute("aria-live", "polite");

  // —— 状态加载 ——
  const reloadStatuses = async (force: boolean): Promise<void> => {
    statusLoading = true;
    render();
    try {
      statuses = force ? await refreshDriverStatuses() : await ensureDriverStatuses();
      loadError = "";
    } catch (error) {
      loadError = String(error);
    } finally {
      statusLoading = false;
      if (!closed) render();
    }
  };

  const reloadRefs = async (): Promise<void> => {
    try {
      connections = await api.listConnections();
      refs = collectRefs(connections);
    } catch {
      // 引用信息拿不到时按「没有连接在用」显示，删除失败时后端仍会兜底报错。
      refs = collectRefs([]);
    }
    if (!closed) render();
  };

  // —— 动作 ——
  const download = (row: DatabaseDriverStatus): void => {
    const key = versionKey(row);
    if (activity.get(key)?.busy) return;
    activity.set(key, { busy: "download", error: "" });
    manualView = false;
    focusKey = key;
    render();
    void (async () => {
      try {
        await driverApi.download(row.kind, row.version || undefined);
        statuses = await refreshDriverStatuses();
        onRuntimeChanged?.();
        activity.delete(key);
      } catch (error) {
        activity.set(key, { busy: "", error: error instanceof Error ? error.message : String(error) });
      } finally {
        if (!closed) render();
      }
    })();
  };

  const removeManaged = (row: DatabaseDriverStatus): void => {
    const key = versionKey(row);
    const holders = refs.byVersion.get(key) ?? [];
    if (holders.length) return;
    const label = `${kindLabel(row.kind)}${row.version ? ` ${row.version}` : ""}`;
    void new Dialog({
      title: "删除本地驱动",
      width: 440,
      render: (content) => {
        content.append(
          node("p", "", `删除 ${label} 的本地驱动文件？`),
          node("p", "drv-muted", "删除后可以从同一位置重新下载；已经建立的连接会继续工作，直到下次连接时才需要驱动。"),
          node("p", "drv-muted", `文件位置：${row.download_path || "（后端未返回路径）"}`),
        );
      },
      buttons: [
        {
          label: "删除",
          onClick: async () => {
            activity.set(key, { busy: "delete", error: "" });
            render();
            try {
              await driverApi.deleteManaged(row.kind, row.version);
              statuses = await refreshDriverStatuses();
              onRuntimeChanged?.();
              activity.delete(key);
            } catch (error) {
              activity.set(key, { busy: "", error: String(error) });
            } finally {
              if (!closed) render();
            }
          },
        },
        { label: "取消" },
      ],
    });
  };

  refresh.addEventListener("click", () => {
    if (statusLoading) return;
    void reloadStatuses(true).then(() => void reloadRefs());
  });
  search.addEventListener("input", () => {
    query = search.value.trim().toLowerCase();
    render();
  });

  // —— 渲染：左侧清单 ——
  const renderList = (groups: ManagedGroup[]): void => {
    list.replaceChildren();
    if (loadError) {
      const failure = node("div", "drv-empty");
      failure.append(
        node("div", "drv-empty-title", "检测失败"),
        node("div", "drv-muted", loadError),
      );
      const retry = node("button", "btn", "重试");
      retry.addEventListener("click", () => void reloadStatuses(true));
      failure.appendChild(retry);
      list.appendChild(failure);
      return;
    }
    if (statusLoading && !groups.length) {
      list.appendChild(node("div", "drv-empty", "正在检查本机的驱动…"));
      return;
    }
    const visible = groups.filter((group) => {
      const state = aggregateState(group.rows, activity);
      if (!matchesFilter(state, filter)) return false;
      if (!query) return true;
      return `${kindLabel(group.kind)} ${group.kind}`.toLowerCase().includes(query);
    });    if (!visible.length) {
      const empty = node("div", "drv-empty");
      if (!groups.length) {
        // 全是内置协议的数据源（MySQL/PG/SQLite…）时列表本来就是空的，别让用户以为是加载失败。
        empty.append(
          node("div", "drv-empty-title", "没有需要额外驱动的数据源"),
          node("div", "drv-muted", "当前数据源都由内置协议直连。只有 JDBC / ODBC 类数据源会出现在这里。"),
        );
      } else {
        empty.append(
          node("div", "drv-empty-title", query ? "没有匹配的数据源" : "这个筛选下没有数据源"),
          node("div", "drv-muted", query ? `没有名称包含「${search.value.trim()}」的数据源。` : "换个筛选条件看看。"),
        );
      }
      list.appendChild(empty);
      return;
    }
    // 按分类分组；分类顺序沿用「新建连接」里的顺序，状态变化不会让行跳位置。
    const categorized = new Map<string, { label: string; rows: ManagedGroup[] }>();
    for (const group of visible) {
      const category = kindMeta(group.kind)?.category;
      const meta = DB_CATEGORIES.find((entry) => entry.id === category);
      const key = meta?.id ?? "other";
      if (!categorized.has(key)) categorized.set(key, { label: meta?.label ?? "其它", rows: [] });
      categorized.get(key)!.rows.push(group);
    }
    const order = [...DB_CATEGORIES.map((entry) => entry.id), "other"];
    for (const key of order) {
      const bucket = categorized.get(key);
      if (!bucket) continue;
      const title = node("div", "drv-group-title", bucket.label);
      title.dataset.category = key;
      list.appendChild(title);
      for (const group of bucket.rows) {
        const state = aggregateState(group.rows, activity);
        const item = node("button", "drv-item");
        item.type = "button";
        item.dataset.driverKind = group.kind;
        item.setAttribute("aria-current", String(group.kind === selectedKind));
        const logo = node("span", "drv-item-logo");
        const glyph = kindIcon(group.kind);
        if (glyph) logo.innerHTML = iconOf(group.kind === "odbc" ? "info_pane_conn_odbc" : glyph, 15);
        const name = node("span", "drv-item-name", kindLabel(group.kind));
        item.append(logo, name, stateChip(state));
        item.addEventListener("click", () => {
          selectedKind = group.kind;
          manualView = false;
          render();
        });
        list.appendChild(item);
      }
    }
  };

  // —— 渲染：右侧详情 ——
  const versionRow = (row: DatabaseDriverStatus, recommended: boolean): HTMLElement => {
    const key = versionKey(row);
    const entry = activity.get(key);
    const holders = refs.byVersion.get(key) ?? [];
    const line = node("div", "drv-vrow");
    line.dataset.driverVersion = row.version;

    const name = node("div", "drv-vname");
    name.append(node("span", "drv-vlabel", row.version ? `v${row.version}` : "默认驱动包"));
    if (recommended) name.appendChild(node("span", "drv-vrec", "推荐"));

    const source = node("div", "drv-vsrc", row.library ? "第三方源" : "官方源");
    if (row.library) source.title = `${row.library}：该版本使用第三方固定提交的原件，不是厂商官方直链`;
    const architecture = node("div", "drv-varch", row.architecture || inventory.architecture || "—");
    const holdersText = node("div", holders.length ? "drv-vref used" : "drv-vref");
    holdersText.textContent = holders.length ? `${holders.length} 个连接在用` : "—";
    if (holders.length) holdersText.title = holders.join("\n");

    const action = node("div", "drv-vaction");
    if (entry?.busy === "download" || entry?.busy === "delete") {
      const busy = node("span", "drv-busy");
      busy.innerHTML = `${svg(ICON_SPINNER, 13)}<span>${entry.busy === "download" ? "下载中…" : "删除中…"}</span>`;
      action.appendChild(busy);
    } else if (row.status === "ready") {
      const del = node("button", "btn sm", "删除");
      del.type = "button";
      if (holders.length) {
        del.disabled = true;
        del.title = `有 ${holders.length} 个连接正在使用这个版本：${holders.join("、")}`;
      } else {
        del.title = row.download_path ? `删除本地文件：${row.download_path}` : "删除本地驱动文件";
      }
      del.addEventListener("click", () => removeManaged(row));
      action.appendChild(del);
    } else if (row.can_download) {
      const get = node("button", "btn sm primary");
      get.type = "button";
      get.innerHTML = `${svg(ICON_DOWNLOAD, 12)}<span>下载</span>`;
      get.title = row.library ? "从第三方固定提交下载原件并校验完整性" : "从官方源下载并校验完整性";
      get.addEventListener("click", () => download(row));
      action.appendChild(get);
    } else {
      const help = node("button", "btn sm", "获取方式");
      help.type = "button";
      help.innerHTML = `${svg(ICON_EXTERNAL_LINK, 12)}<span>获取方式</span>`;
      help.addEventListener("click", () => {
        selectedKind = row.kind;
        manualView = true;
        render();
      });
      action.appendChild(help);
    }

    line.append(name, source, architecture, holdersText, action);

    const wrap = node("div", "drv-vwrap");
    wrap.appendChild(line);
    if (entry?.busy === "download") {
      // 后端只返回最终结果，没有进度事件：用不确定态进度条，不编造百分比。
      const bar = node("div", "drv-progress");
      bar.setAttribute("role", "progressbar");
      bar.setAttribute("aria-label", "正在下载并校验驱动");
      wrap.appendChild(bar);
    }
    if (entry?.error) {
      const failure = node("div", "drv-row-error");
      failure.append(node("span", "drv-row-error-text", entry.error));
      const retry = node("button", "btn sm", "重试");
      retry.type = "button";
      retry.addEventListener("click", () => {
        activity.delete(key);
        render();
        if (row.status === "ready") removeManaged(row);
        else download(row);
      });
      failure.appendChild(retry);
      wrap.appendChild(failure);
    }
    if (row.status === "ready" && holders.length) {
      wrap.appendChild(node("div", "drv-row-hint", `先在这些连接里改选其它版本，或删除连接，才能删除这个驱动：${holders.join("、")}`));
    }
    return wrap;
  };

  const section = (title: string, rows: DatabaseDriverStatus[], recommendedKey: string): HTMLElement | null => {
    if (!rows.length) return null;
    const box = node("section", "drv-section");
    const head = node("div", "drv-section-title", `${title}（${rows.length}）`);
    box.appendChild(head);
    const table = node("div", "drv-table");
    const header = node("div", "drv-vhead");
    for (const label of ["版本", "来源", "架构", "被连接引用", "操作"]) {
      header.appendChild(node("div", "", label));
    }
    table.appendChild(header);
    for (const row of rows) table.appendChild(versionRow(row, versionKey(row) === recommendedKey));
    box.appendChild(table);
    return box;
  };

  const renderManual = (group: ManagedGroup, row: DatabaseDriverStatus | undefined): HTMLElement => {
    const label = kindLabel(group.kind);
    const box = node("div", "drv-manual");
    const back = node("button", "btn link drv-back");
    back.type = "button";
    back.textContent = "← 返回版本列表";
    back.addEventListener("click", () => {
      manualView = false;
      render();
    });
    const head = node("div", "drv-detail-head");
    const titles = node("div", "drv-detail-titles");
    titles.append(
      node("h2", "drv-detail-title", `获取 ${label} 驱动`),
      node("div", "drv-muted", "这一步要装厂商提供的驱动，NavKit 不能替你下载。"),
    );
    head.appendChild(titles);

    const reason = node("div", "drv-reason");
    reason.append(
      node("div", "drv-reason-title", "为什么不能自动下载"),
      node("div", "drv-reason-text", (row?.message || "该数据源没有可用的官方直链，需要从厂商渠道获取。").split("\n")[0]),
    );

    const steps = node("ol", "drv-steps");
    const stepData: [string, string][] = [
      ["到厂商渠道拿到驱动", `去厂商官网或随产品附带的安装介质里拿 ${label} 的客户端/驱动包（安装包、压缩包都行），注意与本机架构 ${inventory.architecture || "未知"} 一致。`],
      ["安装到本机", "按厂商说明完成安装。Windows 上一般会注册成系统 ODBC 驱动；其它平台装好驱动库即可。"],
      ["回来点「重新检测」", `检测到之后，这里的状态会变成「已下载」，新建连接里就能选 ${label}。`],
    ];
    for (const [title, desc] of stepData) {
      const item = node("li", "drv-step");
      item.append(node("div", "drv-step-title", title), node("div", "drv-step-desc", desc));
      steps.appendChild(item);
    }

    const detected = node("div", "drv-section drv-detected");
    detected.appendChild(node("div", "drv-section-title", "当前检测结果"));
    const kv = node("dl", "drv-kv");
    const addRow = (term: string, value: string): void => {
      kv.append(node("dt", "", term), node("dd", "", value));
    };
    addRow("本机架构", inventory.architecture || "未知");
    addRow("系统 ODBC 驱动", inventory.drivers.length ? inventory.drivers.slice(0, 6).map((item) => item.name).join("、") : "未检测到任何 ODBC 驱动");
    addRow("已注册 DSN", inventory.dsns.length ? `${inventory.dsns.length} 个` : "无");
    detected.appendChild(kv);
    // 提示里用厂商真实的 ODBC 驱动名做匹配，而不是数据源的显示名 —— 用户要照着这个名字去核对。
    const driverHint = kindMeta(group.kind)?.driver || "";
    const expect = driverHint || label;
    const hit = inventory.drivers.find((item) => item.name.toLowerCase().includes(expect.toLowerCase()));
    detected.appendChild(node(
      "div",
      "drv-muted drv-detected-hint",
      hit
        ? `已检测到系统驱动「${hit.name}」。点下面的「我装好了，重新检测」刷新状态。`
        : `系统里还没有名称为「${expect}」的驱动 —— 说明还没装好；装完再点下面的「重新检测」。`,
    ));

    const actions = node("div", "drv-manual-actions");
    const recheck = node("button", "btn primary");
    recheck.type = "button";
    recheck.innerHTML = `${svg(ICON_REFRESH, 13)}<span>我装好了，重新检测</span>`;
    recheck.addEventListener("click", () => void reloadStatuses(true));
    const copy = node("button", "btn");
    copy.type = "button";
    copy.innerHTML = `${svg(ICON_COPY, 13)}<span>复制获取说明</span>`;
    copy.title = "复制给同事或 IT，让对方照着装";
    copy.addEventListener("click", () => {
      const text = [
        `获取 ${label} 驱动`,
        row?.message ? `原因：${row.message}` : "",
        ...stepData.map(([title, desc], index) => `${index + 1}. ${title}：${desc}`),
        `本机架构：${inventory.architecture || "未知"}`,
      ].filter(Boolean).join("\n");
      void navigator.clipboard.writeText(text).then(
        () => { flush.textContent = "获取说明已复制到剪贴板。"; },
        () => { flush.textContent = "复制失败：当前环境不允许访问剪贴板。"; },
      );
    });
    actions.append(recheck, copy);

    box.append(back, head, reason, steps, detected, actions);
    return box;
  };

  const renderDetail = (group: ManagedGroup | undefined): void => {
    detail.replaceChildren();
    if (!group) {
      const empty = node("div", "drv-empty drv-empty-detail");
      empty.append(
        node("div", "drv-empty-title", "这里还没有内容"),
        node("div", "drv-muted", "左边可能被筛选条件清空了，也可能当前数据源都由内置协议直连、不需要额外驱动文件。"),
      );
      detail.appendChild(empty);
      return;
    }
    if (manualView) {
      const manualRow = group.rows.find((row) => !row.can_download && row.status !== "ready") ?? group.rows[0];
      detail.appendChild(renderManual(group, manualRow));
      return;
    }

    const state = aggregateState(group.rows, activity);
    const head = node("div", "drv-detail-head");
    const logo = node("div", "drv-detail-logo");
    const glyph = kindIcon(group.kind);
    if (glyph) logo.innerHTML = iconOf(glyph, 22);
    const titles = node("div", "drv-detail-titles");
    const titleLine = node("div", "drv-detail-titleline");
    titleLine.append(node("h2", "drv-detail-title", kindLabel(group.kind)), stateChip(state));
    const holders = refs.byKind.get(group.kind) ?? [];
    const readyCount = group.rows.filter((row) => row.status === "ready").length;
    const meta = [`${group.rows.length} 个版本`, `${readyCount} 个已下载`];
    if (holders.length) meta.push(`${holders.length} 个连接在用`);
    if (inventory.architecture) meta.push(inventory.architecture);
    titles.append(titleLine, node("div", "drv-muted", meta.join(" · ")));
    head.append(logo, titles);
    detail.appendChild(head);

    const ready = group.rows.filter((row) => row.status === "ready");
    const toDownload = group.rows.filter((row) => row.status !== "ready" && row.can_download);
    const manualRows = group.rows.filter((row) => row.status !== "ready" && !row.can_download);
    const recommendedKey = versionKey(group.rows[0] ?? { kind: group.kind, version: "" });

    const sections = [
      section("已下载", ready, recommendedKey),
      section("可下载", toDownload, recommendedKey),
      section("需手动获取", manualRows, recommendedKey),
    ].filter(Boolean) as HTMLElement[];
    if (!sections.length) {
      detail.append(node("div", "drv-empty", "这个数据源当前没有可管理的驱动文件。"));
      return;
    }
    detail.append(...sections);
  };

  const render = (): void => {
    if (closed) return;
    // renderList 会重建整张清单：如果焦点正在清单里，重建会把焦点丢到 body，
    // 之后的 Esc / Tab 就都不再落在这个对话框上。先记下来，重建后放回去。
    const active = document.activeElement as HTMLElement | null;
    const restoreTo = active && list.contains(active) ? (active.dataset.driverKind ?? "") : "";

    const groups = groupManagedStatuses(statuses);
    const states = new Map(groups.map((group) => [group.kind, aggregateState(group.rows, activity)]));

    for (const [id, button] of filterButtons) {
      const count = id === "all"
        ? groups.length
        : groups.filter((group) => matchesFilter(states.get(group.kind)!, id)).length;
      button.setAttribute("aria-pressed", String(filter === id));
      button.querySelector(".drv-filter-count")!.textContent = String(count);
    }
    arch.textContent = inventory.architecture ? `${inventory.architecture} · ${inventory.drivers.length} 个系统 ODBC 驱动` : "";

    // 筛选/搜索会改变左栏可见集合，选中项必须落在可见集合里，否则右栏会显示左边根本看不到的数据源。
    const visible = groups.filter((group) => {
      if (!matchesFilter(states.get(group.kind)!, filter)) return false;
      if (!query) return true;
      return `${kindLabel(group.kind)} ${group.kind}`.toLowerCase().includes(query);
    });
    if (!visible.some((group) => group.kind === selectedKind)) {
      selectedKind = visible[0]?.kind ?? "";
      manualView = false;
    }

    renderList(groups);
    renderDetail(visible.find((group) => group.kind === selectedKind));

    if (focusKey) {
      const target = list.querySelector<HTMLElement>(`[data-driver-kind="${CSS.escape(focusKey.split("@")[0])}"]`);
      target?.focus();
      focusKey = "";
    } else if (restoreTo) {
      list.querySelector<HTMLElement>(`[data-driver-kind="${CSS.escape(restoreTo)}"]`)?.focus();
    }
  };

  const dlg = new Dialog({
    title: "驱动管理",
    width: 1040,
    height: 700,
    render: (content) => {
      content.style.cssText = "display:flex;flex-direction:column;overflow:hidden;padding:0";
      const column = node("div", "drv-shell");
      column.append(toolbar, body, flush);
      content.appendChild(column);
    },
    buttons: [
      {
        label: "添加自定义驱动…",
        alignLeft: true,
        onClick: () => {
          openCustomDriverDialog((drivers) => onChanged?.(drivers));
          return false;
        },
      },
      { label: "关闭" },
    ],
  });

  const unsubscribe = subscribeDriverStatuses((values) => {
    statuses = values;
    if (!closed) render();
  });

  prepareConnectionDialog(dlg);
  dlg.onClose(() => {
    closed = true;
    unsubscribe();
    if (previousFocus?.isConnected) previousFocus.focus();
  });

  void reloadStatuses(false);
  void Promise.all([driverApi.inventory(), api.listConnections()]).then(([installed, list]) => {
    inventory = installed;
    connections = list;
    refs = collectRefs(connections);
    if (!closed) render();
  }).catch((error) => {
    if (!closed) flush.textContent = `读取本机驱动信息失败：${String(error)}`;
  });

  return dlg;
}

// ---------------------------------------------------------------------------
// 自定义驱动：单页表单（原来是三步向导，步骤本身没有价值）
// ---------------------------------------------------------------------------

/** 打开自定义驱动编辑页；保存/删除后把最新列表交回调用方。 */
export function openCustomDriverDialog(
  onChanged?: (drivers: DriverDefinition[]) => void,
  focusId?: string,
): Dialog {
  let drivers: DriverDefinition[] = [];
  let currentId = focusId ?? "";
  let busy = false;
  let closed = false;
  let inventory: OdbcInventory = { drivers: [], dsns: [], architecture: "" };
  let lastVerified = "";

  const name = textInput("", 280);
  const protocol = document.createElement("select");
  for (const [value, label] of [["jdbc", "JDBC（本地 .jar，Java 11+）"], ["odbc", "ODBC（原生库 / DSN）"]] as const) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    protocol.append(option);
  }
  protocol.value = "jdbc";
  const source = document.createElement("select");
  for (const [value, label] of [["installed", "已安装的 ODBC 驱动"], ["file", "本地原生 ODBC 驱动文件"], ["dsn", "已注册的 ODBC DSN"]] as const) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    source.append(option);
  }
  const target = textInput("", 280);
  const targets = document.createElement("datalist");
  targets.id = newConnectionId();
  target.setAttribute("list", targets.id);
  const browse = node("button", "btn", "选择文件…");
  browse.type = "button";
  const driverClass = textInput("", 280);
  driverClass.placeholder = "可选，如 com.mysql.cj.jdbc.Driver";
  const template = document.createElement("textarea");
  template.rows = 3;
  template.className = "drv-textarea";
  template.value = DEFAULT_JDBC_URL;
  const testSql = textInput("SELECT 1", 280);

  // 验证参数（临时，不保存）
  const host = textInput("localhost", 280);
  const port = numberInput(0, 100);
  port.min = "0";
  port.max = "65535";
  const database = textInput("", 280);
  database.placeholder = "数据库名，或 :memory:";
  const file = textInput("", 280);
  file.placeholder = "模板用到 {{file}} 时填写";
  const user = textInput("", 280);
  const password = passwordInput("", 280);
  password.autocomplete = "new-password";

  const listHost = node("div", "drv-custom-list-items");
  const status = node("div", "drv-form-status");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  const hint = node("div", "drv-form-hint");
  const verifyBox = node("div", "drv-verify");
  verifyBox.hidden = true;
  const verifyResult = node("div", "drv-verify-result");
  verifyResult.setAttribute("role", "status");
  verifyResult.setAttribute("aria-live", "polite");

  const isJdbc = (): boolean => protocol.value === "jdbc";
  const sourceField = connectionField("驱动来源:", source);
  const protocolField = connectionField("驱动类型:", protocol);
  const driverClassField = connectionField("驱动类名:", driverClass);
  const targetControls = node("div", "drv-target");
  targetControls.append(target, browse, targets);

  const helpText = (): string => isJdbc()
    ? "JDBC 通过本地 .jar 与 Java 11+ 运行。URL 模板可用 {{host}} {{port}} {{database}} {{file}} {{user}} {{password}}，必须以 jdbc: 开头。密码只用于本次验证，不保存。"
    : "占位符 {{driver}} {{dsn}} {{host}} {{port}} {{database}} {{file}} {{user}} {{password}}，值由后端安全转义，不要再加 {}。密码只用于本次验证，不保存。只加载同架构的、你信任的原生驱动。";

  const draft = (): DriverDefinition => ({
    id: currentId || `driver_${newConnectionId()}`,
    name: name.value.trim(),
    protocol: protocol.value as DriverProtocol,
    source: isJdbc() ? "file" : source.value as DriverSource,
    target: target.value.trim(),
    template: template.value.trim(),
    driver_class: isJdbc() ? driverClass.value.trim() : "",
    test_sql: testSql.value.trim(),
  });

  const syncTargets = (): void => {
    const jdbc = isJdbc();
    targets.replaceChildren();
    sourceField.hidden = jdbc;
    driverClassField.hidden = !jdbc;
    browse.hidden = jdbc ? false : source.value !== "file";
    const values = jdbc ? [] : source.value === "dsn"
      ? inventory.dsns.map((item) => item.name)
      : source.value === "installed" ? inventory.drivers.map((item) => item.name) : [];
    for (const value of values) {
      const option = document.createElement("option");
      option.value = value;
      targets.append(option);
    }
    target.placeholder = jdbc
      ? "驱动 .jar 的绝对路径，如 C:\\drivers\\mysql-connector-j.jar"
      : source.value === "file" ? "绝对路径，如 C:\\drivers\\driver.dll"
        : source.value === "dsn" ? "选择或输入当前架构的 DSN 名称" : "选择或输入已安装驱动的名称";
    hint.textContent = helpText();
  };

  const renderList = (): void => {
    listHost.replaceChildren();
    if (!drivers.length) {
      listHost.appendChild(node("div", "drv-muted drv-custom-empty", "还没有自定义驱动。右边填好保存，就会出现在这里。"));
      return;
    }
    for (const driver of drivers) {
      const item = node("button", "drv-custom-item");
      item.type = "button";
      item.setAttribute("aria-current", String(driver.id === currentId));
      const origin = driver.protocol === "jdbc" ? "本地 JAR"
        : driver.source === "file" ? "本地文件" : driver.source === "dsn" ? "DSN" : "已安装驱动";
      item.append(
        node("span", "drv-custom-name", driver.name || "（未命名）"),
        node("span", "drv-custom-meta", `${driver.protocol.toUpperCase()} · ${origin}`),
      );
      item.addEventListener("click", () => {
        if (!busy) load(driver);
      });
      listHost.appendChild(item);
    }
  };

  const addButton = node("button", "btn drv-custom-add");
  addButton.type = "button";
  addButton.innerHTML = `${svg(ICON_PLUS_LINE, 13)}<span>新建驱动</span>`;
  addButton.addEventListener("click", () => {
    if (busy) return;
    load();
    name.focus();
  });

  const removeButton = node("button", "btn drv-remove");
  removeButton.type = "button";
  removeButton.innerHTML = `${svg(ICON_CROSS, 13)}<span>删除</span>`;

  const load = (driver?: DriverDefinition): void => {
    currentId = driver?.id ?? "";
    name.value = driver?.name ?? "";
    protocol.value = driver?.protocol ?? "jdbc";
    const nextSource = driver?.source ?? "installed";
    source.value = nextSource === "file" ? "file" : nextSource === "dsn" ? "dsn" : "installed";
    target.value = driver?.target ?? "";
    template.value = driver?.template ?? (driver?.protocol === "odbc" ? DEFAULT_TEMPLATE : DEFAULT_JDBC_URL);
    driverClass.value = driver?.driver_class ?? "";
    testSql.value = driver?.test_sql ?? "SELECT 1";
    password.value = "";
    lastVerified = "";
    verifyResult.textContent = "";
    verifyBox.hidden = true;
    status.textContent = "";
    syncTargets();
    renderList();
    removeButton.hidden = !currentId;
  };

  const validate = (): string | null => {
    if (!name.value.trim()) return "请输入驱动显示名称。";
    if (!target.value.trim()) return isJdbc() ? "请选择 JDBC 驱动 JAR 文件。" : "请选择驱动文件、已安装驱动或 DSN。";
    if (!template.value.trim()) return isJdbc() ? "JDBC URL 模板不能为空。" : "连接串模板不能为空。";
    if (!testSql.value.trim()) return "验证 SQL 不能为空。";
    if (isJdbc() && !/^jdbc:/i.test(template.value.trim())) return "JDBC URL 模板必须以 jdbc: 开头。";
    return null;
  };

  const run = async (action: () => Promise<void>): Promise<void> => {
    if (busy) return;
    busy = true;
    dlg.el.setAttribute("aria-busy", "true");
    try {
      await action();
    } catch (error) {
      status.textContent = String(error);
    } finally {
      busy = false;
      dlg.el.removeAttribute("aria-busy");
    }
  };

  protocol.addEventListener("change", () => {
    template.value = isJdbc() ? DEFAULT_JDBC_URL : DEFAULT_TEMPLATE;
    target.value = "";
    driverClass.value = "";
    syncTargets();
  });
  source.addEventListener("change", () => {
    target.value = "";
    template.value = source.value === "dsn" ? "DSN={{dsn}};UID={{user}};PWD={{password}}" : DEFAULT_TEMPLATE;
    syncTargets();
  });
  browse.addEventListener("click", async () => {
    try {
      const path = isJdbc()
        ? await openFile({ title: "选择可信的 JDBC 驱动 JAR", multiple: false, filters: [{ name: "JDBC 驱动", extensions: ["jar"] }] })
        : await openFile({ title: "选择可信的原生 ODBC 驱动", multiple: false, filters: [{ name: "原生驱动", extensions: ["dll", "so", "dylib"] }] });
      if (typeof path === "string") target.value = path;
    } catch (error) {
      status.textContent = String(error);
    }
  });

  const verifyButton = node("button", "btn drv-verify-toggle");
  verifyButton.type = "button";
  verifyButton.innerHTML = `${svg(ICON_REFRESH, 13)}<span>验证连接…</span>`;
  verifyButton.addEventListener("click", () => {
    verifyBox.hidden = !verifyBox.hidden;
    if (!verifyBox.hidden) host.focus();
  });

  const runVerify = node("button", "btn", "开始验证");
  runVerify.type = "button";
  runVerify.addEventListener("click", () => {
    void run(async () => {
      const problem = validate();
      if (problem) {
        status.textContent = problem;
        return;
      }
      verifyResult.textContent = isJdbc() ? "正在加载真实 JDBC 驱动并执行验证 SQL…" : "正在加载真实 ODBC 驱动并执行验证 SQL…";
      const result = await driverApi.test(draft(), {
        kind: "odbc",
        host: host.value.trim(),
        port: Number(port.value) || 0,
        user: user.value.trim(),
        password: password.value,
        database: database.value.trim(),
        file: file.value.trim(),
        params: "",
        timeout_secs: 15,
      });
      verifyResult.textContent = result.ok
        ? `验证成功${result.version ? ` — ${result.version}` : ""}。可以保存了。`
        : `验证失败：${result.message}`;
      verifyResult.dataset.tone = result.ok ? "ok" : "danger";
      lastVerified = result.ok ? new Date().toLocaleTimeString() : "";
      password.value = "";
    });
  });

  removeButton.addEventListener("click", () => {
    if (!currentId || busy) {
      status.textContent = "先选一个已保存的驱动。";
      return;
    }
    const removingId = currentId;
    const removingName = name.value || "该驱动";
    void new Dialog({
      title: "删除自定义驱动",
      width: 430,
      render: (content) => {
        content.append(
          node("p", "", `删除「${removingName}」？`),
          node("p", "drv-muted", "正在被连接引用的驱动后端会拒绝删除：请先在那些连接里换掉驱动。"),
        );
      },
      buttons: [
        {
          label: "删除",
          onClick: async () => {
            await run(async () => {
              drivers = await driverApi.remove(removingId);
              onChanged?.(drivers);
              load();
              status.textContent = "驱动已删除。";
            });
          },
        },
        { label: "取消" },
      ],
    });
  });

  const saveButton = node("button", "btn primary", "保存驱动");
  saveButton.type = "button";
  saveButton.addEventListener("click", () => {
    void run(async () => {
      const problem = validate();
      if (problem) {
        status.textContent = problem;
        return;
      }
      const value = draft();
      drivers = await driverApi.save(value);
      currentId = value.id;
      onChanged?.(drivers);
      renderList();
      removeButton.hidden = !currentId;
      status.textContent = lastVerified
        ? `已保存。最近一次验证成功：${lastVerified}。`
        : "已保存。还没验证过 —— 可以点「验证连接…」确认它能连上。";
    });
  });

  const dlg = new Dialog({
    title: "自定义驱动",
    width: 940,
    height: 620,
    render: (content) => {
      content.style.cssText = "display:flex;flex-direction:column;overflow:hidden;padding:0";
      const shell = node("div", "drv-custom");
      const aside = node("div", "drv-custom-list");
      aside.append(addButton, listHost);
      const form = node("div", "drv-custom-form");
      const targetField = connectionField("驱动目标:", targetControls);
      // aria-label 必须落在输入框本身，包装层拿着它会让读屏和测试都只摸到一个 div。
      targetControls.removeAttribute("aria-label");
      target.setAttribute("aria-label", "驱动目标");
      form.append(
        connectionField("显示名称:", name),
        protocolField,
        sourceField,
        targetField,
        driverClassField,
        connectionField("URL 模板 / 连接串:", template),
        connectionField("验证 SQL:", testSql),
        hint,
        verifyBox,
        status,
      );
      verifyBox.append(
        node("div", "drv-verify-title", "验证参数（临时，不保存）"),
        connectionField("主机:", host),
        connectionField("端口:", port),
        connectionField("数据库:", database),
        connectionField("数据库文件:", file),
        connectionField("用户名:", user),
        connectionField("密码:", password),
        runVerify,
        verifyResult,
      );
      shell.append(aside, form);
      content.appendChild(shell);
    },
    buttons: [],
  });
  // 底部动作条：删除靠左（危险操作远离保存），验证与保存在右。
  removeButton.classList.add("drv-footer-left");
  dlg.footer.append(removeButton, verifyButton, saveButton);

  prepareConnectionDialog(dlg);
  dlg.onClose(() => {
    closed = true;
    password.value = "";
  });
  void Promise.all([driverApi.list(), driverApi.inventory()]).then(([definitions, installed]) => {
    drivers = definitions;
    inventory = installed;
    if (closed) return;
    const initial = focusId ? drivers.find((item) => item.id === focusId) : undefined;
    load(initial);
    if (!initial && focusId) status.textContent = "该驱动已不存在，可以从空白开始新建。";
  }).catch((error) => {
    if (!closed) status.textContent = `读取自定义驱动失败：${String(error)}`;
  });
  syncTargets();
  return dlg;
}

/** Compatibility name used by the application's tool command. */
export const openDriverManagerDialog = openDriverManager;
