//! PDF 数据字典生成 —— 极简 PDF 写入器（不依赖第三方 PDF 库）。
//!
//! 中文支持：读取系统字体（SimHei / 等线 / 楷体 …），按用到的字符做 glyf 子集，
//! 以 CIDFontType2 + Identity-H 内嵌（FontFile2）。ToUnicode 略（搜索文本非关键诉求）。
//! 章节结构照搬 参考模板 PdfDoc（iTextSharp）：标题 → 修订日志 → 目录 → 表结构 → 视图/存储过程。

use super::{DictDatabase, DictExportOptions};

// ---------------------------------------------------------------- 页面常量

const PAGE_W: f64 = 595.28; // A4 宽
const PAGE_H: f64 = 841.89; // A4 高
const MARGIN: f64 = 42.0;
const USABLE: f64 = PAGE_W - MARGIN * 2.0;
const FONT_SIZE: f64 = 9.0;
const HEAD_SIZE: f64 = 12.0;
const TITLE_SIZE: f64 = 26.0;
const LINE_H: f64 = 12.5;
const CELL_PAD: f64 = 3.0;

const COLOR_TEXT: (f64, f64, f64) = (0.137, 0.153, 0.180); // #23272F
const COLOR_MUTED: (f64, f64, f64) = (0.36, 0.40, 0.45); // #5C6673
const COLOR_HEAD_BG: (f64, f64, f64) = (0.906, 0.902, 0.902); // #E7E6E6
const COLOR_BORDER: (f64, f64, f64) = (0.827, 0.847, 0.878); // #D3D8E0
const COLOR_ACCENT: (f64, f64, f64) = (0.208, 0.455, 0.941); // #3574F0

// ---------------------------------------------------------------- 字体（TTF/TTC 解析 + 子集）

/// 从系统字体构建嵌入字体；找不到中文字体时返回错误。
struct EmbeddedFont {
    /// 字体文件字节（子集后）。
    data: Vec<u8>,
    /// char → glyph id（缺字 = 0）。
    map: std::collections::HashMap<char, u16>,
    units_per_em: u16,
    ascender: i16,
    descender: i16,
    bbox: [i16; 4],
    /// glyph id → 字体单位宽度（仅保留用到的 glyph，用于 /W 数组）。
    widths: std::collections::HashMap<u16, u32>,
    name: String,
}

fn font_candidates() -> Vec<&'static str> {
    vec![
        "simhei.ttf",
        "Deng.ttf",
        "Dengb.ttf",
        "simkai.ttf",
        "simfang.ttf",
        "STXIHEI.TTF",
        "STKAITI.TTF",
        "msyh.ttc",
        "msyh.ttf",
        "simsun.ttc",
    ]
}

/// 解析字体并做子集（只保留 `chars` 用到的字形，保留原 glyph 编号）。
fn load_font_subset(chars: &std::collections::BTreeSet<char>) -> anyhow::Result<EmbeddedFont> {
    let fonts_dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut last_err = String::new();
    for name in font_candidates() {
        let path = fonts_dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        match subset_font(&bytes, chars) {
            Ok(mut f) => {
                f.name = name.trim_end_matches(".ttf").trim_end_matches(".ttc").to_string();
                return Ok(f);
            }
            Err(e) => last_err = format!("{name}: {e}"),
        }
    }
    anyhow::bail!("未找到可用的系统中文字体（{last_err}）。PDF 导出需要 C:/Windows/Fonts 下的 SimHei / 等线等字体")
}

