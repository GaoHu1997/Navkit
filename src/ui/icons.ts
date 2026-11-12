/**
 * 手绘内联 SVG 图标集（v3 · 双色扁平 + 单色线性双家族）。
 *
 * 设计语言（docs/design/DESIGN.md v3 §图标）：
 *  · 对象类图标 = 扁平双色：浅色填充 + 中色调描边 + 白/深细节，16px 统一网格
 *    （表=蓝 / 视图=青 / 函数·过程=紫 / 触发器=玫红 / 主键=琥珀 / 外键=橙 /
 *      字段·索引=石墨 / 数据库=靛蓝 / 查询=天蓝 / 备份·文件夹=石墨）
 *  · 操作类图标 = 单色线性 currentColor（随文字/容器变色，状态色由 CSS 控制）
 *  · 语义色仅用于运行（绿）/ 危险（红）/ 解释（琥珀）等强语义入口
 *  · 数据源官方 logo 一律走 dbIcons.ts，本文件不涉及
 */

import { kindIconBody, kindIconBodyOpen } from "./dbIcons";
import { pictBody } from "./pict";

export interface IconDef {
  /** viewBox 宽。 */
  w: number;
  /** viewBox 高。 */
  h: number;
  /** viewBox 内的图形内容（不含 <svg> 壳）。 */
  body: string;
}

/** 生成内联 SVG 字符串（不指定 size 时按 viewBox 原大渲染）。 */
export function svg(def: IconDef, size?: number, cls?: string): string {
  const w = size ?? def.w;
  const h = size ? Math.round((size * def.h) / def.w) : def.h;
  const c = cls ? ` class="${cls}"` : "";
  return `<svg viewBox="0 0 ${def.w} ${def.h}" width="${w}" height="${h}"${c} xmlns="http://www.w3.org/2000/svg" aria-hidden="true">${def.body}</svg>`;
}

/* ------------------------------------------------------------------ */
/* 命令条大图标（24 网格，与 pict.ts 命令条家族同源）                     */
/* ------------------------------------------------------------------ */

/** 命令条入口 → pict 家族（保证树分组 / 命令条 / 菜单同一图形）。 */
function pictDef(label: string): IconDef {
  return { w: 24, h: 24, body: pictBody(label) || "" };
}

export const ICON_CONNECT: IconDef = pictDef("连接");
export const ICON_NEW_QUERY: IconDef = pictDef("新建查询");
export const ICON_TABLE: IconDef = pictDef("表");
export const ICON_VIEW: IconDef = pictDef("视图");
export const ICON_FUNCTION: IconDef = pictDef("函数");
export const ICON_USER: IconDef = pictDef("用户");
export const ICON_OTHER: IconDef = pictDef("其它");
export const ICON_QUERY: IconDef = pictDef("查询");
export const ICON_BACKUP: IconDef = pictDef("备份");
export const ICON_SCHEDULE: IconDef = pictDef("自动任务");
export const ICON_CHART: IconDef = pictDef("图表");

/* ------------------------------------------------------------------ */
/* 标题栏                                                               */
/* ------------------------------------------------------------------ */

/** 应用 logo 小版（18×18，三层等距菱形，透明底；与 src/ui/logo.ts 同源视觉）。 */
export const ICON_LOGO: IconDef = {
  w: 18,
  h: 18,
  body: `
  <path d="M9 8.9L16.2 12.3L9 15.7L1.8 12.3Z" fill="#5B86FF" stroke="#3556DC" stroke-width="0.8" stroke-linejoin="round"/>
  <path d="M9 5.6L16.2 9L9 12.4L1.8 9Z" fill="#FFFFFF" stroke="#C3D6F5" stroke-width="0.8" stroke-linejoin="round"/>
  <path d="M9 2.3L16.2 5.7L9 9.1L1.8 5.7Z" fill="#5EE7F5" stroke="#1CB4CC" stroke-width="0.8" stroke-linejoin="round"/>
`,
};

/** 窗口按钮：最小化 / 最大化 / 关闭（currentColor，悬停由 CSS 控色）。 */
export const ICON_WIN_MIN: IconDef = { w: 10, h: 10, body: `<path d="M0.5 5 h9" stroke="currentColor" stroke-width="1"/>` };
export const ICON_WIN_MAX: IconDef = {
  w: 10,
  h: 10,
  body: `<rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1"/>`,
};
export const ICON_WIN_CLOSE: IconDef = {
  w: 10,
  h: 10,
  body: `<path d="M0.7 0.7 L9.3 9.3 M9.3 0.7 L0.7 9.3" stroke="currentColor" stroke-width="1.1"/>`,
};

/** 搜索放大镜（12，currentColor）。 */
export const ICON_SEARCH: IconDef = {
  w: 12,
  h: 12,
  body: `
  <circle cx="5" cy="5" r="3.7" fill="none" stroke="currentColor" stroke-width="1.4"/><line x1="7.9" y1="7.9" x2="11.1" y2="11.1" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
`,
};

/** 通用 ▼ 下拉箭头（Ribbon / 下拉框）。 */
export function dropArrow(w = 8, h = 5, color = "currentColor"): string {
  return `<svg viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M0.5 0.5 L${w / 2} ${h - 0.5} L${w - 0.5} 0.5 z" fill="${color}"/></svg>`;
}

/* ------------------------------------------------------------------ */
/* 连接树小图标（16px 双色扁平）                                         */
/* ------------------------------------------------------------------ */

/** 展开箭头（树节点左侧小箭头，currentColor）。 */
export const ICON_TREE_ARROW: IconDef = {
  w: 9,
  h: 9,
  body: `<path d="M1.2 2.2 L4.5 6.4 L7.8 2.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>`,
};

/** 文件夹（连接分组，石墨双色）。 */
export const ICON_FOLDER_OPEN: IconDef = {
  w: 16,
  h: 15,
  body: `
  <path d="M1.4 4.6 A1.6 1.6 0 0 1 3 3 h3.4 L7.9 4.7 h5.3 a1.6 1.6 0 0 1 1.6 1.6 v1 H3.5 c-.8 0 -1.5 .55 -1.7 1.3 Z" fill="#C9D2DD"/>
  <path d="M3.3 7.3 h11 a1.3 1.3 0 0 1 1.27 1.6 l-1 4.2 a1.6 1.6 0 0 1 -1.56 1.23 H2 a1.3 1.3 0 0 1 -1.27 -1.6 l1 -4.2 A1.6 1.6 0 0 1 3.3 7.3 Z" fill="#EDF1F6" stroke="#8A94A3" stroke-width="1.2" stroke-linejoin="round"/>
`,
};

/** 数据库（靛蓝圆柱，双色）。 */
export const ICON_DB_GREEN: IconDef = {
  w: 16,
  h: 16,
  body: `
  <path d="M2 4.2 v7.6 c0 1.2 2.69 2.2 6 2.2 s6 -1 6 -2.2 V4.2" fill="#DFE3FC" stroke="#4F63E6" stroke-width="1.3"/>
  <ellipse cx="8" cy="4.2" rx="6" ry="2.2" fill="#98A5F5" stroke="#4F63E6" stroke-width="1.3"/>
  <path d="M2 8 c0 1.2 2.69 2.2 6 2.2 s6 -1 6 -2.2" fill="none" stroke="#4F63E6" stroke-width="1.1" opacity="0.6"/>
`,
};

