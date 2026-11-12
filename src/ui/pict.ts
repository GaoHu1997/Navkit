/**
 * 命令条图标（v3 · 双色扁平家族）。
 *
 * 设计语言（docs/design/DESIGN.md v3 §图标）：
 *  · 24×24 网格，双色扁平：浅色填充 + 中色调描边 + 白色细节线
 *  · 与树/网格中的对象图标同族同色（表=蓝、视图=青、函数=紫、用户=琥珀、
 *    备份=石墨、图表=多彩柱），命令条与树联动时视觉一致
 *  · 「新建」语义 = 右下绿色加号徽章（连接 / 新建查询）
 *  · 圆角端点，描边 1.5–1.6，图形占位 18–20px
 */

/** 「新建」绿加号徽章（右下角）。 */
function plusBadge(cx = 18.8, cy = 17.8): string {
  return `<circle cx="${cx}" cy="${cy}" r="4.7" fill="#2fa858" stroke="#ffffff" stroke-width="1.5"/>
    <path d="M${cx} ${cy - 2.1}v4.2M${cx - 2.1} ${cy}h4.2" stroke="#ffffff" stroke-width="1.6" stroke-linecap="round"/>`;
}

/** 命令条 12 个入口：label → SVG 内容（fill 显式着色，不依赖 currentColor）。 */
const PICT_ICONS: Record<string, string> = {
  连接: `
    <rect x="1.8" y="8.4" width="8.4" height="7.2" rx="2.4" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <rect x="13.8" y="8.4" width="8.4" height="7.2" rx="2.4" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <path d="M10.2 12h3.6" stroke="#64748b" stroke-width="1.5" stroke-linecap="round"/>
    ${plusBadge(18.8, 17.8)}`,
  新建查询: `
    <path d="M5.5 3.5h8.6l4.4 4.4v9.6a2.5 2.5 0 0 1-2.5 2.5H5.5a2.5 2.5 0 0 1-2.5-2.5V6a2.5 2.5 0 0 1 2.5-2.5Z"
      fill="#dce9fc" stroke="#3574f0" stroke-width="1.5" stroke-linejoin="round"/>
    <path d="M14.1 3.5v4.4h4.4" fill="none" stroke="#3574f0" stroke-width="1.5" stroke-linejoin="round"/>
    <path d="M8 11.4h5M8 14.6h3.4" stroke="#3574f0" stroke-width="1.5" stroke-linecap="round"/>
    ${plusBadge(18.8, 17.8)}`,
  表: `
    <rect x="3" y="4.5" width="18" height="15" rx="2.5" fill="#dce9fc" stroke="#3574f0" stroke-width="1.5"/>
    <path d="M3 9.6h18" stroke="#3574f0" stroke-width="1.5"/>
    <path d="M10.4 9.6V19.5M16.2 9.6V19.5" stroke="#3574f0" stroke-width="1.2" opacity="0.55"/>
    <rect x="3.75" y="5.25" width="16.5" height="3.6" rx="1.6" fill="#7fa8f4"/>`,
  视图: `
    <rect x="8.8" y="3" width="12.4" height="12.2" rx="2.2" fill="#e6f6f3" stroke="#0e9384" stroke-width="1.3"/>
    <rect x="9.42" y="3.62" width="11.16" height="3" rx="1.1" fill="#b9e6df"/>
    <rect x="2.8" y="6.6" width="12.4" height="12.2" rx="2.2" fill="#cbeae6" stroke="#0e9384" stroke-width="1.5"/>
    <rect x="3.42" y="7.22" width="11.16" height="3.2" rx="1.2" fill="#86d5cb"/>
    <path d="M2.8 11.5h12.4" stroke="#0e9384" stroke-width="1.25"/>
    <path d="M7.3 11.5v7.3M11.7 11.5v7.3" stroke="#0e9384" stroke-width="1" opacity="0.45"/>`,
  函数: `
    <path d="M15.4 4.4c-2.5 0-3.3 1.5-3.8 3.9l-1.7 8.5c-.5 2.3-1.4 3.6-3.6 3.6"
      fill="none" stroke="#7a5af8" stroke-width="2.2" stroke-linecap="round"/>
    <path d="M6.6 10.6h7" fill="none" stroke="#7a5af8" stroke-width="2.2" stroke-linecap="round"/>
    <path d="M13.9 13.2l5.4 6M19.3 13.2l-5.4 6" fill="none" stroke="#7a5af8" stroke-width="2" stroke-linecap="round"/>`,
  用户: `
    <circle cx="12" cy="8.4" r="4.2" fill="#f5ddb5" stroke="#c08327" stroke-width="1.6"/>
    <path d="M4.4 20.4c.9-3.9 3.9-5.9 7.6-5.9s6.7 2 7.6 5.9Z"
      fill="#f5ddb5" stroke="#c08327" stroke-width="1.6" stroke-linejoin="round"/>`,
  其它: `
    <rect x="3.5" y="3.5" width="7.8" height="7.8" rx="2.2" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <rect x="12.7" y="3.5" width="7.8" height="7.8" rx="2.2" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <rect x="3.5" y="12.7" width="7.8" height="7.8" rx="2.2" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <rect x="12.7" y="12.7" width="7.8" height="7.8" rx="2.2" fill="#dce9fc" stroke="#3574f0" stroke-width="1.5"/>`,
  查询: `
    <path d="M5.5 3.5h8.6l4.4 4.4v9.6a2.5 2.5 0 0 1-2.5 2.5H5.5a2.5 2.5 0 0 1-2.5-2.5V6a2.5 2.5 0 0 1 2.5-2.5Z"
      fill="#dce9fc" stroke="#3574f0" stroke-width="1.5" stroke-linejoin="round"/>
    <path d="M14.1 3.5v4.4h4.4" fill="none" stroke="#3574f0" stroke-width="1.5" stroke-linejoin="round"/>
    <path d="M8.2 11.2l2.6 2.4-2.6 2.4" fill="none" stroke="#3574f0" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="M12.6 16h3.2" stroke="#3574f0" stroke-width="1.6" stroke-linecap="round"/>`,
  备份: `
    <rect x="3.5" y="4.8" width="17" height="14.6" rx="2.5" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <path d="M3.5 9.8h17" stroke="#64748b" stroke-width="1.5"/>
    <rect x="8.2" y="5.5" width="7.6" height="3.6" rx="1.4" fill="#aebacb"/>
    <path d="M12 17.2v-4.4M9.9 14.8l2.1-2.1 2.1 2.1" fill="none" stroke="#64748b" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>`,
  自动任务: `
    <circle cx="12" cy="12.6" r="8.2" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <path d="M12 8.2v4.4l3.2 1.9" fill="none" stroke="#3574f0" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="M5.6 4.4 7.2 3M18.4 4.4 16.8 3" stroke="#64748b" stroke-width="1.6" stroke-linecap="round"/>`,
  图表: `
    <path d="M3.4 3.6v16.8h17.2" fill="none" stroke="#64748b" stroke-width="1.6" stroke-linecap="round"/>
    <rect x="6" y="12.6" width="3.6" height="6" rx="1.1" fill="#3574f0"/>
    <rect x="11.2" y="7.6" width="3.6" height="11" rx="1.1" fill="#0e9384"/>
    <rect x="16.4" y="10.2" width="3.6" height="8.4" rx="1.1" fill="#e8862e"/>`,
  关系图: `
    <rect x="3.2" y="3.2" width="7.2" height="5.6" rx="1.6" fill="#dce9fc" stroke="#3574f0" stroke-width="1.5"/>
    <rect x="13.6" y="3.2" width="7.2" height="5.6" rx="1.6" fill="#e1e6ee" stroke="#64748b" stroke-width="1.5"/>
    <rect x="8.4" y="15.2" width="7.2" height="5.6" rx="1.6" fill="#dce9fc" stroke="#3574f0" stroke-width="1.5"/>
    <path d="M10.4 8.8v3.4M17.2 8.8v3.4a2 2 0 0 1-2 2h-4.8a2 2 0 0 1-2-2"
      fill="none" stroke="#64748b" stroke-width="1.5" stroke-linecap="round"/>`,
};

/** label 别名（命令条文案 ≠ 图标 key 时使用） */
const PICT_ALIAS: Record<string, string> = { 查询工具: "查询" };

/** 命令条图标：label 为命令条入口名；size 为渲染边长（px）。 */
export function pictIcon(label: string, size = 20): string {
  const body = pictBody(label);
  if (!body) return "";
  return `<svg class="pict-icon" width="${size}" height="${size}" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`;
}

/** 图标原始路径体（供 icons.ts 复用同一图标源，保证树/命令条联动一致）。 */
export function pictBody(label: string): string | null {
  return PICT_ICONS[label] || PICT_ICONS[PICT_ALIAS[label] || ""] || null;
}
