//! 多格式导出（exportBook）：TXT / EPUB / HTML 构造器（纯函数，可测）
//!
//! - txt：书名 + 逐章拼接（标题 + 正文）
//! - epub：zip 构造（mimetype 存根 / META-INF/container.xml / OEBPS/content.opf / spine 章节 XHTML）
//! - html：单页（标题 + 章节标题/正文段落）

use std::io::Write;

use futures::StreamExt as _; // buffer_unordered（GAP 104b 并发抓章）

/// 导出章节（标题 + 正文）
pub struct ExportChapter {
    pub title: String,
    pub content: String,
}

/// TXT 导出：书名 + 章节（标题行 + 正文）
pub fn build_txt(title: &str, chapters: &[ExportChapter]) -> String {
    let mut out = String::new();
    if !title.is_empty() {
        out.push_str(title.trim());
        out.push_str("\n\n");
    }
    for ch in chapters {
        if !ch.title.trim().is_empty() {
            out.push_str(ch.title.trim());
            out.push('\n');
        }
        out.push_str(ch.content.trim());
        out.push_str("\n\n");
    }
    out
}

/// HTML 导出：单页（<h1> 书名 + 每章 <h2>/<p>）
pub fn build_html(title: &str, chapters: &[ExportChapter]) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str(&format!("<title>{}</title>\n", escape_html(title)));
    out.push_str("<style>body{max-width:720px;margin:2em auto;padding:0 1em;line-height:1.8;font-size:16px;}h1,h2{text-align:center;}p{text-indent:2em;}</style>\n");
    out.push_str("</head>\n<body>\n");
    if !title.is_empty() {
        out.push_str(&format!("<h1>{}</h1>\n", escape_html(title)));
    }
    for ch in chapters {
        if !ch.title.trim().is_empty() {
            out.push_str(&format!("<h2>{}</h2>\n", escape_html(ch.title.trim())));
        }
        for para in ch.content.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str(&format!("<p>{}</p>\n", escape_html(para.trim())));
        }
    }
    out.push_str("</body>\n</html>\n");
    out
}

/// EPUB 导出：zip（mimetype 必须 Stored 且为首个条目）
///
/// 兼容入口：仅标题/作者/语言的基础元数据（API 导出用）。
/// 双轨同步落盘请用 [`build_epub_full`]（GAP 173：全量元数据零丢失）。
pub fn build_epub(title: &str, author: &str, chapters: &[ExportChapter]) -> Vec<u8> {
    build_epub_full(title, author, &EpubMeta::default(), chapters)
}

/// EPUB 全量元数据（GAP 173：本地书双轨同步自动生成的 epub 必须携带全量元数据，
/// 保证重新导入零丢失——dc:title/creator/description/language/date/publisher/subject/封面）
#[derive(Debug, Clone, Default)]
pub struct EpubMeta {
    /// dc:description（对账生成时取 custom_intro 优先，其次 intro）
    pub description: Option<String>,
    /// dc:language（缺省 zh-CN）
    pub language: Option<String>,
    /// dc:date（出版时间）
    pub published_at: Option<String>,
    /// dc:publisher（出版社）
    pub publisher: Option<String>,
    /// dc:subject（对账生成时取 custom_tag 优先，其次 kind）
    pub subject: Option<String>,
    /// 封面字节（嵌入 OEBPS/cover.{jpg,png} + manifest properties="cover-image" + meta cover）
    pub cover: Option<Vec<u8>>,
}

