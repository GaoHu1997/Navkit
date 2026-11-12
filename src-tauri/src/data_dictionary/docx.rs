//! Word (.docx) 数据字典生成 —— 不依赖第三方 docx 库，直接产出 OOXML。
//!
//! 文档结构照搬 SmartSQL WordDoc（Aspose.Words）：
//!  标题页 → 修订日志 → 目录（TOC 域，Word 打开时自动更新）→ 表结构（每表一节 + 9 列表格）
//!  页眉「NavKit 数据库字典」右对齐；页脚「第 X 页 / 共 Y 页」（PAGE / NUMPAGES 域）。
//! 表头底纹 #E7E6E6，正文宋体 9pt，与 SmartSQL 模板一致。

use super::{xml_escape, DictDatabase, DictExportOptions};

use std::io::Write as _;

/// 版面可用宽度（twips）：A4 11906 - 左右边距 1440*2。
const USABLE: i64 = 9026;

/// 写入一个 zip 内文件。
fn put<W: std::io::Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    name: &str,
    body: &str,
) -> anyhow::Result<()> {
    zip.start_file(
        name,
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )?;
    zip.write_all(body.as_bytes())?;
    Ok(())
}

/// 文本 run（可加粗）。
fn run(text: &str, bold: bool, size_half_pt: u32, color: Option<&str>) -> String {
    let mut rpr = String::new();
    if bold {
        rpr.push_str("<w:b/>");
    }
    rpr.push_str(&format!(
        "<w:sz w:val=\"{size_half_pt}\"/><w:szCs w:val=\"{size_half_pt}\"/>"
    ));
    if let Some(c) = color {
        rpr.push_str(&format!("<w:color w:val=\"{c}\"/>"));
    }
    format!(
        "<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
        xml_escape(text)
    )
}

/// 段落。
fn paragraph(runs: &str, jc: &str, outline: Option<u8>, spacing_before: u32) -> String {
    let mut ppr = String::new();
    if let Some(level) = outline {
        ppr.push_str(&format!("<w:outlineLvl w:val=\"{}\"/>", level - 1));
    }
    if spacing_before > 0 {
        ppr.insert_str(0, &format!("<w:spacing w:before=\"{spacing_before}\" w:after=\"120\"/>"));
    }
    if !jc.is_empty() {
        ppr.push_str(&format!("<w:jc w:val=\"{jc}\"/>"));
    }
    format!("<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>")
}

/// 标题段落（带样式引用 Heading1/2/3，供 TOC 收录）。
fn heading(text: &str, level: u8) -> String {
    format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Heading{level}\"/></w:pPr>{}</w:p>",
        run(text, true, match level { 1 => 32, 2 => 28, _ => 24 }, Some("14171C"))
    )
}