/// 从 TTF/TTC 字节做子集。
fn subset_font(bytes: &[u8], chars: &std::collections::BTreeSet<char>) -> anyhow::Result<EmbeddedFont> {
    // TTC 容器：取第一个 face 的偏移
    let mut base = 0usize;
    if bytes.len() >= 12 && &bytes[0..4] == b"ttcf" {
        let num = read_u32(bytes, 8)? as usize;
        if num == 0 {
            anyhow::bail!("TTC 无字体面");
        }
        base = read_u32(bytes, 12)? as usize;
    }
    let num_tables = read_u16(bytes, base + 4)? as usize;
    // 目录项：tag(4) checksum(4) offset(4) length(4)
    let dir = base + 12;
    let mut tables: std::collections::HashMap<String, (usize, usize)> = Default::default();
    for i in 0..num_tables {
        let entry = dir + i * 16;
        if entry + 16 > bytes.len() {
            anyhow::bail!("表目录越界");
        }
        let tag = String::from_utf8_lossy(&bytes[entry..entry + 4]).to_string();
        let offset = read_u32(bytes, entry + 8)? as usize;
        let length = read_u32(bytes, entry + 12)? as usize;
        if offset + length > bytes.len() {
            anyhow::bail!("表 {tag} 数据越界");
        }
        tables.insert(tag, (offset, length));
    }

    let required = ["head", "hhea", "maxp", "hmtx", "cmap", "loca", "glyf"];
    for tag in required {
        if !tables.contains_key(tag) {
            anyhow::bail!("缺少必需表 {tag}");
        }
    }

    let (head_off, head_len) = tables["head"];
    let units_per_em = read_u16(bytes, head_off + 18)?;
    let index_to_loc = read_i16(bytes, head_off + 50)?;
    let bbox = [
        read_i16(bytes, head_off + 36)?,
        read_i16(bytes, head_off + 38)?,
        read_i16(bytes, head_off + 40)?,
        read_i16(bytes, head_off + 42)?,
    ];
    let (hhea_off, _hhea_len) = tables["hhea"];
    let ascender = read_i16(bytes, hhea_off + 4)?;
    let descender = read_i16(bytes, hhea_off + 6)?;
    let num_hmetrics = read_u16(bytes, hhea_off + 34)? as usize;
    let (maxp_off, _maxp_len) = tables["maxp"];
    let num_glyphs = read_u16(bytes, maxp_off + 4)? as usize;

    // cmap：选最优子表（优先 (3,10) 格式 12 → (3,1) 格式 4 → (0,x)）
    let (cmap_off, _cmap_len) = tables["cmap"];
    let sub_count = read_u16(bytes, cmap_off + 2)? as usize;
    let mut best: Option<(usize, u16, u16)> = None; // (offset, platform, encoding)
    let mut best_rank = -1i32;
    for i in 0..sub_count {
        let rec = cmap_off + 4 + i * 8;
        let platform = read_u16(bytes, rec)?;
        let encoding = read_u16(bytes, rec + 2)?;
        let offset = read_u32(bytes, rec + 4)? as usize;
        let rank = match (platform, encoding) {
            (3, 10) => 4,
            (3, 1) => 3,
            (0, _) => 2,
            (3, 0) => 1, // 符号映射兜底
            _ => 0,
        };
        if rank > best_rank {
            best_rank = rank;
            best = Some((cmap_off + offset, platform, encoding));
        }
    }
    let Some((sub_off, _, _)) = best else {
        anyhow::bail!("cmap 无可用子表");
    };
    let format = read_u16(bytes, sub_off)?;
    let mut map: std::collections::HashMap<char, u16> = Default::default();
    match format {
        4 => {
            // 偏移 6 处是 segCountX2（段数 × 2），必须除以 2 才是真实段数；
            // 否则整张段表按双倍长度解析，char→glyph 映射全部错位，导出中文即乱码。
            let seg_count = read_u16(bytes, sub_off + 6)? as usize / 2;
            let end_base = sub_off + 14;
            let start_base = end_base + (seg_count * 2) + 2;
            let delta_base = start_base + (seg_count * 2);
            let range_base = delta_base + (seg_count * 2);
            for seg in 0..seg_count {
                let end_code = read_u16(bytes, end_base + seg * 2)?;
                let start_code = read_u16(bytes, start_base + seg * 2)?;
                let delta = read_i16(bytes, delta_base + seg * 2)?;
                let range_off = read_u16(bytes, range_base + seg * 2)? as usize;
                if start_code == 0xFFFF {
                    continue;
                }
                for code in start_code..=end_code {
                    let gid = if range_off == 0 {
                        ((code as i32 + delta as i32) & 0xFFFF) as u16
                    } else {
                        // glyphIdArray 索引位置：rangeOffset/2 + (code - start) 相对 range_base 项
                        let idx_pos = range_base + seg * 2 + range_off + (code - start_code) as usize * 2;
                        if idx_pos + 2 > bytes.len() {
                            continue;
                        }
                        let g = read_u16(bytes, idx_pos)?;
                        if g == 0 {
                            0
                        } else {
                            ((g as i32 + delta as i32) & 0xFFFF) as u16
                        }
                    };
                    if gid != 0 {
                        if let Some(ch) = char::from_u32(code as u32) {
                            map.entry(ch).or_insert(gid);
                        }
                    }
                }
            }
        }
        12 => {
            let n_groups = read_u32(bytes, sub_off + 12)? as usize;
            for g in 0..n_groups {
                let rec = sub_off + 16 + g * 12;
                let start = read_u32(bytes, rec)?;
                let end = read_u32(bytes, rec + 4)?;
                let start_gid = read_u32(bytes, rec + 8)?;
                for code in start..=end.min(start + 0xFFFF) {
                    if let Some(ch) = char::from_u32(code) {
                        map.entry(ch).or_insert((start_gid + (code - start)) as u16);
                    }
                }
            }
        }
        other => anyhow::bail!("不支持的 cmap 格式 {other}"),
    }

    // 用到的 glyph（含复合字形引用的成员 glyph）
    let mut used: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
    for ch in chars {
        if let Some(&gid) = map.get(ch) {
            if gid != 0 {
                used.insert(gid);
            }
        }
    }
    if used.is_empty() {
        anyhow::bail!("字体不含任何所需字形");
    }
    // 复合字形递归收集
    let (loca_off, _loca_len) = tables["loca"];
    let (glyf_off, glyf_len) = tables["glyf"];
    let loca_entry = |i: usize| -> anyhow::Result<usize> {
        Ok(if index_to_loc == 1 {
            read_u32(bytes, loca_off + i * 4)? as usize
        } else {
            read_u16(bytes, loca_off + i * 2)? as usize * 2
        })
    };
    let mut queue: Vec<u16> = used.iter().copied().collect();
    while let Some(gid) = queue.pop() {
        if gid as usize >= num_glyphs {
            continue;
        }
        let start = loca_entry(gid as usize)?;
        let end = loca_entry(gid as usize + 1)?;
        if end <= start || start >= glyf_len {
            continue;
        }
        let g0 = glyf_off + start;
        let contours = read_i16(bytes, g0)?;
        if contours < 0 {
            // 复合字形：遍历组件
            let mut pos = g0 + 10;
            loop {
                if pos + 4 > glyf_off + glyf_len {
                    break;
                }
                let flags = read_u16(bytes, pos)?;
                let comp = read_u16(bytes, pos + 2)?;
                used.insert(comp);
                queue.push(comp);
                pos += 4;
                // 参数
                pos += if flags & 0x0002 != 0 { 4 } else { 2 }; // ARG_1_AND_2_ARE_WORDS
                if flags & 0x0008 != 0 {
                    pos += 2; // WE_HAVE_A_SCALE
                } else if flags & 0x0040 != 0 {
                    pos += 4; // X_AND_Y_SCALE
                } else if flags & 0x0080 != 0 {
                    pos += 8; // TWO_BY_TWO
                }
                if flags & 0x0020 == 0 {
                    break; // MORE_COMPONENTS
                }
            }
        }
    }

    // hmtx 宽度表（仅收集用到的 glyph，控制 /W 数组体积）
    let (hmtx_off, _hmtx_len) = tables["hmtx"];
    let mut widths: std::collections::HashMap<u16, u32> = Default::default();
    for &gid in &used {
        if gid as usize >= num_glyphs {
            continue;
        }
        let advance = if (gid as usize) < num_hmetrics {
            read_u16(bytes, hmtx_off + gid as usize * 4)?
        } else {
            read_u16(bytes, hmtx_off + (num_hmetrics - 1) * 4)?
        };
        widths.insert(gid, advance as u32);
    }

    // ---- 重建 glyf/loca（保留原 glyph 编号，丢弃未用字形的轮廓数据）----
    let mut new_glyf: Vec<u8> = Vec::with_capacity(glyf_len);
    let mut offsets: Vec<usize> = Vec::with_capacity(num_glyphs + 1);
    for i in 0..num_glyphs {
        offsets.push(new_glyf.len());
        let gid = i as u16;
        if used.contains(&gid) {
            let start = loca_entry(i)?;
            let end = loca_entry(i + 1)?;
            if end > start && start < glyf_len {
                let end = end.min(glyf_len);
                new_glyf.extend_from_slice(&bytes[glyf_off + start..glyf_off + end]);
                // glyf 数据需 4 字节对齐（loca 约定）
                while new_glyf.len() % 4 != 0 {
                    new_glyf.push(0);
                }
            }
        }
    }
    let glyf_total_len = new_glyf.len();
    offsets.push(glyf_total_len);
    // loca（短格式最大 65535*2，超长自动转长格式）
    let long_loca = glyf_total_len >= 0x20000 || index_to_loc == 1;
    let mut new_loca: Vec<u8> = Vec::with_capacity((num_glyphs + 1) * if long_loca { 4 } else { 2 });
    for off in &offsets {
        if long_loca {
            new_loca.extend_from_slice(&(*off as u32).to_be_bytes());
        } else {
            new_loca.extend_from_slice(&((*off / 2) as u16).to_be_bytes());
        }
    }

    // ---- 组装 sfnt ----
    let keep = ["cmap", "head", "hhea", "hmtx", "maxp", "name", "OS/2", "post"];
    let mut out_tables: Vec<(String, Vec<u8>)> = Vec::new();
    for tag in &keep {
        if let Some(&(off, len)) = tables.get(*tag) {
            out_tables.push((tag.to_string(), bytes[off..off + len].to_vec()));
        }
    }
    out_tables.push(("loca".to_string(), new_loca));
    out_tables.push(("glyf".to_string(), new_glyf));
    // 按标签字典序排列
    out_tables.sort_by(|a, b| a.0.cmp(&b.0));

    let n = out_tables.len();
    let entry_selector = (32 - (n as u32).leading_zeros() - 1) as u16;
    let search_range = 16u32 << entry_selector;
    let mut sfnt: Vec<u8> = Vec::with_capacity(glyf_total_len + 4096);
    sfnt.extend_from_slice(&0x00010000u32.to_be_bytes());
    sfnt.extend_from_slice(&(n as u16).to_be_bytes());
    sfnt.extend_from_slice(&(search_range as u16).to_be_bytes());
    sfnt.extend_from_slice(&entry_selector.to_be_bytes());
    sfnt.extend_from_slice(&((n as u32 * 16 - search_range) as u16).to_be_bytes());
    // 目录占位（记录 head 表在输出中的位置以便回填校验和调整）
    let dir_start = sfnt.len();
    for _ in 0..n {
        sfnt.extend_from_slice(&[0u8; 16]);
    }
    let mut entries: Vec<(String, u32, u32, u32)> = Vec::new(); // tag, checksum, offset, length
    for (tag, data) in &out_tables {
        let offset = sfnt.len() as u32;
        sfnt.extend_from_slice(data);
        // 4 字节对齐
        while sfnt.len() % 4 != 0 {
            sfnt.push(0);
        }
        entries.push((tag.clone(), table_checksum(data), offset, data.len() as u32));
    }
    // 写回目录
    for (i, (tag, checksum, offset, length)) in entries.iter().enumerate() {
        let e = dir_start + i * 16;
        sfnt[e..e + 4].copy_from_slice(tag.as_bytes());
        sfnt[e + 4..e + 8].copy_from_slice(&checksum.to_be_bytes());
        sfnt[e + 8..e + 12].copy_from_slice(&offset.to_be_bytes());
        sfnt[e + 12..e + 16].copy_from_slice(&length.to_be_bytes());
    }
    // head.checkSumAdjustment：head 表在输出中的位置需要重算
    let head_index = entries.iter().position(|(t, ..)| t == "head").unwrap();
    let head_out_off = entries[head_index].2 as usize;
    // 先清零 adjustment，再算整体校验和
    let adj_pos = head_out_off + 8;
    sfnt[adj_pos..adj_pos + 4].copy_from_slice(&[0u8; 4]);
    let total = table_checksum(&sfnt);
    let adjustment = 0xB1B0AFBAu32.wrapping_sub(total);
    sfnt[adj_pos..adj_pos + 4].copy_from_slice(&adjustment.to_be_bytes());

    Ok(EmbeddedFont {
        data: sfnt,
        map,
        units_per_em,
        ascender,
        descender,
        bbox,
        widths,
        name: String::new(),
    })
}

