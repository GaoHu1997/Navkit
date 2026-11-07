/**
 * NavKit 品牌 Logo（v5 · 等距数据层 · 透明无容器）。
 *
 * 三层等距菱形悬浮堆叠，去掉背景板：可直接叠在任意底色、壁纸、标题栏上，不发生色块冲突。
 *   · 顶层青色 = 活动数据 / 当前连接
 *   · 中层白色 = 数据本体
 *   · 底层靛蓝 = 存储基座
 * 每层带一条同族加深的细描边 —— 无容器时这是必需项而非装饰：
 * 白色中层要在浅色标题栏上看清、青色顶层要在深色任务栏上看清，全靠这条边。
 *
 * 用法：标题栏 20px、关于对话框 48px；最小 16px。
 * 禁止拉伸 / 旋转 / 把描边改成异色。
 */

/** 三层菱形中心 y（32 单位画布；无容器 → 图形放大到接近满画布）。 */
const LAYER_CY = [10.1, 16.0, 21.9] as const;
/** 菱形半宽 / 半高（等距 2:1）。 */
const HW = 12.8;
const HH = 6.05;

/** 单色版几何：略收窄，给线宽留空间。 */
const MONO_CY = [9.2, 16.0, 22.8] as const;
const MONO_HW = 11.4;
const MONO_HH = 4.5;

/** 等距菱形路径。 */
function diamond(cx: number, cy: number, hw = HW, hh = HH): string {
  return `M${cx} ${cy - hh}L${cx + hw} ${cy}L${cx} ${cy + hh}L${cx - hw} ${cy}Z`;
}

/** 主版：透明底，仅三层菱形。size 为渲染边长（px）。 */
export function navkitLogo(size = 20): string {
  const uid = `nkg${Math.random().toString(36).slice(2, 8)}`;
  const small = size <= 24;
  // 小尺寸线更粗、投影更短更浓，才能在 2px 的层间缝里留住分隔线。
  const sw = small ? 1.2 : 0.9;
  const dy = small ? 0.55 : 0.9;
  const sd = small ? 0.5 : 0.9;
  const so = small ? 0.3 : 0.22;
  return `<svg class="navkit-logo" width="${size}" height="${size}" viewBox="0 0 32 32" aria-label="NavKit" role="img">
  <defs>
    <linearGradient id="${uid}t" x1="16" y1="${LAYER_CY[0] - HH}" x2="16" y2="${LAYER_CY[0] + HH}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#5ee7f5"/>
      <stop offset="1" stop-color="#22c8e0"/>
    </linearGradient>
    <linearGradient id="${uid}m" x1="16" y1="${LAYER_CY[1] - HH}" x2="16" y2="${LAYER_CY[1] + HH}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="1" stop-color="#eaf1ff"/>
    </linearGradient>
    <linearGradient id="${uid}b" x1="16" y1="${LAYER_CY[2] - HH}" x2="16" y2="${LAYER_CY[2] + HH}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#5b86ff"/>
      <stop offset="1" stop-color="#3a5ce8"/>
    </linearGradient>
    <filter id="${uid}s" x="-30%" y="-30%" width="160%" height="175%" color-interpolation-filters="sRGB">
      <feDropShadow dx="0" dy="${dy}" stdDeviation="${sd}" flood-color="#0a1c5c" flood-opacity="${so}"/>
    </filter>
  </defs>
  <g stroke-width="${sw}" stroke-linejoin="round">
    <path d="${diamond(16, LAYER_CY[2])}" fill="url(#${uid}b)" stroke="#3556dc" filter="url(#${uid}s)"/>
    <path d="${diamond(16, LAYER_CY[1])}" fill="url(#${uid}m)" stroke="#9fb9e0" filter="url(#${uid}s)"/>
    <path d="${diamond(16, LAYER_CY[0])}" fill="url(#${uid}t)" stroke="#1cb4cc" filter="url(#${uid}s)"/>
  </g>
</svg>`;
}

/** 单色版：透明底 + currentColor 三层菱形轮廓（下层减淡表示纵深）。 */
export function navkitLogoMono(size = 20): string {
  const d = (cy: number) => diamond(16, cy, MONO_HW, MONO_HH);
  const w = size <= 24 ? 1.7 : 1.5;
  return `<svg class="navkit-logo-mono" width="${size}" height="${size}" viewBox="0 0 32 32" aria-hidden="true">
  <g fill="none" stroke="currentColor" stroke-width="${w}" stroke-linejoin="round">
    <path d="${d(MONO_CY[0])}"/>
    <path d="${d(MONO_CY[1])}" opacity="0.82"/>
    <path d="${d(MONO_CY[2])}" opacity="0.62"/>
  </g>
</svg>`;
}