/** 系统库 / 灰色数据库。 */
export const ICON_DB_GRAY: IconDef = {
  w: 16,
  h: 16,
  body: `
  <path d="M2 4.2 v7.6 c0 1.2 2.69 2.2 6 2.2 s6 -1 6 -2.2 V4.2" fill="#E8ECF1" stroke="#8A94A3" stroke-width="1.3"/>
  <ellipse cx="8" cy="4.2" rx="6" ry="2.2" fill="#C3CCD6" stroke="#8A94A3" stroke-width="1.3"/>
  <path d="M2 8 c0 1.2 2.69 2.2 6 2.2 s6 -1 6 -2.2" fill="none" stroke="#8A94A3" stroke-width="1.1" opacity="0.6"/>
`,
};

/** 表（蓝，双色）。 */
export const ICON_TABLE_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="2" width="13.6" height="12" rx="2.2" fill="#DCE9FC" stroke="#3574F0" stroke-width="1.3"/>
  <rect x="1.95" y="2.7" width="12.1" height="3" rx="1.5" fill="#7FA8F4"/>
  <path d="M1.2 6.4 h13.6" stroke="#3574F0" stroke-width="1.2"/>
  <path d="M6.6 6.4 v7.6 M11 6.4 v7.6 M1.2 10.2 h13.6" stroke="#3574F0" stroke-width="1" opacity="0.6"/>
`,
};

/** 视图（青，两张表叠放：后卡为源表，前卡带表头与网格）。 */
export const ICON_VIEW_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="5.7" y="1.9" width="8.6" height="8.4" rx="1.6" fill="#E6F6F3" stroke="#0E9384" stroke-width="1"/>
  <rect x="6.15" y="2.35" width="7.7" height="2" rx="0.8" fill="#B9E6DF"/>
  <rect x="1.7" y="4.4" width="8.6" height="8.4" rx="1.6" fill="#CBEAE6" stroke="#0E9384" stroke-width="1.2"/>
  <rect x="2.15" y="4.85" width="7.7" height="2.2" rx="0.9" fill="#86D5CB"/>
  <path d="M1.7 7.9h8.6" stroke="#0E9384" stroke-width="1"/>
  <path d="M6 7.9v4.9" stroke="#0E9384" stroke-width="0.9" opacity="0.45"/>
`,
};

/** 函数 f(x)（紫，线性）。 */
export const ICON_FUNCTION_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="#7A5AF8" stroke-width="1.5" stroke-linecap="round">
    <path d="M10 2.8 c-1.8 0 -2.3 1 -2.6 2.6 L6.3 11 c-.3 1.5 -.9 2.4 -2.4 2.4"/>
    <path d="M4.6 7.4 h4.8"/>
    <path d="M9.8 9.4 l4 4.6 M13.8 9.4 l-4 4.6"/>
  </g>
`,
};

/** 查询（天蓝文档 + 提示符）。 */
export const ICON_QUERY_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <path d="M3.4 1.6 h6.2 l3.4 3.4 v9 a1.6 1.6 0 0 1 -1.6 1.6 H3.4 a1.6 1.6 0 0 1 -1.6 -1.6 V3.2 a1.6 1.6 0 0 1 1.6 -1.6 Z"
    fill="#DCE9FC" stroke="#2E90FA" stroke-width="1.3" stroke-linejoin="round"/>
  <path d="M9.6 1.6 V5 h3.4" fill="none" stroke="#2E90FA" stroke-width="1.3" stroke-linejoin="round"/>
  <path d="M4.6 8.4 l2 1.8 -2 1.8 M7.8 12.4 h3.2" fill="none" stroke="#2E90FA" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
`,
};

/** 备份（石墨归档箱 + 上箭头）。 */
export const ICON_BACKUP_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.6" y="2.4" width="12.8" height="11.2" rx="2" fill="#E1E6EE" stroke="#64748B" stroke-width="1.3"/>
  <path d="M1.6 6 h12.8" stroke="#64748B" stroke-width="1.3"/>
  <path d="M8 13.2 V9.6 M6.4 11 L8 9.4 l1.6 1.6" fill="none" stroke="#64748B" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
`,
};

/** 触发器（玫红闪电方块）。 */
export const ICON_TRIGGER_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="1.2" width="13.6" height="13.6" rx="3" fill="#FBDCEB" stroke="#DD4B93" stroke-width="1.2"/>
  <path d="M9.3 3.2 L4.7 8.9 h2.9 l-.9 4.3 4.6 -5.8 H8.4 Z" fill="#DD4B93"/>
`,
};

/** 字段（蓝色三行字段条，扁平双色；去掉内层卡片边框，16px 下仍清晰）。 */
export const ICON_COLUMN_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="1.2" width="13.6" height="13.6" rx="3" fill="#DCE9FB" stroke="#3E7BD6" stroke-width="1.2"/>
  <path d="M4.3 4.6h7.4" stroke="#3E7BD6" stroke-width="1.7" stroke-linecap="round"/>
  <path d="M4.3 7.8h7.4" stroke="#7FA8E0" stroke-width="1.35" stroke-linecap="round"/>
  <path d="M4.3 11h5" stroke="#7FA8E0" stroke-width="1.35" stroke-linecap="round"/>
`,
};

/** 索引（青绿排序列表，与彩色家族搭配）。 */
export const ICON_INDEX_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="1.2" width="13.6" height="13.6" rx="3" fill="#D9F2EF" stroke="#1F9C92" stroke-width="1.2"/>
  <path d="M4.6 5.4 h6.8 M4.6 8.2 h6.8 M4.6 11 h4.2" fill="none" stroke="#1F9C92" stroke-width="1.2" stroke-linecap="round"/>
`,
};

/** 主键（琥珀钥匙小图，字段列表中与普通字段区分）。 */
export const ICON_PK_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="1.2" width="13.6" height="13.6" rx="3" fill="#FAEDD4"/>
  <g fill="none" stroke="#C08327" stroke-width="1.4" stroke-linecap="round">
    <circle cx="5.6" cy="8" r="2.6"/>
    <path d="M8.2 8 h5.2 M11.4 8 v2.2 M13.4 8 v1.8"/>
  </g>
`,
};

/** 设置（齿轮，currentColor）。 */
export const ICON_TITLEBAR_GEAR: IconDef = {
  w: 24,
  h: 24,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round"><circle cx="12" cy="12" r="2.8"/><path d="M12 3.6v2.2M12 18.2v2.2M4.6 12h2.2M17.2 12h2.2M6.7 6.7l1.6 1.6M15.7 15.7l1.6 1.6M17.3 6.7l-1.6 1.6M8.3 15.7l-1.6 1.6"/></g>`,
};

/** 外键（橙钥匙）。 */
export const ICON_FK_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <rect x="1.2" y="1.2" width="13.6" height="13.6" rx="3" fill="#FCE8D5" stroke="#E8862E" stroke-width="1.2"/>
  <g fill="none" stroke="#E8862E" stroke-width="1.2" stroke-linecap="round">
    <circle cx="5.6" cy="8" r="2.5"/>
    <path d="M8.1 8 h4.8 M10.8 8 v2.2 M12.9 8 v1.8"/>
  </g>
`,
};

/* ------------------------------------------------------------------ */
/* 标签工具栏（16~18px）                                                */
/* ------------------------------------------------------------------ */

/** 打开表（石墨打开文件夹）。 */
export const ICON_OPEN_TABLE: IconDef = {
  w: 18,
  h: 16,
  body: `
  <path d="M1 4 A1.6 1.6 0 0 1 2.6 2.4 H6 l1.5 1.7 h8.3 A1.6 1.6 0 0 1 17.4 5.6 v1 H3.3 c-.8 0 -1.5 .55 -1.68 1.33 Z" fill="#C9D2DD"/>
  <path d="M2.9 8 h13.6 a1.3 1.3 0 0 1 1.26 1.62 l-1.1 4.3 A1.6 1.6 0 0 1 15.1 15.2 H1.9 a1.3 1.3 0 0 1 -1.26 -1.62 l1.1 -4.3 A1.6 1.6 0 0 1 2.9 8 Z"
    fill="#EDF1F6" stroke="#8A94A3" stroke-width="1.2" stroke-linejoin="round"/>
