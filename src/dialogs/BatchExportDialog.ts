import { api, batchExportApi, type BatchExportObject, type BatchExportRequest, type BatchExportResult } from "../core/api";
import { bindExportPath } from "../core/export-path";
import { formatError } from "../core/execution";
import { namespaceLabel } from "../core/database-picker";
import type { ConnectionConfig } from "../core/types";
import { Dialog, field, select, textInput } from "../ui/Dialog";
import { Wizard } from "../ui/Wizard";
import { accessibleDialog, taskFeedback } from "./SyncDialog";
import { taskCenter } from "../core/backgroundTasks";

export function batchOutputPreview(base: string, object: BatchExportObject, index: number, format: string): string {
  const boundary = Math.max(base.lastIndexOf("/"), base.lastIndexOf("\\"));
  const name = base.slice(boundary + 1), dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const part = (value: string) => Array.from(value.replace(/[<>:"/\\|?*\p{Cc}]/gu, "_")).slice(0, 80).join("").replace(/[. ]+$/, "") || "object";
  return `${base.slice(0, boundary + 1)}${part(stem)}_${String(index + 1).padStart(3, "0")}_${part(object.name)}.${format}`;
}
/** Table/view-only export. Opening with no selection shows the real collection picker. */
export function openBatchExportWizard(connection: ConnectionConfig, namespace: string, preselected: readonly BatchExportObject[], onDone?: (result: BatchExportResult) => void): Dialog {
  let objects: BatchExportObject[] = [], loaded = false, loadingError = "", running = false, closed = false;
  const selected = new Set(preselected.map(object => `${object.object_type}\0${object.name}`));
  const key = (object: BatchExportObject) => `${object.object_type}\0${object.name}`;
  const list = document.createElement("div"); list.className = "form-check-group"; list.style.maxHeight = "280px"; list.style.overflow = "auto";
  list.setAttribute("role", "group"); list.setAttribute("aria-label", "选择导出的表和视图");
  const filter = textInput(""); filter.placeholder = "筛选表或视图"; filter.setAttribute("aria-label", "筛选导出对象");
  const metadata = document.createElement("div"); metadata.setAttribute("role", "status"); metadata.setAttribute("aria-live", "polite"); metadata.textContent = "正在读取真实表与视图清单…";
  const count = document.createElement("div"); count.setAttribute("aria-live", "polite");
  const source = document.createElement("p"); source.textContent = `${connection.name} / ${namespaceLabel(connection)}：${namespace}${connection.database ? `（连接数据库：${connection.database}）` : ""}`;
  const all = document.createElement("button"), clear = document.createElement("button");
  for (const button of [all, clear]) { button.className = "btn"; button.type = "button"; }
  all.textContent = "选择全部可见对象"; clear.textContent = "清空选择";
  const actions = document.createElement("div"); actions.className = "form-control-line"; actions.append(all, clear);
  function selectedObjects(): BatchExportObject[] { return objects.filter(object => selected.has(key(object))); }
  function refreshObjects(): void {
    list.replaceChildren(); const search = filter.value.trim().toLocaleLowerCase();
    for (const object of objects.filter(object => object.name.toLocaleLowerCase().includes(search))) {
      const label = document.createElement("label"); label.className = "checkbox";
      const check = document.createElement("input"); check.type = "checkbox"; check.checked = selected.has(key(object));
      check.addEventListener("change", () => { if (check.checked) selected.add(key(object)); else selected.delete(key(object)); count.textContent = `已选择 ${selectedObjects().length} / ${objects.length} 个对象`; });
      const text = document.createElement("span"); text.textContent = `${object.name}（${object.object_type === "view" ? "视图" : "表"}）`; label.append(check, text); list.append(label);
    }
    count.textContent = `已选择 ${selectedObjects().length} / ${objects.length} 个对象`;
  }
  filter.addEventListener("input", refreshObjects);
  all.addEventListener("click", () => { for (const object of objects.filter(object => object.name.toLocaleLowerCase().includes(filter.value.trim().toLocaleLowerCase()))) selected.add(key(object)); refreshObjects(); });
  clear.addEventListener("click", () => { selected.clear(); refreshObjects(); });
  const objectPanel = document.createElement("div"); objectPanel.append(source, filter, actions, metadata, list, count);
  const format = select([{value:"csv",label:"CSV（每对象一个文件）"},{value:"json",label:"JSON（每对象一个文件）"},{value:"sql",label:"SQL（表数据 / 真实视图定义）"}], "csv"); format.setAttribute("aria-label", "导出格式");
  const encoding = select([{value:"utf-8",label:"UTF-8"},{value:"gbk",label:"GBK"},{value:"big5",label:"Big5"},{value:"latin1",label:"ISO-8859-1"}],"utf-8"); encoding.setAttribute("aria-label", "导出编码");
  const file = textInput(""); file.setAttribute("aria-label", "批量导出完整保存路径");
  const path = bindExportPath(file, `${namespace}_export.csv`, format);
  const combined = document.createElement("input"); combined.type = "checkbox"; combined.checked = true; combined.disabled = true;
  const combinedLabel = document.createElement("label"); combinedLabel.className = "checkbox"; combinedLabel.append(combined, document.createTextNode("SQL 合并为一个文件"));
  format.addEventListener("change", () => { combined.disabled = format.value !== "sql"; });
  const browse = document.createElement("button"); browse.type = "button"; browse.className = "btn"; browse.textContent = "浏览…";
  browse.addEventListener("click", async () => {
    try {
      await path.ready;
      const { save } = await import("@tauri-apps/plugin-dialog");
      const output = await save({ title:"保存批量导出",defaultPath:file.value,filters:[{name:format.value.toUpperCase(),extensions:[format.value]}] });
      if (output) { file.value = output; path.refresh(); }
    } catch (error) { console.error("选择批量导出路径失败", error); dlg.showError(formatError(error)); }
  });
  const fileLine = document.createElement("div"); fileLine.className = "form-control-line"; fileLine.append(file,browse);
  const note = document.createElement("p"); note.textContent = "分文件时使用所选文件名作为前缀，添加序号和对象名；完整输出路径在下一步逐项列出。逐对象一致读取，SQL Server 可能持有读锁；取消在对象结束后生效，已完成文件保留。SQL 视图仅导出真实定义，需先具备依赖对象。已有文件不会覆盖。";
  const outputPanel = document.createElement("div"); outputPanel.append(field("格式:",format),field("编码:",encoding),combinedLabel,field("保存路径与文件名前缀:",fileLine),note);
  const summary = document.createElement("div"); summary.style.maxHeight="220px"; summary.style.overflow="auto";
  const results = document.createElement("div"); results.setAttribute("aria-label","逐对象导出结果");
  const status = document.createElement("div"); status.setAttribute("role","status"); status.setAttribute("aria-live","polite");
  const executePanel = document.createElement("div"); executePanel.append(summary,status,results);
  function refreshSummary(): void { summary.replaceChildren(); selectedObjects().forEach((object,index) => { const row=document.createElement("p"); row.textContent=`${object.name} → ${format.value === "sql" && combined.checked ? file.value : batchOutputPreview(file.value,object,index,format.value)}`; row.title=row.textContent; summary.append(row); }); }
  const dlg = new Dialog({ title:"批量导出表与视图",width:760,height:600,render:()=>{},buttons:[{label:"关闭",onClick:()=>!running}] });
  const feedback = taskFeedback(dlg,status);
  const wizard = new Wizard({ steps:[
    {label:"选择对象",hint:"明确选择表或视图；空集合不会导出全库。",render:host=>host.append(objectPanel),validate:()=>!loaded?loadingError||"正在读取对象清单，请稍候":selectedObjects().length===0?"请至少选择一个表或视图":selectedObjects().length>256?"一次最多导出 256 个对象":null},
    {label:"输出文件",hint:"选择格式与完整保存路径。",render:host=>host.append(outputPanel),validate:path.problem},
    {label:"核对与执行",hint:"核对逐对象路径后开始导出。",render:host=>host.append(executePanel)}],onEnter:index=>{if(index===2)refreshSummary();},finishLabel:"开始导出",onFinish:async()=>{
      if(running)return;
      const problem=path.problem(); if(problem){dlg.showError(problem);return;}
      running=true; wizard.setBusy(true); results.replaceChildren();
      const taskId=feedback.begin(); status.textContent="正在逐对象导出；取消将在当前对象读取结束后生效…";
      const exportHandle = taskCenter.register("batch-export", `批量导出 · ${namespace}（${selectedObjects().length} 个对象）`);
      const request: BatchExportRequest={connection_id:connection.id,namespace,objects:selectedObjects(),format:format.value as BatchExportRequest["format"],encoding:encoding.value,delimiter:",",has_header:true,task_id:taskId,destination:format.value==="sql"&&combined.checked?{mode:"sql_file",file:file.value.trim()}:{mode:"files",base_file:file.value.trim()}};
      try{const result=await batchExportApi.run(request); feedback.finish(); const okItems=result.items.filter(item=>item.status==="completed").length; const failedItems=result.items.filter(item=>item.status==="failed").length; exportHandle.finish(`完成 ${okItems} · 失败 ${failedItems}`, result.combined_file ?? undefined); for(const item of result.items){const row=document.createElement("p");row.textContent=`${item.object.name}：${({completed:"完成",failed:"失败",unknown:"结果未知",skipped:"跳过"})[item.status]}，${item.rows} 行${item.file?`；${item.file}`:""}${item.error?`；${item.error}`:""}`;row.title=row.textContent;results.append(row);} status.textContent=result.message;onDone?.(result);}
      catch(error){feedback.finish();exportHandle.fail(String(error));dlg.showError(String(error));status.textContent=`批量导出失败：${String(error)}`;}
      finally{running=false;wizard.setBusy(false);combined.disabled=format.value!=="sql";}
    }});
  dlg.body.append(wizard.stepsEl,wizard.bodyEl);dlg.footer.prepend(...wizard.buttons());
  // 固定内容区高度：三步内容长短不同，切步时窗口不跳动。
  dlg.body.style.padding="0";dlg.body.style.display="flex";dlg.body.style.flexDirection="column";dlg.body.style.overflow="hidden";
  wizard.bodyEl.style.minHeight="0";wizard.bodyEl.style.overflow="hidden";
  accessibleDialog(dlg,"批量导出表与视图");
  dlg.onClose(()=>{closed=true;});
  void (async()=>{try{const ready=await api.ensureConnection(connection.id);if(!ready.ok)throw new Error(ready.message||"连接失败");const rows=await api.listTables(connection.id,namespace);if(closed)return;objects=rows.filter((row):row is [string,"table"|"view"]=>row[1]==="table"||row[1]==="view").map(([name,object_type])=>({name,object_type}));loaded=true;const missing=preselected.filter(object=>!objects.some(current=>key(object)===key(current)));metadata.textContent=objects.length?missing.length?`${missing.length} 个旧选中对象已不在目录中，请核对当前清单。`:"真实表与视图清单已加载。":"当前命名空间没有可导出的表或视图。";refreshObjects();}catch(error){if(closed)return;loadingError=`对象清单读取失败：${String(error)}`;metadata.textContent=loadingError;}})();
  return dlg;
}
