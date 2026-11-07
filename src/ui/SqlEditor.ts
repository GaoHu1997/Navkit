/** CodeMirror 6 SQL 编辑器：语法高亮、快捷键、自动补全。 */

import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  startCompletion,
} from "@codemirror/autocomplete";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  redo,
  undo,
} from "@codemirror/commands";
import { sql } from "@codemirror/lang-sql";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import {
  findNext,
  highlightSelectionMatches,
  openSearchPanel,
  searchKeymap,
} from "@codemirror/search";
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import { analyzeSqlCompletion, sqlExecutionText } from "../core/sqlCompletion";

/** SQL 关键字与函数（用于自动补全）。 */
const SQL_KEYWORDS = [
  "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET",
  "DELETE", "CREATE", "ALTER", "DROP", "TRUNCATE", "TABLE", "VIEW", "INDEX",
  "DATABASE", "SCHEMA", "PRIMARY KEY", "FOREIGN KEY", "REFERENCES", "CONSTRAINT",
  "UNIQUE", "DEFAULT", "CHECK", "CASCADE", "JOIN", "INNER", "LEFT", "RIGHT",
  "FULL", "OUTER", "CROSS", "ON", "USING", "GROUP BY", "ORDER BY", "HAVING",
  "LIMIT", "OFFSET", "UNION", "UNION ALL", "INTERSECT", "EXCEPT", "AS", "AND",
  "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "ILIKE", "IS", "NULL", "CASE",
  "WHEN", "THEN", "ELSE", "END", "DISTINCT", "ALL", "ANY", "ASC", "DESC",
  "BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT", "EXPLAIN", "ANALYZE", "VACUUM",
  "PRAGMA", "WITH", "RECURSIVE", "RETURNING", "CONFLICT", "REPLACE", "AUTOINCREMENT",
  "AUTO_INCREMENT", "TEMPORARY", "IF", "IGNORE", "ADD", "COLUMN", "MODIFY",
  "RENAME", "TO", "AFTER", "BEFORE", "IF EXISTS", "IF NOT EXISTS",
];

const SQL_FUNCTIONS = [
  "COUNT", "SUM", "AVG", "MIN", "MAX", "ROUND", "ABS", "CEIL", "FLOOR",
  "COALESCE", "NULLIF", "CONCAT", "CONCAT_WS", "SUBSTRING", "SUBSTR", "LENGTH",
  "CHAR_LENGTH", "UPPER", "LOWER", "TRIM", "LTRIM", "RTRIM", "REPLACE",
  "NOW", "CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME", "DATE",
  "DATETIME", "TIMESTAMP", "YEAR", "MONTH", "DAY", "HOUR", "MINUTE", "SECOND",
  "DATEDIFF", "DATE_ADD", "DATE_SUB", "IFNULL", "GROUP_CONCAT", "ROW_NUMBER",
  "RANK", "DENSE_RANK", "LAG", "LEAD", "CAST", "CONVERT", "JSON_EXTRACT",
  "JSON_ARRAY", "JSON_OBJECT", "GREATEST", "LEAST", "RAND", "UUID",
];

/** SQL 语法配色（v3 · 与设计令牌同源的冷色系）。 */
const SQL_HIGHLIGHT = HighlightStyle.define([
  { tag: t.keyword, color: "#2b62d9", fontWeight: "600" },
  { tag: [t.string, t.special(t.string)], color: "#1f9254" },
  { tag: [t.number, t.integer, t.float], color: "#b54708" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "#7d8794", fontStyle: "italic" },
  { tag: [t.operator, t.operatorKeyword], color: "#3c434e" },
  { tag: [t.punctuation, t.bracket], color: "#5c6673" },
  { tag: t.typeName, color: "#7a5af8" },
]);

export interface SqlEditorOptions {
  /** 是否启用自动补全。 */
  autoComplete?: boolean;
  /** 字体族。 */
  fontFamily?: string;
  /** 字号。 */
  fontSize?: number;
  /** 表名 / 列名补全数据。 */
  completionData?: () => Record<string, string[]>;
  /** 初始 SQL。 */
  value?: string;
  /** 内容变化回调。 */
  onChange?: (value: string) => void;
  /** 执行回调；所有入口均优先实际选区。 */
  onExecute?: (sql: string, selectionOnly: boolean) => void;
  /** 解释选区或全文。 */
  onExplain?: (sql: string) => void;
  /** 当前连接标识符引用风格。 */
  identifierQuote?: () => string;
  /** 格式化回调。 */
  onFormat?: () => void;
  /** 美化 SQL 回调。 */
  onBeautify?: () => void;
}

