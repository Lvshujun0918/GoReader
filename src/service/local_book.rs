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

/// TXT 解析（编码检测 + 分章）
pub fn parse_txt(bytes: &[u8]) -> Result<ImportedBook> {
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

    // 分章：常见章节标题模式
    let chapter_re = regex::Regex::new(
        r"(?m)^\s*(第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*[章节卷回集部篇]\s*[^\n]{0,40}|Chapter\s+\d+[^\n]{0,40}|序章[^\n]{0,40}|楔子[^\n]{0,40})\s*$",
    )
    .unwrap();

    let mut chapters = Vec::new();
    let mut last_pos = 0usize;
    let mut last_title = "正文".to_string();
    for cap in chapter_re.captures_iter(&text) {
        let m = cap.get(0).unwrap();
        let content = text[last_pos..m.start()].trim().to_string();
        if !content.is_empty() {
            chapters.push(Chapter {
                title: last_title.clone(),
                content,
            });
        }
        last_title = m.as_str().trim().to_string();
        last_pos = m.end();
    }
    let tail = text[last_pos..].trim().to_string();
    if !tail.is_empty() {
        chapters.push(Chapter {
            title: last_title,
            content: tail,
        });
    }
    if chapters.is_empty() && !text.trim().is_empty() {
        chapters.push(Chapter {
            title: "正文".into(),
            content: text.trim().to_string(),
        });
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