/// 表格：header 底纹 E7E6E6；widths 为百分比；rows 为单元格文本。
fn table(widths_pct: &[f64], headers: &[&str], rows: &[Vec<String>], centered: &[bool]) -> String {
    let mut xml = String::from(
        "<w:tbl><w:tblPr><w:tblW w:w=\"9026\" w:type=\"dxa\"/><w:jc w:val=\"center\"/>\
         <w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         </w:tblBorders><w:tblLayout w:type=\"fixed\"/></w:tblPr>",
    );
    // 列宽
    xml.push_str("<w:tblGrid>");
    for pct in widths_pct {
        xml.push_str(&format!(
            "<w:gridCol w:w=\"{}\"/>",
            (USABLE as f64 * pct / 100.0).round() as i64
        ));
    }
    xml.push_str("</w:tblGrid>");
    // 表头行
    xml.push_str(
        "<w:tr><w:trPr><w:trHeight w:val=\"440\" w:hRule=\"atLeast\"/><w:tblHeader/></w:trPr>",
    );
    for (j, h) in headers.iter().enumerate() {
        let w = (USABLE as f64 * widths_pct[j] / 100.0).round() as i64;
        xml.push_str(&format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/><w:shd w:val=\"clear\" w:fill=\"E7E6E6\"/><w:vAlign w:val=\"center\"/></w:tcPr>{}{}</w:tc>",
            paragraph(&run(h, true, 18, None), "center", None, 0),
            ""
        ));
    }
    xml.push_str("</w:tr>");
    // 数据行
    for row in rows {
        xml.push_str("<w:tr><w:trPr><w:trHeight w:val=\"440\" w:hRule=\"atLeast\"/></w:trPr>");
        for (j, cell) in row.iter().enumerate() {
            let w = (USABLE as f64 * widths_pct[j] / 100.0).round() as i64;
            let jc = if centered[j] { "center" } else { "left" };
            xml.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/><w:vAlign w:val=\"center\"/></w:tcPr>{}</w:tc>",
                paragraph(&run(cell, false, 18, None), jc, None, 0)
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl>");
    xml.push_str("<w:p/>"); // 表后空段
    xml
}

/// 复杂域（TOC / PAGE / NUMPAGES）。
fn field(instr: &str, placeholder: &str, dirty: bool) -> String {
    let dirty_attr = if dirty { " w:dirty=\"true\"" } else { "" };
    format!(
        "<w:r><w:fldChar w:fldCharType=\"begin\"{dirty_attr}/></w:r>\
         <w:r><w:instrText xml:space=\"preserve\">{instr}</w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>\
         <w:r><w:rPr><w:noProof/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>\
         <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
        xml_escape(placeholder)
    )
}

/// 生成 .docx 字节流。
pub fn build_docx(dto: &DictDatabase, opts: &DictExportOptions) -> anyhow::Result<Vec<u8>> {
    let tables: Vec<super::DictTable> = if opts.include_tables {
        dto.tables.clone()
    } else {
        Vec::new()
    };
    if tables.is_empty() {
        anyhow::bail!("Word 导出至少需要选择一张表");
    }

    let mut body = String::new();
    // 标题页
    body.push_str(&paragraph(
        &run(&dto.doc_title, true, 50, Some("14171C")),
        "center",
        None,
        240,
    ));
    body.push_str(&paragraph(
        &run(&format!("数据库：{}", dto.name), false, 24, Some("5C6673")),
        "center",
        None,
        0,
    ));
    body.push_str(&paragraph(
        &run(&format!("导出时间：{}", opts.created_at), false, 20, Some("7D8794")),
        "center",
        None,
        0,
    ));
    body.push_str(page_break());

    // 修订日志
    body.push_str(&heading("修订日志", 2));
    let log_rows: Vec<Vec<String>> = (0..12)
        .map(|_| vec![String::new(); 5])
        .collect();
    body.push_str(&table(
        &[16.0, 18.0, 40.0, 13.0, 13.0],
        &super::LOG_HEADERS,
        &log_rows,
        &[true, true, false, true, true],
    ));
    body.push_str(page_break());

    // 目录（TOC 域：Word 打开时提示/自动更新字段）
    body.push_str(&heading("目录", 2));
    body.push_str(&paragraph(
        &field(" TOC \\o \"1-3\" \\h \\z \\u ", "（在 Word 中打开时自动生成目录）", true),
        "left",
        None,
        0,
    ));
    body.push_str(page_break());

    // 表结构
    body.push_str(&heading("表结构", 2));
    for (i, t) in tables.iter().enumerate() {
        let title = if t.comment.trim().is_empty() {
            format!("{}. {}", i + 1, t.name)
        } else {
            format!("{}. {} {}", i + 1, t.name, t.comment)
        };
        body.push_str(&heading(&title, 3));
        let headers = super::structure_headers(&t.db_type);
        let widths = super::structure_widths(&t.db_type);
        let centered = super::structure_centered(&t.db_type);
        let rows: Vec<Vec<String>> = t
            .columns
            .iter()
            .map(|c| super::row_cells(c, &t.db_type))
            .collect();
        body.push_str(&table(&widths, &headers, &rows, &centered));
        if i + 1 < tables.len() {
            body.push_str(page_break());
        }
    }

    // 视图 / 存储过程（SmartSQL 的 Word 不含；这里按勾选追加，保持一致不强制）
    if opts.include_views && !dto.views.is_empty() {
        body.push_str(&heading("视图", 2));
        for v in &dto.views {
            let title = if v.comment.trim().is_empty() {
                v.name.clone()
            } else {
                format!("{} {}", v.name, v.comment)
            };
            body.push_str(&heading(&title, 3));
            body.push_str(&code_paragraph(&v.script));
        }
    }
    if opts.include_routines && !dto.routines.is_empty() {
        body.push_str(&heading("存储过程", 2));
        for r in &dto.routines {
            let title = if r.comment.trim().is_empty() {
                r.name.clone()
            } else {
                format!("{} {}", r.name, r.comment)
            };
            body.push_str(&heading(&title, 3));
            body.push_str(&code_paragraph(&r.script));
        }
    }

    // sectPr：页眉页脚引用 + A4 页面
    body.push_str(
        "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdH1\"/>\
         <w:footerReference w:type=\"default\" r:id=\"rIdF1\"/>\
         <w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
         <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"851\" w:footer=\"992\"/>\
         </w:sectPr>",
    );

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <w:body>{body}</w:body></w:document>"
    );

    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        put(&mut zip, "[Content_Types].xml", &content_types())?;
        put(&mut zip, "_rels/.rels", &root_rels())?;
        put(&mut zip, "docProps/core.xml", &core_props(&dto.doc_title))?;
        put(&mut zip, "docProps/app.xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\">\
             <Application>NavKit</Application></Properties>")?;
        put(&mut zip, "word/_rels/document.xml.rels", &document_rels())?;
        put(&mut zip, "word/document.xml", &document)?;
        put(&mut zip, "word/styles.xml", &styles())?;
        put(&mut zip, "word/settings.xml", &settings())?;
        put(&mut zip, "word/header1.xml", &header())?;
        put(&mut zip, "word/footer1.xml", &footer())?;
        zip.finish()?;
    }
    Ok(buf.into_inner())
}