fn table_checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

fn read_u16(bytes: &[u8], pos: usize) -> anyhow::Result<u16> {
    if pos + 2 > bytes.len() {
        anyhow::bail!("u16 越界 @{pos}");
    }
    Ok(u16::from_be_bytes([bytes[pos], bytes[pos + 1]]))
}

fn read_i16(bytes: &[u8], pos: usize) -> anyhow::Result<i16> {
    Ok(read_u16(bytes, pos)? as i16)
}

fn read_u32(bytes: &[u8], pos: usize) -> anyhow::Result<u32> {
    if pos + 4 > bytes.len() {
        anyhow::bail!("u32 越界 @{pos}");
    }
    Ok(u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]))
}

impl EmbeddedFont {
    fn char_gid(&self, ch: char) -> u16 {
        self.map.get(&ch).copied().unwrap_or(0)
    }
    /// 字符在当前字号下的宽度（pt）。
    fn char_width(&self, ch: char, size: f64) -> f64 {
        let gid = self.char_gid(ch);
        let units = self.widths.get(&gid).copied().unwrap_or(0);
        units as f64 * size / self.units_per_em as f64
    }
    /// 文本宽度（按逐字符求和）。
    fn text_width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.char_width(c, size)).sum()
    }
    /// 按宽度折行（CJK 逐字断行，ASCII 尽量按空格断）。
    fn wrap(&self, text: &str, max_width: f64, size: f64) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        for raw_line in text.split('\n') {
            let mut current = String::new();
            let mut current_w = 0.0;
            let mut last_space: Option<usize> = None;
            let mut last_space_w = 0.0;
            for ch in raw_line.chars() {
                let w = self.char_width(ch, size);
                if current_w + w > max_width && !current.is_empty() {
                    // 回退到最近的空格（若离行尾不远）
                    if ch == ' ' {
                        lines.push(current.trim_end().to_string());
                        current.clear();
                        current_w = 0.0;
                        last_space = None;
                        continue;
                    }
                    if let Some(pos) = last_space {
                        let rest: String = current[pos + 1..].to_string();
                        lines.push(current[..pos].to_string());
                        current = rest;
                        current_w = current_w - last_space_w;
                    } else {
                        lines.push(current.clone());
                        current.clear();
                        current_w = 0.0;
                    }
                    last_space = None;
                }
                if ch == ' ' {
                    last_space = Some(current.chars().count());
                    last_space_w = current_w + w;
                }
                current.push(ch);
                current_w += w;
            }
            lines.push(current);
        }
        lines
    }
}