`,
};

/** 设计表（蓝铅笔，双色）。 */
export const ICON_PENCIL_BLUE: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <path d="M2.2 12.4 L11.8 2.8 a1.9 1.9 0 0 1 2.7 2.7 L4.9 15.1 L1.2 16 Z" fill="#DCE9FC" stroke="#3574F0" stroke-width="1.3" stroke-linejoin="round"/>
    <path d="M11.8 2.8 a1.9 1.9 0 0 1 2.7 2.7 l-1.1 1.1 -2.7 -2.7 Z" fill="#7FA8F4"/>
    <path d="M1.2 16 l1 -3.6 2.6 2.6 Z" fill="#3574F0"/>
  </g>`,
};

/** 新建（绿色实心加号圆，语义「新增」）。 */
export const ICON_PLUS_GREEN: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.2" fill="#2FA858"/>
    <path d="M8.5 4.8 V12.2 M4.8 8.5 H12.2" stroke="#FFFFFF" stroke-width="1.9" stroke-linecap="round"/>
  </g>`,
};

/** 删除（石墨减号圆）。 */
export const ICON_MINUS_GRAY: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.2" fill="#A8B3C1"/>
    <path d="M4.8 8.5 H12.2" stroke="#FFFFFF" stroke-width="1.9" stroke-linecap="round"/>
  </g>`,
};

/** 导入（箭头入托盘，currentColor）。 */
export const ICON_IMPORT_GREEN: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <path d="M8.5 1.6 v6 M8.5 7.6 L6 5.1 M8.5 7.6 L11 5.1"/>
    <path d="M1.6 9.4 v3.4 a1.8 1.8 0 0 0 1.8 1.8 h10.2 a1.8 1.8 0 0 0 1.8 -1.8 V9.4"/>
  </g>`,
};

/** 导出（箭头出托盘，currentColor）。 */
export const ICON_EXPORT_ORANGE: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <path d="M8.5 7.6 v-6 M8.5 1.6 L6 4.1 M8.5 1.6 L11 4.1"/>
    <path d="M1.6 9.4 v3.4 a1.8 1.8 0 0 0 1.8 1.8 h10.2 a1.8 1.8 0 0 0 1.8 -1.8 V9.4"/>
  </g>`,
};

/** 保存（软盘轮廓，currentColor）。 */
export const ICON_SAVE_FLOPPY: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round">
    <path d="M2.4 2.4 h10.2 l2 2 v10.2 H2.4 Z"/>
    <path d="M5.4 2.4 v3.6 h5.4 V2.4"/>
    <path d="M4.6 14.6 V9.8 h7.8 v4.8"/>
  </g>`,
};

/* ------------------------------------------------------------------ */
/* 状态栏（currentColor）                                               */
/* ------------------------------------------------------------------ */

/** 视图切换：明细（点阵）。 */
export const ICON_VIEW_DOTS: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="currentColor">
    <rect x="1" y="2" width="2.6" height="2.6" rx="0.6"/><rect x="5.4" y="2" width="2.6" height="2.6" rx="0.6"/><rect x="9.8" y="2" width="2.6" height="2.6" rx="0.6"/><rect x="14.2" y="2" width="0.8" height="2.6" rx="0.4"/>
    <rect x="1" y="6.7" width="2.6" height="2.6" rx="0.6"/><rect x="5.4" y="6.7" width="2.6" height="2.6" rx="0.6"/><rect x="9.8" y="6.7" width="2.6" height="2.6" rx="0.6"/><rect x="14.2" y="6.7" width="0.8" height="2.6" rx="0.4"/>
    <rect x="1" y="11.4" width="2.6" height="2.6" rx="0.6"/><rect x="5.4" y="11.4" width="2.6" height="2.6" rx="0.6"/><rect x="9.8" y="11.4" width="2.6" height="2.6" rx="0.6"/><rect x="14.2" y="11.4" width="0.8" height="2.6" rx="0.4"/>
  </g>`,
};

/** 视图切换：列表（条目线）。 */
export const ICON_VIEW_LIST: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="currentColor">
    <rect x="1" y="2.2" width="2.6" height="2.2" rx="0.6"/><rect x="5" y="2.4" width="10" height="1.8" rx="0.6"/>
    <rect x="1" y="7" width="2.6" height="2.2" rx="0.6"/><rect x="5" y="7.2" width="10" height="1.8" rx="0.6"/>
    <rect x="1" y="11.8" width="2.6" height="2.2" rx="0.6"/><rect x="5" y="12" width="10" height="1.8" rx="0.6"/>
  </g>`,
};

/** 视图切换：网格（编织格）。 */
export const ICON_VIEW_GRID: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g stroke="currentColor" stroke-width="1.5">
    <path d="M1.2 3 h13.6 M1.2 8 h13.6 M1.2 13 h13.6 M3 1.2 v13.6 M8 1.2 v13.6 M13 1.2 v13.6"/>
  </g>`,
};

/** 面板开关：左侧栏。 */
export const ICON_PANEL_LEFT: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g>
    <rect x="0.8" y="0.8" width="14.4" height="14.4" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <rect x="3.2" y="3.2" width="3.6" height="9.6" fill="currentColor"/>
  </g>`,
};

/** 面板开关：右侧栏。 */
export const ICON_PANEL_RIGHT: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g>
    <rect x="0.8" y="0.8" width="14.4" height="14.4" fill="none" stroke="currentColor" stroke-width="1.5"/>
    <rect x="9.2" y="3.2" width="3.6" height="9.6" fill="currentColor"/>
  </g>`,
};

/** 信息 ⓘ（currentColor 线性）。 */
export const ICON_INFO_CIRCLE: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="6.9" fill="none" stroke="currentColor" stroke-width="1.4"/>
    <rect x="7.65" y="7.6" width="1.7" height="4.8" rx="0.85" fill="currentColor"/>
    <circle cx="8.5" cy="5.2" r="1.05" fill="currentColor"/>
  </g>`,
};

/** 复制。 */
export const ICON_COPY: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.4">
    <rect x="5.2" y="5.2" width="9.4" height="9.4" rx="1.2"/>
    <path d="M10.8 5.2 V3 a1.2 1.2 0 0 0 -1.2 -1.2 H3 a1.2 1.2 0 0 0 -1.2 1.2 v6.6 a1.2 1.2 0 0 0 1.2 1.2 h2.2"/>
  </g>`,
};

/** 加号（网格底部新增行）。 */
export const ICON_PLUS_LINE: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M2 8 h12 M8 2 v12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>`,
};

/** 减号（网格底部删除行）。 */
export const ICON_MINUS_LINE: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M2 8 h12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>`,
};

/** 对勾（提交）。 */
export const ICON_CHECK: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M2.4 8.6 L6.4 12.4 L13.6 3.6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>`,
};

/** 叉（回滚 / 关闭）。 */
export const ICON_CROSS: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M3 3 L13 13 M13 3 L3 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>`,
};