/// EPUB 导出（全量元数据版本）：zip（mimetype Stored 且为首个条目）
///
/// 元数据完备性（parse_opf 可全部读回）：dc:title / dc:creator / dc:language /
/// dc:date / dc:description / dc:publisher / dc:subject；封面通过
/// `manifest item properties="cover-image"` + `<meta name="cover">` 声明
/// （parse_opf 两种方式均可识别），重新 parse_epub 时零丢失。
pub fn build_epub_full(
    title: &str,
    author: &str,
    meta: &EpubMeta,
    chapters: &[ExportChapter],
) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let stored = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // 1. mimetype（首个条目，Stored）
        zip.start_file("mimetype", stored).expect("start mimetype");
        zip.write_all(b"application/epub+zip").expect("write mimetype");

        // 2. container.xml
        zip.start_file("META-INF/container.xml", deflated)
            .expect("start container");
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#,
        )
        .expect("write container");

        // 3. content.opf（manifest + spine + GAP 173 全量元数据）
        let mut manifest = String::new();
        let mut spine = String::new();
        for (i, _ch) in chapters.iter().enumerate() {
            manifest.push_str(&format!(
                "    <item id=\"chap{i}\" href=\"chap_{i:04}.xhtml\" media-type=\"application/xhtml+xml\"/>\n"
            ));
            spine.push_str(&format!("    <itemref idref=\"chap{i}\"/>\n"));
        }
        // 封面条目（manifest properties="cover-image"——parse_opf 可识别）
        let (cover_ext, cover_mediatype) = match &meta.cover {
            Some(c) if c.starts_with(&[0x89, b'P', b'N', b'G']) => ("png", "image/png"),
            Some(_) => ("jpg", "image/jpeg"),
            None => ("jpg", "image/jpeg"),
        };
        let cover_manifest = if meta.cover.is_some() {
            format!(
                "    <item id=\"cover-image\" href=\"cover.{cover_ext}\" media-type=\"{cover_mediatype}\" properties=\"cover-image\"/>\n"
            )
        } else {
            String::new()
        };
        let mut metadata = String::new();
        metadata.push_str(&format!("    <dc:identifier id=\"BookId\">uuid:{uuid}</dc:identifier>\n", uuid = uuid::Uuid::new_v4()));
        metadata.push_str(&format!("    <dc:title>{title}</dc:title>\n", title = escape_xml(title)));
        metadata.push_str(&format!("    <dc:creator>{author}</dc:creator>\n", author = escape_xml(author)));
        metadata.push_str(&format!(
            "    <dc:language>{}</dc:language>\n",
            escape_xml(meta.language.as_deref().unwrap_or("zh-CN"))
        ));
        if let Some(d) = &meta.description {
            if !d.trim().is_empty() {
                metadata.push_str(&format!("    <dc:description>{}</dc:description>\n", escape_xml(d)));
            }
        }
        if let Some(d) = &meta.published_at {
            if !d.trim().is_empty() {
                metadata.push_str(&format!("    <dc:date>{}</dc:date>\n", escape_xml(d)));
            }
        }
        if let Some(p) = &meta.publisher {
            if !p.trim().is_empty() {
                metadata.push_str(&format!("    <dc:publisher>{}</dc:publisher>\n", escape_xml(p)));
            }
        }
        if let Some(s) = &meta.subject {
            if !s.trim().is_empty() {
                metadata.push_str(&format!("    <dc:subject>{}</dc:subject>\n", escape_xml(s)));
            }
        }
        if meta.cover.is_some() {
            metadata.push_str("    <meta name=\"cover\" content=\"cover-image\"/>\n");
        }
        let opf = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
{metadata}  </metadata>
  <manifest>
{manifest}{cover_manifest}  </manifest>
  <spine toc="ncx">
{spine}  </spine>
</package>
"#,
            metadata = metadata,
            manifest = manifest,
            cover_manifest = cover_manifest,
            spine = spine,
        );
        zip.start_file("OEBPS/content.opf", deflated).expect("start opf");
        zip.write_all(opf.as_bytes()).expect("write opf");

        // 3.5 封面文件
        if let Some(cover) = &meta.cover {
            zip.start_file(format!("OEBPS/cover.{cover_ext}"), deflated)
                .expect("start cover");
            zip.write_all(cover).expect("write cover");
        }

        // 4. 章节 XHTML（spine 顺序）
        for (i, ch) in chapters.iter().enumerate() {
            let mut xhtml = String::new();
            xhtml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
            xhtml.push_str("<html xmlns=\"http://www.w3.org/1999/xhtml\">\n<head>\n");
            xhtml.push_str(&format!("<title>{}</title>\n", escape_xml(&ch.title)));
            xhtml.push_str("</head>\n<body>\n");
            if !ch.title.trim().is_empty() {
                xhtml.push_str(&format!("<h1>{}</h1>\n", escape_xml(ch.title.trim())));
            }
            for para in ch.content.lines().filter(|l| !l.trim().is_empty()) {
                xhtml.push_str(&format!("<p>{}</p>\n", escape_xml(para.trim())));
            }
            xhtml.push_str("</body>\n</html>\n");
            zip.start_file(format!("OEBPS/chap_{i:04}.xhtml"), deflated)
                .expect("start chapter");
            zip.write_all(xhtml.as_bytes()).expect("write chapter");
        }

        zip.finish().expect("finish zip");
    }
    buf
}

/// XML 转义（& < > " '）
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// HTML 转义（& < > "）
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ==================== GAP 104 TXT 导出编码 ====================

/// TXT 导出编码（encoding 参数：utf-8/gbk；gb2312 映射 GBK 超集，gb18030 全字符集）
pub fn encode_txt(txt: &str, encoding: &str) -> Result<Vec<u8>, String> {
    let enc = match encoding.trim().to_ascii_lowercase().as_str() {
        "" | "utf-8" | "utf8" | "utf_8" => encoding_rs::UTF_8,
        "gbk" | "gb2312" | "gb_2312" => encoding_rs::GBK,
        "gb18030" => encoding_rs::GB18030,
        other => return Err(format!("不支持的导出编码: {other}（utf-8|gbk|gb2312|gb18030）")),
    };
    let (bytes, _, _) = enc.encode(txt);
    Ok(bytes.into_owned())
}

