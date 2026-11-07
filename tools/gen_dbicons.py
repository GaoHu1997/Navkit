# -*- coding: utf-8 -*-
"""生成 src/ui/dbIcons.ts —— 数据源官方 logo 图标（16x16 内容体）。

素材来源 tools/db_art/off_<kind>.svg|png|jpg（各数据库/数据源的官方品牌 logo，
由 tools/fetch_official_logos.py 等采集脚本获取）。不使用任何自绘品牌图形。

- 矢量 logo：缩放嵌入白色圆角徽章（logo 原样、不变形）；
- 光栅 logo（官方仅提供 PNG/JPG 时）：base64 内嵌 <image>；
- 同时生成彩色与灰阶（未连接 / 规划中）两套内容体。

用法：python tools/gen_dbicons.py
（数据源目录 src/core/dbcatalog.ts 由 catalog.rs 生成，此处解析做覆盖校验。）
"""
import base64
import io
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ART_DIR = os.path.join(ROOT, "tools", "db_art")
CATALOG = os.path.join(ROOT, "src", "core", "dbcatalog.ts")
OUT = os.path.join(ROOT, "src", "ui", "dbIcons.ts")


def catalog_ids():
    s = io.open(CATALOG, encoding="utf-8").read()
    m = re.search(r"export const DB_KINDS.*?\[(.*?)\n\];", s, re.S)
    if not m:
        raise SystemExit("未能定位 DB_KINDS 块")
    return re.findall(r'\{\s*id:\s*"([a-z0-9_]+)"', m.group(1))


def extract_svg(path):
    s = io.open(path, encoding="utf-8").read()
    vb = re.search(r'viewBox="([^"]+)"', s)
    inner = re.sub(r"^.*?<svg[^>]*>", "", s, flags=re.S)
    inner = re.sub(r"</svg>\s*$", "", inner.strip(), flags=re.S)
    inner = re.sub(r"<title>.*?</title>", "", inner, flags=re.S)
    inner = re.sub(r"\s+", " ", inner).strip()
    return (vb.group(1) if vb else "0 0 24 24"), inner


def reid(inner, prefix):
    inner = re.sub(r'\bid="([^"]+)"', lambda m: 'id="%s%s"' % (prefix, m.group(1)), inner)
    inner = re.sub(r"url\(#([^)]+)\)", lambda m: "url(#%s%s)" % (prefix, m.group(1)), inner)
    return inner


def fit_transform(vb):
    """把 logo 包围盒等比放进 12x12 内容区（16 徽章内 2..14）。"""
    x1, y1, w, h = [float(v) for v in vb.replace(",", " ").split()]
    if w <= 0 or h <= 0:
        w, h = 1, 1
    scale = 12.0 / max(w, h)
    tx = 2 + (12 - w * scale) / 2 - x1 * scale
    ty = 2 + (12 - h * scale) / 2 - y1 * scale
    return '<g transform="translate(%.3f %.3f) scale(%.5f)">%s</g>' % (tx, ty, scale, "%s")


CHIP = ('<rect x="0.5" y="0.5" width="15" height="15" rx="3.1" fill="#FFFFFF" '
        'stroke="#C9D1DA" stroke-width="1"/>')
CHIP_GRAY = ('<rect x="0.5" y="0.5" width="15" height="15" rx="3.1" fill="#FFFFFF" '
             'stroke="#C3C8CD" stroke-width="1"/>')


def gray_hex(hexs):
    r, g, b = int(hexs[0:2], 16), int(hexs[2:4], 16), int(hexs[4:6], 16)
    y = round(0.2126 * r + 0.7152 * g + 0.0722 * b)
    return "%02X%02X%02X" % (y, y, y)


def grayify(body):
    def rep(m):
        hexs = m.group(1)
        alpha = ""
        if len(hexs) == 8:
            alpha = hexs[6:]
            hexs = hexs[:6]
        elif len(hexs) == 4:
            alpha = hexs[3:]
            hexs = hexs[0] * 2 + hexs[1] * 2 + hexs[2] * 2
        elif len(hexs) == 3:
            hexs = hexs[0] * 2 + hexs[1] * 2 + hexs[2] * 2
        if hexs.upper() == "FFFFFF":
            return m.group(0)
        return "#" + gray_hex(hexs) + alpha
    return re.sub(r"#([0-9A-Fa-f]{3,8})", rep, body)


def svg_bodies(kind, path):
    vb, inner = extract_svg(path)
    inner = reid(inner, "dk-%s-" % kind)
    g = fit_transform(vb)
    return CHIP + (g % inner), CHIP_GRAY + (g % grayify(inner))


