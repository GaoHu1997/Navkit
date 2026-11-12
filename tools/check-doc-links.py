#!/usr/bin/env python3
"""校验 docs/ 下的文档是否仍与代码一致。

两类引用分开解析：
  - Markdown 链接 [文字](<path>) / [文字](path)：相对**文档所在目录**
  - 行内代码路径 `path/file.ext`：相对**仓库根**（只校验带目录前缀的路径）

`#L12` / `:12` 行号后缀先剥掉；外部 URL、锚点跳过。
任何缺失项打到 stdout，并以退出码 1 结束。

用法：
  python tools/check-doc-links.py           # 校验
  python tools/check-doc-links.py --quiet   # 只打印缺失
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent / "docs"  # docs/
REPO = DOCS.parent

LINK_RE = re.compile(r"\]\((?:<)?([^)>]+?)(?:>)?\)")
CODE_RE = re.compile(
    r"`([A-Za-z0-9_./-]+\.(?:rs|ts|mjs|js|java|py|ps1|json|toml|md|css|html))(?::[0-9]+|#L[0-9]+(?:-L[0-9]+)?)?`"
)
LINE_SUFFIX_RE = re.compile(r":[0-9]+$")

SKIP_PREFIX = ("http://", "https://", "mailto:", "#", "//", "www.")
# 这些前缀表示仓库根相对路径
ROOT_PREFIXES = ("src/", "src-tauri/", "tools/", "bridge/", "docs/", "testdata/", "public/", "data/")


def strip_line_ref(ref: str) -> str:
    """去掉 `#L12`、`#L12-L30`、`:572` 这类行号后缀。"""
    ref = ref.strip().split("#", 1)[0]
    return LINE_SUFFIX_RE.sub("", ref)


def resolve_link(doc: Path, ref: str) -> Path | None:
    ref = strip_line_ref(ref)
    if not ref or ref.startswith(SKIP_PREFIX) or ref.startswith(".."):
        return None if not ref.startswith("..") else (doc.parent / ref).resolve()
    # 以仓库根目录名开头的链接按仓库根解析
    if ref.startswith(ROOT_PREFIXES):
        return (REPO / ref).resolve()
    return (doc.parent / ref).resolve()


def resolve_code(ref: str) -> Path | None:
    ref = strip_line_ref(ref)
    if not ref.startswith(ROOT_PREFIXES):
        return None  # 裸文件名（如 base.css）是简写，不校验
    return (REPO / ref).resolve()


def main() -> int:
    quiet = "--quiet" in sys.argv
    missing: list[tuple[str, str]] = []
    checked = 0
    docs = sorted(DOCS.rglob("*.md"))

    for doc in docs:
        text = doc.read_text(encoding="utf-8", errors="replace")
        pairs: list[tuple[str, Path | None]] = [
            ("链接", resolve_link(doc, m.group(1))) for m in LINK_RE.finditer(text)
        ]
        pairs += [("代码", resolve_code(m.group(1))) for m in CODE_RE.finditer(text)]
        for kind, target in pairs:
            if target is None:
                continue
            checked += 1
            if not target.exists():
                missing.append((str(doc.relative_to(REPO)), target.relative_to(REPO) if target.is_relative_to(REPO) else str(target)))

    if not quiet:
        print(f"检查了 {checked} 条引用，覆盖 {len(docs)} 个文档")

    if missing:
        # active/ 里的计划文档允许引用尚未创建的目标文件；completed/ 与 design/ 必须全部有效
        def is_active(path: str) -> bool:
            return path.replace("\\", "/").startswith("docs/active/")

        hard = [(d, r) for d, r in missing if not is_active(d)]
        soft = [(d, r) for d, r in missing if is_active(d)]
        if soft:
            print(f"\n警告：{len(soft)} 条 active/ 文档引用了尚不存在的目标（进行中计划允许）：")
            seen = set()
            for doc, ref in soft:
                if (doc, ref) in seen:
                    continue
                seen.add((doc, ref))
                print(f"  {doc}  ->  {ref}")
        if hard:
            print(f"\n错误：{len(hard)} 条结果/设计文档引用失效：")
            seen = set()
            for doc, ref in hard:
                if (doc, ref) in seen:
                    continue
                seen.add((doc, ref))
                print(f"  {doc}  ->  {ref}")
            return 1

    print("\ncompleted/ 与 design/ 全部引用有效。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
