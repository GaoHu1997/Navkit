/**
 * ER 关系图画布（模型视图）。
 *
 * 从活库读取表结构（字段 + 主键 + 外键），网格布局绘制实体框与外键连线，
 * 支持滚轮缩放、拖拽平移、适应窗口。
 */

export interface ErColumn {
  name: string;
  type: string;
  primary_key: boolean;
}

export interface ErTable {
  name: string;
  columns: ErColumn[];
  foreign_keys: { name: string; columns: string[]; ref_table: string; ref_columns: string[] }[];
}

const ROW_H = 17;
const HEADER_H = 22;
const PAD_X = 10;
const CHAR_W = 6.4;
const MIN_BOX_W = 128;
const GAP_X = 56;
const GAP_Y = 42;

interface Box {
  table: ErTable;
  x: number;
  y: number;
  w: number;
  h: number;
}

export class ModelCanvas {
  readonly el: HTMLElement;
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private tables: ErTable[] = [];
  private boxes: Box[] = [];
  private scale = 1;
  private offsetX = 40;
  private offsetY = 40;
  private dragging: { x: number; y: number } | null = null;

  constructor() {
    this.el = document.createElement("div");
    this.el.className = "model-canvas-host";
    this.el.style.cssText =
      "flex:1;min-height:0;position:relative;overflow:hidden;background:var(--bg-subtle)";

    this.canvas = document.createElement("canvas");
    this.canvas.style.cssText = "position:absolute;inset:0;cursor:grab";
    this.el.appendChild(this.canvas);
    this.ctx = this.canvas.getContext("2d")!;

    this.canvas.addEventListener("wheel", (e) => {
      e.preventDefault();
      const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
      this.setScale(this.scale * factor);
    });
    this.canvas.addEventListener("mousedown", (e) => {
      if (e.button === 0) {
        this.dragging = { x: e.clientX, y: e.clientY };
        this.canvas.style.cursor = "grabbing";
      }
    });
    window.addEventListener("mousemove", this.onMove);
    window.addEventListener("mouseup", this.onUp);
    window.addEventListener("resize", this.onWindowResize);
  }

  dispose(): void {
    window.removeEventListener("mousemove", this.onMove);
    window.removeEventListener("mouseup", this.onUp);
    window.removeEventListener("resize", this.onWindowResize);
  }

  private onWindowResize = (): void => {
    this.resize();
  };

  private onMove = (e: MouseEvent): void => {
    if (!this.dragging) return;
    this.offsetX += e.clientX - this.dragging.x;
    this.offsetY += e.clientY - this.dragging.y;
    this.dragging = { x: e.clientX, y: e.clientY };
    this.draw();
  };

  private onUp = (): void => {
    this.dragging = null;
    this.canvas.style.cursor = "grab";
  };

  setTables(tables: ErTable[]): void {
    this.tables = tables;
    this.layout();
    this.fit();
  }

  setScale(v: number): void {
    this.scale = Math.min(2.5, Math.max(0.35, v));
    this.draw();
  }

  getScale(): number {
    return this.scale;
  }

  /** 布局：按列数 = ceil(√n) 网格排布。 */
  private layout(): void {
    this.boxes = [];
    const n = this.tables.length;
    if (n === 0) return;
    const cols = Math.max(1, Math.ceil(Math.sqrt(n)));
    let x = 0;
    let y = 0;
    let rowMaxH = 0;
    let colX = 0;
    this.tables.forEach((t, i) => {
      const nameW = t.name.length * CHAR_W + PAD_X * 2;
      const colW = t.columns.length
        ? Math.max(
            ...t.columns.map((c) => (c.name.length + c.type.length + 4) * CHAR_W + 22),
          ) + PAD_X
        : MIN_BOX_W;
      const w = Math.max(MIN_BOX_W, Math.min(300, Math.max(nameW, colW)));
      const h = HEADER_H + Math.max(1, t.columns.length) * ROW_H + 6;
      if (i % cols === 0) {
        x = 0;
        y = this.boxes.length ? Math.max(...this.boxes.map((b) => b.y + b.h)) + GAP_Y : 0;
        rowMaxH = h;
        colX = 0;
      } else {
        x = colX + GAP_X;
        rowMaxH = Math.max(rowMaxH, h);
      }
      this.boxes.push({ table: t, x, y, w, h });
      colX = x + w;
    });
  }

  fit(): void {
    this.resize();
    if (!this.boxes.length) {
      this.draw();
      return;
    }
    const maxX = Math.max(...this.boxes.map((b) => b.x + b.w));
    const maxY = Math.max(...this.boxes.map((b) => b.y + b.h));
    const w = this.el.clientWidth || 800;
    const h = this.el.clientHeight || 600;
    this.scale = Math.min(1.6, Math.max(0.35, Math.min((w - 60) / maxX, (h - 60) / maxY)));
    this.offsetX = (w - maxX * this.scale) / 2;
    this.offsetY = (h - maxY * this.scale) / 2;
    this.draw();
  }

