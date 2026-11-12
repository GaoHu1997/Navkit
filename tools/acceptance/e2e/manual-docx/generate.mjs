/**
 * NavKit 验收报告/用户手册 docx 生成器。
 * 输入：docs/active/全功能桌面验收报告与用户手册-20261112.md（markdown-lite）
 * 输出：docs/active/全功能桌面验收报告与用户手册-20261112.docx
 * 结构：R1 封面（DS-1 深海）→ 目录（罗马页码）→ 正文（阿拉伯页码，含全部截图）
 */
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const {
  Document, Packer, Paragraph, TextRun, ImageRun, Table, TableRow, TableCell,
  HeadingLevel, AlignmentType, WidthType, BorderStyle, ShadingType, TableLayoutType,
  PageBreak, TableOfContents, Header, Footer, PageNumber, NumberFormat, SectionType,
} = require('docx');
const { imageSize } = require('image-size');

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname.replace(/^\/(\w):/, '$1:')), '../../../..');
const MD = path.join(ROOT, 'docs', 'active', '全功能桌面验收报告与用户手册-20261112.md');
const OUT = path.join(ROOT, 'docs', 'active', '全功能桌面验收报告与用户手册-20261112.docx');
const EV = path.join(ROOT, 'docs', 'active');

// ---------- 调色板（DS-1 Deep Sea） ----------
const P = {
  bg: '0B1C2C', accent: '529286',
  cover: { titleColor: 'FFFFFF', subtitleColor: 'B0B8C0', metaColor: '90989F', footerColor: '687078' },
  table: { headerBg: '529286', headerText: 'FFFFFF', accentLine: '529286', innerLine: 'BECFCC', surface: 'E8ECEB' },
};
const NB = { style: BorderStyle.NONE, size: 0, color: 'FFFFFF' };
const allNoBorders = { top: NB, bottom: NB, left: NB, right: NB, insideHorizontal: NB, insideVertical: NB };
const noBorders = { top: NB, bottom: NB, left: NB, right: NB };

