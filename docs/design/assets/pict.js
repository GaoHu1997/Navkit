/**
 * 命令条彩色具象图标（v2.1 原创重绘）
 *
 * 设计语言：
 *  · 24×24 网格，以「填充色块」为主的具象图形（区别于线性功能图标族）
 *  · 每个图标 2–3 个颜色：主色 + 辅色 + 白色细节，颜色取自语义体系
 *  · 圆角统一 2–2.5，图形占位 18–20px，视觉重量一致
 *  · 全部原创构成：链接节点、文档加号、数据表、视窗、函数符、
 *    人物、对象矩阵、终端、数据圆柱 + 上传箭头、闹钟、ER 节点、柱状图
 *  · 不描摹任何商业软件的图标形状
 */

/** 命令条 12 个入口：label → SVG 内容（fill 显式着色，不依赖 currentColor） */
export const PICT_ICONS = {
  连接: `
    <rect x="1.5" y="8.5" width="8" height="7" rx="2.6" fill="#2f6fed"/>
    <rect x="14.5" y="8.5" width="8" height="7" rx="2.6" fill="#14b8a6"/>
    <path d="M9.5 12h5" stroke="#94a3b8" stroke-width="2" stroke-linecap="round"/>
    <circle cx="18.6" cy="18.6" r="4.4" fill="#22c55e" stroke="#ffffff" stroke-width="1.4"/>
    <path d="M18.6 16.6v4M16.6 18.6h4" stroke="#ffffff" stroke-width="1.5" stroke-linecap="round"/>`,
  新建查询: `
    <path d="M6.5 3.5h7.2L18.5 8.3v10.2a2.5 2.5 0 0 1-2.5 2.5H6.5a2.5 2.5 0 0 1-2.5-2.5V6a2.5 2.5 0 0 1 2.5-2.5Z" fill="#6366f1"/>
    <path d="M13.7 3.5V8.3h4.8Z" fill="#c7d2fe"/>
    <path d="M7.5 12.5h6M7.5 15.5h4" stroke="#ffffff" stroke-width="1.5" stroke-linecap="round" opacity="0.9"/>
    <circle cx="18.6" cy="18.6" r="4.4" fill="#22c55e" stroke="#ffffff" stroke-width="1.4"/>
    <path d="M18.6 16.6v4M16.6 18.6h4" stroke="#ffffff" stroke-width="1.5" stroke-linecap="round"/>`,
  表: `
    <rect x="3" y="4.5" width="18" height="15" rx="2.5" fill="#e0f2fe"/>
    <path d="M5.5 4.5h13a2.5 2.5 0 0 1 2.5 2.5v3H3V7a2.5 2.5 0 0 1 2.5-2.5Z" fill="#0284c7"/>
    <path d="M10.5 10v9.5M3 14h18" stroke="#7dd3fc" stroke-width="1.4"/>
    <rect x="3" y="4.5" width="18" height="15" rx="2.5" fill="none" stroke="#0284c7" stroke-width="1.4"/>`,
  视图: `
    <path d="M2.6 12C5.2 7.6 8.4 5.6 12 5.6s6.8 2 9.4 6.4c-2.6 4.4-5.8 6.4-9.4 6.4S5.2 16.4 2.6 12Z" fill="#06b6d4"/>
    <circle cx="12" cy="12" r="3.4" fill="#ffffff"/>
    <circle cx="12" cy="12" r="1.7" fill="#0e7490"/>`,
  函数: `
    <path d="M15.8 4.2c-2.7 0-3.6 1.7-4.1 4.2l-1.9 9.3c-.5 2.5-1.5 3.9-3.9 3.9" stroke="#8b5cf6" stroke-width="2.5" fill="none" stroke-linecap="round"/>
    <path d="M7.2 10.6h7.6" stroke="#8b5cf6" stroke-width="2.5" stroke-linecap="round"/>
    <circle cx="18.8" cy="18.8" r="2.2" fill="#c4b5fd"/>`,
  用户: `
    <circle cx="12" cy="8" r="4.1" fill="#f59e0b"/>
    <path d="M4.6 20.6c.9-4.1 3.9-6.1 7.4-6.1s6.5 2 7.4 6.1Z" fill="#f59e0b"/>
    <path d="M4.6 20.6h14.8" stroke="#d97706" stroke-width="0" />`,
  其它: `
    <rect x="3.5" y="3.5" width="7.6" height="7.6" rx="2.1" fill="#2f6fed"/>
    <rect x="12.9" y="3.5" width="7.6" height="7.6" rx="2.1" fill="#14b8a6"/>
    <rect x="3.5" y="12.9" width="7.6" height="7.6" rx="2.1" fill="#f59e0b"/>
    <rect x="12.9" y="12.9" width="7.6" height="7.6" rx="2.1" fill="#94a3b8"/>`,
  查询工具: `
    <rect x="3" y="4" width="18" height="16" rx="2.5" fill="#10b981"/>
    <path d="M3 8.6h18" stroke="#ffffff" stroke-width="1.3" opacity="0.55"/>
    <path d="M6.6 12.2l3 2.6-3 2.6" stroke="#ffffff" stroke-width="1.9" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="M12 17.4h5.4" stroke="#ffffff" stroke-width="1.9" stroke-linecap="round"/>`,
  备份: `
    <path d="M4 6.6c0-1.6 2.9-2.9 6.5-2.9s6.5 1.3 6.5 2.9v9.8c0 1.6-2.9 2.9-6.5 2.9S4 18 4 16.4Z" fill="#16a34a"/>
    <ellipse cx="10.5" cy="6.6" rx="6.5" ry="2.7" fill="#4ade80"/>
    <path d="M10.5 16v-5.2M8.1 13l2.4-2.4 2.4 2.4" stroke="#ffffff" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>`,
  自动运行: `
    <path d="M4.6 5.2 7.4 3.4M19.4 5.2 16.6 3.4" stroke="#ef4444" stroke-width="2" stroke-linecap="round"/>
    <circle cx="12" cy="13" r="7.6" fill="#ef4444"/>
    <circle cx="12" cy="13" r="4.6" fill="#fee2e2"/>
    <path d="M12 10.4V13l2.2 1.5" stroke="#b91c1c" stroke-width="1.7" fill="none" stroke-linecap="round" stroke-linejoin="round"/>`,
  模型: `
    <rect x="3" y="3.5" width="8" height="6.6" rx="1.9" fill="#6366f1"/>
    <rect x="13" y="3.5" width="8" height="6.6" rx="1.9" fill="#14b8a6"/>
    <rect x="8" y="14" width="8" height="6.6" rx="1.9" fill="#a855f7"/>
    <path d="M7 10.1v3.9h5M17 10.1v3.9h-5" stroke="#64748b" stroke-width="1.5" fill="none"/>`,
  图表: `
    <path d="M3 20.5h18" stroke="#64748b" stroke-width="1.7" stroke-linecap="round"/>
    <rect x="4.6" y="12" width="4.6" height="6.6" rx="1.4" fill="#2f6fed"/>
    <rect x="10.2" y="6" width="4.6" height="12.6" rx="1.4" fill="#f97316"/>
    <rect x="15.8" y="9.4" width="4.6" height="9.2" rx="1.4" fill="#14b8a6"/>`,
};

/** label 别名（命令条文案 ≠ 图标 key 时使用） */
const PICT_ALIAS = { 查询: "查询工具" };

/** 具象图标：label 为命令条入口名 */
export function pictIcon(label, size = 24) {
  const body = PICT_ICONS[label] || PICT_ICONS[PICT_ALIAS[label] || ""];
  if (!body) return "";
  return `<svg class="pict-icon" width="${size}" height="${size}" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`;
}
