/** 图表页的原生键盘控件、SVG与可访问数据替代内容。 */
import { numericChartColumns, projectChart, renderDataChart, type ChartState } from "../core/chart";

export class ChartPanel {
  readonly el = document.createElement("section");
  private chartHost = document.createElement("div");
  private status = document.createElement("div");
  private dataBody = document.createElement("tbody");
  private xHeading = document.createElement("th");
  private yHeading = document.createElement("th");

  constructor(readonly state: ChartState) {
    this.el.className = "chart-panel";
    this.el.setAttribute("aria-label", `图表：${state.source.title}`);
    this.el.style.cssText = "display:flex;flex-direction:column;flex:1;min-height:0;overflow:auto;padding:16px;gap:12px;background:var(--bg-surface);";
    const title = document.createElement("h2");
    title.textContent = state.source.title;
    title.style.cssText = "font-size:14px;margin:0;color:var(--text);";
    const note = document.createElement("div");
    note.textContent = `${state.source.description}；共 ${state.source.rows.length} 行。此图表保留创建时快照，切换或关闭来源页不会改变它；数值坐标为近似刻度，原始精度可查看下方数据表。关闭图表后不保留定义。`;
    note.style.cssText = "font-size:12px;color:var(--text-secondary);line-height:1.5;";
    const controls = document.createElement("div");
    controls.style.cssText = "display:flex;flex-wrap:wrap;gap:12px;align-items:center;";
    const selector = (name: string, options: { value: string; label: string }[], selected: string, change: (value: string) => void) => {
      const label = document.createElement("label");
      label.style.cssText = "display:flex;align-items:center;gap:8px;font-size:12px;";
      const text = document.createElement("span"); text.textContent = name;
      const input = document.createElement("select"); input.setAttribute("aria-label", name);
      for (const option of options) {
        const item = document.createElement("option"); item.value = option.value; item.textContent = option.label; input.appendChild(item);
      }
      input.value = selected;
      input.addEventListener("change", () => { change(input.value); this.redraw(); });
      label.append(text, input); controls.appendChild(label);
    };
    selector("图表类型", [{ value: "bar", label: "柱状图" }, { value: "line", label: "折线图" }], state.type, value => { state.type = value as ChartState["type"]; });
    const columns = state.source.columns.map((name, index) => ({ value: String(index), label: `${name}（第 ${index + 1} 列）` }));
    selector("分类轴", columns, String(state.xIndex), value => { state.xIndex = Number(value); });
    // 所有列可选择，以便非数值字段获得明确解释；带有效数值的列在名称后标识。
    const numeric = new Set(numericChartColumns(state.source));
    selector("数值字段", columns.map((column, index) => ({ ...column, label: `${column.label}${numeric.has(index) ? " · 含数值" : " · 无有效数值"}` })), String(state.yIndex), value => { state.yIndex = Number(value); });
    this.status.setAttribute("role", "status");
    this.status.setAttribute("aria-live", "polite");
    this.status.style.cssText = "font-size:12px;color:var(--text-secondary);line-height:1.5;";
    this.chartHost.className = "chart-plot";
    this.chartHost.style.cssText = "width:100%;max-width:1100px;border:1px solid var(--border-subtle);border-radius:var(--r-2);";
    const details = document.createElement("details");
    details.open = false;
    const summary = document.createElement("summary"); summary.textContent = "查看已绘制的数据";
    summary.style.cssText = "cursor:pointer;font-size:12px;padding:4px 0;";
    const tableHost = document.createElement("div"); tableHost.style.cssText = "overflow:auto;max-height:300px;margin-top:8px;";
    const table = document.createElement("table");
    table.setAttribute("aria-label", "图表数据替代内容");
    table.style.cssText = "border-collapse:collapse;min-width:360px;font-size:12px;text-align:left;";
    const caption = document.createElement("caption"); caption.textContent = "与图中点一一对应的原始分类和值（未聚合）"; caption.style.cssText = "text-align:left;padding:6px;color:var(--text-secondary);";
    const head = document.createElement("thead"), header = document.createElement("tr");
    const rowHeading = document.createElement("th"); rowHeading.textContent = "来源行";
    for (const item of [rowHeading, this.xHeading, this.yHeading]) { item.scope = "col"; item.style.cssText = "padding:6px 12px;border-bottom:1px solid var(--border);background:var(--bg-subtle);"; header.appendChild(item); }
    head.appendChild(header); table.append(caption, head, this.dataBody); tableHost.appendChild(table); details.append(summary, tableHost);
    this.el.append(title, note, controls, this.status, this.chartHost, details);
    this.redraw();
  }

  redraw(): void {
    const projection = projectChart(this.state);
    this.status.textContent = projection.error ?? `已绘制 ${projection.points.length} 行；跳过 ${projection.skipped} 行 NULL、空白或非数值；另有 ${projection.omitted} 行有效数据未展示（最多绘制 ${this.state.maxPoints} 行，按来源顺序，不聚合）。`;
    this.status.style.color = projection.error ? "var(--danger-600, #c62828)" : "var(--text-secondary)";
    this.chartHost.innerHTML = renderDataChart(this.state, projection);
    this.chartHost.hidden = !!projection.error;
    this.xHeading.textContent = this.state.source.columns[this.state.xIndex] ?? "分类轴";
    this.yHeading.textContent = this.state.source.columns[this.state.yIndex] ?? "数值字段";
    this.dataBody.replaceChildren();
    projection.points.forEach(point => {
      const row = document.createElement("tr");
      for (const value of [String(point.rowIndex + 1), point.label, point.raw]) {
        const cell = document.createElement("td"); cell.textContent = value;
        cell.style.cssText = "padding:6px 12px;border-bottom:1px solid var(--border-subtle);"; row.appendChild(cell);
      }
      this.dataBody.appendChild(row);
    });
  }
  focus(): void { this.el.querySelector("select")?.focus(); }
  destroy(): void { this.el.remove(); }
}