// ==================== 书源书导出并发抓章（GAP 104b） ====================

/// 并发抓取章节正文：`chapters` 为 (章节标题, 章节 URL) 原始顺序；
/// 并发度 `concurrency`（默认 4）；结果按原章节顺序重组；失败章节跳过（不影响其他章）。
/// 瓶颈在网络抓取——本地书/文件书解析不走此路径（保持原有顺序逻辑）。
pub async fn fetch_chapters_concurrent<F, Fut>(
    chapters: Vec<(String, String)>,
    concurrency: usize,
    fetch: F,
) -> Vec<(String, String)>
where
    F: Fn(usize, String) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<String, String>> + Send,
{
    let results: Vec<(usize, String, Result<String, String>)> = futures::stream::iter(
        chapters
            .into_iter()
            .enumerate()
            .map(|(i, (title, url))| {
                let f = &fetch;
                async move {
                    let r = f(i, url).await;
                    (i, title, r)
                }
            }),
    )
    .buffer_unordered(concurrency.max(1))
    .collect()
    .await;

    // 按章节索引重组（buffer_unordered 完成顺序 ≠ 章节顺序）
    let mut results = results;
    results.sort_by_key(|(i, _, _)| *i);
    results
        .into_iter()
        .filter_map(|(_, title, r)| r.ok().map(|content| (title, content)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sample() -> (String, Vec<ExportChapter>) {
        (
            "测试书".to_string(),
            vec![
                ExportChapter { title: "第一章".into(), content: "正文一 <甲> & 乙。\n第二段。".into() },
                ExportChapter { title: "第二章".into(), content: "正文二。".into() },
            ],
        )
    }

    #[test]
    fn test_build_txt() {
        let (title, chs) = sample();
        let txt = build_txt(&title, &chs);
        assert!(txt.contains("测试书"));
        assert!(txt.contains("第一章"));
        assert!(txt.contains("正文一 <甲> & 乙。"));
        assert!(txt.contains("第二章"));
        assert!(txt.contains("正文二。"));
    }

    #[test]
    fn test_build_html() {
        let (title, chs) = sample();
        let html = build_html(&title, &chs);
        assert!(html.contains("<h1>测试书</h1>"));
        assert!(html.contains("<h2>第一章</h2>"));
        assert!(html.contains("<p>正文一 &lt;甲&gt; &amp; 乙。</p>"), "HTML 转义");
        assert!(html.contains("</html>"));
    }

    #[test]
    fn test_build_epub_valid_zip() {
        let (title, chs) = sample();
        let bytes = build_epub(&title, "作者甲", &chs);
        // zip 可解包
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("EPUB 应为合法 zip");
        // mimetype 条目存在且内容正确
        let mut mime = String::new();
        zip.by_name("mimetype").unwrap().read_to_string(&mut mime).unwrap();
        assert_eq!(mime, "application/epub+zip");
        // container.xml 指向 OEBPS/content.opf
        let mut container = String::new();
        zip.by_name("META-INF/container.xml").unwrap().read_to_string(&mut container).unwrap();
        assert!(container.contains("OEBPS/content.opf"));
        // OPF 含 spine 两章 + 标题
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        assert!(opf.contains("<dc:title>测试书</dc:title>"));
        assert!(opf.contains("<dc:creator>作者甲</dc:creator>"));
        assert!(opf.contains("chap_0000.xhtml"));
        assert!(opf.contains("chap_0001.xhtml"));
        assert_eq!(opf.matches("<itemref").count(), 2);
        // 章节内容（XML 转义）
        let mut ch0 = String::new();
        zip.by_name("OEBPS/chap_0000.xhtml").unwrap().read_to_string(&mut ch0).unwrap();
        assert!(ch0.contains("<h1>第一章</h1>"));
        assert!(ch0.contains("正文一 &lt;甲&gt; &amp; 乙。"), "XML 转义: {ch0}");
        let mut ch1 = String::new();
        zip.by_name("OEBPS/chap_0001.xhtml").unwrap().read_to_string(&mut ch1).unwrap();
        assert!(ch1.contains("正文二。"));
    }

    #[test]
    fn test_build_epub_full_metadata_roundtrip() {
        let (title, chs) = sample();
        let meta = EpubMeta {
            description: Some("简介内容".into()),
            language: Some("en".into()),
            published_at: Some("2023-01-02".into()),
            publisher: Some("出版社".into()),
            subject: Some("标签".into()),
            cover: Some(vec![0xFF, 0xD8, 0xFF, 0xE0]),
        };
        let bytes = build_epub_full(&title, "作者甲", &meta, &chs);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("合法 zip");
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        // GAP 173 全量元数据元素
        assert!(opf.contains("<dc:description>简介内容</dc:description>"));
        assert!(opf.contains("<dc:language>en</dc:language>"));
        assert!(opf.contains("<dc:date>2023-01-02</dc:date>"));
        assert!(opf.contains("<dc:publisher>出版社</dc:publisher>"));
        assert!(opf.contains("<dc:subject>标签</dc:subject>"));
        assert!(opf.contains("properties=\"cover-image\""), "封面 manifest 声明");
        assert!(opf.contains("<meta name=\"cover\" content=\"cover-image\"/>"));
        // 封面文件存在
        assert!(zip.by_name("OEBPS/cover.jpg").is_ok());
        // 重新 parse_opf：零丢失（cover_href 经 properties=cover-image 识别）
        let parsed = crate::service::epub::parse_opf(&opf);
        assert_eq!(parsed.description.as_deref(), Some("简介内容"));
        assert_eq!(parsed.language.as_deref(), Some("en"));
        assert_eq!(parsed.published_at.as_deref(), Some("2023-01-02"));
        assert_eq!(parsed.publisher.as_deref(), Some("出版社"));
        assert_eq!(parsed.subjects, vec!["标签".to_string()]);
        assert!(parsed.cover_href.is_some());
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<a & b>"), "&lt;a &amp; b&gt;");
    }

    // ---------- GAP 104 编码 ----------

    #[test]
    fn test_encode_txt_utf8_and_gbk() {
        let txt = "书名《测试》\n第一章 正文";
        let utf8 = encode_txt(txt, "utf-8").unwrap();
        assert_eq!(utf8, txt.as_bytes());
        // gbk：中文字符 2 字节（UTF-8 3 字节）——编码后长度变小且可解码回原文
        let gbk = encode_txt(txt, "gbk").unwrap();
        assert!(gbk.len() < utf8.len(), "GBK 中文 2 字节: {} < {}", gbk.len(), utf8.len());
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&gbk);
        assert!(!had_errors);
        assert_eq!(decoded, txt);
        // 大小写/别名
        assert_eq!(encode_txt("x", "GB2312").unwrap(), b"x");
        assert_eq!(encode_txt("x", "GB18030").unwrap(), b"x");
        // 不支持的编码 → 明确错误
        let err = encode_txt("x", "latin1").unwrap_err();
        assert!(err.contains("不支持的导出编码"), "{err}");
    }

    // ---------- GAP 104b 并发抓章 ----------

    /// 慢响应 mock：断言并发度 > 1（串行时恒为 1）、不超过上限、结果顺序正确
    #[tokio::test]
    async fn test_fetch_chapters_concurrent_order_and_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let in_flight = AtomicUsize::new(0);
        let max_in_flight = AtomicUsize::new(0);
        let chapters: Vec<(String, String)> = (0..8)
            .map(|i| (format!("第{i}章"), format!("/c/{i}")))
            .collect();
        let fetched = fetch_chapters_concurrent(chapters, 4, |i, url| {
            let in_flight = &in_flight;
            let max_in_flight = &max_in_flight;
            async move {
                // 慢响应（模拟网络延迟，长短不一）
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis((20 + (i % 3) * 15) as u64)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("正文{i}:{url}"))
            }
        })
        .await;

        // 顺序按章节索引重组
        assert_eq!(fetched.len(), 8);
        for (i, (title, content)) in fetched.iter().enumerate() {
            assert_eq!(title, &format!("第{i}章"));
            assert_eq!(content, &format!("正文{i}:/c/{i}"));
        }
        // 确实并发（>1）且不超过 4
        let max = max_in_flight.load(Ordering::SeqCst);
        assert!(max > 1, "应并发抓取（当前 max={max}）");
        assert!(max <= 4, "并发上限 4（当前 max={max}）");
    }

    /// 错误章跳过继续：其余章节按序保留
    #[tokio::test]
    async fn test_fetch_chapters_concurrent_skips_errors() {
        let chapters: Vec<(String, String)> = (0..5)
            .map(|i| (format!("章{i}"), format!("/{i}")))
            .collect();
        let fetched = fetch_chapters_concurrent(chapters, 2, |i, _url| async move {
            if i == 1 || i == 3 {
                Err("网络错误".to_string())
            } else {
                Ok(format!("正文{i}"))
            }
        })
        .await;
        assert_eq!(
            fetched,
            vec![
                ("章0".to_string(), "正文0".to_string()),
                ("章2".to_string(), "正文2".to_string()),
                ("章4".to_string(), "正文4".to_string()),
            ]
        );
    }
}