// ---------------------------------------------------------------- PDF 组装

/// 内容流中的绘图指令收集器（PDF 坐标系 y 向上）。
struct Page {
    ops: String,
}

impl Page {
    fn new() -> Self {
        Page { ops: String::new() }
    }
    fn fill_rect(&mut self, x: f64, y_top: f64, w: f64, h: f64, color: (f64, f64, f64)) {
        let y = PAGE_H - y_top - h;
        self.ops.push_str(&format!(
            "{:.3} {:.3} {:.3} rg {:.2} {:.2} {:.2} {:.2} re f\n",
            color.0, color.1, color.2, x, y, w, h
        ));
    }
    fn line(&mut self, x1: f64, y1_top: f64, x2: f64, y2_top: f64, color: (f64, f64, f64)) {
        self.ops.push_str(&format!(
            "{:.3} {:.3} {:.3} RG 0.6 w {:.2} {:.2} m {:.2} {:.2} l S\n",
            color.0,
            color.1,
            color.2,
            x1,
            PAGE_H - y1_top,
            x2,
            PAGE_H - y2_top
        ));
    }
    fn text(&mut self, font: &EmbeddedFont, x: f64, y_top: f64, size: f64, color: (f64, f64, f64), content: &str) {
        let mut hex = String::with_capacity(content.len() * 4);
        for ch in content.chars() {
            hex.push_str(&format!("{:04X}", font.char_gid(ch)));
        }
        // 基线：y_top 为文本行顶部，基线下移约 0.8em
        let y = PAGE_H - y_top - size * 0.82;
        self.ops.push_str(&format!(
            "BT /F1 {size:.1} Tf {:.3} {:.3} {:.3} rg {x:.2} {y:.2} Td <{hex}> Tj ET\n",
            color.0, color.1, color.2
        ));
    }
}

