/** 图表对话框：把查询 / 表数据结果绘制成柱状 / 折线 / 饼图。 */

import { Dialog, select } from "../ui/Dialog";

export interface ChartSource {
  title: string;
  columns: string[];
  rows: (string | null)[][];
}

const PALETTE = [
  "#3574f0",
  "#e8862e",
  "#1f9254",
  "#dd4b93",
  "#7a5af8",
  "#0e9384",
  "#d92d20",
  "#8a6d3b",
];

/** 把结果集渲染为 SVG 图表。 */
export function renderChartSvg(
  src: ChartSource,
  type: "bar" | "line" | "pie",
  xCol: string,
  yCol: string,
  maxPoints: number,
): { svg: string; skipped: number } {
  const xi = src.columns.indexOf(xCol);
  const yi = src.columns.indexOf(yCol);
  const points: { label: string; value: number }[] = [];
  let skipped = 0;
  for (const row of src.rows) {
    const raw = row[yi];
    const v = raw === null ? NaN : Number(raw);
    if (!Number.isFinite(v)) {
      skipped++;
      continue;
    }
    points.push({ label: String(row[xi] ?? ""), value: v });
    if (points.length >= maxPoints) break;
  }

  const W = 640;
  const H = 380;
  const padL = 56;
  const padR = 24;
  const padT = 26;
  const padB = 64;
  const parts: string[] = [];
  const esc = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

  parts.push(
    `<rect x="0" y="0" width="${W}" height="${H}" fill="#FFFFFF"/>`,
    `<text x="${W / 2}" y="16" font-size="12" fill="#3A4A5A" text-anchor="middle" font-family="Microsoft YaHei UI">${esc(yCol)} / ${esc(xCol)}</text>`,
  );

  if (points.length === 0) {
    parts.push(
      `<text x="${W / 2}" y="${H / 2}" font-size="13" fill="#8A929E" text-anchor="middle" font-family="Microsoft YaHei UI">没有可绘制的数据（${yCol} 列需要是数字）</text>`,
    );
    return { svg: `<svg viewBox="0 0 ${W} ${H}" width="100%" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`, skipped };
  }

  if (type === "pie") {
    const total = points.reduce((s, p) => s + Math.abs(p.value), 0) || 1;
    const cx = W / 2;
    const cy = (H - padB + padT) / 2 + 8;
    const r = Math.min(W / 2 - 90, (H - padT - padB) / 2);
    let ang = -Math.PI / 2;
    points.forEach((p, i) => {
      const frac = Math.abs(p.value) / total;
      const a2 = ang + frac * Math.PI * 2;
      const large = frac > 0.5 ? 1 : 0;
      const x1 = cx + r * Math.cos(ang);
      const y1 = cy + r * Math.sin(ang);
      const x2 = cx + r * Math.cos(a2);
      const y2 = cy + r * Math.sin(a2);
      const color = PALETTE[i % PALETTE.length];
      parts.push(
        `<path d="M${cx} ${cy} L${x1.toFixed(1)} ${y1.toFixed(1)} A${r} ${r} 0 ${large} 1 ${x2.toFixed(1)} ${y2.toFixed(1)} Z" fill="${color}" stroke="#FFFFFF" stroke-width="1.5"/>`,
      );
      const mid = (ang + a2) / 2;
      const lx = cx + (r + 16) * Math.cos(mid);
      const ly = cy + (r + 16) * Math.sin(mid);
      parts.push(
        `<text x="${lx.toFixed(1)}" y="${ly.toFixed(1)}" font-size="10.5" fill="#3A4A5A" text-anchor="${Math.cos(mid) >= 0 ? "start" : "end"}" font-family="Microsoft YaHei UI">${esc(p.label || "-")} (${(frac * 100).toFixed(1)}%)</text>`,
      );
      ang = a2;
    });
    return { svg: wrap(W, H, parts), skipped };
  }

  // 柱状 / 折线：坐标轴
  const maxV = Math.max(...points.map((p) => p.value), 0);
  const minV = Math.min(...points.map((p) => p.value), 0);
  const top = maxV === minV ? maxV + 1 : maxV;
  const bottom = minV < 0 ? minV : 0;
  const plotW = W - padL - padR;
  const plotH = H - padT - padB;
  const yOf = (v: number) => padT + plotH - ((v - bottom) / (top - bottom)) * plotH;
  const step = plotW / points.length;

  // 网格 + 刻度
  for (let g = 0; g <= 4; g++) {
    const v = bottom + ((top - bottom) * g) / 4;
    const y = yOf(v);
    parts.push(
      `<line x1="${padL}" y1="${y.toFixed(1)}" x2="${W - padR}" y2="${y.toFixed(1)}" stroke="#E4EAF0" stroke-width="1"/>`,
      `<text x="${padL - 6}" y="${(y + 3.5).toFixed(1)}" font-size="10" fill="#8A929E" text-anchor="end" font-family="Microsoft YaHei UI">${formatNum(v)}</text>`,
    );
  }
  parts.push(
    `<line x1="${padL}" y1="${yOf(0)}" x2="${W - padR}" y2="${yOf(0)}" stroke="#B9C4CE" stroke-width="1"/>`,
  );

  const color = PALETTE[0];
  points.forEach((p, i) => {
    const cx = padL + step * (i + 0.5);
    // X 轴标签（隔多显示防重叠）
    const every = Math.ceil(points.length / 16);
    if (i % every === 0) {
      parts.push(
        `<text x="${cx.toFixed(1)}" y="${H - padB + 14}" font-size="9.5" fill="#5A6A7A" text-anchor="end" font-family="Microsoft YaHei UI" transform="rotate(-32 ${cx.toFixed(1)} ${H - padB + 14})">${esc(p.label.length > 14 ? p.label.slice(0, 13) + "…" : p.label)}</text>`,
      );
    }
    if (type === "bar") {
      const bw = Math.max(4, step * 0.6);
      const y = yOf(p.value);
      parts.push(
        `<rect x="${(cx - bw / 2).toFixed(1)}" y="${Math.min(y, yOf(0)).toFixed(1)}" width="${bw.toFixed(1)}" height="${Math.max(1, Math.abs(y - yOf(0))).toFixed(1)}" fill="${color}" rx="1.5"/>`,
      );
    } else {
      if (i > 0) {
        const px = padL + step * (i - 0.5);
        parts.push(
          `<line x1="${px.toFixed(1)}" y1="${yOf(points[i - 1].value).toFixed(1)}" x2="${cx.toFixed(1)}" y2="${yOf(p.value).toFixed(1)}" stroke="${color}" stroke-width="1.8"/>`,
        );
      }
      parts.push(
        `<circle cx="${cx.toFixed(1)}" cy="${yOf(p.value).toFixed(1)}" r="2.6" fill="${color}"/>`,
      );
    }
  });

  return { svg: wrap(W, H, parts), skipped };
}

