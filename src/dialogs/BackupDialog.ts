/** 备份对话框：选择对象与选项，执行整库备份。 */

import { api } from "../core/api";
import type { ConnectionConfig } from "../core/types";
import { Dialog, field, textInput } from "../ui/Dialog";
import { iconSvgByName } from "../ui/icons";

/** 打开「新建备份」对话框。
 *  onDone(rows, bytes) 在备份成功后回调（用于刷新备份列表与提示）。 */
export function openBackupDialog(
  connection: ConnectionConfig,
  schema: string,
  onDone: (rows: number, bytes: number) => void,
): Dialog {
  const commentInput = textInput("", 300);
  const incStructure = document.createElement("input");
  incStructure.type = "checkbox";
  incStructure.checked = true;
  const incData = document.createElement("input");
  incData.type = "checkbox";
  incData.checked = true;
  const status = document.createElement("div");
  status.style.marginTop = "8px";
  status.style.minHeight = "18px";
  status.style.color = "var(--fg-muted)";

  // 对象勾选列表
  const listWrap = document.createElement("div");
  listWrap.style.border = "1px solid var(--border)";
  listWrap.style.height = "180px";
  listWrap.style.overflowY = "auto";
  listWrap.style.padding = "4px 6px";
  listWrap.style.background = "var(--bg, #fff)";

  const checkAll = document.createElement("input");
  checkAll.type = "checkbox";
  checkAll.checked = true;
  const checkAllLabel = document.createElement("label");
  checkAllLabel.className = "checkbox";
  checkAllLabel.appendChild(checkAll);
  const allSpan = document.createElement("span");
  allSpan.textContent = "全选";
  checkAllLabel.appendChild(allSpan);

  const rowChecks: { name: string; view: boolean; input: HTMLInputElement }[] = [];

  const dlg = new Dialog({
    title: "新建备份",
    width: 460,
    render: (body) => {
      body.appendChild(field("连接:", textInput(connection.name, 220)));
      body.appendChild(field("数据库:", textInput(schema, 220)));
      body.appendChild(field("注释:", commentInput));

      const l1 = document.createElement("label");
      l1.className = "checkbox";
      l1.appendChild(incStructure);
      const t1 = document.createElement("span");
      t1.textContent = "包含结构";
      l1.appendChild(t1);
      body.appendChild(l1);

      const l2 = document.createElement("label");
      l2.className = "checkbox";
      l2.appendChild(incData);
      const t2 = document.createElement("span");
      t2.textContent = "包含数据";
      l2.appendChild(t2);
      body.appendChild(l2);

      // 对象列表
      const listLabel = document.createElement("div");
      listLabel.style.margin = "10px 0 4px";
      listLabel.style.fontWeight = "600";
      listLabel.textContent = "对象";
      body.appendChild(listLabel);
      body.appendChild(listWrap);
      body.appendChild(checkAllLabel);
      body.appendChild(status);

      checkAll.addEventListener("change", () => {
        for (const it of rowChecks) it.input.checked = checkAll.checked;
      });

      // 加载表 / 视图清单
      void (async () => {
        status.textContent = "正在加载对象列表...";
        try {
          const tables = await api.listTables(connection.id, schema);
          listWrap.innerHTML = "";
          for (const [name, ty] of tables) {
            const row = document.createElement("label");
            row.className = "checkbox";
            row.style.display = "flex";
            row.style.alignItems = "center";
            row.style.gap = "5px";
            const input = document.createElement("input");
            input.type = "checkbox";
            input.checked = true;
            const icon = document.createElement("span");
            icon.style.display = "inline-flex";
            icon.innerHTML = iconSvgByName(ty === "view" ? "info_pane_view" : "info_pane_table");
            const span = document.createElement("span");
            span.textContent = name;
            row.appendChild(input);
            row.appendChild(icon);
            row.appendChild(span);
            listWrap.appendChild(row);
            rowChecks.push({ name, view: ty === "view", input });
            input.addEventListener("change", () => {
              checkAll.checked = rowChecks.every((it) => it.input.checked);
            });
          }
          if (tables.length === 0) {
            listWrap.textContent = "（该数据库没有可备份的表或视图）";
            listWrap.style.color = "var(--fg-muted)";
          }
          status.textContent = "";
        } catch (e) {
          status.textContent = `对象列表加载失败：${String(e)}`;
          dlg.showError(String(e));
        }
      })();
    },
    buttons: [
      {
        label: "开始",
        primary: true,
        onClick: async () => {
          const tables = rowChecks.filter((it) => it.input.checked).map((it) => it.name);
          if (rowChecks.length > 0 && tables.length === 0) {
            dlg.showError("请至少选择一个要备份的对象");
            return false;
          }
          if (!incStructure.checked && !incData.checked) {
            dlg.showError("「包含结构」与「包含数据」至少选择一项");
            return false;
          }
          status.textContent = "正在备份，请稍候...";
          try {
            const [rows, bytes] = await api.createBackup({
              connection_id: connection.id,
              schema,
              comment: commentInput.value.trim(),
              tables,
              include_structure: incStructure.checked,
              include_data: incData.checked,
            });
            status.textContent = `备份完成：${rows} 行数据，文件 ${(bytes / 1024).toFixed(1)} KB。`;
            onDone(rows, bytes);
          } catch (e) {
            status.textContent = `备份失败：${String(e)}`;
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "关闭" },
    ],
  });

  return dlg;
}