/// 文档生成上下文。
struct Doc {
    pages: Vec<Page>,
    font: EmbeddedFont,
}

impl Doc {
    /// 新开一页（返回新页在列表中的索引）。
    fn new_page(&mut self) -> usize {
        self.pages.push(Page::new());
        self.pages.len() - 1
    }

    /// 章节标题。
    fn heading(&mut self, y: &mut f64, text: &str) {
        // 每个章节另起一页，并把纵向游标重置到页顶；否则沿用上一页残留的 y，
        // 会让标题和表格被挤到页面下半部、上半页整片空白。
        *y = MARGIN;
        let Self { pages, font } = self;
        pages.push(Page::new());
        let page = pages.last_mut().unwrap();
        page.text(font, MARGIN, *y + HEAD_SIZE, HEAD_SIZE, COLOR_ACCENT, text);
        page.line(MARGIN, *y + HEAD_SIZE + 3.0, MARGIN + USABLE, *y + HEAD_SIZE + 3.0, COLOR_ACCENT);
        *y += HEAD_SIZE + 16.0;
    }

    /// 通用表格渲染（自动分页，表头重复，列边界竖线按页段绘制）。
    fn table(&mut self, y: &mut f64, widths: &[f64], headers: &[&str], rows: &[Vec<String>], centered: &[bool]) {
        let col_widths: Vec<f64> = widths.iter().map(|p| USABLE * p / 100.0).collect();
        let draw_header = |doc: &mut Doc, y: &mut f64| {
            let header_h = LINE_H + CELL_PAD * 2.0;
            if *y + header_h > PAGE_H - MARGIN {
                *y = MARGIN;
                doc.pages.push(Page::new());
            }
            let page = doc.pages.last_mut().unwrap();
            page.fill_rect(MARGIN, *y, USABLE, header_h, COLOR_HEAD_BG);
            let mut x = MARGIN;
            for (j, h) in headers.iter().enumerate() {
                page.text(&doc.font, x + CELL_PAD, *y + CELL_PAD, FONT_SIZE, COLOR_TEXT, h);
                x += col_widths[j];
            }
            *y += header_h;
            page.line(MARGIN, *y, MARGIN + USABLE, *y, COLOR_BORDER);
        };

        draw_header(self, y);
        let mut segment_top = *y - (LINE_H + CELL_PAD * 2.0); // 本页段顶部（含表头）
        for row in rows {
            // 折行
            let mut wrapped: Vec<Vec<String>> = Vec::new();
            let mut max_lines = 1usize;
            for (j, cell) in row.iter().enumerate() {
                let w = col_widths[j] - CELL_PAD * 2.0;
                let lines = self.font.wrap(cell, w, FONT_SIZE);
                max_lines = max_lines.max(lines.len());
                wrapped.push(lines);
            }
            let row_h = max_lines as f64 * LINE_H + CELL_PAD * 2.0;
            if *y + row_h > PAGE_H - MARGIN {
                // 画本页段竖线后翻页
                let page = self.pages.last_mut().unwrap();
                let mut x = MARGIN;
                for w in col_widths.iter().take(col_widths.len() - 1) {
                    x += w;
                    page.line(x, segment_top, x, *y, COLOR_BORDER);
                }
                page.line(MARGIN, segment_top, MARGIN, *y, COLOR_BORDER);
                page.line(MARGIN + USABLE, segment_top, MARGIN + USABLE, *y, COLOR_BORDER);
                // 分页：新页 + 重复表头
                *y = MARGIN;
                self.pages.push(Page::new());
                draw_header(self, y);
                segment_top = *y - (LINE_H + CELL_PAD * 2.0);
            }
            let page = self.pages.last_mut().unwrap();
            let mut x = MARGIN;
            for (j, lines) in wrapped.iter().enumerate() {
                let mut ly = *y + CELL_PAD;
                for line in lines {
                    let tx = if centered[j] {
                        let w = self.font.text_width(line, FONT_SIZE);
                        let cw = col_widths[j];
                        x + ((cw - w) / 2.0).max(CELL_PAD)
                    } else {
                        x + CELL_PAD
                    };
                    page.text(&self.font, tx, ly, FONT_SIZE, COLOR_TEXT, line);
                    ly += LINE_H;
                }
                x += col_widths[j];
            }
            *y += row_h;
            let page = self.pages.last_mut().unwrap();
            page.line(MARGIN, *y, MARGIN + USABLE, *y, COLOR_BORDER);
        }
        // 收尾竖线
        let page = self.pages.last_mut().unwrap();
        let mut x = MARGIN;
        for w in col_widths.iter().take(col_widths.len() - 1) {
            x += w;
            page.line(x, segment_top, x, *y, COLOR_BORDER);
        }
        page.line(MARGIN, segment_top, MARGIN, *y, COLOR_BORDER);
        page.line(MARGIN + USABLE, segment_top, MARGIN + USABLE, *y, COLOR_BORDER);
    }
}