// ---------- 封面配方 R1 ----------
function splitTitleLines(title, charsPerLine) {
  if (title.length <= charsPerLine) return [title];
  const breakAfter = new Set([...'\uff0c\u3002\u3001\uff1b\uff1a\uff01\uff1f', ...'\u7684\u4e0e\u548c\u53ca\u4e4b\u5728\u4e8e\u4e3a', ...'-_\u2014\u2013\u00b7/', ...' \t']);
  const lines = []; let remaining = title;
  while (remaining.length > charsPerLine) {
    let breakAt = -1;
    for (let i = charsPerLine; i >= Math.floor(charsPerLine * 0.6); i--) if (i < remaining.length && breakAfter.has(remaining[i - 1])) { breakAt = i; break; }
    if (breakAt === -1) { const limit = Math.min(remaining.length, Math.ceil(charsPerLine * 1.3)); for (let i = charsPerLine + 1; i < limit; i++) if (breakAfter.has(remaining[i - 1])) { breakAt = i; break; } }
    if (breakAt === -1) { breakAt = charsPerLine; const prevChar = remaining[breakAt - 1], nextChar = remaining[breakAt]; if (prevChar && nextChar && !breakAfter.has(prevChar) && !breakAfter.has(nextChar) && /[\u4e00-\u9fff]/.test(prevChar) && /[\u4e00-\u9fff]/.test(nextChar)) breakAt -= 1; }
    lines.push(remaining.slice(0, breakAt).trim());
    remaining = remaining.slice(breakAt).trim();
  }
  if (remaining) lines.push(remaining);
  if (lines.length >= 2 && lines[lines.length - 1].length === 1) { const last = lines.pop(); lines[lines.length - 1] += last; }
  return lines;
}
function calcTitleLayout(title, maxWidthTwips, preferredPt = 40, minPt = 24) {
  const charsPerLine = (pt) => Math.floor(maxWidthTwips / (pt * 20));
  let titlePt = preferredPt, lines;
  while (titlePt >= minPt) { const cpl = charsPerLine(titlePt); if (cpl < 2) { titlePt -= 2; continue; } lines = splitTitleLines(title, cpl); if (lines.length <= 3) break; titlePt -= 2; }
  if (!lines || lines.length > 3) { lines = splitTitleLines(title, charsPerLine(minPt)); titlePt = minPt; }
  return { titlePt, titleLines: lines };
}
function calcCoverSpacing(params) {
  const { titleLineCount = 1, titlePt = 36, hasSubtitle = false, hasEnglishLabel = false, metaLineCount = 0, fixedHeight = 800, pageHeight = 16838, marginTop = 0, marginBottom = 0 } = params;
  const SAFETY = 1200;
  const usableHeight = pageHeight - marginTop - marginBottom - SAFETY;
  const contentHeight = titleLineCount * (titlePt * 23 + 200) + (hasSubtitle ? (12 * 23 + 600) : 0) + (hasEnglishLabel ? (9 * 23 + 600) : 0) + metaLineCount * (10 * 23 + 100) + fixedHeight + 3 * 300;
  const safeRemaining = Math.max(usableHeight - contentHeight, 400);
  const FOOTER_MIN = 800;
  const rawTop = Math.floor(safeRemaining * 0.45), rawBottom = Math.floor(safeRemaining * 0.45);
  const bottomSpacing = Math.max(rawBottom, FOOTER_MIN);
  const topSpacing = Math.max(rawTop - Math.max(0, FOOTER_MIN - rawBottom), 400);
  return { topSpacing, bottomSpacing };
}
function buildCoverR1(config) {
  const PAL = config.palette;
  const padL = 1200, padR = 800;
  const availableWidth = 11906 - padL - padR - 300;
  const { titlePt, titleLines } = calcTitleLayout(config.title, availableWidth, 40, 24);
  const titleSize = titlePt * 2;
  const spacing = calcCoverSpacing({ titleLineCount: titleLines.length, titlePt, hasSubtitle: !!config.subtitle, hasEnglishLabel: !!config.englishLabel, metaLineCount: (config.metaLines || []).length, fixedHeight: 400 });
  const accentLeft = { style: BorderStyle.SINGLE, size: 8, color: PAL.accent, space: 12 };
  const children = [];
  children.push(new Paragraph({ spacing: { before: spacing.topSpacing } }));
  if (config.englishLabel) children.push(new Paragraph({
    indent: { left: padL, right: padR }, spacing: { after: 500 },
    border: { bottom: { style: BorderStyle.SINGLE, size: 6, color: PAL.accent, space: 8 } },
    children: [new TextRun({ text: config.englishLabel.split('').join('  '), size: 18, color: PAL.accent, font: { ascii: 'Calibri', eastAsia: 'SimHei' }, characterSpacing: 40 })],
  }));
  for (let i = 0; i < titleLines.length; i++) children.push(new Paragraph({
    indent: { left: padL }, spacing: { after: i < titleLines.length - 1 ? 100 : 300, line: Math.ceil(titlePt * 23), lineRule: 'atLeast' },
    children: [new TextRun({ text: titleLines[i], size: titleSize, bold: true, color: PAL.cover.titleColor, font: { eastAsia: 'SimHei', ascii: 'Arial' } })],
  }));
  if (config.subtitle) children.push(new Paragraph({
    indent: { left: padL }, spacing: { after: 800 },
    children: [new TextRun({ text: config.subtitle, size: 24, color: PAL.cover.subtitleColor, font: { eastAsia: 'Microsoft YaHei', ascii: 'Arial' } })],
  }));
  for (const line of (config.metaLines || [])) children.push(new Paragraph({
    indent: { left: padL + 200 }, spacing: { after: 80 }, border: { left: accentLeft },
    children: [new TextRun({ text: line, size: 24, color: PAL.cover.metaColor, font: { eastAsia: 'Microsoft YaHei', ascii: 'Arial' } })],
  }));
  children.push(new Paragraph({ spacing: { before: spacing.bottomSpacing } }));
  children.push(new Paragraph({
    indent: { left: padL, right: padR }, spacing: { before: 200 },
    border: { top: { style: BorderStyle.SINGLE, size: 2, color: PAL.accent, space: 8 } },
    children: [
      new TextRun({ text: config.footerLeft || '', size: 16, color: PAL.cover.footerColor, font: { ascii: 'Arial' } }),
      new TextRun({ text: '                                        ' }),
      new TextRun({ text: config.footerRight || '', size: 16, color: PAL.cover.footerColor, font: { ascii: 'Arial' } }),
    ],
  }));
  return [new Table({
    width: { size: 100, type: WidthType.PERCENTAGE }, layout: TableLayoutType.FIXED, borders: allNoBorders,
    rows: [new TableRow({ height: { value: 16838, rule: 'exact' }, children: [new TableCell({ shading: { type: ShadingType.CLEAR, fill: PAL.bg }, borders: noBorders, children })] })],
  })];
}

