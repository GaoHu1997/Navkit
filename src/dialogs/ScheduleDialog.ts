/** 自动运行：任务编辑对话框 + 运行历史对话框。 */

import { api } from "../core/api";
import type { ConnectionConfig, JobHistoryEntry, ScheduleJob } from "../core/types";
import { DataGrid } from "../ui/DataGrid";
import { checkbox, Dialog, field, select, textInput } from "../ui/Dialog";

const INTERVALS: { value: number; label: string }[] = [
  { value: 0, label: "仅手动执行" },
  { value: 60, label: "每 1 分钟" },
  { value: 300, label: "每 5 分钟" },
  { value: 1800, label: "每 30 分钟" },
  { value: 3600, label: "每 1 小时" },
  { value: 21600, label: "每 6 小时" },
  { value: 86400, label: "每天" },
];

/** 打开任务编辑对话框（新建时 job 传空壳）。 */
export function openScheduleJobDialog(
  connections: ConnectionConfig[],
  job: ScheduleJob | null,
  onSaved: () => void,
): Dialog {
  const isEdit = job !== null && job.id !== "";
  const nameInput = textInput(job?.name ?? "", 240);
  const connSelect = select(
    connections.map((c) => ({ value: c.id, label: `${c.name}（${c.kind}）` })),
    job?.connection_id ?? connections[0]?.id ?? "",
    220,
  );
  const dbInput = textInput(job?.database ?? "", 220);
  const sqlArea = document.createElement("textarea");
  sqlArea.value = job?.sql ?? "";
  sqlArea.rows = 9;
  sqlArea.style.cssText =
    "width:100%;font-family:Consolas,monospace;font-size:12.5px;border:1px solid var(--border,#B9B9B9);padding:6px;box-sizing:border-box";
  const intervalSelect = select(
    INTERVALS.map((i) => ({ value: String(i.value), label: i.label })),
    String(job?.interval_secs ?? 0),
    140,
  );
  const enabled = checkbox("启用任务", job?.enabled ?? true);
  const status = document.createElement("div");
  status.style.cssText = "margin-top:6px;min-height:16px;color:var(--fg-muted)";

  const dlg = new Dialog({
    title: isEdit ? "编辑任务" : "新建任务",
    width: 520,
    render: (body) => {
      body.appendChild(field("任务名:", nameInput));
      body.appendChild(field("连接:", connSelect));
      body.appendChild(field("数据库:", dbInput));
      const lbl = document.createElement("div");
      lbl.className = "field-label";
      lbl.textContent = "SQL:";
      body.appendChild(lbl);
      body.appendChild(sqlArea);
      body.appendChild(field("执行间隔:", intervalSelect));
      body.appendChild(enabled);
      body.appendChild(status);
    },
    buttons: [
      {
        label: "保存",
        primary: true,
        onClick: async () => {
          if (!nameInput.value.trim()) {
            dlg.showError("请填写任务名");
            return false;
          }
          if (!sqlArea.value.trim()) {
            dlg.showError("请填写要执行的 SQL");
            return false;
          }
          try {
            await api.saveJob({
              id: job?.id ?? "",
              name: nameInput.value.trim(),
              connection_id: connSelect.value,
              database: dbInput.value.trim(),
              sql: sqlArea.value,
              interval_secs: Number(intervalSelect.value),
              enabled: enabled.querySelector("input")!.checked,
              last_run: job?.last_run ?? null,
              last_status: job?.last_status ?? null,
            });
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  return dlg;
}

/** 运行历史对话框。 */
export function openJobHistoryDialog(history: JobHistoryEntry[], jobId?: string): Dialog {
  const grid = new DataGrid({ onRowDoubleClick: () => {} });
  const rows = (jobId ? history.filter((h) => h.job_id === jobId) : history).map((h) => [
    h.job_name,
    h.started_at,
    h.ok ? "成功" : "失败",
    `${(h.elapsed_ms / 1000).toFixed(2)} s`,
    h.detail,
  ]);
  const dlg = new Dialog({
    title: "运行历史",
    width: 760,
    height: 460,
    render: (body) => {
      body.style.display = "flex";
      body.style.flexDirection = "column";
      body.appendChild(grid.el);
      grid.setData({
        columns: [
          { name: "任务", type: "text", nullable: false, primary_key: false, auto_increment: false },
          { name: "开始时间", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "状态", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "耗时", type: "text", nullable: true, primary_key: false, auto_increment: false },
          { name: "详情", type: "text", nullable: true, primary_key: false, auto_increment: false },
        ],
        rows,
        rowIcons: rows.map(() => "info_pane_query"),
        showRowNumbers: false,
      });
    },
    buttons: [{ label: "关闭" }],
  });
  return dlg;
}