/** 分页：首页 / 上一页 / 下一页 / 末页。 */
export const ICON_PAGE_FIRST: IconDef = {
  w: 16,
  h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 2.5 v11 M13.5 2.5 L7 8 l6.5 5.5"/></g>`,
};
export const ICON_PAGE_PREV: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M10.5 2.5 L4.5 8 l6 5.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`,
};
export const ICON_PAGE_NEXT: IconDef = {
  w: 16,
  h: 16,
  body: `<path d="M5.5 2.5 L11.5 8 l-6 5.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>`,
};
export const ICON_PAGE_LAST: IconDef = {
  w: 16,
  h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2.5 v11 M2.5 2.5 L9 8 l-6.5 5.5"/></g>`,
};

/** 刷新（圆形箭头）。 */
export const ICON_REFRESH: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g>
    <path d="M13.2 8 a5.2 5.2 0 1 1 -1.6 -3.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
    <path d="M13.8 1.6 l-0.3 3.6 -3.4 -1.6 z" fill="currentColor"/>
  </g>`,
};

/** 下载驱动：向下的箭头落入托盘（可直接下载并校验）。 */
export const ICON_DOWNLOAD: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <path d="M8 2.2 V9.8"/>
    <path d="M4.6 6.6 L8 10 L11.4 6.6"/>
    <path d="M3 13.4 H13"/>
  </g>`,
};

/** 获取驱动：外部链接（受授权/平台限制，需自行到官方渠道获取），不再自动下载。 */
export const ICON_EXTERNAL_LINK: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <path d="M9.4 2.4 H13.6 V6.6"/>
    <path d="M13.6 2.4 L8 8"/>
    <path d="M12.6 9.6 V12.4 A1.2 1.2 0 0 1 11.4 13.6 H3.6 A1.2 1.2 0 0 1 2.4 12.4 V5.6 A1.2 1.2 0 0 1 3.6 4.4 H6.4"/>
  </g>`,
};

/** 旋转加载环（下载中/检查中，靠 CSS 旋转；淡环 + 亮弧）。 */
export const ICON_SPINNER: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
    <circle cx="8" cy="8" r="5.4" opacity="0.25"/>
    <path d="M8 2.6 A5.4 5.4 0 0 1 13.4 8"/>
  </g>`,
};

/** 更多（横向三点）。 */
export const ICON_MORE_DOTS: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="currentColor">
    <circle cx="3" cy="8" r="1.4"/><circle cx="8" cy="8" r="1.4"/><circle cx="13" cy="8" r="1.4"/>
  </g>`,
};

/** 搜索（16px）。 */
export const ICON_SEARCH_16: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5">
    <circle cx="6.6" cy="6.6" r="4.6"/>
    <path d="M10.2 10.2 L14.4 14.4" stroke-linecap="round"/>
  </g>`,
};

/** 星标（收藏，琥珀双色）。 */
export const ICON_STAR: IconDef = {
  w: 16,
  h: 16,
  body: `
  <path d="M8 1.6 l1.9 4 4.4 .6 -3.2 3.1 .8 4.4 L8 11.6 l-3.9 2.1 .8 -4.4 L1.7 6.2 l4.4 -.6 Z"
    fill="#F5DDB5" stroke="#C08327" stroke-width="1.1" stroke-linejoin="round"/>`,
};

/** 运行（绿色三角，语义色）。 */
export const ICON_PLAY: IconDef = {
  w: 12,
  h: 12,
  body: `<path d="M2.8 1.6 L10.4 6 L2.8 10.4 Z" fill="#1F9254"/>`,
};

/** 停止（方框，currentColor）。 */
export const ICON_STOP_SQ: IconDef = {
  w: 12,
  h: 12,
  body: `<rect x="2" y="2" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/>`,
};

/** 解释（琥珀闪电，语义色）。 */
export const ICON_BOLT: IconDef = {
  w: 12,
  h: 12,
  body: `<path d="M6.8 1 L2.6 6.8 h2.9 L4.5 11 l4.8 -6 H6.3 Z" fill="#F0A50A"/>`,
};

/** 消息：信息 / 错误（toast）。 */
export const ICON_MSG_INFO: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.6" fill="#3574F0"/>
    <rect x="7.5" y="7.6" width="2" height="5.4" rx="1" fill="#ffffff"/>
    <circle cx="8.5" cy="5" r="1.2" fill="#ffffff"/>
  </g>`,
};
export const ICON_MSG_ERROR: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.6" fill="#D92D20"/>
    <path d="M5.4 5.4 L11.6 11.6 M11.6 5.4 L5.4 11.6" stroke="#ffffff" stroke-width="1.9" stroke-linecap="round"/>
  </g>`,
};

/* ------------------------------------------------------------------ */
/* 标签工具栏第二批（数据页 / 查询页 / 设计器，16~18px）                  */
/* ------------------------------------------------------------------ */

/** 开始事务（圆柱 + 播放，currentColor）。 */
export const ICON_DB_TX: IconDef = {
  w: 18,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.4">
    <ellipse cx="6.6" cy="4" rx="5" ry="1.9"/>
    <path d="M1.6 4 v9 c0 1.05 2.24 1.9 5 1.9 1.05 0 2.02 -.14 2.82 -.38"/>
    <path d="M1.6 8.5 c0 1.05 2.24 1.9 5 1.9 1.4 0 2.67 -.24 3.56 -.62"/>
    <path d="M11.8 9.2 l4.6 2.9 -4.6 2.9 Z" stroke-linejoin="round"/>
  </g>`,
};

/** 文本（文档行，currentColor）。 */
export const ICON_DOC_TEXT: IconDef = {
  w: 16,
  h: 17,
  body: `
  <g>
    <g fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round">
      <path d="M2.6 1.4 h7.2 l3.6 3.6 v10.6 H2.6 Z"/>
      <path d="M9.8 1.4 V5 h3.6"/>
    </g>
    <path d="M4.8 8.4 h6.4 M4.8 10.8 h6.4 M4.8 13.2 h4.2" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
  </g>`,
};

/** 筛选（漏斗，currentColor）。 */
export const ICON_FUNNEL: IconDef = {
  w: 17,
  h: 17,
  body: `
  <path d="M1.8 2 h13.4 l-5.1 5.8 v6 l-3.2 -1.8 V7.8 Z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>
  `,
};

/** 排序（下箭头 + 行，currentColor）。 */
export const ICON_SORT: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4.4 2.4 v10.8 M4.4 13.2 l-2.4 -2.7 M4.4 13.2 l2.4 -2.7"/>
    </g>
    <path d="M9.4 3.6 h6 M9.4 7 h6 M9.4 10.4 h6 M9.4 13.8 h4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" opacity="0.7"/>
  </g>`,
};

/** 列（网格，currentColor）。 */
export const ICON_COLUMNS_GRID: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <rect x="1.6" y="1.6" width="13.8" height="13.8" rx="1.8" fill="none" stroke="currentColor" stroke-width="1.4"/>
    <path d="M1.6 6.4 h13.8 M1.6 11.2 h13.8 M6.4 1.6 v13.8 M11.2 1.6 v13.8" stroke="currentColor" stroke-width="1.1" opacity="0.7"/>
  </g>`,
};

/** 数据生成（圆柱 + 星光，currentColor）。 */
export const ICON_DATA_GEN: IconDef = {
  w: 18,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round">
    <ellipse cx="5.6" cy="3.8" rx="4.4" ry="1.7"/>
    <path d="M1.2 3.8 v9.4 c0 .94 1.97 1.7 4.4 1.7 .8 0 1.56 -.08 2.24 -.22"/>
    <path d="M1.2 8.5 c0 .94 1.97 1.7 4.4 1.7"/>
    <path d="M13.4 7.2 l.9 2 2 .9 -2 .9 -.9 2 -.9 -2 -2 -.9 2 -.9 Z"/>
    <path d="M8.9 12.4 l.45 1 1 .45 -1 .45 -.45 1 -.45 -1 -1 -.45 1 -.45 Z"/>
  </g>`,
};

