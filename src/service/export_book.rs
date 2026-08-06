//! 多格式导出（exportBook）：TXT / EPUB / HTML 构造器（纯函数，可测）
//!
//! - txt：书名 + 逐章拼接（标题 + 正文）
//! - epub：zip 构造（mimetype 存根 / META-INF/container.xml / OEBPS/content.opf / spine 章节 XHTML）
//! - html：单页（标题 + 章节标题/正文段落）
//!
//! 内嵌中文字体（GAP 176）：`EpubMeta.font` 指定后，epub 内嵌 web-ui/public/fonts/ 的
//! woff2 子集（编译期 include_bytes——单源无拷贝，路径相对本文件：src/service → 仓库根），
//! OPF manifest 声明字体条目 + style.css
//! @font-face 引用 + 字体文件写入 OEBPS/fonts/，章节 XHTML 链接 style.css。

use std::io::Write;

use futures::StreamExt as _; // buffer_unordered（GAP 104b 并发抓章）

/// 导出章节（标题 + 正文）
pub struct ExportChapter {
    pub title: String,
    pub content: String,
}

/// EPUB 内嵌中文字体（GAP 176）：web-ui/public/fonts/ 现有 woff2 子集（编译期内嵌，单源无拷贝）
///
/// 格式结论：项目仅有 woff2（无 ttf/otf 来源）。woff2 需 EPUB3 时代阅读器
/// （Apple Books / Google Play Books / Kobo / ADE 4.5+ / 新版 Kindle 均支持）；
/// Kindle 老设备（MOBI/早期 KF8）与不支持字体内嵌的阅读器会忽略 @font-face，
/// 自动回退 font-family 栈里的系统字体（不影响可读性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedFont {
    /// 不内嵌字体（默认）
    #[default]
    None,
    /// 霞鹜文楷（LXGW WenKai Regular，楷体风格）
    LxgwWenKai,
    /// 思源宋体（Source Han Serif CN Regular，宋体风格）
    SourceHanSerif,
}

