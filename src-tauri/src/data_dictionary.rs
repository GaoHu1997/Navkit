//! 数据字典导出：五种格式生成器（Markdown / HTML / Excel / Word / PDF）。
//!
//! 模板结构与列集照搬 参考模板（参考模板.DocUtils.DBDoc.*）：
//!  · 列集：序号 / 列名 / 数据类型 / 长度 / 主键 / 自增 / 允许空 / 默认值 / 列说明
//!  · Word/PDF/Excel 含「修订日志」章节；HTML/Markdown 不含
//!  · Excel 支持「所有表一个 sheet」与「每张表一个 sheet」两种模式
//!  · Word：标题页 → 修订日志 → 目录(TOC 域，打开时自动更新) → 表结构 → 页码页脚
//!  · PDF：内嵌系统中文字体（SimHei / DengXian / KaiTi …），CIDFontType2 + Identity-H
//!
//! 样式按 NavKit 设计令牌适配（表头 #E7E6E6、强调色 #3574F0 等）。
//! 元数据采集在 commands.rs（dictionary_metadata，复用既有 scoped_db / table_schema）。

use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- DTO

/// 列（对应 参考模板 ColumnDto）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictColumn {
    pub order: usize,
    pub name: String,
    pub type_name: String,
    pub length: String,
    pub primary_key: bool,
    pub auto_increment: bool,
    pub nullable: bool,
    pub default_value: String,
    pub comment: String,
}

/// 表（对应 参考模板 TableDto）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictTable {
    pub name: String,
    pub comment: String,
    /// 数据库类型标签（如 "MySQL 8"）；Oracle 类型不输出「自增」列（与 参考模板 一致）。
    pub db_type: String,
    pub columns: Vec<DictColumn>,
}

/// 视图 / 存储过程（对应 参考模板 ViewProDto）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictObject {
    pub name: String,
    pub comment: String,
    pub script: String,
    pub db_type: String,
}

/// 一份数据库字典（对应 参考模板 DBDto）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictDatabase {
    pub name: String,
    pub doc_title: String,
    pub tables: Vec<DictTable>,
    pub views: Vec<DictObject>,
    /// 存储过程 + 函数（Markdown/PDF 输出「存储过程」章节时合并展示）。
    pub routines: Vec<DictObject>,
}

/// 导出选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictExportOptions {
    pub format: String, // markdown | html | excel | word | pdf
    /// Excel 工作表方式：single = 所有表一个 sheet；per_table = 每张表一个 sheet。
    pub excel_sheet_mode: String,
    pub include_tables: bool,
    pub include_views: bool,
    pub include_routines: bool,
    pub created_at: String,
}

impl DictExportOptions {
    fn include(&self, kind: &str) -> bool {
        match kind {
            "views" => self.include_views,
            "routines" => self.include_routines,
            _ => self.include_tables,
        }
    }
}

/// 该表是否隐藏「自增」列（Oracle 类型，与 参考模板 一致）。
fn hide_identity(db_type: &str) -> bool {
    db_type.starts_with("Oracle")
}

/// 是 / 否（参考模板 模板用词）。
fn yn(flag: bool) -> &'static str {
    if flag {
        "是"
    } else {
        "否"
    }
}

/// 表结构列标题（按类型决定是否含「自增」）。
fn structure_headers(db_type: &str) -> Vec<&'static str> {
    let mut headers = vec!["序号", "列名", "数据类型", "长度", "主键"];
    if !hide_identity(db_type) {
        headers.push("自增");
    }
    headers.extend(["允许空", "默认值", "列说明"]);
    headers
}

/// 列宽百分比（参考模板 比例归一化，合计 100；隐藏「自增」时把该列宽度并入列说明）。
fn structure_widths(db_type: &str) -> Vec<f64> {
    let hide = hide_identity(db_type);
    let mut widths = vec![7.0, 18.0, 11.0, 7.0, 7.0];
    if !hide {
        widths.push(7.0);
    }
    widths.extend([7.0, 9.0, if hide { 34.0 } else { 27.0 }]);
    widths
}

/// 列对齐：序号/类型/长度/主键/自增/允许空 居中，其余左对齐。
fn structure_centered(db_type: &str) -> Vec<bool> {
    let mut centered = vec![true, false, true, true, true];
    if !hide_identity(db_type) {
        centered.push(true);
    }
    centered.extend([true, true, false]);
    centered
}

