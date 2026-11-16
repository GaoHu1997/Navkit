//! Excel (.xlsx) 数据字典生成 —— 不依赖第三方 xlsx 库，直接产出 SpreadsheetML。
//!
//! Sheet 结构照搬 参考模板 ExcelDoc（EPPlus）：
//!  · 单 sheet 模式：修订日志 → 目录 → 表（所有表汇总，表名横幅行分隔）
//!  · 每表一 sheet 模式：修订日志 → 目录 → 每张表一个 sheet（表头行尾「返回目录」超链接）
//! 单元格用 inlineStr，样式集中在 styles.xml（表头 E7E6E6 / 横幅 F2F2F2 / 细边框）。

use super::{xml_escape, DictDatabase, DictExportOptions};

use std::io::Write as _;

// 样式索引（styles.xml cellXfs 顺序，勿改）
const S_TITLE: u32 = 1; // 加粗标题
const S_BANNER: u32 = 2; // 表名横幅（F2F2F2 加粗居中）
const S_HEADER: u32 = 3; // 列头（E7E6E6 加粗居中 + 细边框）
const S_CELL: u32 = 4; // 内容单元格（细边框 + 自动换行）
const S_CELL_CENTER: u32 = 5; // 内容单元格（居中）

/// 表结构列宽（字符单位；9 列全定义，按类型裁剪）。
const COL_WIDTHS: [f64; 9] = [6.0, 28.0, 16.0, 10.0, 8.0, 8.0, 10.0, 18.0, 40.0];
const LOG_WIDTHS: [f64; 5] = [12.0, 18.0, 50.0, 14.0, 14.0];
const DIR_WIDTHS: [f64; 3] = [10.0, 50.0, 50.0];

/// Excel sheet 名合法化：去掉非法字符，限 31 字符；去重由调用方处理。
fn sheet_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']' | '\''))
        .collect();
    let short: String = cleaned.trim().chars().take(31).collect();
    if short.is_empty() {
        "表".to_string()
    } else {
        short
    }
}

struct Sheet {
    name: String,
    /// worksheet XML 主体（<sheetData> 之后、</worksheet> 之前的附加段）。
    after_rows: String,
    xml: String,
}

/// 行数据条目：`cells` 为 (样式, 文本) 列表。
fn row_xml(row_index: usize, cells: &[(u32, String)]) -> String {
    let mut xml = format!("<row r=\"{row_index}\">");
    for (col_index, (style, text)) in cells.iter().enumerate() {
        let col = col_letter(col_index);
        xml.push_str(&format!(
            "<c r=\"{col}{row_index}\" s=\"{style}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
            xml_escape(text)
        ));
    }
    xml.push_str("</row>");
    xml
}

/// 快捷构造单元格（文本 → String）。
fn c(style: u32, text: impl Into<String>) -> (u32, String) {
    (style, text.into())
}

/// 列号 → Excel 列字母。
fn col_letter(mut index: usize) -> String {
    let mut out = String::new();
    loop {
        out.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    out
}

/// 列宽定义 XML。
fn cols_xml(widths: &[f64]) -> String {
    let mut xml = String::from("<cols>");
    for (i, w) in widths.iter().enumerate() {
        xml.push_str(&format!(
            "<col min=\"{}\" max=\"{}\" width=\"{}\" customWidth=\"1\"/>",
            i + 1,
            i + 1,
            w
        ));
    }
    xml.push_str("</cols>");
    xml
}

/// 合并单元格区域 XML。
fn merges_xml(ranges: &[String]) -> String {
    if ranges.is_empty() {
        return String::new();
    }
    format!(
        "<mergeCells count=\"{}\">{}</mergeCells>",
        ranges.len(),
        ranges
            .iter()
            .map(|r| format!("<mergeCell ref=\"{r}\"/>"))
            .collect::<String>()
    )
}

/// 修订日志 sheet（参考模板：标题 + 5 列表头 + 15 空白待填行）。
fn log_sheet(dto: &DictDatabase, opts: &DictExportOptions) -> Sheet {
    let mut rows = String::new();
    let mut row = 1;
    rows.push_str(&row_xml(
        row,
        &[c(S_TITLE, format!("{} · 修订日志", dto.doc_title))],
    ));
    row += 1;
    rows.push_str(&row_xml(
        row,
        &[c(S_TITLE, format!("导出时间：{}", opts.created_at))],
    ));
    row += 2;
    rows.push_str(&row_xml(
        row,
        &super::LOG_HEADERS
            .iter()
            .map(|h| c(S_HEADER, h.to_string()))
            .collect::<Vec<_>>(),
    ));
    let blank: Vec<(u32, String)> = super::LOG_HEADERS.iter().map(|_| c(S_CELL, "")).collect();
    for _ in 0..15 {
        row += 1;
        rows.push_str(&row_xml(row, &blank));
    }
    Sheet {
        name: "修订日志".to_string(),
        after_rows: String::new(),
        xml: format!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{}<sheetData>{rows}</sheetData></worksheet>",
            cols_xml(&LOG_WIDTHS)
        ),
    }
}

