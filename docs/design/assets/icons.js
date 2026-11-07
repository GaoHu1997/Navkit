/**
 * NavKit 图标族 v2 —— 统一规格
 *  · 24×24 视框（渲染尺寸 16 / 18 / 20）
 *  · 1.75 描边、round 端点与连接
 *  · 一律 currentColor，颜色由容器决定（只有主操作允许上强调色）
 *  · 不使用彩色伪 3D 图标：Ribbon 与树、工具栏共用同一族
 */

/** 图标名清单（注释用途，运行时无类型约束）。
 * connection query table view function user more backup schedule model chart search
 * chevron-down chevron-right chevron-left chevron-up folder folder-open database schema
 * key index link trash plus minus check cross refresh save play stop settings info
 * warning error success copy filter sort columns import export code brush grid list details
 * panel-left panel-right star clock history lock eye eye-off page-first page-prev page-next
 * page-last spinner transfer dump wizard edit open close
 */


/** 每个图标只写 path 内容；统一 24 网格描边风格。 */
const PATHS = {
  connection:
    '<path d="M9 7V4M15 7V4"/><rect x="5" y="7" width="14" height="9" rx="2.5"/><path d="M12 16v1.5a2.5 2.5 0 0 1-2.5 2.5H8"/>',
  query:
    '<path d="M8 6h9M8 11h9M8 16h4"/><circle cx="5" cy="6" r="1.1" fill="currentColor" stroke="none"/><circle cx="5" cy="11" r="1.1" fill="currentColor" stroke="none"/><circle cx="5" cy="16" r="1.1" fill="currentColor" stroke="none"/>',
  table:
    '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><path d="M3.5 9.5h17M9.5 9.5v10"/>',
  view:
    '<path d="M2.8 12S6.4 5.5 12 5.5 21.2 12 21.2 12 17.6 18.5 12 18.5 2.8 12 2.8 12Z"/><circle cx="12" cy="12" r="2.6"/>',
  function:
    '<path d="M9 4.5c-1.6 0-2.4 1-2.4 2.6V15a2.4 2.4 0 0 1-2.4 2.4"/><path d="M4.6 10.4h7.2"/><path d="M10.6 19.4 15 4.6"/><path d="M13.4 4.6h6"/>',
  user:
    '<circle cx="12" cy="8" r="3.4"/><path d="M5.2 20c.6-3.6 3.4-5.4 6.8-5.4s6.2 1.8 6.8 5.4"/>',
  more:
    '<circle cx="6" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="18" cy="12" r="1.4" fill="currentColor" stroke="none"/>',
  backup:
    '<path d="M12 4.5v5.5"/><path d="M9.2 7.4 12 4.4l2.8 3"/><path d="M4.6 13.2a4 4 0 0 0 2 7.3h11.2a3.6 3.6 0 0 0 .6-7.2"/><path d="M4.6 13.2a6.2 6.2 0 0 1 11.6-3.4"/><path d="M19.6 13.3a5 5 0 0 0-.6-.2"/>',
  schedule:
    '<circle cx="12" cy="13" r="7.2"/><path d="M12 9.6V13l2.4 1.7"/><path d="M9 3.4h6"/>',
  model:
    '<rect x="3.6" y="4.4" width="7" height="5.4" rx="1.4"/><rect x="13.4" y="4.4" width="7" height="5.4" rx="1.4"/><rect x="8.5" y="14.2" width="7" height="5.4" rx="1.4"/><path d="M7.1 9.8v2.2h9.8V9.8M12 12v2.2"/>',
  chart:
    '<path d="M4 19.5h16"/><rect x="6" y="11" width="3.2" height="6" rx="1"/><rect x="11.4" y="6.5" width="3.2" height="10.5" rx="1"/><rect x="16.8" y="13.5" width="3.2" height="3.5" rx="1"/>',
  search: '<circle cx="10.8" cy="10.8" r="5.6"/><path d="M15 15l4.4 4.4"/>',
  "chevron-down": '<path d="M6.5 9.8 12 15.2l5.5-5.4"/>',
  "chevron-right": '<path d="M9.8 6.5 15.2 12l-5.4 5.5"/>',
  "chevron-left": '<path d="M14.2 6.5 8.8 12l5.4 5.5"/>',
  "chevron-up": '<path d="M6.5 14.2 12 8.8l5.5 5.4"/>',
  folder:
    '<path d="M3.6 7.4a1.8 1.8 0 0 1 1.8-1.8h3.4l1.8 2.2h7.8a1.8 1.8 0 0 1 1.8 1.8v8.6a1.8 1.8 0 0 1-1.8 1.8H5.4a1.8 1.8 0 0 1-1.8-1.8Z"/>',
  "folder-open":
    '<path d="M3.6 7.4a1.8 1.8 0 0 1 1.8-1.8h3.4l1.8 2.2h7.8a1.8 1.8 0 0 1 1.8 1.8v1.2"/><path d="M3.6 9.4h16.2a1.2 1.2 0 0 1 1.2 1.4l-1.2 7a1.8 1.8 0 0 1-1.8 1.5H5.4a1.8 1.8 0 0 1-1.8-1.5l-1.2-7a1.2 1.2 0 0 1 1.2-1.4Z"/>',
  database:
    '<ellipse cx="12" cy="6.4" rx="7.2" ry="2.9"/><path d="M4.8 6.4v11.2c0 1.6 3.2 2.9 7.2 2.9s7.2-1.3 7.2-2.9V6.4"/><path d="M4.8 12c0 1.6 3.2 2.9 7.2 2.9s7.2-1.3 7.2-2.9"/>',
  schema:
    '<rect x="3.6" y="3.8" width="16.8" height="16.4" rx="2.2"/><path d="M3.6 8.6h16.8"/><path d="M8.4 12.4h7.2M8.4 16h4.6"/>',
  key:
    '<circle cx="8.2" cy="8.2" r="3.6"/><path d="M10.8 10.8 19 19"/><path d="M15.6 15.6 17.8 13.4M17.8 17.8l2.2-2.2"/>',
  index:
    '<path d="M4.6 6.6h14.8M4.6 12h9.4M4.6 17.4h14.8"/><circle cx="18" cy="12" r="1.2" fill="currentColor" stroke="none"/>',
  link:
    '<path d="M10 13.8a3.6 3.6 0 0 0 5.1 0l2.6-2.6a3.6 3.6 0 1 0-5.1-5.1l-1.3 1.3"/><path d="M14 10.2a3.6 3.6 0 0 0-5.1 0l-2.6 2.6a3.6 3.6 0 1 0 5.1 5.1l1.3-1.3"/>',
  trash:
    '<path d="M4.6 6.8h14.8"/><path d="M9.4 6.8V4.9c0-.6.5-1.1 1.1-1.1h3c.6 0 1.1.5 1.1 1.1v1.9"/><path d="M6.6 6.8l.9 12.1c.1.9.8 1.6 1.7 1.6h5.6c.9 0 1.6-.7 1.7-1.6l.9-12.1"/><path d="M10.6 10.6v6M13.4 10.6v6"/>',
  plus: '<path d="M12 5.6v12.8M5.6 12h12.8"/>',
  minus: '<path d="M5.6 12h12.8"/>',
  check: '<path d="M5.4 12.6 9.8 17l8.8-9.6"/>',
  cross: '<path d="M6.6 6.6l10.8 10.8M17.4 6.6 6.6 17.4"/>',
  refresh:
    '<path d="M19.4 12a7.4 7.4 0 1 1-2.4-5.4"/><path d="M19.8 4.6v4.2h-4.2"/>',
  save: '<path d="M4.6 6.4a1.8 1.8 0 0 1 1.8-1.8h9.2l3.8 3.8v9.2a1.8 1.8 0 0 1-1.8 1.8H6.4a1.8 1.8 0 0 1-1.8-1.8Z"/><path d="M8.2 4.6v4.4h6.4V4.6"/><rect x="8.2" y="13" width="7.6" height="4.4" rx="1"/>',
  play: '<path d="M8.4 5.6 18.4 12l-10 6.4Z"/>',
  stop: '<rect x="6.6" y="6.6" width="10.8" height="10.8" rx="1.8"/>',
  settings:
    '<circle cx="12" cy="12" r="2.8"/><path d="M12 3.6v2.2M12 18.2v2.2M4.6 12H6.8M17.2 12h2.2M6.7 6.7l1.6 1.6M15.7 15.7l1.6 1.6M17.3 6.7l-1.6 1.6M8.3 15.7l-1.6 1.6"/>',
  info: '<circle cx="12" cy="12" r="8.4"/><path d="M12 11v5.2"/><circle cx="12" cy="7.9" r="1.05" fill="currentColor" stroke="none"/>',
  warning:
    '<path d="M12 4.4 20.6 19.4H3.4Z"/><path d="M12 9.6v4"/><circle cx="12" cy="16.5" r="1.05" fill="currentColor" stroke="none"/>',
  error:
    '<circle cx="12" cy="12" r="8.4"/><path d="M9 9l6 6M15 9l-6 6"/>',
  success:
    '<circle cx="12" cy="12" r="8.4"/><path d="M8.2 12.2 11 15l4.8-5.4"/>',
  copy:
    '<rect x="8.6" y="8.6" width="10.8" height="10.8" rx="2"/><path d="M15.4 8.6V6.4a2 2 0 0 0-2-2H6.4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2.2"/>',
  filter: '<path d="M4.4 5.6h15.2l-6 7v6.4l-3.2-1.8V12.6Z"/>',
  sort:
    '<path d="M8 4.6v14.8M8 19.4l-3.2-3.2M8 19.4l3.2-3.2"/><path d="M16 19.4V4.6M16 4.6l-3.2 3.2M16 4.6l3.2 3.2"/>',
  columns:
    '<rect x="3.6" y="5" width="16.8" height="14" rx="2"/><path d="M9.4 5v14M14.6 5v14"/>',
  import:
    '<path d="M12 4.6v9.2"/><path d="M8.6 10.6 12 14l3.4-3.4"/><path d="M4.6 15.4v2.2a1.8 1.8 0 0 0 1.8 1.8h11.2a1.8 1.8 0 0 0 1.8-1.8v-2.2"/>',
  export:
    '<path d="M12 14.2V5"/><path d="M8.6 8.4 12 5l3.4 3.4"/><path d="M4.6 15.4v2.2a1.8 1.8 0 0 0 1.8 1.8h11.2a1.8 1.8 0 0 0 1.8-1.8v-2.2"/>',
  code: '<path d="M8.6 8.4 4.8 12l3.8 3.6M15.4 8.4 19.2 12l-3.8 3.6M13.4 5.6l-2.8 12.8"/>',
  brush:
    '<path d="M15.6 4.6 19.4 8.4 11 16.8l-3.8-.4-.4-3.8Z"/><path d="M6.6 16.4 4.4 19.6"/><path d="M13.2 7 17 10.8"/>',
  grid:
    '<rect x="4" y="4" width="6.4" height="6.4" rx="1.4"/><rect x="13.6" y="4" width="6.4" height="6.4" rx="1.4"/><rect x="4" y="13.6" width="6.4" height="6.4" rx="1.4"/><rect x="13.6" y="13.6" width="6.4" height="6.4" rx="1.4"/>',
  list: '<path d="M8.4 6.6h11.2M8.4 12h11.2M8.4 17.4h11.2"/><circle cx="4.8" cy="6.6" r="1.2" fill="currentColor" stroke="none"/><circle cx="4.8" cy="12" r="1.2" fill="currentColor" stroke="none"/><circle cx="4.8" cy="17.4" r="1.2" fill="currentColor" stroke="none"/>',
  details:
    '<rect x="3.6" y="4.6" width="16.8" height="14.8" rx="2"/><path d="M3.6 9.4h16.8M9.6 9.4v10"/>',
  "panel-left":
    '<rect x="3.6" y="4.6" width="16.8" height="14.8" rx="2"/><path d="M9.4 4.6v14.8"/><path d="M6 8.4h.9M6 11.6h.9"/>',
  "panel-right":
    '<rect x="3.6" y="4.6" width="16.8" height="14.8" rx="2"/><path d="M14.6 4.6v14.8"/><path d="M17 8.4h.9M17 11.6h.9"/>',
  star: '<path d="M12 4.4l2.3 4.9 5.3.7-3.9 3.7 1 5.3-4.7-2.6-4.7 2.6 1-5.3L4.4 10l5.3-.7Z"/>',
  clock: '<circle cx="12" cy="12" r="8.4"/><path d="M12 7.2V12l3.4 2.2"/>',
  history:
    '<path d="M4.4 12a7.6 7.6 0 1 0 2.6-5.7"/><path d="M4.2 4.8v3.8h3.8"/><path d="M12 8.4V12l2.8 1.9"/>',
  lock: '<rect x="5" y="10.4" width="14" height="9.6" rx="2"/><path d="M8.2 10.4V8a3.8 3.8 0 0 1 7.6 0v2.4"/><circle cx="12" cy="15.2" r="1.2" fill="currentColor" stroke="none"/>',
  eye: '<path d="M2.8 12S6.4 5.5 12 5.5 21.2 12 21.2 12 17.6 18.5 12 18.5 2.8 12 2.8 12Z"/><circle cx="12" cy="12" r="2.6"/>',
  "eye-off":
    '<path d="M4 4l16 16"/><path d="M9.6 6.1A8.6 8.6 0 0 1 12 5.5c5.6 0 9.2 6.5 9.2 6.5a15 15 0 0 1-2.6 3.4"/><path d="M6.4 7.8A15.6 15.6 0 0 0 2.8 12s3.6 6.5 9.2 6.5a8.9 8.9 0 0 0 3.3-.6"/><path d="M10.2 10.3a2.5 2.5 0 0 0 3.5 3.5"/>',
  "page-first": '<path d="M7 6v12M17 6.8 11.8 12 17 17.2"/>',
  "page-prev": '<path d="M14.6 6.8 9.4 12l5.2 5.2"/>',
  "page-next": '<path d="M9.4 6.8 14.6 12l-5.2 5.2"/>',
  "page-last": '<path d="M17 6v12M7 6.8 12.2 12 7 17.2"/>',
  spinner: '<path d="M12 4.4a7.6 7.6 0 1 1-7.6 7.6" opacity=".95"/>',
  transfer:
    '<path d="M4.6 8.4h13.2M14.6 5.2l3.2 3.2-3.2 3.2"/><path d="M19.4 15.6H6.2M9.4 12.4 6.2 15.6l3.2 3.2"/>',
  dump:
    '<path d="M4.6 6.6c0-1.2 3.3-2.2 7.4-2.2s7.4 1 7.4 2.2"/><path d="M4.6 6.6v10.8c0 1.2 3.3 2.2 7.4 2.2s7.4-1 7.4-2.2V6.6"/><path d="M4.6 12c0 1.2 3.3 2.2 7.4 2.2s7.4-1 7.4-2.2"/>',
  wizard:
    '<path d="M5 19l3.2-1 9.6-9.6a1.9 1.9 0 0 0 0-2.7l-.5-.5a1.9 1.9 0 0 0-2.7 0L5 14.8Z"/><path d="M14.4 6.6l3 3"/><path d="M17.6 3.2l.5 1.4 1.4.5-1.4.5-.5 1.4-.5-1.4L15.7 5l1.4-.5Z" fill="currentColor" stroke="none"/>',
  edit: '<path d="M5 19h3.4L19 8.4a1.9 1.9 0 0 0 0-2.7l-.7-.7a1.9 1.9 0 0 0-2.7 0L5 15.6Z"/><path d="M14.6 6.4l3 3"/>',
  open: '<path d="M4.6 7.4a1.8 1.8 0 0 1 1.8-1.8h3.4l1.8 2.2h7.8a1.8 1.8 0 0 1 1.8 1.8v1"/><path d="M3.6 9.4h16.2a1.2 1.2 0 0 1 1.2 1.4l-1.2 7a1.8 1.8 0 0 1-1.8 1.5H5.4a1.8 1.8 0 0 1-1.8-1.5l-1.2-7a1.2 1.2 0 0 1 1.2-1.4Z"/>',
  close: '<circle cx="12" cy="12" r="8.4"/><path d="M9 9l6 6M15 9l-6 6"/>',
};

