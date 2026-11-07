/** 当前已加载数据的图表快照与纯渲染逻辑；不访问数据库或持久存储。 */
export interface ChartSource {
  title: string;
  description: string;
  columns: string[];
  rows: (string | null)[][];
}
export interface ChartState {
  source: ChartSource;
  type: "bar" | "line";
  xIndex: number;
  yIndex: number;
  maxPoints: number;
}
export interface ChartPoint { label: string; value: number; raw: string; rowIndex: number }
export interface ChartProjection { points: ChartPoint[]; skipped: number; omitted: number; error: string | null }
export const MAX_CHART_POINTS = 200;

/** 空串/NULL/十六进制/非有限值不冒充零或十进制数字。 */
export function chartNumber(raw: string | null | undefined): number | null {
  if (raw === null || raw === undefined || !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(raw.trim())) return null;
  const value = Number(raw);
  return Number.isFinite(value) ? value : null;
}
export function numericChartColumns(source: ChartSource): number[] {
  return source.columns.flatMap((_, index) => source.rows.some(row => chartNumber(row[index]) !== null) ? [index] : []);
}
export function chartSourceError(source: ChartSource | null): string | null {
  if (!source) return "请先在当前表数据页或查询结果页取得数据，再创建图表。";
  if (!source.columns.length || !source.rows.length) return "当前数据为空，无法创建图表。";
  if (!numericChartColumns(source).length) return "当前数据没有有效数值字段；NULL、空白和非数值文本不会绘制。";
  return null;
}
export function createChartState(source: ChartSource): ChartState {
  const error = chartSourceError(source);
  if (error) throw new Error(error);
  const yIndex = numericChartColumns(source)[0];
  return {
    source: { ...source, columns: [...source.columns], rows: source.rows.map(row => [...row]) },
    type: "bar", xIndex: source.columns.findIndex((_, index) => index !== yIndex) < 0 ? yIndex : source.columns.findIndex((_, index) => index !== yIndex),
    yIndex, maxPoints: MAX_CHART_POINTS,
  };
}
export function projectChart(state: ChartState): ChartProjection {
  const { source, xIndex, yIndex } = state;
  if (![xIndex, yIndex].every(index => Number.isInteger(index) && index >= 0 && index < source.columns.length)) {
    return { points: [], skipped: 0, omitted: 0, error: "请选择有效的分类轴和数值字段。" };
  }
  const limit = Math.min(MAX_CHART_POINTS, Math.max(1, Math.floor(state.maxPoints) || MAX_CHART_POINTS));
  const points: ChartPoint[] = [];
  let skipped = 0, omitted = 0;
  source.rows.forEach((row, rowIndex) => {
    const value = chartNumber(row[yIndex]);
    if (value === null) { skipped++; return; }
    if (points.length >= limit) { omitted++; return; }
    points.push({ label: row[xIndex] === null || row[xIndex] === undefined ? "NULL" : row[xIndex] === "" ? "（空串）" : row[xIndex], value, raw: row[yIndex]!, rowIndex });
  });
  return { points, skipped, omitted, error: points.length ? null : "所选数值字段没有可绘制的有限数字；NULL、空白和非数值文本已跳过。" };
}
const escapeXml = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");
const numberLabel = (value: number) => value === 0 ? "0" : value.toPrecision(5);

/** 使用归一化坐标避免合法大数的 max-min 溢出。SVG标题/文字全部XML转义。 */
export function renderDataChart(state: ChartState, projection = projectChart(state)): string {
  const { points } = projection;
  if (!points.length) return "";
  const width = 840, height = 380, left = 86, right = 20, top = 28, bottom = 82;
  const plotWidth = width - left - right, plotHeight = height - top - bottom;
  const scale = Math.max(...points.map(point => Math.abs(point.value))) || 1;
  const max = Math.max(0, ...points.map(point => point.value / scale));
  const min = Math.min(0, ...points.map(point => point.value / scale));
  const span = max - min || 1;
  const y = (value: number) => top + plotHeight * (1 - (value / scale - min) / span);
  const step = plotWidth / points.length;
  const x = (index: number) => left + step * (index + 0.5);
  const title = `${state.type === "bar" ? "柱状图" : "折线图"}：${state.source.columns[state.yIndex]} / ${state.source.columns[state.xIndex]}`;
  const out = [`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="100%" role="img" aria-label="${escapeXml(title)}"><title>${escapeXml(title)}</title><desc>绘制 ${points.length} 行；跳过 ${projection.skipped} 行无效值。详细数值见下方数据表。</desc>`];
  for (let index = 0; index <= 4; index++) {
    const ratio = min + span * index / 4, yp = top + plotHeight * (1 - index / 4);
    const tick = ratio * scale;
    out.push(`<line x1="${left}" y1="${yp}" x2="${width - right}" y2="${yp}" stroke="var(--border-subtle, #e4eaf0)"/><text x="${left - 8}" y="${yp + 4}" text-anchor="end" fill="var(--text-tertiary, #657184)" font-size="11">${escapeXml(numberLabel(tick))}</text>`);
  }
  out.push(`<line x1="${left}" y1="${y(0)}" x2="${width - right}" y2="${y(0)}" stroke="var(--border-strong, #b9c4ce)"/>`);
  if (state.type === "line") {
    out.push(`<polyline points="${points.map((point, index) => `${x(index)},${y(point.value)}`).join(" ")}" fill="none" stroke="var(--accent-500, #3574f0)" stroke-width="2"/>`);
  }
  points.forEach((point, index) => {
    const label = escapeXml(`${point.label}：${point.raw}`);
    if (state.type === "bar") {
      const barWidth = step * 0.65;
      out.push(`<rect x="${x(index) - barWidth / 2}" y="${Math.min(y(0), y(point.value))}" width="${barWidth}" height="${Math.abs(y(0) - y(point.value))}" fill="var(--accent-500, #3574f0)"><title>${label}</title></rect>`);
    } else {
      out.push(`<circle cx="${x(index)}" cy="${y(point.value)}" r="3" fill="var(--accent-500, #3574f0)"><title>${label}</title></circle>`);
    }
    if (index % Math.ceil(points.length / 16) === 0) {
      const shortLabel = point.label.length > 20 ? `${point.label.slice(0, 19)}…` : point.label;
      out.push(`<text x="${x(index)}" y="${height - bottom + 16}" transform="rotate(-30 ${x(index)} ${height - bottom + 16})" text-anchor="end" fill="var(--text-secondary, #5a6a7a)" font-size="11">${escapeXml(shortLabel)}</text>`);
    }
  });
  return out.join("") + "</svg>";
}
