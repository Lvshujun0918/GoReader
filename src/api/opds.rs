//! OPDS 服务（OPDS 1.2：Atom + OPDS 扩展）
//!
//! 端点：
//! - GET /opds                    根目录（书架 → 书籍条目）
//! - GET /opds/search?q={key}     搜索
//! - GET /opds/books/{id}/download TXT 导出下载（正文拼接）

use anyhow::Result;
use chrono::Utc;

use crate::storage::Storage;

/// 生成 OPDS 根目录（书架书籍列表）
pub async fn catalog(storage: &Storage, ns: &str) -> Result<String> {
    let books = storage.list_books(ns).await?;
    let updated = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:opds=\"http://opds-spec.org/2010/catalog xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\"\">\n",
    );
    xml.push_str(&format!(
        "  <id>urn:uuid:reader-dev-bookshelf-{ns}</id>\n  <title>书架（{ns}）</title>\n  <updated>{updated}</updated>\n"
    ));
    xml.push_str("  <author><name>reader-dev</name></author>\n");
    xml.push_str("  <link rel=\"self\" href=\"/opds\" type=\"application/atom+xml;profile=opds-catalog;kind=acquisition\"/>\n");
    xml.push_str("  <link rel=\"search\" href=\"/opds/search?q={searchTerms}\" type=\"application/atom+xml;profile=opds-catalog;kind=acquisition\"/>\n");

    for book in &books {
        xml.push_str(&book_entry(book, ns));
    }
    xml.push_str("</feed>");
    Ok(xml)
}

/// 搜索书架
pub async fn search(storage: &Storage, ns: &str, q: &str) -> Result<String> {
    let books = storage.list_books(ns).await?;
    let ql = q.to_lowercase();
    let matched: Vec<_> = books
        .iter()
        .filter(|b| {
            b.name.to_lowercase().contains(&ql) || b.author.to_lowercase().contains(&ql)
        })
        .collect();
    let updated = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:opds=\"http://opds-spec.org/2010/catalog xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\"\">\n",
    );
    xml.push_str(&format!(
        "  <id>urn:uuid:reader-dev-search-{ns}</id>\n  <title>搜索：{q}</title>\n  <updated>{updated}</updated>\n  <author><name>reader-dev</name></author>\n"
    ));
    xml.push_str("  <link rel=\"self\" href=\"/opds/search?q={q}\" type=\"application/atom+xml;profile=opds-catalog;kind=acquisition\"/>\n");
    for book in &matched {
        xml.push_str(&book_entry(book, ns));
    }
    xml.push_str("</feed>");
    Ok(xml)
}

fn book_entry(book: &crate::model::Book, ns: &str) -> String {
    let updated = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let id = encode_id(&book.book_url);
    let name_esc = xml_escape(&book.name);
    let author_esc = xml_escape(&book.author);
    let intro_esc = xml_escape(book.intro.as_deref().unwrap_or(""));
    let mut entry = String::from("  <entry>\n");
    entry.push_str(&format!("    <id>urn:uuid:{}</id>\n    <title>{name_esc}</title>\n", id));
    if !book.author.is_empty() {
        entry.push_str(&format!("    <author><name>{author_esc}</name></author>\n"));
    }
    entry.push_str(&format!("    <updated>{updated}</updated>\n"));
    if !intro_esc.is_empty() {
        entry.push_str(&format!("    <content type=\"text\">{intro_esc}</content>\n"));
    }
    if let Some(cover) = &book.custom_cover_url {
        entry.push_str(&format!(
            "    <link rel=\"http://opds-spec.org/cover\" href=\"{}\" type=\"image/jpeg\"/>\n",
            xml_escape(cover)
        ));
    } else if let Some(cover) = &book.cover_url {
        if cover.starts_with('/') || cover.starts_with("http") {
            entry.push_str(&format!(
                "    <link rel=\"http://opds-spec.org/cover\" href=\"{}\" type=\"image/jpeg\"/>\n",
                xml_escape(cover)
            ));
        }
    }
    // OPDS 1.2 元数据：语言/出版时间/出版社（本地书元数据，有则输出）
    if let Some(lang) = &book.language {
        if !lang.is_empty() {
            entry.push_str(&format!("    <dc:language>{}</dc:language>
", xml_escape(lang)));
        }
    }
    if let Some(pub_at) = &book.published_at {
        if !pub_at.is_empty() {
            entry.push_str(&format!("    <dcterms:published>{}</dcterms:published>
", xml_escape(pub_at)));
        }
    }
    if let Some(publisher) = &book.publisher {
        if !publisher.is_empty() {
            entry.push_str(&format!("    <dcterms:publisher>{}</dcterms:publisher>
", xml_escape(publisher)));
        }
    }
    if let Some(kind) = &book.kind {
        if !kind.is_empty() {
            entry.push_str(&format!("    <category term=\"{}\" label=\"{}\" />
",
                xml_escape(kind), xml_escape(kind)));
        }
    }
    // acquisition：TXT 下载
    entry.push_str(&format!(
        "    <link rel=\"http://opds-spec.org/acquisition\" href=\"/opds/download/{id}?format=txt\" type=\"text/plain\"/>\n",
    ));
    let _ = ns;
    entry.push_str("  </entry>\n");
    entry
}

/// 下载：TXT 导出（目录 + 正文拼接，上限章节防超时）
pub async fn download(storage: &Storage, ns: &str, book_id: &str, max_chapters: usize) -> Result<(String, Vec<u8>)> {
    let book_url = decode_id(book_id);
    let book = storage
        .list_books(ns)
        .await?
        .into_iter()
        .find(|b| b.book_url == book_url)
        .ok_or_else(|| anyhow::anyhow!("书籍不存在"))?;

    // 书源
    let source = storage
        .find_book_source(ns, &book.origin)
        .await?
        .ok_or_else(|| anyhow::anyhow!("书源不存在"))?;

    // 目录
    let toc_url = if book.toc_url.is_empty() {
        book.book_url.clone()
    } else {
        book.toc_url.clone()
    };
    let chapters = crate::service::book::analyze_toc(&toc_url, &source, 20).await?;

    // 正文（限前 max_chapters 章）
    let mut txt = String::new();
    txt.push_str(&format!("{}\n{}\n\n", book.name, book.author));
    let mut count = 0usize;
    for ch in chapters.iter().take(max_chapters) {
        if ch.is_volume || ch.url.is_empty() {
            continue;
        }
        match crate::service::book::analyze_content(&ch.url, &source, 5).await {
            Ok(content) => {
                txt.push_str(&format!("\n{}\n\n{}", ch.title, content));
                count += 1;
            }
            Err(e) => {
                tracing::warn!("下载章节失败 {}: {e}", ch.title);
            }
        }
    }
    tracing::info!("OPDS 下载 [{ns}] {}：{count} 章", book.name);
    Ok((format!("{}.txt", book.name), txt.into_bytes()))
}

/// bookUrl → base64url（URL 安全，无 / 等特殊字符——Path 单段可匹配）
fn encode_id(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

fn decode_id(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