/// 目录 sheet：序号 / 表名 / 说明。
fn directory_sheet(dto: &DictDatabase, tables: &[super::DictTable]) -> Sheet {
    let mut rows = String::new();
    let mut row = 1;
    rows.push_str(&row_xml(
        row,
        &[c(S_TITLE, format!("{} · 目录", dto.doc_title))],
    ));
    row += 2;
    rows.push_str(&row_xml(
        row,
        &[
            c(S_HEADER, "序号"),
            c(S_HEADER, "表名"),
            c(S_HEADER, "注释/说明"),
        ],
    ));
    for (i, t) in tables.iter().enumerate() {
        row += 1;
        rows.push_str(&row_xml(
            row,
            &[
                c(S_CELL_CENTER, (i + 1).to_string()),
                c(S_CELL, t.name.clone()),
                c(S_CELL, t.comment.clone()),
            ],
        ));
    }
    Sheet {
        name: "目录".to_string(),
        after_rows: String::new(),
        xml: format!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{}<sheetData>{rows}</sheetData></worksheet>",
            cols_xml(&DIR_WIDTHS)
        ),
    }
}

/// 表结构 sheet：单 sheet 汇总（表名横幅分隔）或 per_table（单表 + 返回目录链接）。
fn table_sheet(tables: &[super::DictTable], per_table: bool) -> Sheet {
    let mut rows = String::new();
    let mut merges: Vec<String> = Vec::new();
    let mut hyperlinks: Vec<(String, String)> = Vec::new(); // (cell, location)
    let mut row = 1;
    for t in tables {
        let headers = super::structure_headers(&t.db_type);
        let centered = super::structure_centered(&t.db_type);
        let name = if t.comment.trim().is_empty() {
            t.name.clone()
        } else {
            format!("{} {}", t.name, t.comment)
        };
        // 表名横幅（合并整行，样式 F2F2F2）
        rows.push_str(&row_xml(row, &[c(S_BANNER, name)]));
        let last_col = col_letter(headers.len() - 1);
        merges.push(format!("A{row}:{last_col}{row}"));
        row += 1;
        // 列头（per_table 模式行尾附「返回目录」超链接）
        let mut header_cells: Vec<(u32, String)> = headers
            .iter()
            .map(|h| c(S_HEADER, h.to_string()))
            .collect::<Vec<_>>();
        if per_table {
            header_cells.push(c(S_HEADER, "返回目录"));
            hyperlinks.push((
                format!("{}{row}", col_letter(headers.len())),
                "'目录'!A1".to_string(),
            ));
        }
        rows.push_str(&row_xml(row, &header_cells));
        row += 1;
        for col in &t.columns {
            let cells = super::row_cells(col, &t.db_type)
                .into_iter()
                .enumerate()
                .map(|(j, text)| {
                    if centered[j] {
                        c(S_CELL_CENTER, text)
                    } else {
                        c(S_CELL, text)
                    }
                })
                .collect::<Vec<_>>();
            rows.push_str(&row_xml(row, &cells));
            row += 1;
        }
        // 表间空行（仅单 sheet 汇总模式）
        if !per_table {
            row += 1;
        }
    }
    let after = format!("{}{}", merges_xml(&merges), hyperlinks_xml(&hyperlinks));
    let xml = format!(
        "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{}<sheetData>{rows}</sheetData>{after}<pageMargins left=\"0.7\" right=\"0.7\" top=\"0.75\" bottom=\"0.75\" header=\"0.3\" footer=\"0.3\"/></worksheet>",
        cols_xml(&COL_WIDTHS)
    );
    Sheet {
        name: "表".to_string(),
        after_rows: after,
        xml,
    }
}

/// 超链接 XML（内部定位）。
fn hyperlinks_xml(links: &[(String, String)]) -> String {
    if links.is_empty() {
        return String::new();
    }
    format!(
        "<hyperlinks>{}</hyperlinks>",
        links
            .iter()
            .map(|(cell, loc)| format!(
                "<hyperlink ref=\"{cell}\" location=\"{}\" display=\"返回目录\"/>",
                xml_escape(loc)
            ))
            .collect::<String>()
    )
}