export class SqlEditor {
  readonly el: HTMLElement;
  private view: EditorView;
  private opts: SqlEditorOptions;

  constructor(opts: SqlEditorOptions = {}) {
    this.opts = opts;
    this.el = document.createElement("div");
    this.el.className = "sql-editor";

    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightActiveLine(),
      history(),
      drawSelection(),
      rectangularSelection(),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      autocompletion({
        override: [this.sqlCompletions.bind(this)],
        activateOnTyping: true,
        closeOnBlur: true,
        icons: false,
      }),
      highlightSelectionMatches(),
      sql({
        upperCaseKeywords: true,
        defaultSchema: "public",
      }),
      keymap.of([
        {
          key: "Mod-Enter",
          run: () => {
            this.executeCurrent();
            return true;
          },
        },
        {
          key: "Mod-Shift-Enter",
          run: () => {
            this.executeCurrent();
            return true;
          },
        },
        {
          key: "Mod-l",
          run: () => {
            this.opts.onFormat?.();
            return true;
          },
        },
        {
          key: "F5",
          run: () => {
            this.executeCurrent();
            return true;
          },
        },
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        ...searchKeymap,
        indentWithTab,
      ]),
      syntaxHighlighting(SQL_HIGHLIGHT),
      EditorView.contentAttributes.of({ "aria-label": "SQL 编辑器" }),
      EditorView.lineWrapping,
      EditorState.tabSize.of(4),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          this.opts.onChange?.(u.state.doc.toString());
          this.maybeStartCompletion(u);
        }
      }),
    ];

    if (opts.fontFamily) {
      extensions.push(
        EditorView.theme({
          "&": { fontFamily: opts.fontFamily },
        }),
      );
    }
    if (opts.fontSize) {
      extensions.push(
        EditorView.theme({
          ".cm-content": { fontSize: `${opts.fontSize}px` },
          ".cm-gutters": { fontSize: `${opts.fontSize - 1}px` },
        }),
      );
    }

    this.view = new EditorView({
      state: EditorState.create({
        doc: opts.value ?? "",
        extensions,
      }),
      parent: this.el,
    });
    this.el.addEventListener("contextmenu", this.onContextMenu);
    this.el.addEventListener("keydown", this.onMenuShortcut);
  }

  /** 获取当前 SQL。 */
  getSql(): string {
    return this.view.state.doc.toString();
  }

  /**
   * 输入补全触发兜底：个别环境（WebView2 输入管线差异）下 activateOnTyping
   * 不生效，这里在「编辑器聚焦 + 本次变更末尾插入了单词字符」时显式拉起补全。
   * startCompletion 与 CM 内部激活机制幂等，重复调用无副作用。
   */
  private maybeStartCompletion(u: import("@codemirror/view").ViewUpdate): void {
    if (this.opts.autoComplete === false) return;
    if (!u.view.hasFocus) return;
    let insertedTail = "";
    u.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
      const s = inserted.toString();
      if (s) insertedTail = s;
    });
    if (!/[\p{L}\p{N}_$.\s]$/u.test(insertedTail)) return;
    // 光标紧跟在刚输入的字符之后才弹（编辑历史中间位置不打扰）
    const pos = u.state.selection.main.head;
    if (pos === 0) return;
    const before = u.state.sliceDoc(pos - 1, pos);
    if (!/[\p{L}\p{N}_$.\s]/u.test(before)) return;
    if (/\s/.test(before) && !/\b(?:FROM|JOIN|INTO|UPDATE|TABLE)\s+$/i.test(u.state.sliceDoc(0, pos))) return;
    startCompletion(u.view);
  }

  /** 获取选中的 SQL（无选区时返回全文）。 */
  getSelection(): string {
    const range = this.view.state.selection.main;
    return sqlExecutionText(this.getSql(), range.from, range.to);
  }

  /** 是否有选区。 */
  hasSelection(): boolean {
    return !this.view.state.selection.main.empty;
  }

  /** 设置 SQL。 */
  setSql(sql: string): void {
    this.view.dispatch({
      changes: { from: 0, to: this.view.state.doc.length, insert: sql },
    });
  }

  /** 全选（编辑菜单「全选」）。 */
  selectAll(): void {
    this.view.dispatch({
      selection: EditorSelection.range(0, this.view.state.doc.length),
    });
  }

  /** 打开查找/替换面板（编辑菜单「查找」「替换」）。 */
  openSearch(): void {
    this.view.focus();
    openSearchPanel(this.view);
  }

  /** 查找下一个。 */
  findNext(): void {
    findNext(this.view);
  }

  /** 撤消。 */
  undo(): void {
    undo(this.view);
  }

  /** 重做。 */
  redo(): void {
    redo(this.view);
  }

  /** 用文本替换当前选区（剪切 / 粘贴共用）。 */
  replaceSelection(text: string): void {
    const range = this.view.state.selection.main;
    this.view.dispatch({
      changes: { from: range.from, to: range.to, insert: text },
      selection: { anchor: range.from + text.length },
    });
  }

  /** 在光标处插入文本。 */
  insertText(text: string): void {
    const pos = this.view.state.selection.main.head;
    this.view.dispatch({
      changes: { from: pos, insert: text },
      selection: { anchor: pos + text.length },
    });
  }

  /** 聚焦编辑器。 */
  focus(): void {
    this.view.focus();
  }

  /** 更新真实元数据；schema用于数据库/模式限定名。 */
  setCompletionData(data: Record<string, string[]>, schema = ""): void {
    this.completionData = data;
    this.completionSchema = schema;
  }

  private completionData: Record<string, string[]> = {};
  private completionSchema = "";
  private selectionMenu: HTMLElement | null = null;

  /** 所有快捷键及选区菜单运行入口共用。 */
  private executeCurrent(): void {
    this.opts.onExecute?.(this.getSelection(), this.hasSelection());
  }

  private sqlCompletions(context: CompletionContext): CompletionResult | null {
    if (this.opts.autoComplete === false) return null;
    const text = context.state.doc.toString();
    const result = analyzeSqlCompletion(text, context.pos, this.opts.completionData?.() ?? this.completionData,
      SQL_KEYWORDS, SQL_FUNCTIONS, this.completionSchema, this.opts.identifierQuote?.() ?? '"');
    if (!result?.options.length) return null;
    if (result.from === context.pos && !context.explicit && !/[.]$/.test(text.slice(0, context.pos)) &&
      !/\b(?:FROM|JOIN|INTO|UPDATE|TABLE)\s+$/i.test(text.slice(0, context.pos))) return null;
    return { ...result, filter: false, validFor: undefined };
  }

  private onContextMenu = (event: MouseEvent): void => {
    // Do not replace the native editor menu without an actual selection.
    if (!this.hasSelection()) return;
    event.preventDefault();
    event.stopPropagation();
    this.showSelectionMenu(event.clientX, event.clientY);
  };

  private onMenuShortcut = (event: KeyboardEvent): void => {
    if (!this.hasSelection() || !(event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) return;
    event.preventDefault();
    const rect = this.view.coordsAtPos(this.view.state.selection.main.head) ?? this.el.getBoundingClientRect();
    this.showSelectionMenu(rect.left, rect.bottom);
  };

  /** Native buttons, roving focus, Escape/Tab close, and selection-preserving actions. */
  private showSelectionMenu(x: number, y: number): void {
    this.closeSelectionMenu(false);
    const menu = document.createElement("div");
    menu.className = "menu-popup sql-selection-menu";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", "SQL 选区操作");
    const selected = this.getSelection();
    const documentBefore = this.view.state.doc;
    const rangeBefore = this.view.state.selection.main;
    const selectionUnchanged = () => this.view.state.doc === documentBefore && this.hasSelection() &&
      this.view.state.selection.main.from === rangeBefore.from && this.view.state.selection.main.to === rangeBefore.to;
    const actions: { label: string; shortcut?: string; run: () => void | Promise<void>; disabled?: boolean }[] = [
      { label: "运行已选择的", shortcut: "Ctrl+Enter", run: () => this.executeCurrent(), disabled: !this.opts.onExecute },
      { label: "解释已选择的", run: () => this.opts.onExplain?.(this.getSelection()), disabled: !this.opts.onExplain },
      { label: "复制", shortcut: "Ctrl+C", run: () => navigator.clipboard.writeText(selected), disabled: !navigator.clipboard?.writeText },
      { label: "剪切", shortcut: "Ctrl+X", run: async () => {
        await navigator.clipboard.writeText(selected);
        // An async clipboard operation must not delete a newly changed selection/document.
        if (selectionUnchanged()) this.replaceSelection("");
      }, disabled: !navigator.clipboard?.writeText },
      { label: "粘贴", shortcut: "Ctrl+V", run: async () => {
        const text = await navigator.clipboard.readText();
        if (selectionUnchanged()) this.replaceSelection(text);
      }, disabled: !navigator.clipboard?.readText },
      { label: "删除选区", run: () => this.replaceSelection("") },
      { label: "查找 / 替换", shortcut: "Ctrl+F", run: () => this.openSearch() },
      { label: "全选", shortcut: "Ctrl+A", run: () => this.selectAll() },
    ];
    for (const action of actions) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "menu-entry" + (action.disabled ? " disabled" : "");
      row.setAttribute("role", "menuitem");
      row.disabled = !!action.disabled;
      row.tabIndex = -1;
      const icon = document.createElement("span");
      icon.className = "menu-entry-icon";
      icon.setAttribute("aria-hidden", "true");
      const label = document.createElement("span");
      label.className = "menu-entry-label";
      label.textContent = action.label;
      row.append(icon, label);
      if (action.shortcut) {
        const shortcut = document.createElement("span");
        shortcut.className = "menu-entry-shortcut";
        shortcut.textContent = action.shortcut;
        row.appendChild(shortcut);
      }
      row.addEventListener("click", () => {
        this.closeSelectionMenu(true);
        Promise.resolve().then(action.run).catch(() => {
          // Clipboard permissions can fail; announce this instead of silently deleting text.
          const status = document.createElement("div");
          status.className = "sql-editor-status";
          status.setAttribute("role", "status");
          status.textContent = "剪贴板操作失败，请使用键盘快捷键。";
          this.el.querySelector(".sql-editor-status")?.remove();
          this.el.appendChild(status);
        });
      });
      menu.appendChild(row);
    }
    menu.addEventListener("keydown", (event) => {
      const rows = [...menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
      const index = rows.indexOf(document.activeElement as HTMLButtonElement);
      let next: number | undefined;
      if (event.key === "ArrowDown") next = (index + 1) % rows.length;
      if (event.key === "ArrowUp") next = (index - 1 + rows.length) % rows.length;
      if (event.key === "Home") next = 0;
      if (event.key === "End") next = rows.length - 1;
      if (next !== undefined) { event.preventDefault(); event.stopPropagation(); rows[next]?.focus(); }
    });
    document.body.appendChild(menu);
    this.selectionMenu = menu;
    const rect = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(2, Math.min(x, window.innerWidth - rect.width - 2))}px`;
    menu.style.top = `${Math.max(2, Math.min(y, window.innerHeight - rect.height - 2))}px`;
    menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
    document.addEventListener("pointerdown", this.onOutsideMenu, true);
    document.addEventListener("keydown", this.onMenuCloseKey, true);
    document.addEventListener("focusin", this.onMenuFocus, true);
    window.addEventListener("blur", this.onMenuBlur);
  }

  private onOutsideMenu = (event: PointerEvent): void => {
    if (!this.selectionMenu?.contains(event.target as Node)) this.closeSelectionMenu(false);
  };
  private onMenuCloseKey = (event: KeyboardEvent): void => {
    if (event.key === "Escape" || event.key === "Tab") {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); }
      this.closeSelectionMenu(true);
    }
  };
  private onMenuFocus = (event: FocusEvent): void => {
    if (!this.selectionMenu?.contains(event.target as Node)) this.closeSelectionMenu(false);
  };
  private onMenuBlur = (): void => this.closeSelectionMenu(false);
  private closeSelectionMenu(restoreFocus: boolean): void {
    const menu = this.selectionMenu;
    this.selectionMenu = null;
    menu?.remove();
    document.removeEventListener("pointerdown", this.onOutsideMenu, true);
    document.removeEventListener("keydown", this.onMenuCloseKey, true);
    document.removeEventListener("focusin", this.onMenuFocus, true);
    window.removeEventListener("blur", this.onMenuBlur);
    if (menu && restoreFocus) this.view.focus();
  }

  /** 销毁。 */
  destroy(): void {
    this.closeSelectionMenu(false);
    this.el.removeEventListener("contextmenu", this.onContextMenu);
    this.el.removeEventListener("keydown", this.onMenuShortcut);
    this.view.destroy();
  }
}