/// 单元格文本。
fn cell_text(column: &DictColumn) -> Vec<String> {
    vec![
        column.order.to_string(),
        column.name.clone(),
        column.type_name.clone(),
        column.length.clone(),
        yn(column.primary_key).to_string(),
        yn(column.auto_increment).to_string(),
        yn(column.nullable).to_string(),
        column.default_value.clone(),
        column.comment.clone(),
    ]
}

/// 截取与 hide_identity 对齐的单元格子集。
fn row_cells(column: &DictColumn, db_type: &str) -> Vec<String> {
    let mut cells = cell_text(column);
    if hide_identity(db_type) {
        cells.remove(5); // 自增
    }
    cells
}

/// 修订日志表头。
const LOG_HEADERS: [&str; 5] = ["版本号", "修订日期", "修订内容", "修订人", "审核人"];

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn md_escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

// ---------------------------------------------------------------- Markdown

/// Markdown 导出（照搬 参考模板 MarkDownDoc 结构：表目录 → 表结构 → 视图 → 存储过程）。
pub fn build_markdown(dto: &DictDatabase, opts: &DictExportOptions) -> String {
    let mut sb = String::new();
    sb.push_str("### 📚 数据库表目录\n\n");
    // 目录行：| 序号 | 表名 | 说明 |（表名做成锚点链接）
    sb.push_str("| 序号 | 表名 | 说明 |\n|:---:|:---|:---|\n");
    let mut objects: Vec<(String, String)> = Vec::new();
    if opts.include("tables") {
        for t in &dto.tables {
            objects.push((t.name.clone(), t.comment.clone()));
        }
    }
    if opts.include("views") {
        for v in &dto.views {
            objects.push((v.name.clone(), v.comment.clone()));
        }
    }
    if opts.include("routines") {
        for r in &dto.routines {
            objects.push((r.name.clone(), r.comment.clone()));
        }
    }
    for (i, (name, comment)) in objects.iter().enumerate() {
        sb.push_str(&format!(
            "| {} | [{}]({}) | {} |\n",
            i + 1,
            md_escape_cell(name),
            format!("#{name}"),
            md_escape_cell(comment)
        ));
    }

    if opts.include("tables") && !dto.tables.is_empty() {
        sb.push_str("\n### 📒 表结构\n");
        for t in &dto.tables {
            sb.push_str(&format!("\n#### <a id=\"{}\"></a>表名： {}\n", t.name, t.name));
            sb.push_str(&format!("说明： {}\n\n", t.comment));
            let headers = structure_headers(&t.db_type);
            sb.push_str(&format!(
                "| {} |\n|{}|\n",
                headers.join(" | "),
                headers
                    .iter()
                    .map(|h| if h == &"列名" || h == &"列说明" { ":---" } else { ":---:" })
                    .collect::<Vec<_>>()
                    .join("|")
            ));
            for c in &t.columns {
                let cells: Vec<String> = row_cells(c, &t.db_type)
                    .iter()
                    .map(|s| md_escape_cell(s))
                    .collect();
                sb.push_str(&format!("| {} |\n", cells.join(" | ")));
            }
        }
    }

    if opts.include("views") && !dto.views.is_empty() {
        sb.push_str("\n### 📰 视图\n");
        for v in &dto.views {
            sb.push_str(&format!("\n#### <a id=\"{}\"></a>视图名： {}\n", v.name, v.name));
            sb.push_str(&format!("说明： {}\n\n", v.comment));
            sb.push_str("``` sql\n");
            sb.push_str(v.script.trim_end());
            sb.push_str("\n```\n");
        }
    }

    if opts.include("routines") && !dto.routines.is_empty() {
        sb.push_str("\n### 📜 存储过程\n");
        for r in &dto.routines {
            sb.push_str(&format!(
                "\n#### <a id=\"{}\"></a>存储过程名： {}\n",
                r.name, r.name
            ));
            sb.push_str(&format!("说明： {}\n\n", r.comment));
            sb.push_str("``` sql\n");
            sb.push_str(r.script.trim_end());
            sb.push_str("\n```\n");
        }
    }
    sb
}

// ---------------------------------------------------------------- HTML

