//! 本地书籍导入（EPUB / TXT / MOBI / AZW3 / PDF / FB2 / DOCX）
//!
//! - EPUB：zip 解包 → container.xml → OPF 元数据 → spine 章节（XHTML → 纯文本）→ 封面
//! - TXT：编码检测（UTF-8/GBK）→ 分章（章节标题正则）
//! - MOBI/AZW3：mobi crate（PalmDB header + 记录表 + 解压）→ rawml HTML → 纯文本 → 分章；
//!   azw3（KF8）暂走 mobi 兼容层，结构差异/加密时返回友好错误
//! - PDF：lopdf 按页提取文本（每页解压上限防炸弹；大 PDF 限 300 页）→ 标题分章或页分章
//! - FB2：quick-xml 解析 body/section/title/p → 分章（每 section 一章）
//! - DOCX：zip + word/document.xml → 段落提取（标题样式分章或按规则/字数回退）

use anyhow::{Context, Result};
use serde::Serialize;

use crate::service::epub::{parse_opf, OpfMeta};

/// 导入的书籍
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedBook {
    pub meta: OpfMeta,
    /// 章节（标题 + 正文文本）
    pub chapters: Vec<Chapter>,
    /// 封面（原始字节）
    #[serde(skip)]
    pub cover: Option<Vec<u8>>,
    /// 格式（epub/txt/mobi/azw3/pdf/fb2/docx）
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Chapter {
    pub title: String,
    pub content: String,
}

/// EPUB 解析
pub fn parse_epub(bytes: &[u8]) -> Result<ImportedBook> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .context("EPUB 不是有效的 zip")?;

    // 1. container.xml → OPF 路径
    let container = read_zip(&mut zip, "META-INF/container.xml")
        .context("缺少 META-INF/container.xml")?;
    let container_str = String::from_utf8_lossy(&container);
    let opf_path = extract_attr_simple(&container_str, "rootfile", "full-path")
        .context("container.xml 缺少 rootfile")?;

    // 2. OPF 元数据
    let opf = read_zip(&mut zip, &opf_path).context("读取 OPF 失败")?;
    let meta = parse_opf(&String::from_utf8_lossy(&opf));

    // 3. spine（章节顺序）
    let opf_str = String::from_utf8_lossy(&opf);
    let spine_refs: Vec<String> = extract_all_attr(&opf_str, "itemref", "idref");
    let manifest: std::collections::HashMap<String, (String, String)> = extract_manifest(&opf_str);

    // 4. 章节内容（spine 顺序）
    let mut chapters = Vec::new();
    for idref in &spine_refs {
        let Some((href, mediatype)) = manifest.get(idref) else { continue };
        if !mediatype.contains("xhtml") && !mediatype.contains("html") {
            continue;
        }
        // href 相对 OPF 目录
        let full_path = resolve_opf_path(&opf_path, href);
        let Ok(content_bytes) = read_zip(&mut zip, &full_path) else { continue };
        let html = String::from_utf8_lossy(&content_bytes);
        let text = html_to_text(&html);
        if text.trim().is_empty() {
            continue;
        }
        let title = extract_title(&html).unwrap_or_else(|| format!("第 {} 节", chapters.len() + 1));
        chapters.push(Chapter { title, content: text });
    }
    if chapters.is_empty() {
        // fallback：manifest 所有 xhtml
        for ((href, mediatype)) in manifest.values() {
            if !mediatype.contains("xhtml") && !mediatype.contains("html") {
                continue;
            }
            let full_path = resolve_opf_path(&opf_path, href);
            if let Ok(content_bytes) = read_zip(&mut zip, &full_path) {
                let html = String::from_utf8_lossy(&content_bytes);
                let text = html_to_text(&html);
                if !text.trim().is_empty() {
                    let title = extract_title(&html)
                        .unwrap_or_else(|| format!("第 {} 节", chapters.len() + 1));
                    chapters.push(Chapter { title, content: text });
                }
            }
        }
    }

    // 5. 封面
    let cover = meta.cover_href.as_ref().and_then(|href| {
        let full = resolve_opf_path(&opf_path, href);
        read_zip(&mut zip, &full).ok()
    });

    Ok(ImportedBook {
        meta,
        chapters,
        cover,
        format: "epub".into(),
    })
}

/// TXT 解析（编码检测 + 分章；使用内置默认规则）
pub fn parse_txt(bytes: &[u8]) -> Result<ImportedBook> {
    parse_txt_with_rules(bytes, &[])
}

/// TXT 解析（编码检测 + 分章；rules 为空时用内置 DEFAULT_TOC_RULES，否则用用户自定义规则）
pub fn parse_txt_with_rules(bytes: &[u8], user_rules: &[String]) -> Result<ImportedBook> {
    // 编码检测：UTF-8 优先，失败用 GBK/GB18030
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let encoding = encoding_rs::GBK;
            let (decoded, _, _) = encoding.decode(bytes);
            decoded.into_owned()
        }
    };
    // 去掉 BOM
    let text = text.trim_start_matches('\u{feff}').to_string();

    // 分章：优先用户自定义 TXT 目录规则（txt_toc_rules），无则用内置默认规则
    let rules: Vec<String> = if user_rules.is_empty() {
        DEFAULT_TOC_RULES.iter().map(|s| s.to_string()).collect()
    } else {
        user_rules.to_vec()
    };
    let mut chapters = split_by_rules(&text, &rules);
    if chapters.is_empty() {
        // 无章节标记：长文本按 10000 字分块 / 短文本整本一章
        chapters = chunk_fallback(&text);
    }

    // 元数据（文件名信息由调用方补充——这里取首行做书名猜测）
    let title = text.lines().next().unwrap_or("本地书籍").trim().to_string();
    let meta = OpfMeta {
        title: title.clone(),
        author: String::new(),
        ..Default::default()
    };

    Ok(ImportedBook {
        meta,
        chapters,
        cover: None,
        format: "txt".into(),
    })
}

