import { Dialog } from "../ui/Dialog";
import type { TableDesignPreview } from "../core/api";

function elem<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

interface Classified { kind: string; cls: string; destructive: boolean; }
function classify(step: { label: string; sql: string }): Classified {
  const s = step.sql.trim().toLowerCase();
  const destructive = /drop/.test(s);
  if (/^creates+table/.test(s)) return { kind: "新建", cls: "new", destructive: false };
  if (destructive) return { kind: "删除", cls: "drop", destructive: true };
  if (/alters+table/.test(s) && /adds+column/.test(s)) return { kind: "新增列", cls: "alter", destructive: false };
  return { kind: "修改", cls: "alter", destructive: false };
}

export interface ErSyncOptions { connectionName: string; schema: string; table: string; preview: TableDesignPreview; }

/** 按设计稿渲染「同步到数据库」变更清单 + 执行脚本预览；返回是否确认执行。 */
export function reviewErSyncPlan(opts: ErSyncOptions): Promise<boolean> {
  return new Promise((resolve) => {
    let confirmed = false;
    const preview = opts.preview;
    const sqlText = preview.steps.map((s, i) => "-- " + (i + 1) + ". " + s.label + "\n" + s.sql).join("\n\n");
    const dlg = new Dialog({
      title: "同步到数据库",
      width: 900,
      closable: false,
      render: (body) => {
        body.classList.add("er-sync-body");
        const wrap = elem("div", "er-sync");
        wrap.appendChild(elem("div", "er-sync-sub", "连接 " + opts.connectionName + " · 数据库 " + opts.schema + " · 将执行 " + preview.steps.length + " 项结构变更"));
        const hasDestructive = preview.destructive || preview.steps.some((s) => classify(s).destructive);
        if (hasDestructive) wrap.appendChild(elem("div", "er-sync-risk", "⚠ 检测到破坏性变更（DROP），执行后不可恢复，请确认后再执行。"));
        if (!preview.steps.length) wrap.appendChild(elem("p", "er-sync-empty", "当前没有待执行的结构变更。"));
        for (const step of preview.steps) {
          const info = classify(step);
          const row = elem("div", "er-sync-chg" + (info.destructive ? " danger" : ""));
          const cb = elem("input");
          cb.type = "checkbox"; cb.checked = true; cb.disabled = true;
          cb.title = "后端计划为整体执行，不能逐项拆分";
          const main = elem("div", "er-sync-main");
          const t1 = elem("div", "er-sync-t1");
          t1.appendChild(elem("span", "er-pill " + info.cls, info.kind));
          t1.appendChild(elem("b", undefined, step.label));
          main.appendChild(t1);
          const code = elem("pre", "er-sync-code"); code.textContent = step.sql; main.appendChild(code);
          row.append(cb, main); wrap.appendChild(row);
        }
        if (preview.steps.length) {
          wrap.appendChild(elem("div", "er-sync-sec", "执行脚本预览"));
          const pre = elem("pre", "er-sync-sql"); pre.textContent = sqlText; wrap.appendChild(pre);
        }
        body.appendChild(wrap);
      },
      buttons: [
        { label: "取消" },
        { label: "复制 SQL", onClick: () => { void navigator.clipboard?.writeText(sqlText).catch(() => undefined); return false; } },
        { label: "执行同步（" + preview.steps.length + "）", primary: true, onClick: () => { confirmed = true; } },
      ],
    });
    dlg.onClose(() => resolve(confirmed));
  });
}
