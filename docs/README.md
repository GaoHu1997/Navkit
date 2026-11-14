# NavKit 文档索引

`docs/` 按工作生命周期分三条线，外加一份常驻设计输入。任何文档都必须属于其中一条。

| 目录 | 放什么 | 当前状态 |
|---|---|---|
| [`completed/`](completed/) | **只有结果**：[产品状态](completed/PRODUCT-STATE.md)，含能力矩阵、25 项实现归档、验收状态 | 与当前工作树一致（2026-11-11 核对） |
| [`active/`](active/) | 当前轮次：进行中的过程文档与证据日志 | 当前为空（上一轮已归档） |
| [`planned/`](planned/) | 尚未开工的计划，含范围、非目标、进入条件 | 当前无待开工计划，仅留候选备忘 |
| [`design/`](design/) | 常驻设计输入：[设计规范 DESIGN.md](design/DESIGN.md)（唯一权威）、UI 原型与设计资产索引 | 被 `src/` 源码注释按路径引用，**不搬动** |

## 从哪开始

- **想知道产品现在能做什么、做不到什么** → [`completed/PRODUCT-STATE.md`](completed/PRODUCT-STATE.md)
- **想接着干活** → [`completed/PRODUCT-STATE.md`](completed/PRODUCT-STATE.md) 的实现归档表与 §六 已知限制；新计划写进 [`planned/`](planned/)
- **想改界面** → [`design/DESIGN.md`](design/DESIGN.md)（唯一权威设计规范；已合并原界面规格表与画廊说明）
- **想开工一件没开始的事** → [`planned/`](planned/)，开工后移到 `active/`

## 维护规则

文档随代码走，规则写在 skill `.agents/skills/docs-lifecycle/SKILL.md`：

1. 计划开工 → `planned/` 移入 `active/`
2. 开发完成 → 结论并入 `completed/PRODUCT-STATE.md`（标 ✅/⏳/🚧），过程文档删除
3. 每次改代码后跑一致性闸门：

```bash
python tools/check-doc-links.py    # 结果/设计文档引用必须 0 失效
grep -rn '](<\.\./src-tauri' docs/active/*.md   # 层级必须是 ../../
npm run build
cargo check --manifest-path src-tauri/Cargo.toml --lib --offline
```

## 目录内约定

- `active/` 需要时再建 `evidence/`，存放被过程文档按行号引用的跑批日志，只增不改。
- 任务书命名沿用 `PL` 编号（`01-`…`27-`），已取消的 PL-24 / PL-26 / PL-28 保留编号不复用。
- 文档里的代码引用统一写成 `../../src-tauri/...` 或 `../../src/...`（从 `active/` 出发的仓库根相对路径）。