  private resize(): void {
    const dpr = window.devicePixelRatio || 1;
    const w = this.el.clientWidth;
    const h = this.el.clientHeight;
    if (w === 0 || h === 0) return;
    this.canvas.width = Math.round(w * dpr);
    this.canvas.height = Math.round(h * dpr);
    this.canvas.style.width = `${w}px`;
    this.canvas.style.height = `${h}px`;
    this.draw();
  }

  private draw(): void {
    const ctx = this.ctx;
    const dpr = window.devicePixelRatio || 1;
    const w = this.el.clientWidth;
    const h = this.el.clientHeight;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    if (!this.boxes.length) {
      ctx.fillStyle = "#7d8794";
      ctx.font = "12px 'Segoe UI','Microsoft YaHei UI'";
      ctx.textAlign = "center";
      ctx.fillText("当前数据库没有表", w / 2, h / 2);
      return;
    }

    ctx.save();
    ctx.translate(this.offsetX, this.offsetY);
    ctx.scale(this.scale, this.scale);

    // 外键连线（先画线再画框，线压在框下面）
    const byName = new Map<string, Box>();
    for (const b of this.boxes) byName.set(b.table.name, b);
    ctx.strokeStyle = "#e8862e";
    ctx.lineWidth = 1.2;
    for (const b of this.boxes) {
      for (const fk of b.table.foreign_keys) {
        const target = byName.get(fk.ref_table);
        if (!target) continue;
        const fromCol = fk.columns[0] ?? "";
        const toCol = fk.ref_columns[0] ?? "";
        const fromIdx = Math.max(0, b.table.columns.findIndex((c) => c.name === fromCol));
        const toIdx = Math.max(0, target.table.columns.findIndex((c) => c.name === toCol));
        const x1 = b.x + (b.x <= target.x ? 0 : b.w);
        const y1 = b.y + HEADER_H + fromIdx * ROW_H + ROW_H / 2;
        const x2 = target.x + (b.x <= target.x ? target.w : 0);
        const y2 = target.y + HEADER_H + toIdx * ROW_H + ROW_H / 2;
        const midX = (x1 + x2) / 2;
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.bezierCurveTo(midX, y1, midX, y2, x2, y2);
        ctx.stroke();
        // 箭头
        const ang = Math.atan2(y2 - y2, 1);
        ctx.beginPath();
        ctx.moveTo(x2, y2);
        ctx.lineTo(x2 - 5 * (b.x <= target.x ? 1 : -1), y2 - 3);
        ctx.lineTo(x2 - 5 * (b.x <= target.x ? 1 : -1), y2 + 3);
        ctx.closePath();
        ctx.fillStyle = "#e8862e";
        ctx.fill();
        void ang;
      }
    }

    // 实体框
    for (const b of this.boxes) {
      ctx.fillStyle = "#FFFFFF";
      ctx.strokeStyle = "#d3d8e0";
      ctx.lineWidth = 1;
      // 阴影
      ctx.fillStyle = "rgba(16,24,40,0.08)";
      ctx.fillRect(b.x + 2, b.y + 3, b.w, b.h);
      ctx.fillStyle = "#FFFFFF";
      ctx.fillRect(b.x, b.y, b.w, b.h);
      ctx.strokeStyle = "#d3d8e0";
      ctx.strokeRect(b.x + 0.5, b.y + 0.5, b.w - 1, b.h - 1);
      // 标题
      ctx.fillStyle = "#3574f0";
      ctx.fillRect(b.x, b.y, b.w, HEADER_H);
      ctx.fillStyle = "#FFFFFF";
      ctx.font = "600 11px 'Segoe UI','Microsoft YaHei UI'";
      ctx.textAlign = "left";
      ctx.fillText(b.table.name, b.x + PAD_X, b.y + 15);
      // 字段
      ctx.font = "11px 'Segoe UI','Microsoft YaHei UI'";
      b.table.columns.forEach((c, i) => {
        const cy = b.y + HEADER_H + i * ROW_H;
        if (i > 0) {
          ctx.strokeStyle = "#f1f3f6";
          ctx.beginPath();
          ctx.moveTo(b.x + 1, cy);
          ctx.lineTo(b.x + b.w - 1, cy);
          ctx.stroke();
        }
        if (c.primary_key) {
          ctx.fillStyle = "#c08327";
          ctx.fillText("◆", b.x + 4, cy + 13);
        }
        ctx.fillStyle = c.primary_key ? "#23272f" : "#3c434e";
        ctx.fillText(c.name, b.x + (c.primary_key ? 17 : PAD_X), cy + 13);
        ctx.fillStyle = "#7d8794";
        ctx.textAlign = "right";
        ctx.fillText(c.type, b.x + b.w - PAD_X, cy + 13);
        ctx.textAlign = "left";
      });
    }
    ctx.restore();
  }
}