// ---------- markdown-lite 解析 ----------
const md = fs.readFileSync(MD, 'utf8');
const lines = md.split(/\r?\n/);
const CONTENT_W_PX = 600; // 正文图片最大宽度（px，96dpi ≈ 15.9cm）
function stripInline(text) { return text.replace(/\[([^\]]*)\]\(([^)]*)\)/g, '$1').replace(/\*\*/g, '').replace(/`/g, ''); }
function inlineRuns(text, opts = {}) {
  const runs = [];
  const re = /(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)]+\))/g;
  let last = 0, m;
  while ((m = re.exec(text))) {
    if (m.index > last) runs.push(new TextRun({ text: text.slice(last, m.index), ...opts }));
    const tok = m[0];
    if (tok.startsWith('**')) runs.push(new TextRun({ text: tok.slice(2, -2), bold: true, ...opts }));
    else if (tok.startsWith('`')) runs.push(new TextRun({ text: tok.slice(1, -1), font: { ascii: 'Consolas', eastAsia: 'Microsoft YaHei' }, shading: { type: ShadingType.CLEAR, fill: 'F0F2F5' }, ...opts }));
    else { const mm = tok.match(/\[([^\]]+)\]\(([^)]+)\)/); runs.push(new TextRun({ text: mm[1], color: '2B62D9', ...opts })); }
    last = m.index + tok.length;
  }
  if (last < text.length) runs.push(new TextRun({ text: text.slice(last), ...opts }));
  if (!runs.length) runs.push(new TextRun({ text, ...opts }));
  return runs;
}
function imgElement(relPath, alt, maxW = CONTENT_W_PX) {
  const abs = path.join(EV, relPath);
  if (!fs.existsSync(abs)) return new Paragraph({ children: [new TextRun({ text: `[缺图: ${relPath}]`, color: 'C0392B' })] });
  const buf = fs.readFileSync(abs);
  const dim = imageSize(buf);
  const w = Math.min(maxW, dim.width);
  const h = Math.round(dim.height * (w / dim.width));
  return new Paragraph({
    alignment: AlignmentType.CENTER, spacing: { before: 100, after: 100 },
    children: [new ImageRun({ type: 'png', data: buf, transformation: { width: w, height: h } })],
  });
}
/** 从一段 markdown 文本中抽取 evidence png 引用；范围引用（NNN–NNN 链到首帧）展开为该范围内全部文件。 */
function extractImgRefs(text) {
  const refs = [];
  const re = /\[([^\]]*)\]\((evidence\/[^)]+\.png)\)/g;
  let m;
  while ((m = re.exec(text))) {
    const label = m[1], rel = m[2];
    refs.push({ label, rel });
    const range = label.match(/^(\d+)[\u2013\u2014-](\d+)$/);
    if (range) {
      const first = rel.match(/^(.*\/)\d+-(.*)$/);
      if (first) {
        const dir = first[1], slug = first[2];
        const start = parseInt(range[1], 10), end = parseInt(range[2], 10);
        for (let n = start + 1; n <= end; n++) {
          const cand = `${dir}${String(n).padStart(3, '0')}-${slug}`;
          if (fs.existsSync(path.join(EV, cand))) refs.push({ label: String(n), rel: cand });
        }
      }
    }
  }
  return refs;
}
/** 表格单元格：文本 + 内嵌截图缩略图（最多 3 张/格，240px） */
function cellChildren(text, isHeader) {
  const paras = [new Paragraph({ spacing: { line: 276 }, children: inlineRuns(text, { size: isHeader ? 20 : 19, color: isHeader ? P.table.headerText : '333333', bold: isHeader }) })];
  if (!isHeader) {
    const refs = extractImgRefs(text).slice(0, 4);
    for (const r of refs) paras.push(imgElement(r.rel, r.label, 240));
  }
  return paras;
}
function tableElement(header, rows) {
  const cellMargins = { top: 60, bottom: 60, left: 100, right: 100 };
  const mk = (text, isHeader, colIdx) => new TableCell({
    margins: cellMargins,
    shading: { type: ShadingType.CLEAR, fill: isHeader ? P.table.headerBg : 'FFFFFF' },
    borders: {
      top: { style: BorderStyle.SINGLE, size: 4, color: P.table.accentLine },
      bottom: { style: BorderStyle.SINGLE, size: 4, color: P.table.innerLine },
      left: NB, right: NB,
    },
    children: cellChildren(text, isHeader),
  });
  const trs = [new TableRow({ tableHeader: true, cantSplit: true, children: header.map((c, i) => mk(c, true, i)) })];
  for (const r of rows) trs.push(new TableRow({ cantSplit: true, children: r.map((c, i) => mk(c, false, i)) }));
  return new Table({ width: { size: 100, type: WidthType.PERCENTAGE }, layout: TableLayoutType.FIXED, rows: trs });
}