/** @typedef {{size?: number, className?: string, stroke?: number}} IconOptions */

/** 生成图标 SVG 字符串。 */
export function icon(name, opts = {}) {
  const size = opts.size ?? 16;
  const sw = opts.stroke ?? 1.75;
  const cls = opts.className ? ` class="${opts.className}"` : "";
  const style = opts.color ? ` style="color:${opts.color}"` : "";
  return `<svg${cls}${style} width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${PATHS[name]}</svg>`;
}

/* ---------------------------------------------------------------- 彩色图标 */

/**
 * 命令条图标配色：每个入口一个主色 + 对应浅底（soft）。
 * 规则：饱和度 / 明度对齐，禁止再来一个"全灰工具栏"或"全彩虹"。
 */
export const COMMAND_COLORS = {
  连接:     { main: "#2f6fed", soft: "#e8f0fe" }, // 蓝 · 建立连接
  新建查询: { main: "#6366f1", soft: "#eceefe" }, // 靛 · 新建
  表:       { main: "#0ea5e9", soft: "#e5f5fe" }, // 天蓝 · 数据
  视图:     { main: "#06b6d4", soft: "#e2f8fc" }, // 青 · 查看
  函数:     { main: "#8b5cf6", soft: "#f1ecfe" }, // 紫 · 代码
  用户:     { main: "#f59e0b", soft: "#fdf2df" }, // 琥珀 · 人
  其它:     { main: "#64748b", soft: "#edf1f5" }, // 石板 · 杂项
  查询工具: { main: "#10b981", soft: "#e3f8ef" }, // 翠绿 · 执行
  备份:     { main: "#16a34a", soft: "#e5f6ea" }, // 绿 · 安全
  自动运行: { main: "#ef4444", soft: "#feebeb" }, // 红 · 定时
  模型:     { main: "#14b8a6", soft: "#e1f7f4" }, // 青 · 结构
  图表:     { main: "#f97316", soft: "#feece2" }, // 橙 · 可视化
};