/** 创建图表（柱状，currentColor）。 */
export const ICON_BARS_BLUE: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="currentColor">
    <rect x="2" y="8.6" width="3.4" height="6.4" rx="0.9"/>
    <rect x="7" y="2.6" width="3.4" height="12.4" rx="0.9"/>
    <rect x="12" y="6" width="3.4" height="9" rx="0.9"/>
  </g>`,
};

/** 查询创建工具（调节器，currentColor）。 */
export const ICON_QUERY_TOOL: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round">
    <path d="M2 4.2 h13 M2 8.5 h13 M2 12.8 h13"/>
    <circle cx="6" cy="4.2" r="1.7" fill="#ffffff"/>
    <circle cx="11.5" cy="8.5" r="1.7" fill="#ffffff"/>
    <circle cx="5.2" cy="12.8" r="1.7" fill="#ffffff"/>
  </g>`,
};

/** 美化 SQL（魔法棒 + 星光，currentColor）。 */
export const ICON_WAND: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round">
    <path d="M2.8 14.2 L11 6" stroke-width="1.8"/>
    <path d="M11 6 l1.5 -1.5 a1.3 1.3 0 0 1 1.9 1.9 L12.9 7.9 Z" stroke-width="1.4"/>
    <path d="M13.8 1.2 v2.6 M12.5 2.5 h2.6" stroke-width="1.2"/>
    <path d="M8.4 1.6 v2 M7.4 2.6 h2" stroke-width="1.1"/>
  </g>`,
};

/** 代码段（圆括号，currentColor）。 */
export const ICON_PARENS: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
    <path d="M6.2 2.6 c-2.6 4 -2.6 7.8 0 11.8"/>
    <path d="M10.8 2.6 c2.6 4 2.6 7.8 0 11.8"/>
  </g>`,
};

/** 添加字段（绿色描边加号圆，语义「新增」）。 */
export const ICON_PLUS_GREEN_SM: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.2" fill="none" stroke="#2FA858" stroke-width="1.5"/>
    <path d="M8.5 4.8 V12.2 M4.8 8.5 H12.2" stroke="#2FA858" stroke-width="1.5" stroke-linecap="round"/>
  </g>`,
};

/** 插入字段（左箭头圆，currentColor）。 */
export const ICON_INSERT_LEFT: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.2" fill="none" stroke="currentColor" stroke-width="1.4"/>
    <path d="M11.6 8.5 H5.6 M8.2 6 L5.6 8.5 l2.6 2.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
  </g>`,
};

/** 删除字段（红色减号圆，语义「危险」）。 */
export const ICON_MINUS_RED: IconDef = {
  w: 17,
  h: 17,
  body: `
  <g>
    <circle cx="8.5" cy="8.5" r="7.2" fill="none" stroke="#D92D20" stroke-width="1.5"/>
    <path d="M4.8 8.5 H12.2" stroke="#D92D20" stroke-width="1.5" stroke-linecap="round"/>
  </g>`,
};

/** 主键（琥珀钥匙，双色）。 */
export const ICON_KEY_GOLD: IconDef = {
  w: 18,
  h: 17,
  body: `
  <g>
    <circle cx="5.2" cy="8.5" r="3.4" fill="#F5DDB5" stroke="#C08327" stroke-width="1.7"/>
    <path d="M8.6 8.5 h7.2 M13.4 8.5 v2.6 M15.8 8.5 v2.2" fill="none" stroke="#C08327" stroke-width="1.7" stroke-linecap="round"/>
  </g>`,
};

/** 上移 / 下移（currentColor）。 */
export const ICON_ARROW_UP: IconDef = {
  w: 14,
  h: 16,
  body: `<path d="M7 14 V3 M7 3 L3 7 M7 3 l4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>`,
};
export const ICON_ARROW_DOWN: IconDef = {
  w: 14,
  h: 16,
  body: `<path d="M7 2 v11 M7 13 l4 -4 M7 13 L3 9" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>`,
};

/* ------------------------------------------------------------------ */
/* 名称映射：旧资源名 → 手绘 SVG（树 / 菜单 / 标签页等沿用原键名）        */
/* ------------------------------------------------------------------ */

/** 用户小图标（16×16，用户网格行）。 */
export const ICON_USER_MINI: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g>
    <circle cx="8" cy="5.8" r="3.1" fill="#F5DDB5" stroke="#C08327" stroke-width="1.2"/>
    <path d="M2.4 14.4 c.7 -2.9 2.9 -4.4 5.6 -4.4 s4.9 1.5 5.6 4.4 Z" fill="#F5DDB5" stroke="#C08327" stroke-width="1.2" stroke-linejoin="round"/>
  </g>`,
};

/** 应用 logo 大图（关于对话框，96 设计稿，仅此处使用 defs 渐变）。 */
export const ICON_APP_LOGO: IconDef = {
  w: 96,
  h: 96,
  body: `
  <defs><linearGradient id="nkapp3-bg" x1="22" y1="6" x2="74" y2="90" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#5B95F7"/><stop offset="1" stop-color="#2E63E6"/></linearGradient></defs>
  <rect x="3" y="3" width="90" height="90" rx="21.5" fill="url(#nkapp3-bg)"/>
  <path d="M24 31.8c0-5.4 8.7-9.8 19.5-9.8s19.5 4.4 19.5 9.8v29.4c0 5.4-8.7 9.8-19.5 9.8S24 66.6 24 61.2Z" fill="#FFFFFF"/>
  <path d="M24 31.8c0 5.4 8.7 9.8 19.5 9.8s19.5-4.4 19.5-9.8" fill="none" stroke="#C4D9FA" stroke-width="4.5"/>
  <path d="M24 46.5c0 5.4 8.7 9.8 19.5 9.8s19.5-4.4 19.5-9.8" fill="none" stroke="#C4D9FA" stroke-width="4.5" opacity="0.55"/>
  <circle cx="69.6" cy="67.8" r="14.1" fill="#FFFFFF"/>
  <circle cx="69.6" cy="67.8" r="9.6" fill="#34C566"/>