def raster_bodies(path):
    from PIL import Image
    im = Image.open(path).convert("RGBA")
    im.thumbnail((192, 192))
    buf = io.BytesIO()
    im.save(buf, format="PNG")
    uri = base64.b64encode(buf.getvalue()).decode("ascii")
    gray = im.convert("LA")
    rgba = Image.new("RGBA", im.size)
    rgba.putdata([(p[0], p[0], p[0], p[1]) for p in gray.getdata()])
    buf2 = io.BytesIO()
    rgba.save(buf2, format="PNG")
    uri_g = base64.b64encode(buf2.getvalue()).decode("ascii")
    img = ('<image x="1.6" y="1.6" width="12.8" height="12.8" preserveAspectRatio="xMidYMid meet" '
           'href="data:image/png;base64,%s"/>')
    return CHIP + (img % uri), CHIP_GRAY + (img % uri_g)


def main():
    ids = catalog_ids()
    bodies, bodies_off, missing = {}, {}, []
    for kid in ids:
        svg_p = os.path.join(ART_DIR, "off_%s.svg" % kid)
        png_p = os.path.join(ART_DIR, "off_%s.png" % kid)
        jpg_p = os.path.join(ART_DIR, "off_%s.jpg" % kid)
        webp_p = os.path.join(ART_DIR, "off_%s.webp" % kid)
        if os.path.exists(svg_p):
            c, g = svg_bodies(kid, svg_p)
        elif os.path.exists(png_p):
            c, g = raster_bodies(png_p)
        elif os.path.exists(jpg_p):
            c, g = raster_bodies(jpg_p)
        elif os.path.exists(webp_p):
            c, g = raster_bodies(webp_p)
        else:
            missing.append(kid)
            continue
        bodies[kid], bodies_off[kid] = c, g

    for name, body in list(bodies.items()) + list(bodies_off.items()):
        if "`" in body or "${" in body:
            raise SystemExit("body 含模板字符：%s" % name)

    lines = [
        "/**",
        " * 数据源官方 logo 图标（16x16 内容体）—— 由 tools/gen_dbicons.py 生成，请勿手工编辑。",
        " *",
        " * 素材为各数据库/数据源的官方品牌 logo（SVG 矢量或官方光栅版），不含自绘品牌图形；",
        " * 灰阶体用于未连接 / 规划中状态。",
        " */",
        "",
        "export const DB_ICON_BODIES: Record<string, string> = {",
    ]
    for k in sorted(bodies):
        lines.append('  "%s": `%s`,' % (k, bodies[k]))
    lines += [
        "};",
        "",
        "/** 灰阶内容体（未连接 / 规划中）。 */",
        "export const DB_ICON_BODIES_OFF: Record<string, string> = {",
    ]
    for k in sorted(bodies_off):
        lines.append('  "%s": `%s`,' % (k, bodies_off[k]))
    lines += [
        "};",
        "",
        "/** 按 kind 取官方 logo 内容体（off=true 取灰阶；未收录返回 null）。 */",
        "export function kindIconBody(id: string, off = false): string | null {",
        "  return (off ? DB_ICON_BODIES_OFF[id] : DB_ICON_BODIES[id]) ?? null;",
        "}",
        "",
        "/** 连接已打开：官方 logo 原图，仅把自带外框换成连接绿底框。 */",
        """const OPEN_CHIP = 'fill="#FFFFFF" stroke="#C9D1DA" stroke-width="1"';""",
        """const OPEN_CHIP_CONNECTED = 'fill="#EAF9F1" stroke="#22A06B" stroke-width="1"';""",
        "const OPEN_BODY_CACHE = new Map<string, string>();",
        "",
        "export function kindIconBodyOpen(id: string): string | null {",
        "  const body = DB_ICON_BODIES[id];",
        "  if (!body) return null;",
        "  const hit = OPEN_BODY_CACHE.get(id);",
        "  if (hit) return hit;",
        "  const open = body.replace(OPEN_CHIP, OPEN_CHIP_CONNECTED);",
        "  OPEN_BODY_CACHE.set(id, open);",
        "  return open;",
        "}",
        "",
    ]
    io.open(OUT, "w", encoding="utf-8", newline="\n").write("\n".join(lines))
    nvec = sum(1 for k in bodies if os.path.exists(os.path.join(ART_DIR, "off_%s.svg" % k)))
    print("dbIcons.ts：%d 个官方 logo（矢量 %d + 光栅 %d）" % (len(bodies), nvec, len(bodies) - nvec))
    if missing:
        print("尚无官方素材（前端回退徽章）：%s" % ", ".join(missing))


if __name__ == "__main__":
    main()
