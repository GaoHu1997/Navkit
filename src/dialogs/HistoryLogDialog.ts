/** 仅展示本工具已保存的 SQL 执行历史，不读取其他日志来源。 */
import { sql } from "@codemirror/lang-sql";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { Decoration, EditorView, lineNumbers } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { api } from "../core/api";
import type { SqlLogEntry } from "../core/api";
import { Dialog, select, textInput } from "../ui/Dialog";
import { ICON_REFRESH, svg } from "../ui/icons";
import { SQL_KIND_OPTIONS, classifySql, classifySqlKinds, type SqlKind } from "../core/sql-kind";
import { accessibleToolDialog, toolButton } from "./toolDialog";
import "../styles/history-log-dialog.css";
import { sqlHistoryParts, type HistoryLineKind } from "../core/log-format";
export { formatLogTimestamp, sqlHistoryParts } from "../core/log-format";

export type HistoryLogApi = Pick<typeof api, "listSqlLog" | "clearSqlLog">;
export interface HistoryLogRow {
  timestamp: string;
  category: string;
  outcome: "success" | "error" | "unknown";
  message: string;
  raw: string;
  detail?: SqlLogEntry;
  /** 数据源（连接）标识与名称，供数据源筛选。 */
  connectionId: string;
  connectionName: string;
  /** 库 / 模式；未指定时为空串。 */
  database: string;
  /** 语句类别（DQL / DML / DDL …），供类型筛选；多语句时是去重后的集合。 */
  sqlKind: SqlKind;
  sqlKinds: SqlKind[];
}

/** 历史窗口的筛选条件；值为 "all" 表示不限制。 */
export interface HistoryLogFilter {
  connection?: string;
  database?: string;
  kind?: string;
}

/** 数据源下拉的最小形状（App 传入已保存连接）。 */
export interface HistoryLogConnection {
  id: string;
  name: string;
  kind: string;
}

const HISTORY_LIMIT = 2000;
const HISTORY_HIGHLIGHT = HighlightStyle.define([
  { tag: tags.keyword, class: "history-log-keyword" },
  { tag: tags.string, class: "history-log-string" },
  { tag: tags.number, class: "history-log-number" },
  { tag: tags.comment, class: "history-log-comment" },
]);

/** raw 中的 SQL 使用已保存原文，供搜索，不压平、不截断、不更改换行符。 */
export function sqlLogRows(entries: SqlLogEntry[]): HistoryLogRow[] {
  return entries.slice(0, HISTORY_LIMIT).map(entry => {
    const raw = sqlHistoryParts(entry).map(part => part.text).join("\n");
    return {
      timestamp: entry.ts, category: "SQL", outcome: entry.ok ? "success" : "error",
      message: entry.sql, raw, detail: entry,
      connectionId: entry.connection_id ?? "",
      connectionName: entry.connection_name ?? "",
      database: entry.database ?? "",
      sqlKind: classifySql(entry.sql),
      sqlKinds: classifySqlKinds(entry.sql),
    };
  });
}

/** 保留既有 SQL helper 的前四个参数顺序（搜索 / 分类 / 结果），追加数据源、库与语句类型筛选。 */
export function filterHistoryLog(
  rows: HistoryLogRow[],
  search: string,
  category = "all",
  outcome = "all",
  filter: HistoryLogFilter = {},
): HistoryLogRow[] {
  const needle = search.trim().toLocaleLowerCase();
  const connection = filter.connection ?? "all";
  const database = filter.database ?? "all";
  const kind = filter.kind ?? "all";
  /** 真实库名恰好叫 all 时用 \u0000all 编码，避免与「全部数据库」哨兵冲突。 */
  const decodedDatabase = (value: string) => (value.startsWith("\u0000") ? value.slice(1) : value);
  return rows.filter(row => (category === "all" || category === row.category) &&
    (outcome === "all" || outcome === row.outcome) &&
    (connection === "all" || row.connectionId === connection) &&
    (database === "all" || row.database === decodedDatabase(database)) &&
    (kind === "all" || row.sqlKinds.includes(kind as SqlKind)) &&
    (!needle || `${row.raw}\n${row.detail?.origin ?? ""}\n${row.connectionName}\n${row.database}`.toLocaleLowerCase().includes(needle)));
}