/** 树 / 信息面板用的小图标配色（与命令条同源，明度略深以保对比）。 */
export const ICON_COLORS = {
  connection: "#2f6fed",
  query: "#6366f1",
  table: "#0ea5e9",
  view: "#06b6d4",
  function: "#8b5cf6",
  user: "#b5720a",
  code: "#10b981",
  backup: "#16a34a",
  schedule: "#d93636",
  model: "#14b8a6",
  chart: "#c25f0c",
  database: "#2f6fed",
  schema: "#64748b",
  folder: "#b5760a",
  "folder-open": "#b5760a",
  key: "#b5760a",
};

/**
 * 彩色图标瓦片：浅色圆角底 + 主色线性图标，用于命令条等高权重入口。
 * 返回整段 SVG（底与图标一体，不依赖容器 CSS）；描边按瓦片尺寸反向缩放，视觉粗细恒定。
 */
export function iconTile(name, colorKey, tile = 30) {
  const c = COMMAND_COLORS[colorKey] || { main: "#64748b", soft: "#edf1f5" };
  const pad = Math.round(tile * 0.19);
  const inner = tile - pad * 2;
  const scale = inner / 24;
  const sw = (1.6 / scale).toFixed(2);
  return `<svg class="icon-tile" width="${tile}" height="${tile}" viewBox="0 0 ${tile} ${tile}" aria-hidden="true">
    <rect width="${tile}" height="${tile}" rx="${(tile * 0.26).toFixed(1)}" fill="${c.soft}"/>
    <g transform="translate(${pad} ${pad}) scale(${scale.toFixed(4)})" style="color:${c.main}" fill="none" stroke="${c.main}" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round">${PATHS[name] || ""}</g>
  </svg>`;
}
