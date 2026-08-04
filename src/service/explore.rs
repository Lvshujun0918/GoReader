//! 发现/探索（ruleExplore）：exploreUrl 集合 + 书单解析
//!
//! 对齐 legacy WebBook.exploreBook：URL 列表 → 抓取 → ruleExplore 字段 → SearchBook

use anyhow::Result;

use crate::model::BookSource;
use crate::service::crawler;
use crate::service::search::SearchBook;

/// 探索条目（title + url）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExploreEntry {
    pub title: String,
    pub url: String,
    /// book=书单分类（探索加载）/ link=外部链接（点击打开）
    #[serde(default)]
    pub r#type: String,
}

/// 判断分类类型：外部链接（群/导入/渠道/发布等）vs 书单
fn entry_type(title: &str, url: &str) -> String {
    let t = title.to_lowercase();
    let keywords = ["导入", "群", "发布", "渠道", "交流", "更新", "关注", "频道", "公众号"];
    if keywords.iter().any(|k| t.contains(k)) {
        return "link".to_string();
    }
    let domains = ["qm.qq.com", "bilibili.com", "mp.weixin.qq.com", "shuyuan-api", "yckceo.com", "t.me"];
    if domains.iter().any(|d| url.contains(d)) {
        return "link".to_string();
    }
    "book".to_string()
}