/// HTML 导出（参考模板 html.cshtml 结构 + NavKit 设计令牌样式）。
pub fn build_html(dto: &DictDatabase, opts: &DictExportOptions) -> String {
    let mut out = String::new();
    let title = xml_escape(&dto.doc_title);
    let db_name = xml_escape(&dto.name);
    out.push_str("<!DOCTYPE html>\n<html lang=\"zh-cn\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str(&format!("<title>{title}</title>\n"));
    out.push_str(
        "<style type=\"text/css\">\n\
        :root{--accent:#3574f0;--border:#d3d8e0;--head:#f1f3f6;--band:#f7f8fa;}\n\
        body{padding:0 0 48px;font-family:\"Segoe UI\",\"Microsoft YaHei UI\",sans-serif;font-size:13px;line-height:1.6;color:#23272f;background:#ffffff}\n\
        h2{color:#14171c;letter-spacing:.02em}\n\
        .wrap{max-width:960px;margin:24px auto;padding:0 16px}\n\
        .meta{background:var(--band);border:1px solid var(--border);border-radius:6px;padding:10px 14px;margin:16px 0;font-size:12px}\n\
        table{width:100%;border-collapse:collapse;margin:8px 0 28px;font-size:12px}\n\
        th,td{border:1px solid var(--border);padding:5px 8px;text-align:left;vertical-align:top}\n\
        thead th{background:var(--head);font-weight:600;white-space:nowrap}\n\
        tbody tr:nth-child(even) td{background:#fbfcfd}\n\
        td.c{text-align:center}\n\
        a{color:var(--accent);text-decoration:none}\n\
        a:hover{text-decoration:underline}\n\
        .back{float:right;font-size:11px}\n\
        pre{background:#f7f8fa;border:1px solid var(--border);border-radius:4px;padding:10px 12px;overflow:auto;font-family:Consolas,monospace;font-size:12px}\n\
        .tname{font-size:15px;font-weight:600;margin-top:18px}\n\
        .tcmt{color:#7d8794;margin:2px 0 8px}\n\
        .cap{font-size:11px;color:#7d8794;margin:8px 0 4px}\n\
        </style>\n</head>\n<body>\n<div class=\"wrap\">\n",
    );
    out.push_str(&format!("<h2 style=\"text-align:center\">{title}</h2>\n"));
    out.push_str(&format!(
        "<div class=\"meta\"><b>数据库名：</b>{db_name}<br><b>文档版本：</b>v1.0<br><b>导出时间：</b>{}</div>\n",
        xml_escape(&opts.created_at)
    ));

    // 目录
    out.push_str("<table><thead><tr><th style=\"width:48px\">序号</th><th>表名</th><th>说明</th></tr></thead><tbody>\n");
    let mut idx = 0;
    if opts.include("tables") {
        for t in &dto.tables {
            idx += 1;
            out.push_str(&format!(
                "<tr><td class=\"c\">{}</td><td><a href=\"#{}\">{}</a></td><td>{}</td></tr>\n",
                idx,
                xml_escape(&t.name),
                xml_escape(&t.name),
                xml_escape(&t.comment)
            ));
        }
    }
    out.push_str("</tbody></table>\n");

    // 表结构
    if opts.include("tables") {
        for t in &dto.tables {
            let name = xml_escape(&t.name);
            let comment = xml_escape(&t.comment);
            out.push_str(&format!(
                "<a id=\"{name}\"></a>\n<div class=\"tname\"><a href=\"#index\" class=\"back\">返回目录</a>表名：{name}</div>\n<div class=\"tcmt\">说明：{comment}</div>\n<div class=\"cap\">数据列：</div>\n"
            ));
            let headers = structure_headers(&t.db_type);
            let centered = structure_centered(&t.db_type);
            out.push_str("<table><thead><tr>");
            for h in &headers {
                out.push_str(&format!("<th>{h}</th>"));
            }
            out.push_str("</tr></thead><tbody>\n");
            for c in &t.columns {
                let cells = row_cells(c, &t.db_type);
                out.push_str("<tr>");
                for (j, cell) in cells.iter().enumerate() {
                    let cls = if centered[j] { " class=\"c\"" } else { "" };
                    out.push_str(&format!("<td{cls}>{}</td>", xml_escape(cell)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody></table>\n");
        }
    }

    // 视图 / 存储过程（HTML 版与 参考模板 一致只导出表；但勾选时按需输出 SQL 段）
    if opts.include("views") && !dto.views.is_empty() {
        out.push_str("<h2>视图</h2>\n");
        for v in &dto.views {
            out.push_str(&format!(
                "<div class=\"tname\">视图名：{}</div>\n<div class=\"tcmt\">说明：{}</div>\n<pre>{}</pre>\n",
                xml_escape(&v.name),
                xml_escape(&v.comment),
                xml_escape(v.script.trim_end())
            ));
        }
    }
    if opts.include("routines") && !dto.routines.is_empty() {
        out.push_str("<h2>存储过程</h2>\n");
        for r in &dto.routines {
            out.push_str(&format!(
                "<div class=\"tname\">存储过程名：{}</div>\n<div class=\"tcmt\">说明：{}</div>\n<pre>{}</pre>\n",
                xml_escape(&r.name),
                xml_escape(&r.comment),
                xml_escape(r.script.trim_end())
            ));
        }
    }
    out.push_str("</div>\n</body>\n</html>\n");
    out
}

// ---------------------------------------------------------------- Excel / Word / PDF 在独立模块中

mod xlsx;
mod docx;
mod pdf_writer;

/// 按格式导出到目标文件。返回写入的字节数。
pub fn export_to_file(dto: &DictDatabase, opts: &DictExportOptions, path: &Path) -> anyhow::Result<u64> {
    let bytes = match opts.format.as_str() {
        "markdown" => build_markdown(dto, opts).into_bytes(),
        "html" => build_html(dto, opts).into_bytes(),
        "excel" => xlsx::build_xlsx(dto, opts)?,
        "word" => docx::build_docx(dto, opts)?,
        "pdf" => pdf_writer::build_pdf(dto, opts)?,
        other => anyhow::bail!("不支持的导出格式：{other}"),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::File::create(path)?.write_all(&bytes)?;
    Ok(bytes.len() as u64)
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用字典数据（含中文列注释，覆盖所有字段）。
    fn sample() -> DictDatabase {
        DictDatabase {
            name: "navkit_demo".into(),
            doc_title: "navkit_demo数据库设计文档".into(),
            tables: vec![
                DictTable {
                    name: "sys_user".into(),
                    comment: "用户表".into(),
                    db_type: "MySQL".into(),
                    columns: vec![
                        DictColumn {
                            order: 1, name: "id".into(), type_name: "bigint".into(),
                            length: String::new(), primary_key: true, auto_increment: true,
                            nullable: false, default_value: String::new(),
                            comment: "主键".into(),
                        },
                        DictColumn {
                            order: 2, name: "user_name".into(), type_name: "varchar".into(),
                            length: "50".into(), primary_key: false, auto_increment: false,
                            nullable: false, default_value: String::new(),
                            comment: "用户名，含中文说明".into(),
                        },
                        DictColumn {
                            order: 3, name: "score".into(), type_name: "decimal".into(),
                            length: "10,2".into(), primary_key: false, auto_increment: false,
                            nullable: true, default_value: "0.00".into(),
                            comment: "分数 | 带竖线".into(),
                        },
                    ],
                },
                DictTable {
                    name: "sys_role".into(),
                    comment: String::new(),
                    db_type: "MySQL".into(),
                    columns: vec![DictColumn {
                        order: 1, name: "id".into(), type_name: "int".into(),
                        length: String::new(), primary_key: true, auto_increment: false,
                        nullable: false, default_value: String::new(), comment: String::new(),
                    }],
                },
            ],
            views: vec![DictObject {
                name: "v_user".into(),
                comment: "用户视图".into(),
                script: "CREATE VIEW v_user AS SELECT * FROM sys_user WHERE age < 18;".into(),
                db_type: "MySQL".into(),
            }],
            routines: vec![DictObject {
                name: "sp_reset".into(),
                comment: String::new(),
                script: "CREATE PROCEDURE sp_reset() BEGIN SELECT 1; END".into(),
                db_type: "MySQL".into(),
            }],
        }
    }

    fn opts(format: &str) -> DictExportOptions {
        DictExportOptions {
            format: format.into(),
            excel_sheet_mode: "single".into(),
            include_tables: true,
            include_views: true,
            include_routines: true,
            created_at: "2026-11-11 20:35:21".into(),
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("navkit-dict-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(tag)
    }

    #[test]
    fn markdown_包含目录与表结构() {
        let md = build_markdown(&sample(), &opts("markdown"));
        assert!(md.contains("### 📚 数据库表目录"), "{md}");
        assert!(md.contains("[sys_user](#sys_user)"));
        assert!(md.contains("### 📒 表结构"));
        assert!(md.contains("#### <a id=\"sys_user\"></a>表名： sys_user"));
        assert!(md.contains("| 序号 | 列名 | 数据类型 | 长度 | 主键 | 自增 | 允许空 | 默认值 | 列说明 |"));
        assert!(md.contains("### 📰 视图"));
        assert!(md.contains("``` sql"));
        assert!(md.contains("CREATE VIEW v_user"));
        assert!(md.contains("### 📜 存储过程"));
    }

    #[test]
    fn markdown_竖线转义() {
        let md = build_markdown(&sample(), &opts("markdown"));
        assert!(md.contains("分数 \\| 带竖线"), "竖线必须转义");
    }

    #[test]
    fn html_结构与样式() {
        let html = build_html(&sample(), &opts("html"));
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>navkit_demo数据库设计文档</title>"));
        assert!(html.contains("<th>序号</th>"));
        assert!(html.contains("<th>列名</th>"));
        assert!(html.contains("#3574f0"), "应包含设计令牌强调色");
        assert!(html.contains("CREATE VIEW v_user"));
        assert!(html.contains("&lt;"), "SQL 需转义（如 &lt;）或至少完整转义流程");
    }

    /// 读取 zip 内成员为字符串（顺序调用，避免同时借用）。
    fn zip_read<R: std::io::Read + std::io::Seek>(
        archive: &mut zip::ZipArchive<R>,
        name: &str,
    ) -> String {
        use std::io::Read as _;
        let mut file = archive.by_name(name).unwrap();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    #[test]
    fn xlsx_单sheet模式_合法zip结构() {
        let bytes = xlsx::build_xlsx(&sample(), &opts("excel")).unwrap();
        assert!(bytes.len() > 1000);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert!(archive.by_name("[Content_Types].xml").is_ok());
        assert!(archive.by_name("xl/workbook.xml").is_ok());
        assert!(archive.by_name("xl/worksheets/sheet3.xml").is_ok());
        let content = zip_read(&mut archive, "xl/workbook.xml");
        assert_eq!(content.matches("<sheet ").count(), 3, "单 sheet 模式 3 个工作表：{content}");
        assert!(content.contains("name=\"修订日志\""));
        assert!(content.contains("name=\"目录\""));
        assert!(content.contains("name=\"表\""));
    }

    #[test]
    fn xlsx_每表一个sheet() {
        let mut o = opts("excel");
        o.excel_sheet_mode = "per_table".into();
        let bytes = xlsx::build_xlsx(&sample(), &o).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let content = zip_read(&mut archive, "xl/workbook.xml");
        // 修订日志 + 目录 + 2 张表
        assert_eq!(content.matches("<sheet ").count(), 4, "{content}");
        assert!(content.contains("name=\"sys_user\""));
        assert!(content.contains("name=\"sys_role\""));
        // per-sheet 模式含返回目录超链接
        let content = zip_read(&mut archive, "xl/worksheets/sheet3.xml");
        assert!(content.contains("返回目录"), "{content}");
        assert!(content.contains("<hyperlink ref="));
        assert!(content.contains("mergeCell"), "表名横幅需要合并单元格");
    }

    #[test]
    fn xlsx_表名去重与非法字符() {
        let mut dto = sample();
        dto.tables[1].name = "sys_user".into(); // 与表 1 重名
        let mut o = opts("excel");
        o.excel_sheet_mode = "per_table".into();
        let bytes = xlsx::build_xlsx(&dto, &o).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let content = zip_read(&mut archive, "xl/workbook.xml");
        assert!(content.contains("name=\"sys_user~2\""), "重名 sheet 需加序号：{content}");
    }

    #[test]
    fn docx_合法zip结构() {
        let bytes = docx::build_docx(&sample(), &opts("word")).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        for name in [
            "word/document.xml",
            "word/styles.xml",
            "word/settings.xml",
            "word/header1.xml",
            "word/footer1.xml",
            "[Content_Types].xml",
        ] {
            assert!(archive.by_name(name).is_ok(), "缺少 {name}");
        }
        let content = zip_read(&mut archive, "word/document.xml");
        assert!(content.contains("修订日志"));
        assert!(content.contains("TOC \\o"));
        assert!(content.contains("表结构"));
        assert!(content.contains("E7E6E6"), "表头底纹");
        assert!(content.contains("用户名，含中文说明"));
        // 页脚页码域（PAGE / NUMPAGES 在 footer1.xml）
        let footer = zip_read(&mut archive, "word/footer1.xml");
        assert!(footer.contains("PAGE"), "页脚页码域");
        assert!(footer.contains("NUMPAGES"));
        // settings 打开时更新目录域
        let settings = zip_read(&mut archive, "word/settings.xml");
        assert!(settings.contains("updateFields"));
    }

    #[test]
    fn docx_oracle隐藏自增列() {
        let mut dto = sample();
        dto.tables[0].db_type = "Oracle".into();
        let bytes = docx::build_docx(&dto, &opts("word")).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let content = zip_read(&mut archive, "word/document.xml");
        let header_pos = content.find("主键").unwrap();
        let next = &content[header_pos..header_pos + 220];
        assert!(!next.contains(">自增<"), "Oracle 类型表头不应含自增：{next}");
    }

    #[test]
    fn pdf_结构合法_含内嵌字体() {
        // 中文字体可用性由生产代码自己判断（候选字体不止 SimHei/等线，
        // 复制一份字体清单必然与实现漂移）。没有可用字体时必须给出明确的
        // 字体缺失原因，而不是别的失败；有字体时才校验 PDF 内部结构。
        let bytes = match pdf_writer::build_pdf(&sample(), &opts("pdf")) {
            Ok(bytes) => bytes,
            Err(e) => {
                assert!(
                    format!("{e:#}").contains("字体"),
                    "PDF 构建失败必须说明缺少中文字体，实际错误：{e:#}"
                );
                return;
            }
        };
        assert!(bytes.starts_with(b"%PDF-1.7"));
        assert!(bytes.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Type /Catalog"));
        assert!(text.contains("/Subtype /Type0"));
        assert!(text.contains("/CIDFontType2"));
        assert!(text.contains("/FontFile2"));
        assert!(text.contains("xref"));
        // 内嵌字体流不能太大（子集生效），也不能太小（字形确实嵌入）
        assert!(bytes.len() > 20_000, "子集字体 + 内容应 > 20KB，实际 {}", bytes.len());
        assert!(bytes.len() < 8_000_000, "子集后应远小于完整字体，实际 {}", bytes.len());
    }

    #[test]
    fn export_to_file_全格式冒烟() {
        let dto = sample();
        for format in ["markdown", "html", "excel", "word", "pdf"] {
            let path = temp_path(&format!("out.{format}"));
            let result = export_to_file(&dto, &opts(format), &path);
            if format == "pdf" {
                // PDF 依赖系统中文字体：要么写出真实 PDF，要么明确报字体缺失，
                // 不得用其它失败掩盖，也不得静默产出空文档。
                match result {
                    Ok(n) => {
                        assert!(n > 0, "pdf 应写出内容");
                        let bytes = std::fs::read(&path).unwrap();
                        assert!(bytes.starts_with(b"%PDF-1.7"), "pdf 应是合法 PDF 头");
                    }
                    Err(e) => assert!(
                        format!("{e:#}").contains("字体"),
                        "pdf 导出失败必须说明缺少中文字体，实际错误：{e:#}"
                    ),
                }
                continue;
            }
            match result {
                Ok(n) => assert!(n > 0, "{format} 应写出内容"),
                Err(e) => panic!("{format} 导出失败: {e}"),
            }
        }
    }

    #[test]
    fn 列宽与列集一致() {
        let headers = structure_headers("MySQL");
        let widths = structure_widths("MySQL");
        let centered = structure_centered("MySQL");
        assert_eq!(headers.len(), widths.len());
        assert_eq!(headers.len(), centered.len());
        assert!((widths.iter().sum::<f64>() - 100.0).abs() < 0.01, "列宽合计应为 100");
        assert_eq!(headers, vec!["序号", "列名", "数据类型", "长度", "主键", "自增", "允许空", "默认值", "列说明"]);

        let o_headers = structure_headers("Oracle 19c");
        assert!(!o_headers.contains(&"自增"));
        let o_widths = structure_widths("Oracle 19c");
        assert!((o_widths.iter().sum::<f64>() - 100.0).abs() < 0.01);
    }
}
