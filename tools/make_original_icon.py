"""生成 NavKit 应用图标（v5 · 等距数据层 · 透明无容器）。

三层等距菱形堆叠，无背景板（透明底），每层带同族加深的细描边，
让白色中层在浅色背景（资源管理器 / 关于页）上仍有清晰轮廓。

输出：src-tauri/icons/{16,32,48,128,256,512}.png + icon.ico
运行：python tools/make_original_icon.py
"""

import os

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src-tauri", "icons")

# 96 单位设计稿
W = 96
SS = 4  # 超采样倍数（抗锯齿 + 圆角）

# 三层：中心 y、半宽、半高、上色、下色、描边色（自底向顶绘制，列表顺序即绘制顺序）
LAYERS = [
    (67.32, 42.0, 19.6, (0x5B, 0x86, 0xFF), (0x3A, 0x5C, 0xE8), (0x35, 0x56, 0xDC)),  # 底层 靛蓝
    (48.00, 42.0, 19.6, (0xFF, 0xFF, 0xFF), (0xEA, 0xF1, 0xFF), (0x9F, 0xB9, 0xE0)),  # 中层 白
    (28.68, 42.0, 19.6, (0x5E, 0xE7, 0xF5), (0x22, 0xC8, 0xE0), (0x1C, 0xB4, 0xCC)),  # 顶层 青
]
# 小尺寸变体（<= 24px）：投影更短更浓；几何不变
SMALL_MAX = 24
CX = 48.0
CORNER_R = 5.6       # 菱形圆角（设计稿单位，随几何放大）
STROKE_W = 2.7       # 描边宽（设计稿单位，≈ viewBox32 的 0.9）
STROKE_W_SMALL = 3.6  # ≈ viewBox32 的 1.2
SHADOW_DY = 2.7      # 层间投影垂直偏移
SHADOW_BLUR = 2.7    # 层间投影模糊半径
SHADOW_ALPHA = 0.22
SHADOW_DY_SMALL = 1.7
SHADOW_BLUR_SMALL = 1.5
SHADOW_ALPHA_SMALL = 0.30
SHADOW_RGB = (0x0A, 0x1C, 0x5C)


def layer_rgba(size: int, cy: float, hw: float, hh: float, c_top, c_bot, stroke_w: float):
    """返回 (RGB 数组, 填充蒙版 L, 描边蒙版 L 或 None)。菱形带圆角。"""
    s = size * SS
    r = s / W

    def make_mask(dx: float, dy: float):
        poly = [
            (CX * r, (cy - hh - dy) * r),
            ((CX + hw + dx) * r, cy * r),
            (CX * r, (cy + hh + dy) * r),
            ((CX - hw - dx) * r, cy * r),
        ]
        m = Image.new("L", (s, s), 0)
        ImageDraw.Draw(m).polygon(poly, fill=255)
        # 模糊后阈值化 → 圆角
        m = m.filter(ImageFilter.GaussianBlur(CORNER_R * r * 0.75))
        m = m.point(lambda p: 255 if p >= 128 else 0)
        return m

    fill_mask = make_mask(0.0, 0.0).resize((size, size), Image.LANCZOS)
    stroke_mask = None
    if stroke_w > 0:
        half = stroke_w / 2.0
        stroke_mask = make_mask(half, half).resize((size, size), Image.LANCZOS)

    # 层内垂直渐变
    ys = np.arange(size, dtype=np.float32)[:, None]
    t = np.clip((ys / size * W - (cy - hh)) / (2 * hh), 0.0, 1.0)
    rgb = (np.array(c_top, dtype=np.float32)[None, None, :] * (1 - t[..., None])
           + np.array(c_bot, dtype=np.float32)[None, None, :] * t[..., None])
    rgb = np.ascontiguousarray(np.repeat(rgb, size, axis=1).astype(np.uint8))
    return rgb, fill_mask, stroke_mask


def draw_icon(size: int) -> Image.Image:
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))  # 透明底
    small = size <= SMALL_MAX
    stroke_w = STROKE_W_SMALL if small else STROKE_W
    shadow_alpha = SHADOW_ALPHA_SMALL if small else SHADOW_ALPHA
    shadow_dy = SHADOW_DY_SMALL if small else SHADOW_DY
    shadow_blur = SHADOW_BLUR_SMALL if small else SHADOW_BLUR

    # 逐层：投影 → 描边 → 填充（后画的层压在前一层上，形成悬浮堆叠）
    for cy, hw, hh, c_top, c_bot, c_stroke in LAYERS:
        rgb, fill_mask, stroke_mask = layer_rgba(size, cy, hw, hh, c_top, c_bot, stroke_w)
        silhouette = stroke_mask if stroke_mask is not None else fill_mask

        # 投影：把轮廓蒙版下移 + 模糊，染成深色
        s_off = int(round(shadow_dy * size / W))
        m = silhouette.filter(ImageFilter.GaussianBlur(shadow_blur * size / W))
        m = Image.fromarray(np.roll(np.array(m), s_off, axis=0))
        shadow = Image.new("RGBA", (size, size), SHADOW_RGB + (0,))
        shadow.putalpha(m.point(lambda p: int(p * shadow_alpha)))
        canvas.alpha_composite(shadow)

        # 描边（同族加深）
        if stroke_mask is not None:
            line = Image.new("RGBA", (size, size), c_stroke + (0,))
            line.putalpha(stroke_mask)
            canvas.alpha_composite(line)

        # 填充
        layer = Image.fromarray(rgb, "RGB").convert("RGBA")
        layer.putalpha(fill_mask)
        canvas.alpha_composite(layer)
    return canvas


def main() -> None:
    os.makedirs(OUT, exist_ok=True)
    sizes = [16, 32, 48, 128, 256, 512]
    icons = {}
    for s in sizes:
        im = draw_icon(s)
        im.save(os.path.join(OUT, f"{s}x{s}.png"))
        icons[s] = im
    icons[256].save(
        os.path.join(OUT, "icon.ico"),
        format="ICO",
        sizes=[(16, 16), (32, 32), (48, 48), (128, 128), (256, 256)],
    )
    print("icons written to", os.path.abspath(OUT))


if __name__ == "__main__":
    main()