`,
};

const NAME_MAP: Record<string, IconDef> = {
  // 树 / 信息面板
  info_pane_user: ICON_USER_MINI,
  info_pane_project: ICON_FOLDER_OPEN,
  info_pane_table: ICON_TABLE_MINI,
  info_pane_view: ICON_VIEW_MINI,
  info_pane_query: ICON_QUERY_MINI,
  info_pane_function: ICON_FUNCTION_MINI,
  info_pane_database: ICON_DB_GREEN,
  info_pane_schema: ICON_DB_GRAY,
  dbn_backup: ICON_BACKUP_MINI,
  trigger: ICON_TRIGGER_MINI,
  info_pane_column: ICON_COLUMN_MINI,
  info_pane_index: ICON_INDEX_MINI,
  info_pane_pk: ICON_PK_MINI,
  info_pane_fk: ICON_FK_MINI,
  settings_gear: ICON_TITLEBAR_GEAR,
  // 标签页 / 菜单
  editor: ICON_DOC_TEXT,
  columns: ICON_COLUMNS_GRID,
  msg_info: ICON_MSG_INFO,
  msg_error: ICON_MSG_ERROR,
  // 标签工具栏：数据页
  dbn_applyupdates: ICON_DB_TX,
  dbn_cancel: ICON_CROSS,
  data_filter: ICON_FUNNEL,
  tab_sort: ICON_SORT,
  data_transfer_ent_preview_image: ICON_IMPORT_GREEN,
  data_sync_ent_preview_image: ICON_EXPORT_ORANGE,
  data_generation_ent_preview_image: ICON_DATA_GEN,
  data_profiling_ent_preview_image: ICON_BARS_BLUE,
  // 标签工具栏：查询页
  dbn_post: ICON_SAVE_FLOPPY,
  query_builder: ICON_QUERY_TOOL,
  query_pin_ent_preview_image: ICON_PARENS,
  // 标签工具栏：设计器
  dbn_insert: ICON_PLUS_GREEN_SM,
  insert_field: ICON_INSERT_LEFT,
  dbn_delete: ICON_MINUS_RED,
  dbn_edit: ICON_KEY_GOLD,
  dbn_prior: ICON_ARROW_UP,
  dbn_next: ICON_ARROW_DOWN,
  // 对象页工具栏
  open_table: ICON_OPEN_TABLE,
  design_table: ICON_PENCIL_BLUE,
  new_table: ICON_PLUS_GREEN,
  delete_table: ICON_MINUS_GRAY,
  import_wizard: ICON_IMPORT_GREEN,
  export_wizard: ICON_EXPORT_ORANGE,
  btn_search: ICON_SEARCH_16,
  object_star: ICON_STAR,
  run_sel: ICON_PLAY,
  stop_sel: ICON_STOP_SQ,
  explain_sel: ICON_BOLT,
};

/* ------------------------------------------------------------------ */
/* 资源名解析：NAME_MAP → 数据源品牌图标（dbIcons）→ `_off` 灰阶变体       */
/* ------------------------------------------------------------------ */

/** 灰阶缓存（同名 `_off` 变体只生成一次）。 */
const grayCache = new Map<string, IconDef>();

/** 亮度灰阶（白色保持白色）。 */
function grayHex(hex: string): string {
  const r = parseInt(hex.slice(0, 2), 16);
  const g = parseInt(hex.slice(2, 4), 16);
  const b = parseInt(hex.slice(4, 6), 16);
  const y = Math.round(0.2126 * r + 0.7152 * g + 0.0722 * b);
  const s = y.toString(16).padStart(2, "0");
  return s + s + s;
}

/**
 * 生成灰色变体（未连接 / 规划中）：颜色转灰阶、渐变 id 加 g- 前缀
 * （避免与彩色实例共存时互链）。
 */
function grayVariant(def: IconDef, key: string): IconDef {
  const hit = grayCache.get(key);
  if (hit) return hit;
  let body = def.body;
  body = body.replace(/#([0-9a-fA-F]{3,8})/g, (m) => {
    let hex = m.slice(1);
    let alpha = "";
    if (hex.length === 8) {
      alpha = hex.slice(6);
      hex = hex.slice(0, 6);
    } else if (hex.length === 4) {
      alpha = hex.slice(3);
      hex = hex.slice(0, 3);
    }
    if (hex.length === 3) hex = hex[0] + hex[0] + hex[1] + hex[1] + hex[2] + hex[2];
    if (hex.toUpperCase() === "FFFFFF") return m;
    return "#" + grayHex(hex) + alpha;
  });
  body = body.replace(/\bid="([^"]+)"/g, (_m, id: string) => `id="g-${id}"`);
  body = body.replace(/url\(#([^)]+)\)/g, (_m, id: string) => `url(#g-${id})`);
  const out: IconDef = { w: def.w, h: def.h, body };
  grayCache.set(key, out);
  return out;
}

/** 数据源官方 logo 缓存（info_pane_conn_* -> dbIcons，含灰阶体）。 */
const kindDefCache = new Map<string, IconDef>();

/** info_pane_conn_* -> dbIcons 官方 logo（16x16；off=true 为灰阶体）。 */
function kindDef(name: string, off: boolean): IconDef | null {
  const key = name + (off ? "#off" : "");
  const hit = kindDefCache.get(key);
  if (hit) return hit;
  const body = kindIconBody(name.slice("info_pane_conn_".length), off);
  if (!body) return null;
  const def: IconDef = { w: 16, h: 16, body };
  kindDefCache.set(key, def);
  return def;
}

/**
 * 连接已打开：官方 logo 原图 + 仅把自带外框描边染成连接绿。
 * 资源名后缀约定为 `_open`（如 `info_pane_conn_mysql_open`）。
 */
function kindDefOpen(name: string): IconDef | null {
  const key = name + "#open";
  const hit = kindDefCache.get(key);
  if (hit) return hit;
  const body = kindIconBodyOpen(name.slice("info_pane_conn_".length));
  if (!body) return null;
  const def: IconDef = { w: 16, h: 16, body };
  kindDefCache.set(key, def);
  return def;
}

/** 按资源名取图标定义（未知名称回退为灰色数据库）。 */
function iconDefByName(name: string | undefined): IconDef {
  if (!name) return ICON_DB_GRAY;
  const direct = NAME_MAP[name];
  if (direct) return direct;
  const off = name.endsWith("_off");
  const open = name.endsWith("_open");
  const baseName = off ? name.slice(0, -4) : open ? name.slice(0, -5) : name;
  if (baseName.startsWith("info_pane_conn_")) {
    // 顺序：打开态绿框 → 未连接灰阶 → 普通原图
    if (open) {
      const opened = kindDefOpen(baseName);
      if (opened) return opened;
    }
    const official = kindDef(baseName, off);
    if (official) return official;
  }
  // pict_*：命令条图标族（树分组与命令条同名入口联动一致，见 pict.ts）
  const pictMatch = /^pict_([a-z_]+)$/.exec(baseName);
  if (pictMatch) {
    const label = PICT_TREE_LABELS[pictMatch[1]];
    const body = label ? pictBody(label) : null;
    if (body) {
      const def: IconDef = { w: 24, h: 24, body };
      return off ? grayVariant(def, name) : def;
    }
  }
  const base = NAME_MAP[baseName];
  if (base) return off ? grayVariant(base, name) : base;
  return ICON_DB_GRAY;
}

/** 按资源名生成内联 SVG（`_off` 后缀为灰色变体，用于未连接 / 规划中）。 */
export function iconSvgByName(name: string | undefined): string {
  const def = iconDefByName(name);
  // pict_* 渲染时带上类名，容器可按族调整尺寸
  return name && name.startsWith("pict_") ? svg(def, undefined, "pict-icon") : svg(def);
}

/** pict_* 资源名 → 命令条图标 label（与 Ribbon 同源，见 pict.ts）。 */
const PICT_TREE_LABELS: Record<string, string> = {
  connect: "连接",
  newquery: "新建查询",
  table: "表",
  view: "视图",
  "function": "函数",
  user: "用户",
  other: "其它",
  query: "查询",
  backup: "备份",
  schedule: "自动任务",
  chart: "图表",
};

/* ------------------------------------------------------------------ */
/* 信息面板大图标（45~56px）                                            */
/* ------------------------------------------------------------------ */

/** 大号靛蓝数据库（信息面板对象图标）。 */
export const ICON_DATABASE_LG: IconDef = {
  w: 56,
  h: 56,
  body: `
  <g>
    <path d="M7 15.5 c0 -4.6 9.4 -8.3 21 -8.3 s21 3.7 21 8.3 v26 c0 4.6 -9.4 8.3 -21 8.3 S7 46.1 7 41.5 Z" fill="#DFE3FC"/>
    <path d="M7 28.5 c0 4.6 9.4 8.3 21 8.3 s21 -3.7 21 -8.3" fill="none" stroke="#4F63E6" stroke-width="2.4" opacity="0.55"/>
    <ellipse cx="28" cy="15.5" rx="21" ry="8.3" fill="#98A5F5"/>
    <path d="M7 15.5 c0 4.6 9.4 8.3 21 8.3 s21 -3.7 21 -8.3" fill="none" stroke="#4F63E6" stroke-width="2.4"/>
    <path d="M7 15.5 v26 c0 4.6 9.4 8.3 21 8.3 s21 -3.7 21 -8.3 v-26" fill="none" stroke="#4F63E6" stroke-width="2.4"/>
  </g>`,
};