/// 解析 exploreUrl（legado 语义）：
/// - `@js:代码`：执行 JS（返回 JSON.stringify([{title,url},...])）→ 解析条目
/// - 普通多行 URL：每行一个条目（title 从 URL 尾部提取）
pub fn parse_explore_entries(explore_url: &str) -> Vec<ExploreEntry> {
    let mut entries = Vec::new();
    let lines: Vec<&str> = explore_url.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        // @js: 格式：同行（@js:代码）或独立行（@js: 后所有行为代码——legado 常见）
        if line == "@js:" || line.starts_with("@js:") {
            let code = if line == "@js:" {
                // 独立行：后续所有行拼接为代码
                let rest = &lines[i + 1..];
                i = lines.len();
                rest.join("
")
            } else {
                i += 1;
                line[4..].to_string()
            };
            // eval 直接取结构化结果（数组/对象递归 JSON 转换——避免 ToString 的
            // "[object Object]" 导致条目解析为空；JSON.stringify 字符串出口自动解析）
            if let Ok(list) = crate::parser::js::eval_js_json_with_bridge(
                &code,
                &Default::default(),
                &crate::parser::js::JsBridge::default(),
            ) {
                if let serde_json::Value::Array(items) = list {
                    for item in items {
                        let title = item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let url = item
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !url.is_empty() {
                            entries.push(ExploreEntry { title: title.clone(), url: url.clone(), r#type: entry_type(&title, &url) });
                        }
                    }
                    i = lines.len();
                    continue;
                }
            }
            continue;
        }
        // JSON 数组格式：[{"title":"...","url":"..."}, ...]（inline 或跨行）
        if line.starts_with('[') || line.starts_with('{') {
            // 收集到匹配的 ]（多行 JSON）
            let mut json_str = line.to_string();
            let mut j = i + 1;
            while !json_str.trim_end().ends_with(']') && j < lines.len() {
                json_str.push('\n');
                json_str.push_str(lines[j]);
                j += 1;
            }
            i = j;
            if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&json_str) {
                for item in list {
                    let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !url.is_empty() {
                        entries.push(ExploreEntry { title: title.clone(), url: url.clone(), r#type: entry_type(&title, &url) });
                    }
                }
                continue;
            }
            continue;
        }
        // "标题::URL" 格式（legado 常见）
        if let Some((title, url)) = line.split_once("::") {
            let title = title.trim().to_string();
            let url = url.trim().to_string();
            if !url.is_empty() {
                entries.push(ExploreEntry { title: title.clone(), url: url.clone(), r#type: entry_type(&title, &url) });
                i += 1;
                continue;
            }
        }
        // 普通 URL 行：title 从尾部提取
        let title = url_title(&line);
        entries.push(ExploreEntry {
            title: title.clone(),
            url: line.to_string(),
            r#type: entry_type(&title, line),
        });
        i += 1;
    }
    entries
}

/// 从 URL 提取分类名（尾部路径段/查询参数，解码）
fn url_title(url: &str) -> String {
    let cleaned = url.split(['?', '&', '#']).next().unwrap_or(url);
    let seg = cleaned
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(cleaned);
    let decoded = percent_decode(seg);
    if !decoded.is_empty() && decoded != "/" {
        return decoded;
    }
    // 查询参数 name/type/id
    for param in ["name", "type", "id"] {
        for pair in url.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == param && !v.is_empty() {
                    return percent_decode(v);
                }
            }
        }
    }
    url.to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 探索分页 hasMore 阈值：单页书数达到该值认为可能还有下一页（无总数信号的
/// 分页站点通用启发式——与 RSS 列表分页同策略；小于阈值说明已到底）
pub const EXPLORE_PAGE_SIZE: usize = 20;

/// 判断是否还有下一页：本页非空且达到阈值
pub fn has_more(books: &[SearchBook]) -> bool {
    !books.is_empty() && books.len() >= EXPLORE_PAGE_SIZE
}

/// 构造分页探索 URL（GAP #51：服务端解析书源规则分页变量 {{page}}/{page}）
pub fn build_explore_url(url: &str, page: i64) -> String {
    url.replace("{{page}}", &page.to_string())
        .replace("{page}", &page.to_string())
}

/// 单页发现：抓取 + 解析（复用搜索的 SearchRule 语义）
///
/// GAP #51：page 参数由服务端替换书源分页变量（{{page}}/{page}，URL 与 POST body）
pub async fn explore_url(
    ns: &str,
    url: &str,
    page: i64,
    source: &BookSource,
) -> Result<Vec<SearchBook>> {
    // URL 模板（{{page}}/{page}）→ 页码
    let url = build_explore_url(url, page);
    // 相对 URL 拼书源 baseUrl
    let raw_url = if url.starts_with('/') && !url.starts_with("//") {
        let base = source.book_source_url.split("##").next().unwrap_or("").trim_end_matches('/');
        format!("{base}{url}")
    } else {
        url.to_string()
    };
    // URL 后缀（,{...}：charset/method/body——对齐搜索链路）
    let (final_url, suffix) = crate::service::search::split_url_suffix(&raw_url);
    let mut headers = source.header.as_deref().map(crawler::parse_header).unwrap_or_default();
    if let Some(extra) = &suffix.headers {
        for (k, v) in extra {
            headers.insert(k.clone(), v.clone());
        }
    }
    let post_body = suffix.body.as_ref().map(|b| build_explore_url(b, page));
    // 书源抓取（自动带书源 cookie——按用户命名空间）
    let method = suffix.method.as_deref().unwrap_or("GET");
    let resp = if method.eq_ignore_ascii_case("POST") {
        crawler::http_post(ns, &final_url, &headers, 15, post_body.as_deref(), suffix.charset.as_deref()).await
    } else {
        crawler::http_get(ns, &final_url, &headers, 15).await
    }
    .map_err(|e| anyhow::anyhow!("抓取失败（{}）: {}", final_url, e))?;

    let rule: crate::service::search::SearchRule = match &source.rule_explore {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => return Ok(vec![]),
    };
    let Some(book_list_rule) = rule.book_list.clone() else {
        return Ok(vec![]);
    };
    let books = crate::service::search::analyze_book_list_for_explore(
        &resp.body,
        &resp.url,
        source,
        &rule,
        &book_list_rule,
    );
    Ok(books)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_urls() {
        let urls = "https://a.com/list\n#注释\nhttps://b.com/{{page}}\n";
        let parsed = parse_explore_entries(urls);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[1].url.contains("{{page}}"));
        // @js: 代码行生成条目
        let js = "@js:JSON.stringify([{title:'分类A',url:'https://a.com/x'}])";
        let parsed = parse_explore_entries(js);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "分类A");
    }

    /// exploreUrl JS 返回数组字面量（非 JSON.stringify 字符串）——此前 ToString
    /// 输出 "[object Object]" 导致条目解析为空
    #[test]
    fn test_parse_js_entries_array_literal() {
        let js = "@js:[{title:'分类X',url:'https://a.com/x'},{title:'分类Y',url:'https://a.com/y'}]";
        let parsed = parse_explore_entries(js);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "分类X");
        assert_eq!(parsed[0].url, "https://a.com/x");
        assert_eq!(parsed[1].title, "分类Y");
        assert_eq!(parsed[1].url, "https://a.com/y");
        // JSON.parse 数组出口
        let js = "@js:JSON.parse('[{\"title\":\"类P\",\"url\":\"https://a.com/p\"}]')";
        let parsed = parse_explore_entries(js);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "类P");
        // 无 url 条目丢弃
        let js = "@js:[{title:'空',url:''}]";
        assert!(parse_explore_entries(js).is_empty());
    }

    /// GAP #51：分页变量替换（{{page}}/{page} 双格式，URL 与 POST body 一致）
    #[test]
    fn test_build_explore_url_page() {
        assert_eq!(build_explore_url("https://a.com/list/{{page}}", 3), "https://a.com/list/3");
        assert_eq!(build_explore_url("https://a.com/list/{page}", 2), "https://a.com/list/2");
        assert_eq!(build_explore_url("https://a.com/list?p={{page}}", 7), "https://a.com/list?p=7");
        // 无占位符：原样返回
        assert_eq!(build_explore_url("https://a.com/list", 5), "https://a.com/list");
    }

    /// GAP #51：hasMore 启发式（本页达到阈值且非空 → 可能有下一页）
    #[test]
    fn test_has_more() {
        assert!(!has_more(&[]), "空页无更多");
        let book = |i: usize| SearchBook {
            book_url: format!("https://a.com/b{i}"),
            ..Default::default()
        };
        assert!(!has_more(&(0..10).map(book).collect::<Vec<_>>()), "不足阈值无更多");
        assert!(has_more(&(0..EXPLORE_PAGE_SIZE).map(book).collect::<Vec<_>>()), "满页可能有更多");
        assert!(has_more(&(0..30).map(book).collect::<Vec<_>>()));
    }
}