impl EmbedFont {
    /// 解析 exportBook 的 font 参数（none|lxk-wenkai|source-han-serif；空串/缺省 = none；大小写不敏感）
    pub fn parse_param(s: &str) -> Result<EmbedFont, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" => Ok(EmbedFont::None),
            "lxk-wenkai" => Ok(EmbedFont::LxgwWenKai),
            "source-han-serif" => Ok(EmbedFont::SourceHanSerif),
            other => Err(format!(
                "不支持的字体（none|lxk-wenkai|source-han-serif）：{other}"
            )),
        }
    }

    /// @font-face 的 font-family 名（与 web-ui 阅读器一致）
    pub fn family(self) -> Option<&'static str> {
        match self {
            EmbedFont::None => None,
            EmbedFont::LxgwWenKai => Some("LXGW WenKai"),
            EmbedFont::SourceHanSerif => Some("Source Han Serif CN"),
        }
    }

    /// zip 内相对路径（OEBPS/ 下；CSS url() 与 OPF href 共用）
    pub fn href(self) -> Option<&'static str> {
        match self {
            EmbedFont::None => None,
            EmbedFont::LxgwWenKai => Some("fonts/lxgw-wenkai-regular.woff2"),
            EmbedFont::SourceHanSerif => Some("fonts/source-han-serif-cn-regular.woff2"),
        }
    }

    /// 正文 font-family 栈（内嵌字体优先，兜底系统字体）
    pub fn css_family_stack(self) -> Option<&'static str> {
        match self {
            EmbedFont::None => None,
            EmbedFont::LxgwWenKai => Some("'LXGW WenKai', 'Kaiti SC', '楷体', serif"),
            EmbedFont::SourceHanSerif => {
                Some("'Source Han Serif CN', 'Songti SC', 'SimSun', '宋体', serif")
            }
        }
    }

    /// 字体字节（编译期内嵌 web-ui/public/fonts/——构建时 web-ui 必须在仓库内，Dockerfile 已同步拷贝）
    pub fn bytes(self) -> Option<&'static [u8]> {
        match self {
            EmbedFont::None => None,
            EmbedFont::LxgwWenKai => Some(include_bytes!(
                "../../web-ui/public/fonts/lxgw-wenkai-regular.woff2"
            )),
            EmbedFont::SourceHanSerif => Some(include_bytes!(
                "../../web-ui/public/fonts/source-han-serif-cn-regular.woff2"
            )),
        }
    }
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
    /// 内嵌中文字体（GAP 176：默认 None 不内嵌；指定后 OPF manifest 字体条目 + style.css
    /// @font-face + OEBPS/fonts/*.woff2 文件 + 章节 XHTML 链接样式表）
    pub font: EmbedFont,
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
        let stored =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        // 1. mimetype（首个条目，Stored）
        zip.start_file("mimetype", stored).expect("start mimetype");
        zip.write_all(b"application/epub+zip")
            .expect("write mimetype");

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
        // uuid 全局唯一：OPF dc:identifier 与 NCX dtb:uid 共用同一值（EPUB 规范要求一致）
        let uuid = uuid::Uuid::new_v4();
        // 章节 href 表（spine 顺序）——manifest / nav.xhtml / toc.ncx 共用，保证目录跳转与 spine 一一对应
        let chapter_hrefs: Vec<String> = (0..chapters.len())
            .map(|i| format!("chap_{i:04}.xhtml"))
            .collect();
        let mut manifest = String::new();
        let mut spine = String::new();
        for (i, href) in chapter_hrefs.iter().enumerate() {
            manifest.push_str(&format!(
                "    <item id=\"chap{i}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>\n"
            ));
            spine.push_str(&format!("    <itemref idref=\"chap{i}\"/>\n"));
        }
        // 目录导航双格式：nav.xhtml（EPUB3 导航文档，manifest properties="nav"）+
        // toc.ncx（EPUB2 NCX——Kindle 等老阅读器靠它出目录；spine toc="ncx" 指向其 id）
        manifest.push_str(
            "    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n",
        );
        manifest.push_str(
            "    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
        );
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
        // GAP 176 内嵌字体：OPF manifest 字体条目（properties="font-face"——EPUB3 阅读器
        // 识别；EPUB2 阅读器忽略该属性但 CSS @font-face 仍生效）+ style.css 条目
        let (font_manifest, style_manifest, font_css) = if meta.font != EmbedFont::None {
            let font_href = meta.font.href().expect("非 None 字体必有 href");
            let style = "    <item id=\"style\" href=\"style.css\" media-type=\"text/css\"/>\n";
            let css = format!(
                "/* GAP 176 内嵌字体（woff2 子集——EPUB3 阅读器支持；老设备忽略 @font-face 自动回退系统字体） */\n@font-face {{\n  font-family: '{}';\n  src: url('{font_href}') format('woff2');\n  font-weight: 400;\n  font-display: swap;\n}}\nbody {{\n  font-family: {};\n}}\n",
                meta.font.family().expect("非 None 字体必有 family"),
                meta.font.css_family_stack().expect("非 None 字体必有栈")
            );
            (
                format!(
                    "    <item id=\"font-embedded\" href=\"{font_href}\" media-type=\"font/woff2\" properties=\"font-face\"/>\n"
                ),
                style.to_string(),
                css,
            )
        } else {
            (String::new(), String::new(), String::new())
        };
        let mut metadata = String::new();
        metadata.push_str(&format!(
            "    <dc:identifier id=\"BookId\">uuid:{uuid}</dc:identifier>\n"
        ));
        metadata.push_str(&format!(
            "    <dc:title>{title}</dc:title>\n",
            title = escape_xml(title)
        ));
        metadata.push_str(&format!(
            "    <dc:creator>{author}</dc:creator>\n",
            author = escape_xml(author)
        ));
        metadata.push_str(&format!(
            "    <dc:language>{}</dc:language>\n",
            escape_xml(meta.language.as_deref().unwrap_or("zh-CN"))
        ));
        if let Some(d) = &meta.description {
            if !d.trim().is_empty() {
                metadata.push_str(&format!(
                    "    <dc:description>{}</dc:description>\n",
                    escape_xml(d)
                ));
            }
        }
        if let Some(d) = &meta.published_at {
            if !d.trim().is_empty() {
                metadata.push_str(&format!("    <dc:date>{}</dc:date>\n", escape_xml(d)));
            }
        }
        if let Some(p) = &meta.publisher {
            if !p.trim().is_empty() {
                metadata.push_str(&format!(
                    "    <dc:publisher>{}</dc:publisher>\n",
                    escape_xml(p)
                ));
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
{manifest}{cover_manifest}{font_manifest}{style_manifest}  </manifest>
  <spine toc="ncx">
{spine}  </spine>
</package>
"#,
            metadata = metadata,
            manifest = manifest,
            cover_manifest = cover_manifest,
            font_manifest = font_manifest,
            style_manifest = style_manifest,
            spine = spine,
        );
        zip.start_file("OEBPS/content.opf", deflated)
            .expect("start opf");
        zip.write_all(opf.as_bytes()).expect("write opf");

        // 3.25 nav.xhtml（EPUB3 导航文档：toc 列表，href 与 spine 章节一致）
        let lang = meta.language.as_deref().unwrap_or("zh-CN");
        let mut nav = String::new();
        nav.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        nav.push_str("<!DOCTYPE html>\n");
        nav.push_str(&format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{}\" xml:lang=\"{}\">\n",
            escape_xml(lang),
            escape_xml(lang)
        ));
        nav.push_str("<head>\n<title>目录</title>\n</head>\n<body>\n");
        nav.push_str("<nav epub:type=\"toc\" id=\"toc\">\n<h1>目录</h1>\n<ol>\n");
        for (href, ch) in chapter_hrefs.iter().zip(chapters.iter()) {
            nav.push_str(&format!(
                "<li><a href=\"{href}\">{}</a></li>\n",
                escape_xml(ch.title.trim())
            ));
        }
        nav.push_str("</ol>\n</nav>\n</body>\n</html>\n");
        zip.start_file("OEBPS/nav.xhtml", deflated)
            .expect("start nav");
        zip.write_all(nav.as_bytes()).expect("write nav");

        // 3.5 toc.ncx（EPUB2 NCX：navMap/navPoint，Kindle 等老阅读器目录来源）
        let mut ncx = String::new();
        ncx.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        ncx.push_str(&format!(
            "<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\" xml:lang=\"{}\">\n",
            escape_xml(lang)
        ));
        ncx.push_str("<head>\n");
        ncx.push_str(&format!(
            "<meta name=\"dtb:uid\" content=\"uuid:{uuid}\"/>\n"
        ));
        ncx.push_str("<meta name=\"dtb:depth\" content=\"1\"/>\n");
        ncx.push_str("<meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
        ncx.push_str("<meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
        ncx.push_str("</head>\n");
        ncx.push_str(&format!(
            "<docTitle><text>{}</text></docTitle>\n",
            escape_xml(title.trim())
        ));
        ncx.push_str("<navMap>\n");
        for (i, (href, ch)) in chapter_hrefs.iter().zip(chapters.iter()).enumerate() {
            ncx.push_str(&format!(
                "  <navPoint id=\"navPoint-{}\" playOrder=\"{}\">\n",
                i + 1,
                i + 1
            ));
            ncx.push_str(&format!(
                "    <navLabel><text>{}</text></navLabel>\n",
                escape_xml(ch.title.trim())
            ));
            ncx.push_str(&format!("    <content src=\"{href}\"/>\n"));
            ncx.push_str("  </navPoint>\n");
        }
        ncx.push_str("</navMap>\n</ncx>\n");
        zip.start_file("OEBPS/toc.ncx", deflated)
            .expect("start ncx");
        zip.write_all(ncx.as_bytes()).expect("write ncx");

        // 3.5 封面文件
        if let Some(cover) = &meta.cover {
            zip.start_file(format!("OEBPS/cover.{cover_ext}"), deflated)
                .expect("start cover");
            zip.write_all(cover).expect("write cover");
        }

        // 3.6 GAP 176 内嵌字体：style.css（@font-face + 正文应用）+ OEBPS/fonts/*.woff2
        if meta.font != EmbedFont::None {
            zip.start_file("OEBPS/style.css", deflated)
                .expect("start style");
            zip.write_all(font_css.as_bytes()).expect("write style");
            if let (Some(href), Some(bytes)) = (meta.font.href(), meta.font.bytes()) {
                zip.start_file(format!("OEBPS/{href}"), deflated)
                    .expect("start font");
                zip.write_all(bytes).expect("write font");
            }
        }

        // 4. 章节 XHTML（spine 顺序）
        for (i, ch) in chapters.iter().enumerate() {
            let mut xhtml = String::new();
            xhtml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
            xhtml.push_str("<html xmlns=\"http://www.w3.org/1999/xhtml\">\n<head>\n");
            xhtml.push_str(&format!("<title>{}</title>\n", escape_xml(&ch.title)));
            if meta.font != EmbedFont::None {
                xhtml.push_str("<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n");
            }
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
///
/// P2：GBK/GB2312 不可映射字符不再静默替换为 `?`——逐字符转义为 NCR（`&#x…;`，
/// 原文可逆保留），并返回不可映射字符数（调用方转导出警告，前端提示）。
/// 返回 (编码字节, 不可映射字符数)；gb18030/utf-8 全字符集恒为 0。
pub fn encode_txt(txt: &str, encoding: &str) -> Result<(Vec<u8>, usize), String> {
    let enc = match encoding.trim().to_ascii_lowercase().as_str() {
        "" | "utf-8" | "utf8" | "utf_8" => encoding_rs::UTF_8,
        "gbk" | "gb2312" | "gb_2312" => encoding_rs::GBK,
        "gb18030" => encoding_rs::GB18030,
        other => {
            return Err(format!(
                "不支持的导出编码: {other}（utf-8|gbk|gb2312|gb18030）"
            ))
        }
    };
    let mut out: Vec<u8> = Vec::with_capacity(txt.len() * 2);
    let mut scratch = vec![0u8; 8192];
    let mut encoder = enc.new_encoder();
    let mut src = txt;
    let mut unmappable = 0usize;
    let mut last = false;
    loop {
        let (result, read, written) =
            encoder.encode_from_utf8_without_replacement(src, &mut scratch, last);
        out.extend_from_slice(&scratch[..written]);
        src = &src[read..];
        match result {
            encoding_rs::EncoderResult::InputEmpty => {
                if !last {
                    last = true; // 末次调用（flush 尾部状态）
                    continue;
                }
                break;
            }
            encoding_rs::EncoderResult::OutputFull => {
                scratch.resize(scratch.len() * 2, 0); // 大块不可映射时扩容
            }
            encoding_rs::EncoderResult::Unmappable(c) => {
                // P2：不可映射字符转义为 NCR（&#x…;）保留原文，并计数供警告
                unmappable += 1;
                out.extend_from_slice(format!("&#x{:X};", c as u32).as_bytes());
            }
        }
    }
    Ok((out, unmappable))
}

// ==================== 书源书导出并发抓章（GAP 104b） ====================

/// 并发抓章失败记录（P2：不再静默丢弃——随导出响应警告返回，前端提示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct FetchChapterFailure {
    /// 章节序号（原始目录顺序）
    pub index: usize,
    /// 章节标题
    pub title: String,
    /// 章节 URL
    pub url: String,
    /// 失败原因
    pub error: String,
}

/// 并发抓章结果：成功章节（按原顺序重组）+ 失败记录
#[derive(Debug, Clone, Default)]
pub struct FetchChaptersOutcome {
    pub chapters: Vec<(String, String)>,
    pub failed: Vec<FetchChapterFailure>,
}

/// 并发抓取章节正文：`chapters` 为 (章节标题, 章节 URL) 原始顺序；
/// 并发度 `concurrency`（默认 4）；结果按原章节顺序重组；失败章节不再静默丢弃——
/// 逐条记录到 `outcome.failed`（含序号/标题/URL/原因），由调用方随导出响应警告返回。
/// 瓶颈在网络抓取——本地书/文件书解析不走此路径（保持原有顺序逻辑）。
pub async fn fetch_chapters_concurrent<F, Fut>(
    chapters: Vec<(String, String)>,
    concurrency: usize,
    fetch: F,
) -> FetchChaptersOutcome
where
    F: Fn(usize, String) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<String, String>> + Send,
{
    let results: Vec<(usize, String, String, Result<String, String>)> =
        futures::stream::iter(chapters.into_iter().enumerate().map(|(i, (title, url))| {
            let f = &fetch;
            async move {
                let r = f(i, url.clone()).await;
                (i, title, url, r)
            }
        }))
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // 按章节索引重组（buffer_unordered 完成顺序 ≠ 章节顺序）
    let mut results = results;
    results.sort_by_key(|(i, _, _, _)| *i);
    let mut outcome = FetchChaptersOutcome::default();
    for (i, title, url, r) in results {
        match r {
            Ok(content) => outcome.chapters.push((title, content)),
            Err(error) => outcome.failed.push(FetchChapterFailure {
                index: i,
                title,
                url,
                error,
            }),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sample() -> (String, Vec<ExportChapter>) {
        (
            "测试书".to_string(),
            vec![
                ExportChapter {
                    title: "第一章".into(),
                    content: "正文一 <甲> & 乙。\n第二段。".into(),
                },
                ExportChapter {
                    title: "第二章".into(),
                    content: "正文二。".into(),
                },
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
        assert!(
            html.contains("<p>正文一 &lt;甲&gt; &amp; 乙。</p>"),
            "HTML 转义"
        );
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
        zip.by_name("mimetype")
            .unwrap()
            .read_to_string(&mut mime)
            .unwrap();
        assert_eq!(mime, "application/epub+zip");
        // container.xml 指向 OEBPS/content.opf
        let mut container = String::new();
        zip.by_name("META-INF/container.xml")
            .unwrap()
            .read_to_string(&mut container)
            .unwrap();
        assert!(container.contains("OEBPS/content.opf"));
        // OPF 含 spine 两章 + 标题
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut opf)
            .unwrap();
        assert!(opf.contains("<dc:title>测试书</dc:title>"));
        assert!(opf.contains("<dc:creator>作者甲</dc:creator>"));
        assert!(opf.contains("chap_0000.xhtml"));
        assert!(opf.contains("chap_0001.xhtml"));
        assert_eq!(opf.matches("<itemref").count(), 2);
        // 章节内容（XML 转义）
        let mut ch0 = String::new();
        zip.by_name("OEBPS/chap_0000.xhtml")
            .unwrap()
            .read_to_string(&mut ch0)
            .unwrap();
        assert!(ch0.contains("<h1>第一章</h1>"));
        assert!(
            ch0.contains("正文一 &lt;甲&gt; &amp; 乙。"),
            "XML 转义: {ch0}"
        );
        let mut ch1 = String::new();
        zip.by_name("OEBPS/chap_0001.xhtml")
            .unwrap()
            .read_to_string(&mut ch1)
            .unwrap();
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
            ..Default::default()
        };
        let bytes = build_epub_full(&title, "作者甲", &meta, &chs);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("合法 zip");
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut opf)
            .unwrap();
        // GAP 173 全量元数据元素
        assert!(opf.contains("<dc:description>简介内容</dc:description>"));
        assert!(opf.contains("<dc:language>en</dc:language>"));
        assert!(opf.contains("<dc:date>2023-01-02</dc:date>"));
        assert!(opf.contains("<dc:publisher>出版社</dc:publisher>"));
        assert!(opf.contains("<dc:subject>标签</dc:subject>"));
        assert!(
            opf.contains("properties=\"cover-image\""),
            "封面 manifest 声明"
        );
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

    // ---------- GAP 176 内嵌中文字体 ----------

    #[test]
    fn test_embed_font_parse_param() {
        // 缺省/空串/none → 不内嵌
        assert_eq!(EmbedFont::parse_param(""), Ok(EmbedFont::None));
        assert_eq!(EmbedFont::parse_param("none"), Ok(EmbedFont::None));
        assert_eq!(EmbedFont::parse_param("  none  "), Ok(EmbedFont::None));
        // 两种字体（大小写不敏感）
        assert_eq!(
            EmbedFont::parse_param("lxk-wenkai"),
            Ok(EmbedFont::LxgwWenKai)
        );
        assert_eq!(
            EmbedFont::parse_param("LXK-WENKAI"),
            Ok(EmbedFont::LxgwWenKai)
        );
        assert_eq!(
            EmbedFont::parse_param("source-han-serif"),
            Ok(EmbedFont::SourceHanSerif)
        );
        // 未知 → 明确错误
        let err = EmbedFont::parse_param("comic-sans").unwrap_err();
        assert!(err.contains("不支持的字体"), "{err}");
    }

    /// GAP 176：构造 epub 断言——zip 含字体文件（字节与源一致）+ OPF manifest 字体条目
    /// + style.css @font-face 及正文应用 + 章节链接样式表；无字体对照无条目
    #[test]
    fn test_build_epub_embedded_font() {
        let (title, chs) = sample();
        for font in [EmbedFont::LxgwWenKai, EmbedFont::SourceHanSerif] {
            let href = font.href().unwrap();
            let meta = EpubMeta {
                font,
                ..Default::default()
            };
            let bytes = build_epub_full(&title, "作者甲", &meta, &chs);
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("合法 zip");
            // 1) zip 含字体文件且字节与 web-ui/public/fonts 源完全一致
            let mut font_bytes = Vec::new();
            zip.by_name(&format!("OEBPS/{href}"))
                .expect("zip 应含字体文件")
                .read_to_end(&mut font_bytes)
                .unwrap();
            assert_eq!(
                font_bytes,
                font.bytes().unwrap(),
                "字体字节应完整内嵌（{href}）"
            );
            // 2) OPF manifest 字体条目 + style.css 条目
            let mut opf = String::new();
            zip.by_name("OEBPS/content.opf")
                .unwrap()
                .read_to_string(&mut opf)
                .unwrap();
            assert!(
                opf.contains(&format!(
                    "<item id=\"font-embedded\" href=\"{href}\" media-type=\"font/woff2\" properties=\"font-face\"/>"
                )),
                "OPF 应含字体条目: {opf}"
            );
            assert!(opf.contains("<item id=\"style\" href=\"style.css\" media-type=\"text/css\"/>"));
            // 3) CSS @font-face 引用 + 正文应用
            let mut css = String::new();
            zip.by_name("OEBPS/style.css")
                .unwrap()
                .read_to_string(&mut css)
                .unwrap();
            assert!(
                css.contains(&format!("font-family: '{}';", font.family().unwrap())),
                "CSS: {css}"
            );
            assert!(
                css.contains(&format!("url('{href}') format('woff2')")),
                "CSS: {css}"
            );
            assert!(
                css.contains(&format!(
                    "font-family: {};",
                    font.css_family_stack().unwrap()
                )),
                "CSS 正文应应用: {css}"
            );
            // 4) 章节 XHTML 链接样式表
            let mut ch0 = String::new();
            zip.by_name("OEBPS/chap_0000.xhtml")
                .unwrap()
                .read_to_string(&mut ch0)
                .unwrap();
            assert!(ch0.contains("<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>"));
        }
        // 5) 对照：未指定字体 → 无字体文件/无 style.css/无字体条目（既有导出不变）
        let plain = build_epub(&title, "作者甲", &chs);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(plain)).expect("合法 zip");
        assert!(
            zip.by_name("OEBPS/style.css").is_err(),
            "无字体不应有 style.css"
        );
        assert!(zip
            .by_name("OEBPS/fonts/lxgw-wenkai-regular.woff2")
            .is_err());
        assert!(zip
            .by_name("OEBPS/fonts/source-han-serif-cn-regular.woff2")
            .is_err());
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut opf)
            .unwrap();
        assert!(!opf.contains("font-embedded"), "无字体不应有字体条目");
        let mut ch0 = String::new();
        zip.by_name("OEBPS/chap_0000.xhtml")
            .unwrap()
            .read_to_string(&mut ch0)
            .unwrap();
        assert!(!ch0.contains("style.css"), "无字体章节不应链接样式表");
    }

    // ---------- 目录导航完整性（nav.xhtml EPUB3 + toc.ncx EPUB2） ----------

    #[test]
    fn test_build_epub_navigation_nav_ncx() {
        let (title, chs) = sample();
        let bytes = build_epub(&title, "作者甲", &chs);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("合法 zip");

        // OPF：manifest 声明 nav.xhtml（EPUB3 properties=nav）+ toc.ncx（EPUB2 x-dtbncx）
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut opf)
            .unwrap();
        assert!(
            opf.contains("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>")
                || opf.contains("<item id=\"nav\" href=\"nav.xhtml\""),
            "OPF manifest 声明 nav.xhtml: {opf}"
        );
        assert!(
            opf.contains(
                "<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>"
            ),
            "OPF manifest 声明 toc.ncx: {opf}"
        );
        // spine toc="ncx" 有对应 manifest id（老阅读器经 NCX 出目录）
        assert!(opf.contains("<spine toc=\"ncx\">"), "spine 指向 ncx: {opf}");
        // spine itemref ↔ manifest 章节 id 对应
        assert!(opf.contains(
            "<item id=\"chap0\" href=\"chap_0000.xhtml\" media-type=\"application/xhtml+xml\"/>"
        ));
        assert!(opf.contains("<itemref idref=\"chap0\"/>"));
        assert!(opf.contains("<itemref idref=\"chap1\"/>"));

        // nav.xhtml：EPUB3 toc 列表，href 与章节一一对应（顺序一致）
        let mut nav = String::new();
        zip.by_name("OEBPS/nav.xhtml")
            .unwrap()
            .read_to_string(&mut nav)
            .unwrap();
        assert!(
            nav.contains("<nav epub:type=\"toc\""),
            "EPUB3 toc nav: {nav}"
        );
        assert!(nav.contains("<ol>"));
        for (i, ch) in chs.iter().enumerate() {
            assert!(
                nav.contains(&format!("<a href=\"chap_{i:04}.xhtml\">{}</a>", ch.title)),
                "nav 第 {i} 项 href/标题匹配"
            );
        }

        // toc.ncx：navMap/navPoint + playOrder + content src 与章节 href 匹配
        let mut ncx = String::new();
        zip.by_name("OEBPS/toc.ncx")
            .unwrap()
            .read_to_string(&mut ncx)
            .unwrap();
        assert!(ncx.contains("<navMap>"), "NCX navMap: {ncx}");
        assert!(ncx.contains("<navPoint id=\"navPoint-1\" playOrder=\"1\">"));
        assert!(ncx.contains("<navLabel><text>第一章</text></navLabel>"));
        assert!(ncx.contains("<navLabel><text>第二章</text></navLabel>"));
        assert!(ncx.contains("<content src=\"chap_0000.xhtml\"/>"));
        assert!(ncx.contains("<content src=\"chap_0001.xhtml\"/>"));
        assert!(ncx.contains("<docTitle><text>测试书</text></docTitle>"));
        // dtb:uid 与 OPF dc:identifier 的 uuid 一致（EPUB 规范：NCX uid 必须与包标识一致）
        let opf_uuid = opf
            .split("uuid:")
            .nth(1)
            .unwrap()
            .split('<')
            .next()
            .unwrap()
            .to_string();
        assert!(
            ncx.contains(&format!(
                "<meta name=\"dtb:uid\" content=\"uuid:{opf_uuid}\"/>"
            )) || ncx.contains(&format!("dtb:uid\" content=\"uuid:{opf_uuid}\"")),
            "NCX dtb:uid 与 OPF 标识一致（{opf_uuid}）"
        );
    }

    #[test]
    fn test_build_epub_navigation_cover_and_full_meta() {
        // 封面 + 全量元数据路径下目录导航同样完整（nav/ncx/封面 href 引用正确）
        let (title, chs) = sample();
        let meta = EpubMeta {
            language: Some("en".into()),
            cover: Some(vec![0xFF, 0xD8, 0xFF, 0xE0]),
            ..Default::default()
        };
        let bytes = build_epub_full(&title, "作者甲", &meta, &chs);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("合法 zip");
        assert!(zip.by_name("OEBPS/nav.xhtml").is_ok());
        assert!(zip.by_name("OEBPS/toc.ncx").is_ok());
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut opf)
            .unwrap();
        assert!(
            opf.contains("properties=\"cover-image\""),
            "封面 manifest 声明"
        );
        assert!(opf.contains("<meta name=\"cover\" content=\"cover-image\"/>"));
        assert!(opf.contains("<item id=\"cover-image\" href=\"cover.jpg\" media-type=\"image/jpeg\" properties=\"cover-image\"/>"));
        assert!(zip.by_name("OEBPS/cover.jpg").is_ok(), "封面文件存在");
        let mut ncx = String::new();
        zip.by_name("OEBPS/toc.ncx")
            .unwrap()
            .read_to_string(&mut ncx)
            .unwrap();
        // language 透传到 nav/ncx 的 xml:lang
        assert!(ncx.contains("xml:lang=\"en\""));
    }

    // ---------- GAP 104 编码 ----------

    #[test]
    fn test_encode_txt_utf8_and_gbk() {
        let txt = "书名《测试》\n第一章 正文";
        let (utf8, n) = encode_txt(txt, "utf-8").unwrap();
        assert_eq!(n, 0, "utf-8 全字符集无不可映射");
        assert_eq!(utf8, txt.as_bytes());
        // gbk：中文字符 2 字节（UTF-8 3 字节）——编码后长度变小且可解码回原文
        let (gbk, n) = encode_txt(txt, "gbk").unwrap();
        assert_eq!(n, 0, "GBK 内字符（书名号/汉字）应可映射");
        assert!(
            gbk.len() < utf8.len(),
            "GBK 中文 2 字节: {} < {}",
            gbk.len(),
            utf8.len()
        );
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&gbk);
        assert!(!had_errors);
        assert_eq!(decoded, txt);
        // 大小写/别名
        let (b, _) = encode_txt("x", "GB2312").unwrap();
        assert_eq!(b, b"x");
        let (b, _) = encode_txt("x", "GB18030").unwrap();
        assert_eq!(b, b"x");
        // 不支持的编码 → 明确错误
        let err = encode_txt("x", "latin1").unwrap_err();
        assert!(err.contains("不支持的导出编码"), "{err}");
    }

    /// P2：GBK 不可映射字符——不再静默替换为 ?，转义 NCR（&#x…;）保留原文 + 计数
    #[test]
    fn test_encode_txt_gbk_unmappable_escaped_with_count() {
        // 😀(U+1F600)、𝕏(U+1D54F) 不在 GBK/GB2312；gb18030 全字符集可映射
        let txt = "书名😀尾𝕏完";
        let (gbk, n) = encode_txt(txt, "gbk").unwrap();
        assert_eq!(n, 2, "两个不可映射字符应计数（实际 {n}）");
        let s = encoding_rs::GBK.decode(&gbk).0.into_owned();
        assert!(!s.contains('?'), "不可映射字符不应被替换为 ?: {s}");
        assert!(s.contains("&#x1F600;"), "😀 应转义为 NCR: {s}");
        assert!(s.contains("&#x1D54F;"), "𝕏 应转义为 NCR: {s}");
        assert!(
            s.contains("书名") && s.contains("尾") && s.contains("完"),
            "可映射部分原样保留: {s}"
        );
        // gb18030 全字符集：不转义、计数 0
        let (b18030, n) = encode_txt(txt, "gb18030").unwrap();
        assert_eq!(n, 0, "gb18030 全字符集（实际 {n}）");
        assert_eq!(encoding_rs::GB18030.decode(&b18030).0, txt);
        // GB2312 与 GBK 同（映射超集语义一致）
        let (_, n) = encode_txt(txt, "gb2312").unwrap();
        assert_eq!(n, 2);
    }

    /// P2：大文本多段不可映射（跨 scratch 缓冲区）——计数与转义完整
    #[test]
    fn test_encode_txt_gbk_many_unmappable() {
        let txt = "😀".repeat(50_000) + &"中".repeat(100);
        let (gbk, n) = encode_txt(&txt, "gbk").unwrap();
        assert_eq!(n, 50_000);
        let s = encoding_rs::GBK.decode(&gbk).0;
        assert_eq!(s.matches("&#x1F600;").count(), 50_000, "NCR 转义应完整");
        assert_eq!(s.matches('中').count(), 100);
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
        let fetched = fetched.chapters;

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
        let outcome = fetch_chapters_concurrent(chapters, 2, |i, _url| async move {
            if i == 1 || i == 3 {
                Err("网络错误".to_string())
            } else {
                Ok(format!("正文{i}"))
            }
        })
        .await;
        assert_eq!(
            outcome.chapters,
            vec![
                ("章0".to_string(), "正文0".to_string()),
                ("章2".to_string(), "正文2".to_string()),
                ("章4".to_string(), "正文4".to_string()),
            ]
        );
        // P2：失败章节逐条记录（序号/标题/URL/原因）——不再静默丢弃
        assert_eq!(outcome.failed.len(), 2, "两条失败记录");
        assert_eq!(outcome.failed[0].index, 1);
        assert_eq!(outcome.failed[0].title, "章1");
        assert_eq!(outcome.failed[0].url, "/1");
        assert_eq!(outcome.failed[0].error, "网络错误");
        assert_eq!(outcome.failed[1].index, 3);
        assert_eq!(outcome.failed[1].title, "章3");
        // 全成功 → failed 为空
        let ok = fetch_chapters_concurrent(
            vec![("a".to_string(), "u".to_string())],
            1,
            |_, _| async move { Ok("c".to_string()) },
        )
        .await;
        assert!(ok.failed.is_empty());
    }
}