/// 产出 .xlsx 字节流。
pub fn build_xlsx(dto: &DictDatabase, opts: &DictExportOptions) -> anyhow::Result<Vec<u8>> {
    let tables: Vec<super::DictTable> = if opts.include_tables {
        dto.tables.clone()
    } else {
        Vec::new()
    };
    if tables.is_empty() {
        anyhow::bail!("Excel 导出至少需要选择一张表");
    }

    let mut sheets: Vec<Sheet> = Vec::new();
    sheets.push(log_sheet(dto, opts));
    sheets.push(directory_sheet(dto, &tables));
    if opts.excel_sheet_mode == "per_table" {
        for t in &tables {
            let mut sheet = table_sheet(std::slice::from_ref(t), true);
            sheet.name = sheet_name(&t.name);
            sheets.push(sheet);
        }
    } else {
        sheets.push(table_sheet(&tables, false));
    }

    // sheet 名去重（Excel 要求唯一）
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sheet in &mut sheets {
        if used.contains(&sheet.name) {
            let base = sheet.name.clone();
            let mut n = 2;
            while used.contains(&format!("{base}~{n}")) {
                n += 1;
            }
            sheet.name = format!("{base}~{n}");
        }
        used.insert(sheet.name.clone());
    }

    // 打包
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        let sheet_count = sheets.len();
        // [Content_Types].xml
        let mut content_types = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
             <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
             <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
             <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>",
        );
        for i in 1..=sheet_count {
            content_types.push_str(&format!(
                "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
            ));
        }
        content_types.push_str("</Types>");
        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(content_types.as_bytes())?;

        // _rels/.rels
        zip.start_file("_rels/.rels", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#,
        )?;

        // docProps
        zip.start_file("docProps/core.xml", options)?;
        zip.write_all(
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
                 <dc:title>{}</dc:title><dc:creator>NavKit</dc:creator><cp:lastModifiedBy>NavKit</cp:lastModifiedBy>\
                 </cp:coreProperties>",
                xml_escape(&dto.doc_title)
            )
            .as_bytes(),
        )?;
        zip.start_file("docProps/app.xml", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>NavKit</Application></Properties>"#,
        )?;

        // xl/workbook.xml
        let mut workbook = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
        );
        for (i, sheet) in sheets.iter().enumerate() {
            workbook.push_str(&format!(
                "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                xml_escape(&sheet.name),
                i + 1,
                i + 1
            ));
        }
        workbook.push_str("</sheets></workbook>");
        zip.start_file("xl/workbook.xml", options)?;
        zip.write_all(workbook.as_bytes())?;

        // xl/_rels/workbook.xml.rels
        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for i in 1..=sheet_count {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
            ));
        }
        rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
            sheet_count + 1
        ));
        rels.push_str("</Relationships>");
        zip.start_file("xl/_rels/workbook.xml.rels", options)?;
        zip.write_all(rels.as_bytes())?;

        // xl/styles.xml
        zip.start_file("xl/styles.xml", options)?;
        zip.write_all(styles_xml().as_bytes())?;

        // worksheets
        for (i, sheet) in sheets.iter().enumerate() {
            zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), options)?;
            zip.write_all(sheet.xml.as_bytes())?;
        }

        zip.finish()?;
    }
    Ok(buf.into_inner())
}

/// styles.xml：默认字体 10pt + 标题/横幅/表头/内容样式。
fn styles_xml() -> String {
    String::new()
        // 手工拼接，保持可读
        + "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>"
        + "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">"
        + "<fonts count=\"3\">"
        + "<font><sz val=\"10\"/><name val=\"等线\"/></font>"
        + "<font><b/><sz val=\"10\"/><name val=\"等线\"/></font>"
        + "<font><b/><sz val=\"11\"/><name val=\"等线\"/><color rgb=\"FF23272F\"/></font>"
        + "</fonts>"
        + "<fills count=\"4\">"
        + "<fill><patternFill patternType=\"none\"/></fill>"
        + "<fill><patternFill patternType=\"gray125\"/></fill>"
        + "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFE7E6E6\"/><bgColor indexed=\"64\"/></patternFill></fill>"
        + "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFF2F2F2\"/><bgColor indexed=\"64\"/></patternFill></fill>"
        + "</fills>"
        + "<borders count=\"2\">"
        + "<border><left/><right/><top/><bottom/><diagonal/></border>"
        + "<border><left style=\"thin\"><color rgb=\"FFD3D8E0\"/></left><right style=\"thin\"><color rgb=\"FFD3D8E0\"/></right><top style=\"thin\"><color rgb=\"FFD3D8E0\"/></top><bottom style=\"thin\"><color rgb=\"FFD3D8E0\"/></bottom><diagonal/></border>"
        + "</borders>"
        + "<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>"
        + "<cellXfs count=\"6\">"
        // 0 默认
        + "<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>"
        // 1 标题（加粗）
        + "<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>"
        // 2 表名横幅（加粗 + F2F2F2 + 居中）
        + "<xf numFmtId=\"0\" fontId=\"2\" fillId=\"3\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\" vertical=\"center\"/></xf>"
        // 3 列头（加粗 + E7E6E6 + 边框 + 居中换行）
        + "<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\" vertical=\"center\" wrapText=\"1\"/></xf>"
        // 4 内容（边框 + 换行）
        + "<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\" applyAlignment=\"1\"><alignment vertical=\"center\" wrapText=\"1\"/></xf>"
        // 5 内容居中
        + "<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\" vertical=\"center\"/></xf>"
        + "</cellXfs>"
        + "<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>"
        + "</styleSheet>"
}
