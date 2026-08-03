//! 本地书籍导入（EPUB / TXT）
//!
//! - EPUB：zip 解包 → container.xml → OPF 元数据 → spine 章节（XHTML → 纯文本）→ 封面
//! - TXT：编码检测（UTF-8/GBK）→ 分章（章节标题正则）

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
    /// 格式（epub/txt）
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
    if chapters.is_empty() && !text.trim().is_empty() {
        // 无章节标记的长文本：按 10000 字分章（避免单章过大渲染卡顿）
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

/// 判断是否本地书（local:// 或文件路径型 legacy 本地书）
pub fn is_local_book(book_url: &str, origin: &str) -> bool {
    book_url.starts_with("local://")
        || origin == "loc_book"
        || book_url.starts_with("storage/")
        || book_url.ends_with(".txt")
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
}