/** 大号蓝色表（信息面板对象图标）。 */
export const ICON_TABLE_LG: IconDef = {
  w: 56,
  h: 56,
  body: `
  <g>
    <rect x="6" y="8" width="44" height="40" rx="5" fill="#DCE9FC"/>
    <rect x="8.4" y="10.4" width="39.2" height="7.2" rx="2.6" fill="#7FA8F4"/>
    <path d="M6 20 h44 M21.7 20 v28 M37.3 20 v28 M6 34 h44" stroke="#3574F0" stroke-width="1.8" opacity="0.55"/>
    <rect x="6" y="8" width="44" height="40" rx="5" fill="none" stroke="#3574F0" stroke-width="2.4"/>
    <path d="M6 20 h44" stroke="#3574F0" stroke-width="2.4"/>
  </g>`,
};

/** 灰色服务器（信息面板连接引用行）。 */
export const ICON_DOLPHIN_GRAY: IconDef = {
  w: 17,
  h: 15,
  body: `
  <g fill="none" stroke="#8A94A3" stroke-width="1.3">
    <rect x="1.5" y="1.5" width="14" height="5" rx="1.4"/>
    <rect x="1.5" y="8.5" width="14" height="5" rx="1.4"/>
  </g>
  <circle cx="4.4" cy="4" r="0.9" fill="#8A94A3"/>
  <circle cx="4.4" cy="11" r="0.9" fill="#8A94A3"/>
`,
};

/** 信息面板大图标映射。 */
const LARGE_MAP: Record<string, IconDef> = {
  info_pane_database: ICON_DATABASE_LG,
  info_pane_table: ICON_TABLE_LG,
};

/** 信息面板大图标（按资源名）。未命中专有大图标的对象按其矢量图标放大渲染。 */
export function iconSvgLarge(name: string | undefined): string {
  const def = (name && LARGE_MAP[name]) || null;
  if (def) return svg(def);
  // 数据源官方 logo 等按资源名解析后，等比放大到信息面板 56px 规格
  return svg(iconDefByName(name), 56);
}

/** 代码段条目图标（天蓝小文档）。 */
export const ICON_SNIPPET: IconDef = {
  w: 15,
  h: 16,
  body: `
  <g>
    <path d="M2.2 2.6 a1.6 1.6 0 0 1 1.6 -1.6 h5.8 l3.6 3.6 v8.8 a1.6 1.6 0 0 1 -1.6 1.6 H2.2 a1.6 1.6 0 0 1 -1.6 -1.6 Z"
      fill="#DCE9FC" stroke="#2E90FA" stroke-width="1.2" stroke-linejoin="round"/>
    <path d="M9.4 1 v3.6 H13" fill="none" stroke="#2E90FA" stroke-width="1.2" stroke-linejoin="round"/>
    <path d="M4 7.4 h6.2 M4 9.8 h6.2 M4 12.2 h3.8" stroke="#2E90FA" stroke-width="1.1" stroke-linecap="round"/>
  </g>`,
};

/* ------------------------------------------------------------------ */
/* 标题栏工具图标（24 网格线性族，currentColor）                          */
/* ------------------------------------------------------------------ */

/** 线性图标族（24 网格 / 1.75 描边 / currentColor）：信息面板对象头等使用。 */
const LINEAR_PATHS: Record<string, string> = {
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
  backup:
    '<path d="M12 4.5v5.5"/><path d="M9.2 7.4 12 4.4l2.8 3"/><path d="M4.6 13.2a4 4 0 0 0 2 7.3h11.2a3.6 3.6 0 0 0 .6-7.2"/><path d="M4.6 13.2a6.2 6.2 0 0 1 11.6-3.4"/><path d="M19.6 13.3a5 5 0 0 0-.6-.2"/>',
  folder:
    '<path d="M3.6 7.4a1.8 1.8 0 0 1 1.8-1.8h3.4l1.8 2.2h7.8a1.8 1.8 0 0 1 1.8 1.8v8.6a1.8 1.8 0 0 1-1.8 1.8H5.4a1.8 1.8 0 0 1-1.8-1.8Z"/>',
  database:
    '<ellipse cx="12" cy="6.4" rx="7.2" ry="2.9"/><path d="M4.8 6.4v11.2c0 1.6 3.2 2.9 7.2 2.9s7.2-1.3 7.2-2.9V6.4"/><path d="M4.8 12c0 1.6 3.2 2.9 7.2 2.9s7.2-1.3 7.2-2.9"/>',
  schema:
    '<rect x="3.6" y="3.8" width="16.8" height="16.4" rx="2.2"/><path d="M3.6 8.6h16.8"/><path d="M8.4 12.4h7.2M8.4 16h4.6"/>',
  columns:
    '<rect x="3.6" y="5" width="16.8" height="14" rx="2"/><path d="M9.4 5v14M14.6 5v14"/>',
  editor:
    '<path d="M6 3.4h7.2l4.4 4.4v12.8H6Z"/><path d="M13.2 3.4v4.4h4.4"/><path d="M9 12.4h6M9 15.8h6M9 19h3.6"/>',
};

/** 线性图标（currentColor）：信息面板对象头等高权重位置使用。 */
export function linearIcon(name: string, size = 28): string {
  const body = LINEAR_PATHS[name];
  if (!body) return "";
  return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
}

/** 空态主图形（插头，currentColor，24 网格）。 */
export const ICON_EMPTY_CONNECT: IconDef = {
  w: 24,
  h: 24,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M9 3.4v4.2M15 3.4v4.2"/><path d="M7 7.6h10v3.2a5 5 0 0 1-5 5 5 5 0 0 1-5-5Z"/><path d="M12 15.8v4.8"/></g>`,
};

/** 标题栏搜索（放大镜，currentColor）。 */
export const ICON_TITLEBAR_SEARCH: IconDef = {
  w: 24,
  h: 24,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round"><circle cx="10.8" cy="10.8" r="5.6"/><path d="M15 15l4.4 4.4"/></g>`,
};