/** 仅编辑器的显示换行归一化；复制始终使用 row.raw，保留原始 CR/LF 与缩进。 */
export function historyLogDocument(rows: HistoryLogRow[]): {
  text: string; annotations: { line: number; kind: Exclude<HistoryLineKind, "sql"> }[];
} {
  const lines: string[] = [];
  const annotations: { line: number; kind: Exclude<HistoryLineKind, "sql"> }[] = [];
  for (const row of rows.slice(0, HISTORY_LIMIT)) {
    if (lines.length) lines.push("");
    const parts = row.detail ? sqlHistoryParts(row.detail) : [{ kind: "sql" as const, text: row.raw }];
    for (const part of parts) {
      for (const line of part.text.split(/\r\n?|\n/)) {
        lines.push(line);
        if (part.kind !== "sql") annotations.push({ line: lines.length, kind: part.kind });
      }
    }
  }
  return { text: lines.join("\n"), annotations };
}

function historyEditorState(rows: HistoryLogRow[]): EditorState {
  const document = historyLogDocument(rows);
  // 先建立 Text，再按逻辑行定位元信息；CRLF 不会使 decoration 的偏移错位。
  const state = EditorState.create({ doc: document.text });
  const decorations = Decoration.set(document.annotations.map(annotation =>
    Decoration.line({ class: `history-log-${annotation.kind}` }).range(state.doc.line(annotation.line).from)), true);
  return EditorState.create({ doc: state.doc, extensions: [
    EditorState.readOnly.of(true), EditorState.tabSize.of(4), EditorView.editable.of(false),
    EditorView.contentAttributes.of({ "aria-label": "已保存 SQL 原文", "aria-readonly": "true", tabindex: "0" }),
    EditorView.lineWrapping, lineNumbers(), sql(), syntaxHighlighting(HISTORY_HIGHLIGHT),
    EditorView.decorations.of(decorations),
  ] });
}

/** 构造下拉选项（不依赖 window.Option，测试与浏览器行为一致）。 */
function optionEl(value: string, label: string): HTMLOptionElement {
  const el = document.createElement("option");
  el.value = value;
  el.textContent = label;
  return el;
}

function labelledControl(label: string, control: HTMLElement, id: string): HTMLLabelElement {
  const wrapper = document.createElement("label");
  wrapper.className = "history-log-control"; wrapper.htmlFor = id; control.id = id;
  const name = document.createElement("span"); name.className = "history-log-sr-only"; name.textContent = label;
  wrapper.append(name, control); return wrapper;
}