/// 生成 PDF 字节流。
pub fn build_pdf(dto: &DictDatabase, opts: &DictExportOptions) -> anyhow::Result<Vec<u8>> {
    // 1) 收集全部用到的字符
    let mut chars: std::collections::BTreeSet<char> = std::collections::BTreeSet::new();
    collect_chars(&dto.doc_title, &mut chars);
    collect_chars(&dto.name, &mut chars);
    collect_chars(&opts.created_at, &mut chars);
    for h in super::LOG_HEADERS {
        collect_chars(h, &mut chars);
    }
    for t in &dto.tables {
        if opts.include_tables {
            collect_chars(&t.name, &mut chars);
            collect_chars(&t.comment, &mut chars);
            for h in super::structure_headers(&t.db_type) {
                collect_chars(h, &mut chars);
            }
            for c in &t.columns {
                for s in super::row_cells(c, &t.db_type) {
                    collect_chars(&s, &mut chars);
                }
            }
        }
    }
    for v in &dto.views {
        if opts.include_views {
            for s in [&v.name, &v.comment, &v.script] {
                collect_chars(s, &mut chars);
            }
        }
    }
    for r in &dto.routines {
        if opts.include_routines {
            for s in [&r.name, &r.comment, &r.script] {
                collect_chars(s, &mut chars);
            }
        }
    }
    collect_chars("修订日志目录表结构视图存储过程第页共返回说明版本号修订日期内容修订人审核人序号列名数据类型长度主键自增允许空默认值是", &mut chars);
    collect_chars(&format!("{} 数据库设计文档 NavKit 导出时间：数据库名", dto.name), &mut chars);

    // 2) 装载字体子集
    let font = load_font_subset(&chars)?;
    let mut doc = Doc { pages: Vec::new(), font };
    doc.new_page();

    // 3) 版面
    let mut y = MARGIN + 60.0;
    {
        let page = doc.pages.last_mut().unwrap();
        let title = &dto.doc_title;
        let tw = doc.font.text_width(title, TITLE_SIZE);
        page.text(&doc.font, MARGIN + (USABLE - tw) / 2.0, y, TITLE_SIZE, COLOR_TEXT, title);
        y += TITLE_SIZE + 14.0;
        let sub = format!("数据库：{} · 导出时间：{}", dto.name, opts.created_at);
        let sw = doc.font.text_width(&sub, FONT_SIZE);
        page.text(&doc.font, MARGIN + (USABLE - sw) / 2.0, y, FONT_SIZE, COLOR_MUTED, &sub);
        y += 30.0;
    }

    // 修订日志
    doc.heading(&mut y, "修订日志");
    let log_rows: Vec<Vec<String>> = (0..10).map(|_| vec![String::new(); 5]).collect();
    doc.table(&mut y, &[16.0, 18.0, 40.0, 13.0, 13.0], &super::LOG_HEADERS, &log_rows, &[true, true, false, true, true]);
    y += 16.0;

    // 目录
    if opts.include_tables && !dto.tables.is_empty() {
        doc.heading(&mut y, "目录");
        let rows: Vec<Vec<String>> = dto
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| vec![(i + 1).to_string(), t.name.clone(), t.comment.clone()])
            .collect();
        doc.table(&mut y, &[10.0, 45.0, 45.0], &["序号", "表名", "说明"], &rows, &[true, false, false]);
        y += 16.0;
    }

    // 表结构
    if opts.include_tables && !dto.tables.is_empty() {
        doc.heading(&mut y, "表结构");
        for t in &dto.tables {
            let title = if t.comment.trim().is_empty() {
                t.name.clone()
            } else {
                format!("{}  {}", t.name, t.comment)
            };
            doc.heading(&mut y, &title);
            let headers = super::structure_headers(&t.db_type);
            let widths = super::structure_widths(&t.db_type);
            let centered = super::structure_centered(&t.db_type);
            let rows: Vec<Vec<String>> =
                t.columns.iter().map(|c| super::row_cells(c, &t.db_type)).collect();
            doc.table(&mut y, &widths, &headers, &rows, &centered);
            y += 18.0;
        }
    }

    // 视图 / 存储过程（参考模板 PDF 支持；按勾选输出）
    if opts.include_views && !dto.views.is_empty() {
        doc.heading(&mut y, "视图");
        for v in &dto.views {
            doc.heading(&mut y, &v.name);
            if !v.comment.is_empty() {
                let page = doc.pages.last_mut().unwrap();
                page.text(&doc.font, MARGIN, y, FONT_SIZE, COLOR_MUTED, &format!("说明：{}", v.comment));
                y += LINE_H + 4.0;
            }
            render_code(&mut doc, &mut y, &v.script);
        }
    }
    if opts.include_routines && !dto.routines.is_empty() {
        doc.heading(&mut y, "存储过程");
        for r in &dto.routines {
            doc.heading(&mut y, &r.name);
            if !r.comment.is_empty() {
                let page = doc.pages.last_mut().unwrap();
                page.text(&doc.font, MARGIN, y, FONT_SIZE, COLOR_MUTED, &format!("说明：{}", r.comment));
                y += LINE_H + 4.0;
            }
            render_code(&mut doc, &mut y, &r.script);
        }
    }

    // 4) 组装 PDF 对象
    assemble(&doc, &dto.doc_title)
}