/// 内置默认 TXT 目录规则（对齐 legado 常见章节标题格式）
pub const DEFAULT_TOC_RULES: &[&str] = &[
    // 第X章 / 第X节 / 第X卷 第X章 等（常见中文格式）
    r"^\s*第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*[章节卷回集部篇][^
]{0,40}[ 	]*$",
    // 卷标题（"第X卷" 或 "第一卷 标题"）
    r"^\s*第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*卷[^
]{0,40}[ 	]*$",
    // 序章/楔子/番外/后记/尾声/前言/引子/正文 等
    r"^\s*(序章|楔子|番外|后记|尾声|前言|引子|正文|终章)[^
]{0,40}[ 	]*$",
    // 英文 Chapter / CHAPTER
    r"^\s*[Cc][Hh][Aa][Pp][Tt][Ee][Rr]\s+\d+[^
]{0,40}[ 	]*$",
    // 数字+空格+标题（常见"1 标题"格式）
    r"^\s*\d{1,4}[\s、.．:：][^
]{0,40}[ 	]*$",
];

/// 用规则列表分章（txtTocRule 语义——正则匹配行作为章节标题）
/// 规则按 legado TextFile 语义以 MULTILINE 编译（`^`/`$` 按行锚定，规则匹配整行章节标题）
fn split_by_rules(text: &str, rules: &[String]) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    let mut last_pos = 0usize;
    let mut last_title = "正文".to_string();
    // 收集所有规则匹配
    let mut matches: Vec<(usize, usize, String)> = Vec::new();
    for rule in rules {
        if let Ok(re) = regex::RegexBuilder::new(rule).multi_line(true).build() {
            for cap in re.captures_iter(text) {
                if let Some(m) = cap.get(0) {
                    let title = m.as_str().trim().to_string();
                    if !title.is_empty() {
                        matches.push((m.start(), m.end(), title));
                    }
                }
            }
        }
    }
    matches.sort_by_key(|m| m.0);
    matches.dedup_by_key(|m| m.0);
    // 无任何匹配 → 返回空（调用方回退：长文本按字数分块，短文本整本一章）
    if matches.is_empty() {
        return Vec::new();
    }
    for (start, end, title) in matches {
        let content = text[last_pos..start].trim().to_string();
        if !content.is_empty() {
            chapters.push(Chapter {
                title: last_title.clone(),
                content,
            });
        }
        last_title = title;
        last_pos = end;
    }
    let tail = text[last_pos..].trim().to_string();
    if !tail.is_empty() {
        chapters.push(Chapter {
            title: last_title,
            content: tail,
        });
    }
    chapters
}

/// 读 TXT 文件并分章（legacy 本地书：bookUrl = storage/data/.../xx.txt）
pub fn parse_txt_file(path: &std::path::Path) -> Result<ImportedBook> {
    let bytes = std::fs::read(path)?;
    parse_txt(&bytes)
}

/// 读 TXT 文件并分章（用户自定义规则版本）
pub fn parse_txt_file_with_rules(path: &std::path::Path, user_rules: &[String]) -> Result<ImportedBook> {
    let bytes = std::fs::read(path)?;
    parse_txt_with_rules(&bytes, user_rules)
}

// ---------- 通用分章 ----------

/// 无章节标记回退：长文本按 10000 字分块（避免单章过大渲染卡顿），短文本整本一章
fn chunk_fallback(text: &str) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    if text.trim().is_empty() {
        return chapters;
    }
    const CHUNK: usize = 10_000;
    let body = text.trim().to_string();
    if body.chars().count() > CHUNK * 2 {
        let mut start = 0usize;
        let chars: Vec<char> = body.chars().collect();
        let mut part = 1;
        while start < chars.len() {
            let end = (start + CHUNK).min(chars.len());
            let chunk: String = chars[start..end].iter().collect();
            chapters.push(Chapter {
                title: format!("第 {part} 部分"),
                content: chunk,
            });
            start = end;
            part += 1;
        }
    } else {
        chapters.push(Chapter {
            title: "正文".into(),
            content: body,
        });
    }
    chapters
}

/// 纯文本分章（内置默认规则；无匹配时回退 chunk_fallback）
fn chapters_from_plain_text(text: &str) -> Vec<Chapter> {
    let rules: Vec<String> = DEFAULT_TOC_RULES.iter().map(|s| s.to_string()).collect();
    let chapters = split_by_rules(text, &rules);
    if chapters.is_empty() {
        chunk_fallback(text)
    } else {
        chapters
    }
}

// ---------- MOBI / AZW3 ----------

/// MOBI（mobi7）解析：PalmDB header → 记录表 → 解压（Palmdoc/Huff/无压缩）→ rawml HTML → 纯文本分章
pub fn parse_mobi(bytes: &[u8]) -> Result<ImportedBook> {
    parse_mobi_impl(bytes, "mobi")
}

/// AZW3（KF8）解析：先走 mobi 兼容层（部分 azw3 携带 mobi7 回退正文；纯 KF8 结构返回友好错误）
pub fn parse_azw3(bytes: &[u8]) -> Result<ImportedBook> {
    parse_mobi_impl(bytes, "azw3")
}

fn parse_mobi_impl(bytes: &[u8], format: &str) -> Result<ImportedBook> {
    let book = mobi::Mobi::new(bytes.to_vec())
        .context("MOBI/AZW3 解析失败（不是有效的 PalmDB/MOBI 文件，或 KF8 加密暂不支持）")?;
    let raw = book.content_as_string_lossy();
    if raw.trim().is_empty() {
        anyhow::bail!("MOBI 未包含可读文本（可能已加密）");
    }
    // mobi7 正文是 rawml HTML（<mbp:pagebreak/> 分隔章节）——转纯文本再分章
    let text = html_to_text(&raw);
    let mut chapters = chapters_from_plain_text(&text);
    if chapters.is_empty() && !text.trim().is_empty() {
        chapters.push(Chapter {
            title: "正文".into(),
            content: text.trim().to_string(),
        });
    }
    let mut meta = OpfMeta {
        title: book.title(),
        author: book.author().unwrap_or_default(),
        ..Default::default()
    };
    if let Some(d) = book.description() {
        meta.description = Some(d);
    }
    if let Some(p) = book.publisher() {
        meta.publisher = Some(p);
    }
    meta.language = Some(format!("{:?}", book.language()));
    // 封面：首个图片记录（MOBI 约定 record 0 为封面）
    let cover = book.image_records().into_iter().next().map(|r| r.content.to_vec());
    Ok(ImportedBook {
        meta,
        chapters,
        cover,
        format: format.into(),
    })
}