/** 标题栏后台任务（队列清单，currentColor）。 */
export const ICON_TITLEBAR_TASKS: IconDef = {
  w: 24,
  h: 24,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="4.2" y="4.6" width="15.6" height="14.8" rx="2.2"/><path d="M8 9.2h8M8 12h8M8 14.8h5"/></g>`,
};

/* ------------------------------------------------------------------ */
/* 数据源目录图标（生成段）：分类配色 + 标识字母，与 dbcatalog.ts 联动。  */
/* ------------------------------------------------------------------ */

import { DB_KINDS } from "../core/dbcatalog";

/** 各分类的徽章底色盘。 */
const KIND_CATEGORY_COLORS: Record<string, string[]> = {
  relational: ["#3D6FB4", "#2E9E5B", "#C0574F", "#7E64C1", "#2C8C99", "#B07B3A", "#5A76AD", "#4E9E4E"],
  analytics: ["#D97B29", "#2E8B8B", "#5468C4", "#8C4FA8", "#3E8E3E", "#C2452F", "#6272A8", "#A8552E"],
  domestic: ["#C7150B", "#1E5ED8", "#E0492E", "#2E8BE8", "#17A05E", "#8A4FBF", "#C75450", "#2C6E63", "#B07B3A"],
  lightweight: ["#52A8E8", "#7EC24A", "#E8A13C", "#9A6BC9", "#48B0A8", "#C0574F"],
  nosql: ["#D23B3B", "#3FA34D", "#2B6CB0", "#E07A30", "#7A4FC9", "#38A3A5", "#B04A7E"],
  graph: ["#C2385A", "#2E8B57", "#3B7DD8", "#8B5CF6", "#D97706", "#0891B2"],
  timeseries: ["#0E7490", "#B45309", "#4D7C0F", "#7C3AED", "#BE185D"],
  mq: ["#E11D48", "#2563EB", "#EA580C", "#7C3AED", "#059669"],
  registry: ["#475569", "#0E7490", "#6D28D9", "#B45309"],
};

/** 数据源徽章图标：圆角方块 + 标识字母。 */
function kindBadgeIcon(color: string, id: string): IconDef {
  const letters = (id.replace(/[^a-zA-Z0-9]/g, "").slice(0, 2) || "DB").toUpperCase();
  return {
    w: 16,
    h: 16,
    body:
      `<g><rect x="0.5" y="0.5" width="15" height="15" rx="2.6" fill="${color}"/>` +
      `<text x="8" y="8.9" text-anchor="middle" dominant-baseline="central" ` +
      `font-family="'Segoe UI',Arial,sans-serif" font-size="7.2" font-weight="700" ` +
      `fill="#FFFFFF">${letters}</text></g>`,
  };
}

for (const k of DB_KINDS) {
  const key = `info_pane_conn_${k.id}`;
  // 手绘图标（MySQL / PostgreSQL 等）与 dbIcons 品牌图标优先；
  // 仅当 dbIcons 尚未收录该 kind（目录先行扩充）时用字母徽章兜底。
  // `_off` 灰阶变体由 grayVariant 运行时生成，不在此预填。
  if (!NAME_MAP[key] && !kindIconBody(k.id)) {
    const palette = KIND_CATEGORY_COLORS[k.category] ?? ["#5A76AD"];
    const peers = DB_KINDS.filter((x) => x.category === k.category);
    const idx = Math.max(0, peers.findIndex((x) => x.id === k.id));
    NAME_MAP[key] = kindBadgeIcon(palette[idx % palette.length], k.id);
  }
}

/** 视图切换：表单（卡片 + 两行字段）。 */
export const ICON_FORM_VIEW: IconDef = {
  w: 16,
  h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="1.6" y="1.9" width="12.8" height="12.2" rx="1.6"/><path d="M4.2 6h2.6M8.6 6h3M4.2 10h2.6M8.6 10h3"/></g>`,
};

/** 视图切换：网格。 */
export const ICON_GRID_VIEW: IconDef = {
  w: 16,
  h: 16,
  body: `<g fill="currentColor"><rect x="1.6" y="1.6" width="5.4" height="5.4" rx="1"/><rect x="9" y="1.6" width="5.4" height="5.4" rx="1"/><rect x="1.6" y="9" width="5.4" height="5.4" rx="1"/><rect x="9" y="9" width="5.4" height="5.4" rx="1"/></g>`,
};

/** 视图切换：列表。 */
export const ICON_LIST_VIEW: IconDef = {
  w: 16,
  h: 16,
  body: `<g fill="currentColor"><rect x="1.6" y="2.2" width="2.6" height="2.6" rx="0.7"/><rect x="5.6" y="2.6" width="9" height="1.8" rx="0.7"/><rect x="1.6" y="6.7" width="2.6" height="2.6" rx="0.7"/><rect x="5.6" y="7.1" width="9" height="1.8" rx="0.7"/><rect x="1.6" y="11.2" width="2.6" height="2.6" rx="0.7"/><rect x="5.6" y="11.6" width="9" height="1.8" rx="0.7"/></g>`,
};

/* ------------------------------------------------------------------ */
/* 视图操作（适应窗口 / 放大 / 缩小 / 刷新）                               */
/* ------------------------------------------------------------------ */

/** 适应窗口：四角外扩箭头（currentColor）。 */
export const ICON_FIT_WINDOW: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
    <path d="M2 6V2.6h3.4M14 6V2.6h-3.4M2 10v3.4h3.4M14 10v3.4h-3.4"/>
  </g>`,
};

/** 放大：放大镜 + 加号（currentColor）。 */
export const ICON_ZOOM_IN: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
    <circle cx="6.9" cy="6.9" r="4.6"/>
    <path d="M10.4 10.4 14.2 14.2"/>
    <path d="M4.9 6.9h4M6.9 4.9v4"/>
  </g>`,
};

/** 缩小：放大镜 + 减号（currentColor）。 */
export const ICON_ZOOM_OUT: IconDef = {
  w: 16,
  h: 16,
  body: `
  <g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round">
    <circle cx="6.9" cy="6.9" r="4.6"/>
    <path d="M10.4 10.4 14.2 14.2"/>
    <path d="M4.9 6.9h4"/>
  </g>`,
};

/* 通用图标名补充进 NAME_MAP（此前 full_screen / bb_zoom_in / dbn_refresh
   未注册，全部退化为灰色数据库图标——见设计规范 §5.1「未知名称回退」）。 */
NAME_MAP.full_screen = ICON_FIT_WINDOW;
NAME_MAP.bb_zoom_in = ICON_ZOOM_IN;
NAME_MAP.bb_zoom_out = ICON_ZOOM_OUT;
NAME_MAP.dbn_refresh = ICON_REFRESH;

/* ------------------------------------------------------------------ */
/* 文本显示格式（文本 / 十六进制 / 图像 / 网页）                          */
/* 「文本▼」工具栏按钮按当前格式切换图标，让选中状态在按钮上可见。          */
/* ------------------------------------------------------------------ */

/** 文本格式：三行文本 + 光标。 */
NAME_MAP.text_format_text = {
  w: 16, h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M2.6 4.2h10.8M2.6 8h10.8M2.6 11.8h6.6"/><path d="M11.4 10.6v3.2" stroke-width="1.8"/></g>`,
};

/** 十六进制格式：方框内 "0F"。 */
NAME_MAP.text_format_hex = {
  w: 16, h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.8" y="2.6" width="12.4" height="10.8" rx="1.6"/><path d="M6 6.2 4.4 8l1.6 1.8M10 6.2 11.6 8 10 9.8"/></g>`,
};

/** 图像格式：相框 + 山与太阳。 */
NAME_MAP.text_format_image = {
  w: 16, h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.6"/><circle cx="5.6" cy="6.4" r="1.2"/><path d="M2.4 11.6 6.6 7.8l2.6 2.4 1.8-1.6 2.6 3"/></g>`,
};

/** 电源（关机）符号：断开会话等终止型操作的图标。 */
export const ICON_POWER: IconDef = {
  w: 16, h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M8 2.4v5.2"/><path d="M4.6 4.4a5.2 5.2 0 1 0 6.8 0"/></g>`,
};
NAME_MAP.power = ICON_POWER;

/** 网页格式：地球经纬。 */
NAME_MAP.text_format_web = {
  w: 16, h: 16,
  body: `<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="8" cy="8" r="5.8"/><path d="M2.2 8h11.6M8 2.2c2.6 2.4 2.6 9.2 0 11.6-2.6-2.4-2.6-9.2 0-11.6Z"/></g>`,
};
NAME_MAP.info_pane_backup = ICON_BACKUP_MINI;