fn page_break() -> &'static str {
    "<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"
}

/// SQL 代码段（等宽字体浅底段落）。
fn code_paragraph(script: &str) -> String {
    let mut xml = String::from(
        "<w:tbl><w:tblPr><w:tblW w:w=\"9026\" w:type=\"dxa\"/>\
         <w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:color=\"D3D8E0\"/>\
         </w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w=\"9026\"/></w:tblGrid>\
         <w:tr><w:tc><w:tcPr><w:tcW w:w=\"9026\" w:type=\"dxa\"/><w:shd w:val=\"clear\" w:fill=\"F7F8FA\"/></w:tcPr>",
    );
    for line in script.lines() {
        let mono = format!(
            "<w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:eastAsia=\"宋体\"/><w:sz w:val=\"18\"/></w:rPr>"
        );
        xml.push_str(&format!(
            "<w:p><w:r>{mono}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            xml_escape(line.trim_end())
        ));
    }
    if script.trim().is_empty() {
        xml.push_str("<w:p/>");
    }
    xml.push_str("</w:tc></w:tr></w:tbl><w:p/>");
    xml
}

fn content_types() -> String {
    String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
         <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
         <Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
         <Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>\
         <Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
         <Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
         </Types>",
    )
}

fn root_rels() -> String {
    String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
         <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
         <Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>\
         </Relationships>",
    )
}

fn document_rels() -> String {
    String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rIdS1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
         <Relationship Id=\"rIdS2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>\
         <Relationship Id=\"rIdH1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/>\
         <Relationship Id=\"rIdF1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>\
         </Relationships>",
    )
}

fn core_props(title: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
         <dc:title>{}</dc:title><dc:creator>NavKit</dc:creator><cp:lastModifiedBy>NavKit</cp:lastModifiedBy>\
         </cp:coreProperties>",
        xml_escape(title)
    )
}

fn settings() -> String {
    String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:updateFields w:val=\"true\"/>\
         </w:settings>",
    )
}

fn styles() -> String {
    String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:docDefaults><w:rPrDefault><w:rPr>\
         <w:rFonts w:ascii=\"宋体\" w:hAnsi=\"宋体\" w:eastAsia=\"宋体\" w:cs=\"宋体\"/>\
         <w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>\
         </w:rPr></w:rPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Heading1\">\
         <w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"0\"/></w:pPr>\
         <w:rPr><w:b/><w:sz w:val=\"32\"/><w:color w:val=\"14171C\"/></w:rPr></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Heading2\">\
         <w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"1\"/></w:pPr>\
         <w:rPr><w:b/><w:sz w:val=\"28\"/><w:color w:val=\"14171C\"/></w:rPr></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Heading3\">\
         <w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"2\"/></w:pPr>\
         <w:rPr><w:b/><w:sz w:val=\"24\"/><w:color w:val=\"14171C\"/></w:rPr></w:style>\
         </w:styles>",
    )
}

fn header() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:hdr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{}</w:hdr>",
        paragraph(&run("NavKit 数据库字典", false, 18, Some("7D8794")), "right", None, 0)
    )
}

fn footer() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:ftr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{}</w:ftr>",
        paragraph(
            &format!("{}{}{}{}{}",
                field(" PAGE ", "1", false),
                run(" / ", false, 18, None),
                field(" NUMPAGES ", "1", false),
                run("", false, 18, None),
                ""
            ),
            "center",
            None,
            0
        )
    )
}