// ---------- PDF ----------

/// 大 PDF 防卡：最多提取前 300 页
pub const PDF_MAX_PAGES: usize = 300;

/// PDF 分章规则：默认规则去掉“数字+空格+标题”（PDF 页码行易误匹配）
pub const PDF_TOC_RULES: &[&str] = &[
    r"^\s*第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*[章节卷回集部篇][^\n]{0,40}[ \t]*$",
    r"^\s*第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*卷[^\n]{0,40}[ \t]*$",
    r"^\s*(序章|楔子|番外|后记|尾声|前言|引子|正文|终章)[^\n]{0,40}[ \t]*$",
    r"^\s*[Cc][Hh][Aa][Pp][Tt][Ee][Rr]\s+\d+[^\n]{0,40}[ \t]*$",
];

/// PDF 解析：lopdf 按页提取文本（每页解压上限 8MB 防炸弹）→ 标题分章或页分章
pub fn parse_pdf(bytes: &[u8]) -> Result<ImportedBook> {
    let doc = lopdf::Document::load_mem(bytes).context("PDF 解析失败（文件损坏、加密或非 PDF）")?;
    let total_pages = doc.get_pages().len();
    if total_pages == 0 {
        anyhow::bail!("PDF 没有页面");
    }
    let limit = total_pages.min(PDF_MAX_PAGES);
    let mut pages = Vec::with_capacity(limit);
    for num in 1..=limit {
        match doc.extract_text_with_limit(&[num as u32], 8 * 1024 * 1024) {
            Ok(t) => pages.push(t),
            Err(e) => {
                tracing::warn!("PDF 第 {num} 页文本提取失败：{e}");
                pages.push(String::new());
            }
        }
    }
    if pages.iter().all(|p| p.trim().is_empty()) {
        anyhow::bail!("PDF 未提取到文本（扫描版/图片型 PDF 暂不支持 OCR）");
    }
    // 元数据（Info 字典）
    let mut meta = OpfMeta::default();
    if let Ok(info_id) = doc.trailer.get(b"Info").map(|o| o.as_reference()).unwrap_or(Err(lopdf::Error::ObjectType {
        expected: "reference",
        found: "none",
    })) {
        if let Ok(info) = doc.get_dictionary(info_id) {
            meta.title = pdf_meta_string(info.get(b"Title"));
            meta.author = pdf_meta_string(info.get(b"Author"));
            meta.description = Some(pdf_meta_string(info.get(b"Subject").or_else(|_| info.get(b"Keywords"))));
        }
    }
    let chapters = chapters_from_pages(pages);
    Ok(ImportedBook {
        meta,
        chapters,
        cover: None,
        format: "pdf".into(),
    })
}

/// PDF 元数据字符串解码（PDFDocEncoding/UTF-16BE/UTF-8；lopdf 0.44 Dictionary::get 返回 Result）
fn pdf_meta_string(v: Result<&lopdf::Object, lopdf::Error>) -> String {
    v.ok()
        .and_then(|o| lopdf::decode_text_string(o).ok())
        .map(|s| s.trim().trim_start_matches('\u{feff}').trim().to_string())
        .unwrap_or_default()
}

/// PDF 分章：优先按章节标题规则（跨页全文匹配）；无标题 → 按页分章（每页一章）
fn chapters_from_pages(pages: Vec<String>) -> Vec<Chapter> {
    let rules: Vec<String> = PDF_TOC_RULES.iter().map(|s| s.to_string()).collect();
    let joined = pages.join("\n\n");
    let by_rules = split_by_rules(&joined, &rules);
    if !by_rules.is_empty() {
        return by_rules;
    }
    let mut chapters = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        let t = page.trim();
        if !t.is_empty() {
            chapters.push(Chapter {
                title: format!("第 {} 页", i + 1),
                content: t.to_string(),
            });
        }
    }
    if chapters.is_empty() {
        return chunk_fallback(&joined);
    }
    chapters
}

// ---------- FB2 ----------