/** App 可直接调用；可注入仅包含 SQL 日志读/清空能力的 API。 */
export function openHistoryLogDialog(
  source: HistoryLogApi = api,
  connections: readonly HistoryLogConnection[] = [],
): Dialog {
  const returnFocus = document.activeElement as HTMLElement | null;
  const id = `history-log-${crypto.randomUUID()}`;
  const outcome = select([{ value: "all", label: "全部结果" }, { value: "success", label: "成功" }, { value: "error", label: "失败" }], "all");
  outcome.className = "history-log-outcome"; outcome.setAttribute("aria-label", "结果筛选");
  // 数据源 / 数据库 / SQL 类型：由已读取记录与已保存连接推导，不额外连接数据库。
  const connectionSelect = select([{ value: "all", label: "全部数据源" }], "all");
  connectionSelect.className = "history-log-connection"; connectionSelect.setAttribute("aria-label", "数据源筛选");
  const databaseSelect = select([{ value: "all", label: "全部数据库" }], "all");
  databaseSelect.className = "history-log-database"; databaseSelect.setAttribute("aria-label", "数据库筛选");
  const kindSelect = select(SQL_KIND_OPTIONS, "all");
  kindSelect.className = "history-log-kind"; kindSelect.setAttribute("aria-label", "SQL 类型筛选");
  const search = textInput(); search.type = "search"; search.className = "history-log-search";
  search.placeholder = "搜索时间、连接、数据库或 SQL…"; search.setAttribute("aria-label", "搜索 SQL 历史");
  const refresh = toolButton("刷新"), more = toolButton("⋯"), clear = toolButton("清空 SQL 日志…");
  refresh.classList.add("history-log-refresh");
  more.classList.add("history-log-more"); clear.classList.add("history-log-clear");
  const decoration = document.createElement("span"); decoration.className = "history-log-action-icon";
  decoration.setAttribute("aria-hidden", "true"); decoration.innerHTML = svg(ICON_REFRESH); refresh.insertBefore(decoration, refresh.firstChild);
  const menu = document.createElement("div"); menu.className = "history-log-more-menu";
  menu.id = `${id}-more-menu`; menu.hidden = true; menu.append(clear);
  more.setAttribute("aria-label", "更多操作"); more.setAttribute("aria-expanded", "false"); more.setAttribute("aria-controls", menu.id);
  const count = document.createElement("div"); count.className = "history-log-count";
  const status = document.createElement("div"); status.className = "history-log-status";
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite"); status.setAttribute("aria-atomic", "true");
  const viewer = document.createElement("div"); viewer.className = "history-log-viewer";
  viewer.setAttribute("role", "region"); viewer.setAttribute("aria-label", "SQL 历史正文");
  const editorHost = document.createElement("div"); editorHost.className = "history-log-editor";
  const empty = document.createElement("div"); empty.className = "history-log-empty";
  viewer.append(editorHost, empty);
  let rows: HistoryLogRow[] = [], visible: HistoryLogRow[] = [];
  let closed = false, busy: "load" | "clear" | null = null, version = 0, noticeVersion = 0;
  let loaded = false, loadError = "", sampledAt = "";
  let editor: EditorView | null = null, confirmation: Dialog | null = null;

  const setStatus = (message: string, error = false) => {
    if (closed) return;
    noticeVersion++; status.textContent = message;
    status.setAttribute("role", error ? "alert" : "status"); status.setAttribute("aria-live", error ? "assertive" : "polite");
  };
  const updateControls = () => {
    refresh.disabled = !!busy || !!confirmation;
    // 清空请求仍在进行时取消确认框，也应能把焦点归还给更多按钮。
    more.disabled = !!confirmation;
    clear.disabled = !!busy || !rows.length;
    viewer.setAttribute("aria-busy", String(!!busy));
  };
  /** 用已读取记录 + 已保存连接重建数据源下拉；仍有效的选择保留。 */
  const refreshSourceOptions = () => {
    const wanted = connectionSelect.value;
    const seen = new Map<string, string>();
    for (const connection of connections) seen.set(connection.id, connection.name);
    for (const row of rows) {
      if (row.connectionId && !seen.has(row.connectionId)) seen.set(row.connectionId, row.connectionName || row.connectionId);
    }
    const options = [optionEl("all", "全部数据源")];
    for (const [value, label] of seen) options.push(optionEl(value, label));
    connectionSelect.replaceChildren(...options);
    connectionSelect.value = seen.has(wanted) ? wanted : "all";
  };
  /** 数据库下拉只列出当前数据源出现过的库。 */
  const refreshDatabaseOptions = () => {
    const wanted = databaseSelect.value;
    const names = new Set<string>();
    for (const row of rows) {
      if (connectionSelect.value !== "all" && row.connectionId !== connectionSelect.value) continue;
      if (row.database) names.add(row.database);
    }
    const options = [optionEl("all", "全部数据库")];
    const values = new Set<string>();
    for (const name of [...names].sort((a, b) => a.localeCompare(b))) {
      const value = name === "all" ? "\u0000all" : name;
      values.add(value); options.push(optionEl(value, name));
    }
    databaseSelect.replaceChildren(...options);
    databaseSelect.value = values.has(wanted) || wanted === "all" ? wanted : "all";
  };
  const render = () => {
    if (closed) return;
    refreshSourceOptions();
    refreshDatabaseOptions();
    visible = filterHistoryLog(rows, search.value, "all", outcome.value, {
      connection: connectionSelect.value,
      database: databaseSelect.value,
      kind: kindSelect.value,
    });
    editor?.setState(historyEditorState(visible));
    editorHost.hidden = !visible.length; empty.hidden = !!visible.length;
    empty.textContent = busy ? "正在读取 SQL 历史…" : loadError && !loaded ? "SQL 历史读取失败，请刷新重试。" :
      rows.length ? "没有符合当前搜索或结果筛选的 SQL 记录。" : "暂无 SQL 执行记录；执行 SQL 后会出现在这里。";
    count.textContent = `SQL 执行记录 · 按时间倒序 · ${visible.length} / ${rows.length} 条`;
    if (busy) setStatus(busy === "load" ? "正在读取 SQL 历史…" : "正在清空 SQL 日志…");
    else if (loadError) setStatus(`${loadError}${loaded ? "；正文保留上次读取的记录。" : ""}`, true);
    else setStatus(`${sampledAt ? `读取 ${sampledAt} · ` : ""}显示 ${visible.length} / ${rows.length} 条`);
    updateControls();
  };
  const closeMenu = (restoreFocus = false) => {
    const wasOpen = !menu.hidden; menu.hidden = true; more.setAttribute("aria-expanded", "false");
    if (restoreFocus && wasOpen && !closed) more.focus();
  };
  const load = async () => {
    if (busy || closed || confirmation) return;
    const request = ++version; busy = "load"; closeMenu(); updateControls(); setStatus("正在读取 SQL 历史…");
    if (!loaded) empty.textContent = "正在读取 SQL 历史…";
    try {
      const entries = await source.listSqlLog(HISTORY_LIMIT);
      if (closed || request !== version) return;
      rows = sqlLogRows(entries); loaded = true; loadError = ""; sampledAt = new Date().toLocaleTimeString();
    } catch (error) {
      if (closed || request !== version) return;
      loadError = `SQL 历史读取失败：${error instanceof Error ? error.message : String(error)}`;
    } finally {
      if (!closed && request === version) { busy = null; render(); }
    }
  };

  const dlg = new Dialog({ title: "历史日志", width: 1240, render: (body, dialog) => {
    dialog.el.classList.add("history-log-dialog");
    const toolbar = document.createElement("div"); toolbar.className = "history-log-toolbar";
    const actions = document.createElement("div"); actions.className = "history-log-actions";
    const moreWrap = document.createElement("div"); moreWrap.className = "history-log-more-wrap"; moreWrap.append(more, menu);
    actions.append(refresh, moreWrap);
    const searchWrap = labelledControl("搜索 SQL 历史", search, `${id}-search`);
    searchWrap.classList.add("history-log-search-wrap");
    toolbar.append(
      labelledControl("数据源筛选", connectionSelect, `${id}-connection`),
      labelledControl("数据库筛选", databaseSelect, `${id}-database`),
      labelledControl("SQL 类型筛选", kindSelect, `${id}-kind`),
      labelledControl("结果筛选", outcome, `${id}-outcome`),
      searchWrap,
      actions,
    );
    const note = document.createElement("div"); note.className = "history-log-note";
    note.textContent = "最近 2000 条 · 30 天内，超限自动清理。此处显示已保存的 SQL 原文；此前在存储时截断的 SQL 或错误信息无法恢复。";
    body.append(toolbar, count, viewer, note);
    editor = new EditorView({ state: historyEditorState([]), parent: editorHost });
  }, buttons: [{ label: "关闭" }] });
  dlg.footer.prepend(status);
  accessibleToolDialog(dlg, "历史日志", () => {
    closed = true; version++; noticeVersion++;
    confirmation?.close(); editor?.destroy(); editor = null;
    document.removeEventListener("pointerdown", outsideMenu);
  }, returnFocus);

  function outsideMenu(event: PointerEvent) {
    if (!(event.target instanceof Node) || !more.parentElement?.contains(event.target)) closeMenu();
  }
  document.addEventListener("pointerdown", outsideMenu);
  // 捕获 Escape，先收起更多操作，再由通用辅助函数处理关闭窗口。
  dlg.el.addEventListener("keydown", event => {
    if (event.key === "Escape" && !menu.hidden) { event.preventDefault(); event.stopImmediatePropagation(); closeMenu(true); }
  }, true);
  more.addEventListener("click", () => {
    if (more.disabled || closed) return;
    menu.hidden = !menu.hidden; more.setAttribute("aria-expanded", String(!menu.hidden));
    if (!menu.hidden) clear.focus();
  });
  menu.addEventListener("focusout", event => {
    if (event.relatedTarget instanceof Node && !more.parentElement?.contains(event.relatedTarget)) closeMenu();
  });
  outcome.addEventListener("change", render); search.addEventListener("input", render);
  kindSelect.addEventListener("change", render);
  connectionSelect.addEventListener("change", () => { refreshDatabaseOptions(); render(); });
  databaseSelect.addEventListener("change", render);
  refresh.addEventListener("click", () => void load());
  clear.addEventListener("click", () => {
    if (busy || closed || confirmation || !rows.length) return;
    closeMenu();
    const confirmStatus = document.createElement("div"); confirmStatus.className = "history-log-confirm-status";
    confirmStatus.id = `${id}-confirm-status`; confirmStatus.setAttribute("role", "status"); confirmStatus.setAttribute("aria-live", "polite");
    const prompt = document.createElement("p"); prompt.id = `${id}-confirm-prompt`;
    prompt.textContent = "确定清空全部 SQL 执行日志？此操作不可恢复。";
    let confirmClosed = false;
    const confirm = new Dialog({ title: "清空 SQL 日志", width: 440, render: (body, dialog) => {
      dialog.el.classList.add("history-log-dialog", "history-log-confirm"); body.append(prompt, confirmStatus);
    }, buttons: [
      { label: "取消" },
      { label: "清空 SQL 日志", onClick: async () => {
        if (busy || closed || confirmClosed) return false;
        const request = ++version; busy = "clear"; updateControls(); setStatus("正在清空 SQL 日志…");
        const buttons = [...confirm.footer.querySelectorAll<HTMLButtonElement>("button")];
        buttons.forEach(button => { button.disabled = true; });
        confirmStatus.setAttribute("role", "status"); confirmStatus.textContent = "正在清空 SQL 日志…";
        try {
          await source.clearSqlLog();
          if (closed || request !== version) return false;
          rows = []; loaded = true; loadError = ""; sampledAt = new Date().toLocaleTimeString();
          busy = null; render(); setStatus("SQL 执行日志已清空。");
          return true;
        } catch (error) {
          if (closed || request !== version) return false;
          const message = `清空 SQL 日志失败：${error instanceof Error ? error.message : String(error)}`;
          setStatus(message, true);
          if (!confirmClosed) { confirmStatus.setAttribute("role", "alert"); confirmStatus.textContent = message; }
          return false;
        } finally {
          if (!closed && request === version) {
            busy = null; updateControls();
            if (!confirmClosed) buttons.forEach(button => { button.disabled = false; });
          }
        }
      } },
    ] });
    confirmation = confirm; dlg.el.inert = true; updateControls();
    accessibleToolDialog(confirm, "清空 SQL 日志", () => {
      confirmClosed = true;
      if (confirmation === confirm) confirmation = null;
      if (!closed) { dlg.el.inert = false; updateControls(); }
    }, more);
    confirm.el.setAttribute("role", "alertdialog"); confirm.el.setAttribute("aria-describedby", `${prompt.id} ${confirmStatus.id}`);
    confirm.footer.lastElementChild?.classList.add("danger");
    queueMicrotask(() => { if (!closed && !confirmClosed) confirm.footer.querySelector<HTMLButtonElement>("button")?.focus(); });
  });
  render(); void load(); return dlg;
}