// ---------- 逐行构建正文 ----------
const body = [];
let i = 0;
let chapterIdx = 0;
const APPENDIX_FILES = fs.readdirSync(path.join(EV, 'evidence', 'manual-20261112')).filter(f => f.endsWith('.png')).sort();
while (i < lines.length) {
  const line = lines[i];
  if (line.startsWith('# ')) { i++; continue; } // 主标题交给封面
  if (line.startsWith('## ')) {
    const title = stripInline(line.slice(3));
    // 附录在生成器末尾单独构建
    if (title.startsWith('七、') || title.includes('附录')) { break; }
    body.push(new Paragraph({ heading: HeadingLevel.HEADING_1, spacing: { before: 360, after: 200 }, children: inlineRuns(title, { bold: true }) }));
    i++; continue;
  }
  if (line.startsWith('### ')) {
    body.push(new Paragraph({ heading: HeadingLevel.HEADING_2, spacing: { before: 280, after: 160 }, children: inlineRuns(stripInline(line.slice(4)), { bold: true }) }));
    i++; continue;
  }
  if (line.startsWith('> ')) {
    const text = stripInline(line.slice(2));
    body.push(new Paragraph({
      spacing: { before: 120, after: 120, line: 312 }, indent: { left: 300, right: 200 },
      border: { left: { style: BorderStyle.SINGLE, size: 12, color: P.accent, space: 10 } },
      shading: { type: ShadingType.CLEAR, fill: 'F3F7F6' },
      children: inlineRuns(text, { size: 20, color: '333333' }),
    }));
    i++; continue;
  }
  if (line.startsWith('```')) {
    i++;
    const code = [];
    while (i < lines.length && !lines[i].startsWith('```')) { code.push(lines[i]); i++; }
    i++;
    for (const c of code) body.push(new Paragraph({
      spacing: { line: 240 }, shading: { type: ShadingType.CLEAR, fill: 'F4F6F8' },
      indent: { left: 300, right: 300 },
      children: [new TextRun({ text: c || ' ', font: { ascii: 'Consolas', eastAsia: 'Microsoft YaHei' }, size: 18, color: '24323D' })],
    }));
    continue;
  }
  if (line.startsWith('|')) {
    const tbl = [];
    while (i < lines.length && lines[i].startsWith('|')) { tbl.push(lines[i]); i++; }
    // 保留原始链接（cellChildren 需要从中提取截图）；inlineRuns 负责渲染加粗/代码/链接
    const parse = (r) => r.replace(/^\||\|$/g, '').split('|').map(c => c.trim().replace(/\*\*/g, '').replace(/`/g, ''));
    const header = parse(tbl[0]).map(stripInline);
    const rows = tbl.slice(2).map(parse);
    body.push(tableElement(header, rows));
    body.push(new Paragraph({ spacing: { after: 120 } }));
    continue;
  }
  const img = line.match(/!\[([^\]]*)\]\(([^)]+)\)/);
  if (img) { body.push(imgElement(img[2], img[1])); i++; continue; }
  if (/^\d+\.\s/.test(line)) { body.push(new Paragraph({ spacing: { line: 312, after: 60 }, indent: { left: 360, hanging: 240 }, children: inlineRuns(line, { size: 21 }) })); i++; continue; }
  if (line.startsWith('- ')) {
    body.push(new Paragraph({ spacing: { line: 312, after: 60 }, indent: { left: 360, hanging: 240 }, children: inlineRuns('\u2022  ' + line.slice(2), { size: 21 }) }));
    for (const r of extractImgRefs(line).slice(0, 10)) body.push(imgElement(r.rel, r.label, 280));
    i++; continue;
  }
  if (line.trim() === '---' || line.trim() === '') { i++; continue; }
  // 普通段落
  body.push(new Paragraph({ spacing: { line: 312, after: 100 }, children: inlineRuns(line, { size: 21 }) }));
  if (!line.startsWith('#') && !line.startsWith('|')) { for (const r of extractImgRefs(line).slice(0, 10)) body.push(imgElement(r.rel, r.label, 280)); }
  i++;
}

// ---------- 附录：全部截图清单 ----------
body.push(new Paragraph({ heading: HeadingLevel.HEADING_1, spacing: { before: 400, after: 200 }, children: [new TextRun({ text: '附录：证据截图清单（186 张）', bold: true })] }));
body.push(new Paragraph({ spacing: { line: 312, after: 120 }, children: [new TextRun({ text: '全部截图存于 docs/active/evidence/manual-20261112/（含 bugfix 子目录与 bug #2 修改前证据备份 manual-20261112-round8-backup/）。以下为主目录清单：', size: 21 })] }));
const half = Math.ceil(APPENDIX_FILES.length / 2);
const appRows = [];
for (let k = 0; k < half; k++) {
  appRows.push(new TableRow({ cantSplit: true, children: [APPENDIX_FILES[k], APPENDIX_FILES[k + half] ?? ''].map(name => new TableCell({
    margins: { top: 40, bottom: 40, left: 100, right: 100 },
    borders: { top: NB, bottom: { style: BorderStyle.SINGLE, size: 2, color: 'DDDDDD' }, left: NB, right: NB },
    children: [new Paragraph({ spacing: { line: 240 }, children: [new TextRun({ text: name, font: { ascii: 'Consolas', eastAsia: 'Microsoft YaHei' }, size: 16, color: '555555' })] })],
  })) }));
}
body.push(new Table({ width: { size: 100, type: WidthType.PERCENTAGE }, layout: TableLayoutType.FIXED, rows: appRows }));

// ---------- 组装文档 ----------
const footerPage = (fmt) => new Footer({ children: [new Paragraph({ alignment: AlignmentType.CENTER, children: [new TextRun({ children: [PageNumber.CURRENT], size: 18, color: '888888' })] })] });
const doc = new Document({
  creator: 'NavKit 验收自动化',
  title: 'NavKit 全功能桌面验收报告与用户手册',
  styles: {
    default: { document: { run: { font: { ascii: 'Calibri', eastAsia: 'Microsoft YaHei' }, size: 21 } } },
    paragraphStyles: [
      { id: 'Heading1', name: 'Heading 1', basedOn: 'Normal', next: 'Normal', quickFormat: true,
        run: { size: 32, bold: true, color: '0B1C2C', font: { ascii: 'Arial', eastAsia: 'SimHei' } },
        paragraph: { spacing: { before: 360, after: 200 }, outlineLevel: 0 } },
      { id: 'Heading2', name: 'Heading 2', basedOn: 'Normal', next: 'Normal', quickFormat: true,
        run: { size: 26, bold: true, color: '1F4E5F', font: { ascii: 'Arial', eastAsia: 'Microsoft YaHei' } },
        paragraph: { spacing: { before: 280, after: 160 }, outlineLevel: 1 } },
    ],
  },
  features: { updateFields: true },
  sections: [
    { // 封面
      properties: { page: { margin: { top: 0, bottom: 0, left: 0, right: 0 } } },
      children: buildCoverR1({
        title: 'NavKit 全功能桌面验收报告与用户手册',
        subtitle: 'IPC + GUI 全遍历 · 79 项功能 · 186 张现场截图',
        englishLabel: 'NAVKIT DESKTOP ACCEPTANCE',
        metaLines: ['测试形态：真实桌面（Tauri 2 + WebView2 CDP）+ 真实 MySQL / SQLite', '结果：79/79 通过 · 发现并修复 2 处产品缺陷 · 风格检查 0 修改项', '证据目录：docs/active/evidence/manual-20261112/', '报告日期：2026-11-12'],
        footerLeft: 'NavKit · 数据库管理工具', footerRight: '2026-11-12', palette: P,
      }),
    },
    { // 目录（罗马页码）
      properties: { type: SectionType.NEXT_PAGE, page: { pageNumbers: { start: 1, formatType: NumberFormat.LOWER_ROMAN } } },
      footers: { default: footerPage() },
      children: [
        new Paragraph({ heading: HeadingLevel.HEADING_1, children: [new TextRun({ text: '目录', bold: true })] }),
        new TableOfContents('\u76ee\u5f55', { hyperlink: true, headingStyleRange: '1-2' }),
        new Paragraph({ spacing: { before: 200 }, children: [new TextRun({ text: '提示：在 Word / WPS 中右键目录 →「更新域」以刷新页码。', italics: true, size: 18, color: '999999' })] }),
        // 不加 PageBreak：下一节已是 NEXT_PAGE，双重分页会产生空白页
      ],
    },
    { // 正文（阿拉伯页码从 1 起）
      properties: { type: SectionType.NEXT_PAGE, page: { pageNumbers: { start: 1, formatType: NumberFormat.DECIMAL } } },
      headers: { default: new Header({ children: [new Paragraph({ alignment: AlignmentType.RIGHT, children: [new TextRun({ text: 'NavKit 全功能桌面验收报告与用户手册 · 2026-11-12', size: 16, color: '999999' })] })] }) },
      footers: { default: footerPage() },
      children: body,
    },
  ],
});
const buf = await Packer.toBuffer(doc);
fs.writeFileSync(OUT, buf);
console.log('WROTE', OUT, (buf.length / 1024 / 1024).toFixed(1), 'MB');