/// FB2 解析：quick-xml 提取 description（书名/作者/简介）+ 第一个 body 的 section 分章
pub fn parse_fb2(bytes: &[u8]) -> Result<ImportedBook> {
    let xml = String::from_utf8_lossy(bytes);
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut buf = Vec::new();

    let mut meta = OpfMeta::default();
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut cur: Option<(String, String)> = None; // 当前 section 的 (title, content)
    let mut body_count = 0usize;
    let mut in_main_body = false;
    let mut section_depth = 0usize;
    let mut in_title = false;
    let mut in_para = false;
    let mut para_break = false; // 段落之间插换行
    let mut in_book_title = false;
    let mut in_annotation = false;
    let mut in_author_field = false;
    let mut author_parts: Vec<String> = Vec::new();

    // 段落文本入缓冲（段落之间插换行；同一段落内多个文本片断保留原始空白直接拼接）
    fn push_para(dst: &mut String, s: &str, break_before: &mut bool) {
        if s.trim().is_empty() {
            return;
        }
        if *break_before && !dst.is_empty() && !dst.ends_with('\n') {
            dst.push('\n');
        }
        dst.push_str(s);
        *break_before = false;
    }
    macro_rules! flush_section {
        () => {
            if let Some((title, content)) = cur.take() {
                let content = content.trim().to_string();
                if !content.is_empty() || !title.trim().is_empty() {
                    let title = if title.trim().is_empty() {
                        format!("第 {} 节", chapters.len() + 1)
                    } else {
                        title.split_whitespace().collect::<Vec<_>>().join(" ")
                    };
                    chapters.push(Chapter { title, content });
                }
            }
        };
    }

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "body" => {
                    if body_count == 0 {
                        in_main_body = true;
                    }
                    body_count += 1;
                }
                "section" => {
                    if in_main_body {
                        if section_depth == 0 {
                            flush_section!();
                            cur = Some((String::new(), String::new()));
                        }
                        section_depth += 1;
                    }
                }
                "title" => {
                    if in_main_body && section_depth > 0 {
                        in_title = true;
                    }
                }
                "p" | "subtitle" | "cite" | "poem" | "stanza" | "epigraph" | "text-author" => {
                    if in_main_body && section_depth > 0 {
                        in_para = true;
                        para_break = true;
                    }
                }
                "book-title" => in_book_title = true,
                "annotation" => in_annotation = true,
                "first-name" | "last-name" | "middle-name" | "nickname" => {
                    in_author_field = true;
                }
                "FictionBook" => {
                    for attr in e.attributes().flatten() {
                        if std::str::from_utf8(attr.key.local_name().as_ref()).unwrap_or("") == "lang" {
                            if let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                                meta.language = Some(v.into_owned());
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(quick_xml::events::Event::End(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "body" => {
                    if in_main_body {
                        flush_section!();
                        in_main_body = false;
                    }
                }
                "section" => {
                    if in_main_body && section_depth > 0 {
                        section_depth -= 1;
                        if section_depth == 0 {
                            flush_section!();
                        }
                    }
                }
                "title" => in_title = false,
                "p" | "subtitle" | "cite" | "poem" | "stanza" | "epigraph" | "text-author" => {
                    in_para = false;
                }
                "book-title" => in_book_title = false,
                "annotation" => in_annotation = false,
                "first-name" | "last-name" | "middle-name" | "nickname" => {
                    in_author_field = false;
                }
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(t)) => {
                let Ok(s) = t.xml10_content() else { continue };
                if s.trim().is_empty() {
                    continue;
                }
                if in_book_title {
                    meta.title.push_str(&s);
                } else if in_annotation {
                    meta.description.get_or_insert_with(String::new).push_str(&s);
                } else if in_author_field {
                    author_parts.push(s.trim().to_string());
                } else if in_main_body && section_depth > 0 {
                    if let Some((title, content)) = cur.as_mut() {
                        if in_title {
                            push_para(title, &s, &mut para_break);
                        } else if in_para {
                            push_para(content, &s, &mut para_break);
                        }
                    }
                }
            }
            // CDATA 段与文本同处理（xml10_content 对两者均可用）
            Ok(quick_xml::events::Event::CData(t)) => {
                let Ok(s) = t.xml10_content() else { continue };
                if s.trim().is_empty() {
                    continue;
                }
                if in_book_title {
                    meta.title.push_str(&s);
                } else if in_annotation {
                    meta.description.get_or_insert_with(String::new).push_str(&s);
                } else if in_author_field {
                    author_parts.push(s.trim().to_string());
                } else if in_main_body && section_depth > 0 {
                    if let Some((title, content)) = cur.as_mut() {
                        if in_title {
                            push_para(title, &s, &mut para_break);
                        } else if in_para {
                            push_para(content, &s, &mut para_break);
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) => {
                let s = if r.is_char_ref() {
                    r.resolve_char_ref().ok().flatten().map(|c| c.to_string())
                } else {
                    r.decode()
                        .ok()
                        .and_then(|name| quick_xml::escape::unescape(&format!("&{name};")).ok().map(|c| c.into_owned()))
                };
                if let Some(s) = s {
                    if in_book_title {
                        meta.title.push_str(&s);
                    } else if in_annotation {
                        meta.description.get_or_insert_with(String::new).push_str(&s);
                    } else if in_author_field {
                        author_parts.push(s);
                    } else if in_main_body && section_depth > 0 {
                        if let Some((title, content)) = cur.as_mut() {
                            if in_title {
                                title.push_str(&s);
                            } else if in_para {
                                content.push_str(&s);
                            }
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => anyhow::bail!("FB2 XML 解析失败：{e}"),
            _ => {}
        }
    }
    flush_section!();

    meta.title = meta.title.trim().to_string();
    if meta.author.is_empty() {
        meta.author = author_parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(d) = meta.description.as_mut() {
        *d = d.trim().to_string();
    }
    if chapters.is_empty() {
        anyhow::bail!("FB2 未解析到章节内容（缺少 body/section）");
    }
    Ok(ImportedBook {
        meta,
        chapters,
        cover: None,
        format: "fb2".into(),
    })
}

// ---------- DOCX ----------

/// DOCX 解析：zip + word/document.xml → 段落（含标题样式）→ 标题样式分章；无标题样式时回退纯文本规则分章
pub fn parse_docx(bytes: &[u8]) -> Result<ImportedBook> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("DOCX 不是有效的 zip")?;
    let document = read_zip(&mut zip, "word/document.xml").context("DOCX 缺少 word/document.xml")?;
    // 元数据（可选 docProps/core.xml）
    let mut meta = OpfMeta::default();
    if let Ok(core) = read_zip(&mut zip, "docProps/core.xml") {
        let (title, author) = docx_core_meta(&String::from_utf8_lossy(&core));
        meta.title = title;
        meta.author = author;
    }

    let xml = String::from_utf8_lossy(&document);
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut buf = Vec::new();
    let mut paras: Vec<(Option<String>, String)> = Vec::new(); // (样式, 文本)
    let mut in_p = false;
    let mut in_t = false;
    let mut p_style: Option<String> = None;
    let mut p_buf = String::new();

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "p" => {
                    in_p = true;
                    p_style = None;
                    p_buf.clear();
                }
                "pStyle" => {
                    if in_p {
                        for attr in e.attributes().flatten() {
                            if std::str::from_utf8(attr.key.local_name().as_ref()).unwrap_or("") == "val" {
                                if let Ok(v) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                                    p_style = Some(v.into_owned());
                                }
                            }
                        }
                    }
                }
                "t" => in_t = true,
                "tab" => {
                    if in_p {
                        p_buf.push('\t');
                    }
                }
                "br" | "cr" => {
                    if in_p {
                        p_buf.push('\n');
                    }
                }
                _ => {}
            },
            Ok(quick_xml::events::Event::End(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "p" => {
                    in_p = false;
                    let text = p_buf.trim().to_string();
                    if !text.is_empty() {
                        paras.push((p_style.take(), text));
                    }
                }
                "t" => in_t = false,
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(t)) => {
                if in_p && in_t {
                    if let Ok(s) = t.xml10_content() {
                        p_buf.push_str(&s);
                    }
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) => {
                if in_p && in_t {
                    let s = if r.is_char_ref() {
                        r.resolve_char_ref().ok().flatten().map(|c| c.to_string())
                    } else {
                        r.decode()
                            .ok()
                            .and_then(|name| quick_xml::escape::unescape(&format!("&{name};")).ok().map(|c| c.into_owned()))
                    };
                    if let Some(s) = s {
                        p_buf.push_str(&s);
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => anyhow::bail!("DOCX 解析失败：{e}"),
            _ => {}
        }
    }

    let has_heading = paras.iter().any(|(s, _)| s.as_deref().map(is_heading_style).unwrap_or(false));
    let chapters = if has_heading {
        docx_heading_chapters(&paras)
    } else {
        // 无标题样式：纯文本规则分章（或按字数分块）
        let joined = paras.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n\n");
        chapters_from_plain_text(&joined)
    };
    if chapters.is_empty() {
        anyhow::bail!("DOCX 未解析到章节内容");
    }
    Ok(ImportedBook {
        meta,
        chapters,
        cover: None,
        format: "docx".into(),
    })
}

/// 标题样式判断（Word 内置 Heading1..9 / 中文“标题 1” / 旧版数字样式 1..9）
fn is_heading_style(style: &str) -> bool {
    let s = style.trim().to_lowercase();
    s.starts_with("heading")
        || s.starts_with("标题")
        || (s.len() == 1 && s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
}

/// 按标题样式段落分章：标题段落开启新章节，其余段落并入当前章节
fn docx_heading_chapters(paras: &[(Option<String>, String)]) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut cur: Option<Chapter> = None;
    for (style, text) in paras {
        if style.as_deref().map(is_heading_style).unwrap_or(false) {
            if let Some(c) = cur.take() {
                if !c.content.trim().is_empty() || !c.title.trim().is_empty() {
                    chapters.push(c);
                }
            }
            cur = Some(Chapter {
                title: text.clone(),
                content: String::new(),
            });
        } else if let Some(c) = cur.as_mut() {
            if !c.content.is_empty() {
                c.content.push_str("\n\n");
            }
            c.content.push_str(text);
        } else {
            // 首个标题前的正文 → 归入“正文”章
            cur = Some(Chapter {
                title: "正文".into(),
                content: text.clone(),
            });
        }
    }
    if let Some(c) = cur.take() {
        if !c.content.trim().is_empty() || !c.title.trim().is_empty() {
            chapters.push(c);
        }
    }
    chapters
}

/// docProps/core.xml → (标题, 作者)
fn docx_core_meta(xml: &str) -> (String, String) {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut title = String::new();
    let mut author = String::new();
    let mut in_title = false;
    let mut in_creator = false;
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "title" => in_title = true,
                "creator" => in_creator = true,
                _ => {}
            },
            Ok(quick_xml::events::Event::End(e)) => match std::str::from_utf8(e.local_name().as_ref()).unwrap_or("") {
                "title" => in_title = false,
                "creator" => in_creator = false,
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(t)) => {
                if let Ok(s) = t.xml10_content() {
                    if in_title {
                        title.push_str(s.trim());
                    } else if in_creator {
                        author.push_str(s.trim());
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            _ => {}
        }
    }
    (title.trim().to_string(), author.trim().to_string())
}

/// 判断是否本地书（local:// 或文件路径型 legacy 本地书）
pub fn is_local_book(book_url: &str, origin: &str) -> bool {
    book_url.starts_with("local://")
        || origin == "loc_book"
        || book_url.starts_with("storage/")
        || has_supported_ext(book_url)
}

/// 支持的本地书扩展名白名单（上传 / getBookToc / getBookContent 分派共用）
pub const SUPPORTED_EXTENSIONS: &[&str] = &["epub", "txt", "mobi", "azw3", "pdf", "fb2", "docx"];

/// 取文件名/路径的小写扩展名（不含点；无扩展名返回空串）
pub fn file_ext(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// 路径是否带白名单扩展名（大小写不敏感）
fn has_supported_ext(name: &str) -> bool {
    let ext = file_ext(name);
    !ext.is_empty() && SUPPORTED_EXTENSIONS.contains(&ext.as_str())
}

/// 按扩展名分派解析（bytes 版本；扩展名小写、不含点）
pub fn parse_file_bytes(bytes: &[u8], ext: &str, user_rules: &[String]) -> Result<ImportedBook> {
    match ext {
        "epub" => parse_epub(bytes),
        "txt" => parse_txt_with_rules(bytes, user_rules),
        "mobi" => parse_mobi(bytes),
        "azw3" => parse_azw3(bytes),
        "pdf" => parse_pdf(bytes),
        "fb2" => parse_fb2(bytes),
        "docx" => parse_docx(bytes),
        other => anyhow::bail!("不支持的格式：{other}"),
    }
}

/// 按文件扩展名分派解析（路径版本；getBookToc/getBookContent 的 loc_book 分支共用）
pub fn parse_loc_book_path(path: &std::path::Path, user_rules: &[String]) -> Result<ImportedBook> {
    let bytes = std::fs::read(path)?;
    let ext = file_ext(&path.to_string_lossy());
    parse_file_bytes(&bytes, &ext, user_rules)
}

// ---------- 工具 ----------

fn read_zip<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Vec<u8>> {
    let mut f = zip.by_name(path)?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut buf)?;
    Ok(buf)
}

/// 提取第一个指定属性（如 <rootfile full-path="content.opf">）
fn extract_attr_simple(xml: &str, tag: &str, attr: &str) -> Option<String> {
    // 匹配 <tag 后跟 空格/>/换行（排除 <tags> 等更长标签名）
    let idx = xml.find(&format!("<{tag} ")).or_else(|| xml.find(&format!("<{tag}>")))?;
    let rest = &xml[idx..];
    let end = rest.find('>')?;
    let block = &rest[..end];
    let pat = format!("{attr}=\"");
    let pat2 = format!("{attr}='");
    if let Some(i) = block.find(&pat) {
        return block[i + pat.len()..].split('"').next().map(str::to_string);
    }
    if let Some(i) = block.find(&pat2) {
        return block[i + pat2.len()..].split('\'').next().map(str::to_string);
    }
    None
}

/// 提取所有 itemref 的 idref
fn extract_all_attr(xml: &str, tag: &str, attr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(idx) = rest.find(&format!("<{tag}")) {
        let after = &rest[idx..];
        let Some(end) = after.find('>') else { break };
        let block = &after[..end];
        let pat = format!("{attr}=\"");
        if let Some(i) = block.find(&pat) {
            if let Some(v) = block[i + pat.len()..].split('"').next() {
                out.push(v.to_string());
            }
        }
        rest = &after[end + 1..];
    }
    out
}

/// manifest：id → (href, mediatype)
fn extract_manifest(xml: &str) -> std::collections::HashMap<String, (String, String)> {
    let mut map = std::collections::HashMap::new();
    let mut rest = xml;
    while let Some(idx) = rest.find("<item") {
        let after = &rest[idx..];
        let Some(end) = after.find('>') else { break };
        let block = &after[..end];
        let id = attr_value(block, "id");
        let href = attr_value(block, "href");
        let mediatype = attr_value(block, "media-type");
        if let (Some(id), Some(href)) = (id, href) {
            map.insert(id, (href, mediatype.unwrap_or_default()));
        }
        rest = &after[end + 1..];
    }
    map
}

fn attr_value(block: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=\"");
    let pat2 = format!("{attr}='");
    if let Some(i) = block.find(&pat) {
        return block[i + pat.len()..].split('"').next().map(str::to_string);
    }
    if let Some(i) = block.find(&pat2) {
        return block[i + pat2.len()..].split('\'').next().map(str::to_string);
    }
    None
}

/// OPF 相对路径 → zip 全路径
fn resolve_opf_path(opf_path: &str, href: &str) -> String {
    let href_clean = href.split('#').next().unwrap_or(href);
    if let Some(idx) = opf_path.rfind('/') {
        format!("{}/{}", &opf_path[..idx], href_clean)
    } else {
        href_clean.to_string()
    }
}

/// XHTML → 纯文本（保留段落）
fn html_to_text(html: &str) -> String {
    let doc = scraper::Html::parse_document(html);
    let mut parts = Vec::new();
    for el in doc.root_element().descendants() {
        if let scraper::node::Node::Element(e) = el.value() {
            // 跳过样式/脚本（EPUB 封面/内嵌 CSS 噪音）
            if matches!(e.name(), "style" | "script") {
                continue;
            }
            if matches!(e.name(), "p" | "div" | "h1" | "h2" | "h3" | "br" | "li") {
                let text = el
                    .descendants()
                    .filter_map(|d| match d.value() {
                        scraper::node::Node::Text(t) => Some(t.text.trim().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("")
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
    }
    if parts.is_empty() {
        // fallback：body 全部文本
        return doc.root_element().text().collect::<String>().trim().to_string();
    }
    parts.join("\n\n")
}

/// 提取 <title>（优先 h1/h2，其次 head title）
fn extract_title(html: &str) -> Option<String> {
    let doc = scraper::Html::parse_document(html);
    for sel in ["h1", "h2", "h3", "title"] {
        if let Ok(selector) = scraper::Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                let t = el.text().collect::<String>().trim().to_string();
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_epub_dir() {
        let p = "C:/Users/chong/pr-review/reader-dev/target/search-test/storage/data/transwarp/狼爱似火_迷羊/狼爱似火.epub/index.epub";
        let bytes = std::fs::read(p).expect("read");
        match parse_epub(&bytes) {
            Ok(b) => println!("OK: {} 章, title={}", b.chapters.len(), b.meta.title),
            Err(e) => println!("ERR: {e}"),
        }
    }

    const SAMPLE: &str = "第一章 起点\n内容一。\n第二章 成长\n内容二。\n尾声\n结局。";

    /// 默认规则分章：第X章 + 尾声
    #[test]
    fn test_parse_txt_default_rules() {
        let book = parse_txt(SAMPLE.as_bytes()).unwrap();
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长", "尾声"]);
        assert_eq!(book.chapters[1].content, "内容二。");
        assert_eq!(book.chapters[2].content, "结局。");
    }

    /// 用户自定义规则分章（规则传入时替代默认规则）
    #[test]
    fn test_parse_txt_custom_rules() {
        // 用户规则只匹配「第X章」（不匹配尾声）→ 尾声并入上一章
        let rules = vec![r"^\s*第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*[章节卷回集部篇].*".to_string()];
        let book = parse_txt_with_rules(SAMPLE.as_bytes(), &rules).unwrap();
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"]);
        assert_eq!(book.chapters[1].content, "内容二。\n尾声\n结局。");
    }

    /// 空规则列表回退默认规则
    #[test]
    fn test_parse_txt_empty_rules_falls_back() {
        let book = parse_txt_with_rules(SAMPLE.as_bytes(), &[]).unwrap();
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长", "尾声"]);
    }

    /// 无章节标记长文本按 10000 字分块
    #[test]
    fn test_parse_txt_long_text_chunked() {
        let body = "字".repeat(25_000);
        let book = parse_txt(body.as_bytes()).unwrap();
        assert_eq!(book.chapters.len(), 3);
        assert!(book.chapters.iter().all(|c| c.title.starts_with("第 ") && c.title.ends_with(" 部分")));
    }

    /// GBK 编码文本可解析
    #[test]
    fn test_parse_txt_gbk() {
        let text = "第一章 测试\n内容。";
        let (gbk_bytes, _, _) = encoding_rs::GBK.encode(text);
        let book = parse_txt(&gbk_bytes).unwrap();
        assert_eq!(book.chapters[0].title, "第一章 测试");
        assert_eq!(book.chapters[0].content, "内容。");
    }

    // ---------------- 新格式：MOBI/AZW3 ----------------

    /// 损坏数据：错误友好（提示 MOBI/AZW3 而非 panic）
    #[test]
    fn test_parse_mobi_garbage_friendly_error() {
        let err = parse_mobi(b"not a mobi file at all").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("MOBI") || msg.contains("mobi"), "错误应提及 MOBI：{msg}");
        assert!(parse_azw3(b"garbage").is_err(), "azw3 兼容层对垃圾数据应报错");
    }

    /// 分派：mobi/azw3 走兼容层，未知扩展名报“不支持的格式”
    #[test]
    fn test_parse_file_bytes_dispatch() {
        assert!(parse_file_bytes(b"x", "mobi", &[]).is_err());
        assert!(parse_file_bytes(b"x", "azw3", &[]).is_err());
        let err = parse_file_bytes(b"x", "epub", &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("EPUB") || msg.contains("zip"), "EPUB 错误应友好：{msg}");
        let err = parse_file_bytes(b"x", "rar", &[]).unwrap_err();
        assert!(format!("{err:#}").contains("不支持的格式"));
    }

    /// 小样本 MOBI：手工构造最小 PalmDB（mobi7 未压缩文本记录）
    /// 布局：PDB header(78B) + 记录表(3×8B + extra 2B) + 记录0（PalmDocHeader 16B +
    ///   MobiHeader 232B + 名字 8B） + 记录1（正文 HTML） + 记录2（尾部占位）
    /// 注：mobi crate 的 RawRecords::range 会排除末条记录（end = (b-1).min(len-1)），
    /// 故 first_non_book_index 需指向第 3 条（1..3 才包含记录 1）。
    #[test]
    fn test_parse_mobi_minimal_sample() {
        let html: &[u8] = "<html><body><p>第一章 起点</p><mbp:pagebreak/><p>内容一。</p><mbp:pagebreak/><p>第二章 成长</p><mbp:pagebreak/><p>内容二。</p></body></html>".as_bytes();
        let mut pdb = Vec::new();
        // PDB header（78B）：name(32) attributes(2) version(2) created(4) modified(4)
        //   backup(4) modnum(4) app_info(4) sort_info(4) type(4) creator(4) uid(4) next(4) num_records(2)
        pdb.extend_from_slice(b"TestBook\0");
        pdb.resize(32, 0);
        pdb.extend_from_slice(&[0, 0, 0, 0]); // attributes + version
        pdb.extend_from_slice(&[0u8; 12]); // created + modified + backup
        pdb.extend_from_slice(&[0u8; 8]); // modnum + app_info
        pdb.extend_from_slice(&[0u8; 4]); // sort_info
        pdb.extend_from_slice(b"BOOK"); // type
        pdb.extend_from_slice(b"READ"); // creator
        pdb.extend_from_slice(&[0u8; 8]); // uid + next
        pdb.extend_from_slice(&3u16.to_be_bytes()); // num_records
        assert_eq!(pdb.len(), 78);
        // 记录表：3 条（offset + id）+ extra_bytes(2)
        let rec0_off = 78 + 8 * 3 + 2; // 104
        let rec0_len = 16 + 232 + 8 + 8; // PalmDocHeader + MobiHeader + 填充 + 名字
        let rec1_off = rec0_off + rec0_len;
        let rec2_off = rec1_off + html.len();
        for off in [rec0_off, rec1_off, rec2_off] {
            pdb.extend_from_slice(&(off as u32).to_be_bytes());
            pdb.extend_from_slice(&[0u8; 4]);
        }
        pdb.extend_from_slice(&[0u8; 2]); // extra_bytes
        assert_eq!(pdb.len(), rec0_off);
        // 记录 0：PalmDocHeader（16B）——compression=1（No，未压缩）
        pdb.extend_from_slice(&1u16.to_be_bytes());
        pdb.extend_from_slice(&[0u8; 2]);
        pdb.extend_from_slice(&(html.len() as u32).to_be_bytes()); // text_length
        pdb.extend_from_slice(&3u16.to_be_bytes()); // record_count
        pdb.extend_from_slice(&4096u16.to_be_bytes()); // record_size
        pdb.extend_from_slice(&[0u8; 4]); // encryption(0) + unused
        // MobiHeader（232B）："MOBI" + header_length + 224B payload
        pdb.extend_from_slice(b"MOBI");
        pdb.extend_from_slice(&232u32.to_be_bytes());
        let mut mobi = vec![0u8; 224];
        let mut put = |off: usize, bytes: &[u8]| {
            mobi[off..off + bytes.len()].copy_from_slice(bytes);
        };
        put(0, &2u32.to_be_bytes()); // mobi_type = MobiPocketBook
        put(4, &65001u32.to_be_bytes()); // text_encoding = UTF-8
        put(56, &3u32.to_be_bytes()); // first_non_book_index（可读文本 = 1..3 → 记录 1）
        put(60, &256u32.to_be_bytes()); // name_offset（记录 0 内偏移）
        put(64, &8u32.to_be_bytes()); // name_length
        put(84, &3u32.to_be_bytes()); // first_image_index（无图片）
        put(168, &1u16.to_be_bytes()); // first_content_record
        pdb.extend_from_slice(&mobi);
        pdb.extend_from_slice(&[0u8; 8]); // 填充到 name_offset
        pdb.extend_from_slice(b"TestBook"); // 书名（8B）
        assert_eq!(pdb.len(), rec1_off);
        // 记录 1：正文 HTML
        pdb.extend_from_slice(html);
        // 记录 2：尾部占位（空）
        pdb.extend_from_slice(&[]);
        assert_eq!(pdb.len(), rec2_off);
        let book = parse_mobi(&pdb).unwrap();
        assert_eq!(book.meta.title, "TestBook");
        assert!(!book.chapters.is_empty(), "应解析出章节");
        let joined: String = book
            .chapters
            .iter()
            .map(|c| c.title.clone() + &c.content)
            .collect();
        assert!(
            joined.contains("内容一") && joined.contains("内容二"),
            "应提取到正文：{joined}"
        );
    }

    // ---------------- PDF ----------------

    /// 损坏数据：错误友好（提示 PDF）
    #[test]
    fn test_parse_pdf_garbage_friendly_error() {
        let err = parse_pdf(b"%PDF-1.4 this is not a real pdf").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("PDF"), "错误应提及 PDF：{msg}");
    }

    /// 页分章：无标题规则时按页分章（每页一章，空页跳过）
    #[test]
    fn test_chapters_from_pages_page_split() {
        let pages = vec!["第一页内容".into(), "".into(), "第二页内容".into()];
        let chapters = chapters_from_pages(pages);
        let titles: Vec<&str> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第 1 页", "第 3 页"]);
        assert_eq!(chapters[0].content, "第一页内容");
    }

    /// 标题分章：跨页出现“第一章/第二章”时按标题分章而非按页（标题前内容归入“正文”章）
    #[test]
    fn test_chapters_from_pages_title_split() {
        let pages = vec!["序言\n第一章 起点\n内容一。".into(), "第二章 成长\n内容二。".into()];
        let chapters = chapters_from_pages(pages);
        let titles: Vec<&str> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["正文", "第一章 起点", "第二章 成长"]);
    }

    /// PDF 元数据字符串解码（UTF-16BE / UTF-8 BOM / 纯 ASCII / 错误）
    #[test]
    fn test_pdf_meta_string_decode() {
        use lopdf::Object;
        let utf16 = Object::String(b"\xFE\xFF\x00T\x00e\x00s\x00t".to_vec(), lopdf::StringFormat::Literal);
        assert_eq!(pdf_meta_string(Ok(&utf16)), "Test");
        let utf8 = Object::String(b"\xEF\xBB\xBF\xE4\xB9\xA6".to_vec(), lopdf::StringFormat::Literal);
        assert_eq!(pdf_meta_string(Ok(&utf8)), "书");
        let plain = Object::String(b"plain".to_vec(), lopdf::StringFormat::Literal);
        assert_eq!(pdf_meta_string(Ok(&plain)), "plain");
        assert_eq!(pdf_meta_string(Err(lopdf::Error::DictKey("x".into()))), "");
    }

    // ---------------- FB2 ----------------

    /// 小样本 FB2：description（书名/作者/简介）+ body 两个 section → 分章
    #[test]
    fn test_parse_fb2_sample() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" lang="zh">
  <description>
    <title-info>
      <genre>fantasy</genre>
      <author><first-name>刘</first-name><last-name>慈欣</last-name></author>
      <book-title>三体</book-title>
      <annotation><p>黑暗森林法则。</p></annotation>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>第一章 起点</p></title>
      <p>内容一。</p>
    </section>
    <section>
      <title><p>第二章 成长</p></title>
      <p>内容二。</p>
      <p>第二段。</p>
    </section>
  </body>
</FictionBook>"#;
        let book = parse_fb2(xml.as_bytes()).unwrap();
        assert_eq!(book.meta.title, "三体");
        assert_eq!(book.meta.author, "刘 慈欣");
        assert_eq!(book.meta.language.as_deref(), Some("zh"));
        assert!(book.meta.description.as_deref().unwrap().contains("黑暗森林"));
        assert_eq!(book.format, "fb2");
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"]);
        assert!(book.chapters[1].content.contains("内容二") && book.chapters[1].content.contains("第二段"));
    }

    /// FB2 实体引用（&amp; 等）正确解码
    #[test]
    fn test_parse_fb2_entities() {
        let xml = r#"<FictionBook><description><title-info><book-title>A &amp; B</book-title></title-info></description><body><section><title><p>第 1 节</p></title><p>1 &lt; 2 &amp;&amp; 3</p></section></body></FictionBook>"#;
        let book = parse_fb2(xml.as_bytes()).unwrap();
        assert_eq!(book.meta.title, "A & B");
        assert!(book.chapters[0].content.contains("1 < 2 && 3"));
    }

    /// FB2 损坏/空数据：错误友好
    #[test]
    fn test_parse_fb2_garbage_friendly_error() {
        // 空 body（无 section）→ “未解析到章节内容”
        let err = parse_fb2(b"<FictionBook><description/><body/></FictionBook>").unwrap_err();
        assert!(format!("{err:#}").contains("FB2"), "应提示 FB2");
        // 标签不匹配 → XML 解析错误
        let err = parse_fb2(b"<FictionBook><body><section></FictionBook>").unwrap_err();
        assert!(format!("{err:#}").contains("FB2"), "应提示 FB2");
    }

    // ---------------- DOCX ----------------

    /// 小样本 DOCX：内存构造 zip（word/document.xml 含 Heading1 段落）→ 标题样式分章
    #[test]
    fn test_parse_docx_sample() {
        use std::io::Write;
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>第一章 起点</w:t></w:r></w:p>
    <w:p><w:r><w:t>内容一。</w:t></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>第二章 成长</w:t></w:r></w:p>
    <w:p><w:r><w:t>内容二。</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let core_xml = r#"<?xml version="1.0"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>测试书</dc:title><dc:creator>作者甲</dc:creator></cp:coreProperties>"#;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::FileOptions::default();
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(document_xml.as_bytes()).unwrap();
            zw.start_file("docProps/core.xml", opts).unwrap();
            zw.write_all(core_xml.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let bytes = cursor.into_inner();
        let book = parse_docx(&bytes).unwrap();
        assert_eq!(book.meta.title, "测试书");
        assert_eq!(book.meta.author, "作者甲");
        assert_eq!(book.format, "docx");
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"]);
        assert_eq!(book.chapters[1].content, "内容二。");
    }

    /// DOCX 无标题样式：回退纯文本规则分章（第X章 段落）
    #[test]
    fn test_parse_docx_no_heading_falls_back_to_rules() {
        use std::io::Write;
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>第一章 起点</w:t></w:r></w:p>
    <w:p><w:r><w:t>内容一。</w:t></w:r></w:p>
    <w:p><w:r><w:t>第二章 成长</w:t></w:r></w:p>
    <w:p><w:r><w:t>内容二。</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::FileOptions::default();
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(document_xml.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let book = parse_docx(&cursor.into_inner()).unwrap();
        let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["第一章 起点", "第二章 成长"]);
    }

    /// DOCX 损坏数据：错误友好
    #[test]
    fn test_parse_docx_garbage_friendly_error() {
        let err = parse_docx(b"not a zip").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("DOCX") || msg.contains("zip"), "错误应提及 DOCX/zip：{msg}");
        // 合法 zip 但缺 document.xml
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut cursor);
            zw.start_file("other.txt", zip::write::FileOptions::default()).unwrap();
            zw.write_all(b"x").unwrap();
            zw.finish().unwrap();
        }
        let err = parse_docx(&cursor.into_inner()).unwrap_err();
        assert!(format!("{err:#}").contains("document.xml"), "应提示缺少 document.xml");
    }

    /// 扩展名工具与白名单
    #[test]
    fn test_file_ext_and_whitelist() {
        assert_eq!(file_ext("book.PDF"), "pdf");
        assert_eq!(file_ext("book.azw3"), "azw3");
        assert_eq!(file_ext("book"), "");
        assert!(SUPPORTED_EXTENSIONS.contains(&"mobi"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"fb2"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"docx"));
        assert!(is_local_book("storage/data/x/book.mobi", ""));
        assert!(is_local_book("C:/tmp/book.fb2", ""));
        assert!(!is_local_book("https://a.com/book", ""));
    }
}