function wrap(W: number, H: number, parts: string[]): string {
  return `<svg viewBox="0 0 ${W} ${H}" width="100%" xmlns="http://www.w3.org/2000/svg">${parts.join("")}</svg>`;
}

function formatNum(v: number): string {
  if (Math.abs(v) >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
  if (Math.abs(v) >= 1_000) return `${(v / 1_000).toFixed(1)}k`;
  return String(Math.round(v * 100) / 100);
}

/** 打开图表对话框。 */
export function openChartDialog(src: ChartSource): Dialog {
  const typeSelect = select(
    [
      { value: "bar", label: "柱状图" },
      { value: "line", label: "折线图" },
      { value: "pie", label: "饼图" },
    ],
    "bar",
    110,
  );
  const numericCols = src.columns.filter((c) =>
    src.rows.some((r) => {
      const i = src.columns.indexOf(c);
      const v = r[i];
      return v !== null && v !== "" && Number.isFinite(Number(v));
    }),
  );
  const yCols = numericCols.length ? numericCols : src.columns;
  const xSelect = select(src.columns.map((c) => ({ value: c, label: c })), src.columns[0], 150);
  const ySelect = select(yCols.map((c) => ({ value: c, label: c })), yCols[0], 150);

  const chartHost = document.createElement("div");
  chartHost.style.border = "1px solid var(--border, #DBDBDB)";

  const redraw = () => {
    const { svg } = renderChartSvg(
      src,
      typeSelect.value as "bar" | "line" | "pie",
      xSelect.value,
      ySelect.value,
      200,
    );
    chartHost.innerHTML = svg;
  };
  typeSelect.addEventListener("change", redraw);
  xSelect.addEventListener("change", redraw);
  ySelect.addEventListener("change", redraw);

  const dlg = new Dialog({
    title: "创建图表",
    width: 700,
    render: (body) => {
      const bar = document.createElement("div");
      bar.style.cssText = "display:flex;gap:14px;align-items:center;margin-bottom:8px;flex-wrap:wrap";
      const mk = (label: string, el: HTMLElement) => {
        const s = document.createElement("span");
        s.style.cssText = "font-size:12px;color:#5A6A7A";
        s.textContent = label;
        bar.appendChild(s);
        bar.appendChild(el);
      };
      mk("类型:", typeSelect);
      mk("X 轴:", xSelect);
      mk("数值列:", ySelect);
      body.appendChild(bar);
      body.appendChild(chartHost);
      redraw();
    },
    buttons: [{ label: "关闭" }],
  });
  return dlg;
}
