# -*- coding: utf-8 -*-
"""并排/差分对比工具：compare.py <参考图> <实现截图> <输出图>

输出:
  out_side.png  上下并排（参考在上, 实现在下）
  out_diff.png  差异热区（红=不一致）
"""
import sys
from PIL import Image, ImageChops


def main():
    ref_path, mine_path, out_prefix = sys.argv[1], sys.argv[2], sys.argv[3]
    ref = Image.open(ref_path).convert("RGB")
    mine = Image.open(mine_path).convert("RGB")
    # Tauri 无边框窗口带不可见 resize 边框（左9/上3），裁剪后与参考对齐
    if mine.size != ref.size and mine.size[0] >= ref.size[0] + 9 and mine.size[1] >= ref.size[1] + 3:
        mine = mine.crop((9, 3, 9 + ref.size[0], 3 + ref.size[1]))
    if ref.size != mine.size:
        mine = mine.resize(ref.size)
        print(f"! 尺寸不一致: ref={ref.size} mine resized")

    # 并排
    w, h = ref.size
    side = Image.new("RGB", (w, h * 2 + 8), (255, 0, 255))
    side.paste(ref, (0, 0))
    side.paste(mine, (0, h + 8))
    side.save(f"{out_prefix}_side.png")

    # 差分
    diff = ImageChops.difference(ref, mine).convert("L")
    px = diff.load()
    heat = Image.new("RGB", ref.size, (255, 255, 255))
    hp = heat.load()
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            v = px[x, y]
            if v > 28:
                hp[x, y] = (255, 0, 0)
            elif v > 10:
                hp[x, y] = (255, 200, 120)
    heat.save(f"{out_prefix}_diff.png")

    # 差异统计（分块）
    import statistics
    total = w * h
    diffpx = sum(1 for y in range(0, h, 3) for x in range(0, w, 3) if px[x, y] > 28)
    print(f"差异像素占比(抽样): {diffpx / (total / 9) * 100:.2f}%")


if __name__ == "__main__":
    main()
