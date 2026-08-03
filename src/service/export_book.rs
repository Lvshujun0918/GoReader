//! 多格式导出（exportBook）：TXT / EPUB / HTML 构造器（纯函数，可测）
//!
//! - txt：书名 + 逐章拼接（标题 + 正文）
//! - epub：zip 构造（mimetype 存根 / META-INF/container.xml / OEBPS/content.opf / spine 章节 XHTML）
//! - html：单页（标题 + 章节标题/正文段落）

use std::io::Write;

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
pub fn build_epub(title: &str, author: &str, chapters: &[ExportChapter]) -> Vec<u8> {
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

        // 3. content.opf（manifest + spine）
        let mut manifest = String::new();
        let mut spine = String::new();
        for (i, _ch) in chapters.iter().enumerate() {
            manifest.push_str(&format!(
                "    <item id=\"chap{i}\" href=\"chap_{i:04}.xhtml\" media-type=\"application/xhtml+xml\"/>\n"
            ));
            spine.push_str(&format!("    <itemref idref=\"chap{i}\"/>\n"));
        }
        let opf = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="BookId">uuid:{uuid}</dc:identifier>
    <dc:title>{title}</dc:title>
    <dc:creator>{author}</dc:creator>
    <dc:language>zh-CN</dc:language>
  </metadata>
  <manifest>
{manifest}  </manifest>
  <spine toc="ncx">
{spine}  </spine>
</package>
"#,
            uuid = uuid::Uuid::new_v4(),
            title = escape_xml(title),
            author = escape_xml(author),
            manifest = manifest,
            spine = spine,
        );
        zip.start_file("OEBPS/content.opf", deflated).expect("start opf");
        zip.write_all(opf.as_bytes()).expect("write opf");

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
    fn test_escape_xml() {
        assert_eq!(escape_xml("<a & b>"), "&lt;a &amp; b&gt;");
    }
}