/// 代码段渲染（等宽近似：直接用同一字体浅灰底块）。
fn render_code(doc: &mut Doc, y: &mut f64, script: &str) {
    for line in script.lines().take(400) {
        if *y + LINE_H > PAGE_H - MARGIN {
            *y = MARGIN;
            doc.new_page();
        }
        let page = doc.pages.last_mut().unwrap();
        page.fill_rect(MARGIN, *y, USABLE, LINE_H, (0.969, 0.973, 0.980));
        page.text(&doc.font, MARGIN + 6.0, *y + 1.5, FONT_SIZE, COLOR_TEXT, line.trim_end());
        *y += LINE_H;
    }
    *y += 10.0;
}

fn collect_chars(text: &str, out: &mut std::collections::BTreeSet<char>) {
    out.extend(text.chars());
}

/// 组装最终 PDF（对象 + xref）。
fn assemble(doc: &Doc, title: &str) -> anyhow::Result<Vec<u8>> {
    let n_pages = doc.pages.len();
    if n_pages == 0 {
        anyhow::bail!("PDF 无内容页");
    }
    let mut out: Vec<u8> = Vec::with_capacity(doc.font.data.len() + n_pages * 4096 + 4096);
    let mut offsets: Vec<usize> = Vec::new();
    let mut push = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\n");
    };
    // 对象编号：1 Catalog, 2 Pages, 3 Type0 Font, 4 CIDFont, 5 Descriptor, 6 FontFile2,
    // 页 i（0 基）：content = 7 + 2i, page = 8 + 2i
    // PDF 头 + 二进制标记行（四个 >128 字节，提示阅读器按二进制处理）
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

    let font = &doc.font;
    let page_ids: Vec<usize> = (0..n_pages).map(|i| 8 + 2 * i).collect();
    // 1 Catalog
    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    push(&mut out, &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    // 2 Pages
    push(&mut out, &mut offsets,
        &format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {n_pages} >>\nendobj\n"));
    // 3 Type0
    push(&mut out, &mut offsets,
        "3 0 obj\n<< /Type /Font /Subtype /Type0 /BaseFont /SimHei /Encoding /Identity-H /DescendantFonts [4 0 R] >>\nendobj\n");
    // 4 CIDFontType2
    let mut w_array = String::from("[");
    let mut gids: Vec<u16> = font.widths.keys().copied().collect();
    gids.sort_unstable();
    let mut runs: Vec<(u16, u16, Vec<u32>)> = Vec::new();
    for &gid in &gids {
        let w = (font.widths[&gid] as f64 * 1000.0 / font.units_per_em as f64).round() as u32;
        if let Some(last) = runs.last_mut() {
            if last.1 + 1 == gid {
                last.1 = gid;
                last.2.push(w);
                continue;
            }
        }
        runs.push((gid, gid, vec![w]));
    }
    for (start, _end, ws) in &runs {
        let widths = ws.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(" ");
        w_array.push_str(&format!("{start} [{widths}] "));
    }
    w_array.push_str("]");
    push(&mut out, &mut offsets,
        &format!(
            "4 0 obj\n<< /Type /Font /Subtype /CIDFontType2 /BaseFont /SimHei \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor 5 0 R /W {w_array} /CIDToGIDMap /Identity >>\nendobj\n"
        ));
    // 5 FontDescriptor
    let ascent = (font.ascender as f64 * 1000.0 / font.units_per_em as f64).round() as i64;
    let descent = (font.descender as f64 * 1000.0 / font.units_per_em as f64).round() as i64;
    let bbox = font
        .bbox
        .iter()
        .map(|v| (*v as f64 * 1000.0 / font.units_per_em as f64).round() as i64)
        .collect::<Vec<_>>();
    push(&mut out, &mut offsets,
        &format!(
            "5 0 obj\n<< /Type /FontDescriptor /FontName /SimHei /Flags 4 /FontBBox [{} {} {} {}] \
             /ItalicAngle 0 /Ascent {ascent} /Descent {descent} /CapHeight {ascent} /StemV 80 /FontFile2 6 0 R >>\nendobj\n",
            bbox[0], bbox[1], bbox[2], bbox[3]
        ));
    // 6 FontFile2（二进制流）
    offsets.push(out.len());
    out.extend_from_slice(
        format!("6 0 obj\n<< /Length {} /Length1 {} >>\nstream\n", font.data.len(), font.data.len()).as_bytes(),
    );
    out.extend_from_slice(&font.data);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    // 页对象
    for (i, page) in doc.pages.iter().enumerate() {
        let content_id = 7 + 2 * i;
        let page_id = 8 + 2 * i;
        // 内容流
        let ops = &page.ops;
        offsets.push(out.len());
        out.extend_from_slice(
            format!("{content_id} 0 obj\n<< /Length {} >>\nstream\n", ops.len()).as_bytes(),
        );
        out.extend_from_slice(ops.as_bytes());
        out.extend_from_slice(b"\nendstream\nendobj\n");
        // 页
        push(&mut out, &mut offsets,
            &format!(
                "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>\nendobj\n"
            ));
    }
    // Info（标题仅 ASCII；非 ASCII 字符以 '?' 占位，元数据用途足够）
    push(&mut out, &mut offsets,
        &format!(
            "{} 0 obj\n<< /Producer (NavKit) /Creator (NavKit) /Title ({}) >>\nendobj\n",
            8 + 2 * n_pages,
            escape_pdf_ascii(title)
        ));
    let info_id = 8 + 2 * n_pages;

    // xref
    let xref_pos = out.len();
    let count = offsets.len();
    out.extend_from_slice(format!("xref\n0 {}\n", count + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n",
            count + 1,
            info_id
        )
        .as_bytes(),
    );
    Ok(out)
}

fn escape_pdf_ascii(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii() && ch != '(' && ch != ')' && ch != '\\' {
            out.push(ch);
        } else if ch == '(' || ch == ')' || ch == '\\' {
            out.push('\\');
            out.push(ch);
        } else {
            // 非 ASCII 用 '?' 占位（Info 字段仅为元数据）
            out.push('?');
        }
    }
    out
}
