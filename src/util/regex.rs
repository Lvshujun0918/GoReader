//! 正则兼容层（GAP 153）：regex 快路径优先，fancy-regex 回退
//!
//! 背景：书源规则正则 / 替换规则 / TXT 目录规则可能使用 lookbehind（`(?<=...)`）等
//! regex crate 不支持的语法。本包装统一两引擎：
//! - 优先 regex 编译（快路径，绝大多数规则走此）；
//! - regex 编译失败（如含 lookbehind）→ 自动升级 fancy-regex 编译执行；
//! - 两引擎均失败 → 返回带双方原因的明确错误（调用方记日志/报错，不再静默吞掉）。
//!
//! 对外 API 与 regex crate 常用子集一致（new/is_match/captures_iter/replace_all +
//! RegexBuilder::multi_line/case_insensitive），便于逐点替换。

use std::borrow::Cow;
use std::ops::Range;

/// 编译后的正则（std 或 fancy 引擎之一）
#[derive(Debug, Clone)]
pub struct Regex {
    inner: Inner,
}

#[derive(Debug, Clone)]
enum Inner {
    Std(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl Regex {
    /// 编译：regex 优先；失败回退 fancy-regex（lookbehind 等）；均失败 → Err
    pub fn new(pattern: &str) -> Result<Self, String> {
        match regex::Regex::new(pattern) {
            Ok(re) => Ok(Regex { inner: Inner::Std(re) }),
            Err(std_err) => match fancy_regex::Regex::new(pattern) {
                Ok(re) => Ok(Regex { inner: Inner::Fancy(re) }),
                Err(fancy_err) => Err(format!(
                    "正则编译失败: {pattern:?}（regex: {std_err}；fancy-regex: {fancy_err}）"
                )),
            },
        }
    }

    /// 是否匹配（fancy 引擎求值出错视为不匹配）
    pub fn is_match(&self, text: &str) -> bool {
        match &self.inner {
            Inner::Std(re) => re.is_match(text),
            Inner::Fancy(re) => re.is_match(text).unwrap_or(false),
        }
    }

    /// 捕获迭代器（fancy 引擎单次求值出错跳过该项）
    pub fn captures_iter<'t>(&'t self, text: &'t str) -> CaptureMatches<'t> {
        match &self.inner {
            Inner::Std(re) => CaptureMatches {
                inner: CaptureMatchesInner::Std(re.captures_iter(text)),
            },
            Inner::Fancy(re) => CaptureMatches {
                inner: CaptureMatchesInner::Fancy(re.captures_iter(text)),
            },
        }
    }

    /// 全部替换（fancy 引擎替换出错 → 原样返回）
    pub fn replace_all<'t>(&'t self, text: &'t str, rep: &str) -> Cow<'t, str> {
        match &self.inner {
            Inner::Std(re) => re.replace_all(text, rep),
            Inner::Fancy(re) => re.try_replacen(text, 0, rep).unwrap_or(Cow::Borrowed(text)),
        }
    }

    /// 仅替换第一个匹配（legado `###` replaceFirst 语义；无匹配 → 原样返回）
    pub fn replace_first<'t>(&'t self, text: &'t str, rep: &str) -> Cow<'t, str> {
        match &self.inner {
            Inner::Std(re) => re.replacen(text, 1, rep),
            Inner::Fancy(re) => re.try_replacen(text, 1, rep).unwrap_or(Cow::Borrowed(text)),
        }
    }
}

/// 单次捕获（统一两引擎的 get(i) 语义）
#[derive(Debug, Clone, Copy)]
pub struct Match<'t> {
    start: usize,
    end: usize,
    text: &'t str,
}

impl<'t> Match<'t> {
    pub fn start(&self) -> usize {
        self.start
    }
    pub fn end(&self) -> usize {
        self.end
    }
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }
    pub fn as_str(&self) -> &'t str {
        self.text
    }
}

/// 捕获组集合
#[derive(Debug)]
pub struct Captures<'t> {
    inner: CapturesInner<'t>,
}

#[derive(Debug)]
enum CapturesInner<'t> {
    Std(regex::Captures<'t>),
    Fancy(fancy_regex::Captures<'t, str>),
}

impl<'t> Captures<'t> {
    /// 第 i 组（0 = 全匹配；缺失组 → None）
    pub fn get(&self, i: usize) -> Option<Match<'t>> {
        match &self.inner {
            CapturesInner::Std(c) => c.get(i).map(|m| Match {
                start: m.start(),
                end: m.end(),
                text: m.as_str(),
            }),
            CapturesInner::Fancy(c) => c.get(i).map(|m| Match {
                start: m.start(),
                end: m.end(),
                text: m.as_str(),
            }),
        }
    }
}

/// 捕获迭代器
pub struct CaptureMatches<'t> {
    inner: CaptureMatchesInner<'t>,
}

enum CaptureMatchesInner<'t> {
    Std(regex::CaptureMatches<'t, 't>),
    Fancy(fancy_regex::CaptureMatches<'t, 't, str>),
}

impl<'t> Iterator for CaptureMatches<'t> {
    type Item = Captures<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            CaptureMatchesInner::Std(it) => it
                .next()
                .map(|c| Captures { inner: CapturesInner::Std(c) }),
            CaptureMatchesInner::Fancy(it) => it
                .next()
                .and_then(|r| r.ok()) // 单次求值出错跳过（如回溯超限）
                .map(|c| Captures { inner: CapturesInner::Fancy(c) }),
        }
    }
}

/// 构建器（multi_line/case_insensitive，与 regex::RegexBuilder 用法一致）
pub struct RegexBuilder<'a> {
    pattern: &'a str,
    multi_line: bool,
    case_insensitive: bool,
}

impl<'a> RegexBuilder<'a> {
    pub fn new(pattern: &'a str) -> Self {
        Self {
            pattern,
            multi_line: false,
            case_insensitive: false,
        }
    }

    pub fn multi_line(&mut self, yes: bool) -> &mut Self {
        self.multi_line = yes;
        self
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.case_insensitive = yes;
        self
    }

    pub fn build(&self) -> Result<Regex, String> {
        let mut sb = regex::RegexBuilder::new(self.pattern);
        sb.multi_line(self.multi_line).case_insensitive(self.case_insensitive);
        match sb.build() {
            Ok(re) => Ok(Regex { inner: Inner::Std(re) }),
            Err(std_err) => {
                let mut fb = fancy_regex::RegexBuilder::new(self.pattern);
                fb.multi_line(self.multi_line).case_insensitive(self.case_insensitive);
                match fb.build() {
                    Ok(re) => Ok(Regex { inner: Inner::Fancy(re) }),
                    Err(fancy_err) => Err(format!(
                        "正则编译失败: {:?}（regex: {}；fancy-regex: {}）",
                        self.pattern, std_err, fancy_err
                    )),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookbehind_compiles_and_matches() {
        // regex crate 不支持 (?<=...)——wrapper 应自动升级 fancy-regex
        let re = Regex::new(r"(?<=书名[:：])\S+").expect("lookbehind 应可编译");
        assert!(re.is_match("书名：测试书"));
        assert!(!re.is_match("作者：张三"));
        let caps: Vec<String> = re
            .captures_iter("书名：测试书 书名：第二本")
            .filter_map(|c| c.get(0).map(|m| m.as_str().to_string()))
            .collect();
        assert_eq!(caps, vec!["测试书".to_string(), "第二本".to_string()]);
    }

    #[test]
    fn test_std_fast_path_unchanged() {
        let re = Regex::new(r"第(.+?)章").unwrap();
        let caps: Vec<String> = re
            .captures_iter("第一章 第二章")
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        assert_eq!(caps, vec!["一".to_string(), "二".to_string()]);
    }

    #[test]
    fn test_replace_all() {
        let re = Regex::new(r"\s+").unwrap();
        assert_eq!(re.replace_all("a  b   c", " "), "a b c");
        // lookbehind 替换路径
        let re = Regex::new(r"(?<=第)\d+").unwrap();
        assert_eq!(re.replace_all("第1章 第2章", "X"), "第X章 第X章");
    }

    #[test]
    fn test_builder_multi_line() {
        let re = RegexBuilder::new(r"^第.+$")
            .multi_line(true)
            .build()
            .unwrap();
        let caps: Vec<String> = re
            .captures_iter("第一章 内容\n中间\n第二章 内容")
            .filter_map(|c| c.get(0).map(|m| m.as_str().to_string()))
            .collect();
        assert_eq!(caps, vec!["第一章 内容".to_string(), "第二章 内容".to_string()]);
        // lookbehind + multiline 组合
        let re = RegexBuilder::new(r"(?<=^第)\d+")
            .multi_line(true)
            .build()
            .unwrap();
        assert!(re.is_match("第1章\n第2章"));
    }

    #[test]
    fn test_invalid_pattern_returns_clear_error() {
        let err = Regex::new(r"(?<=unclosed").unwrap_err();
        assert!(err.contains("正则编译失败"), "错误信息应明确: {err}");
        assert!(err.contains("fancy-regex"), "应包含 fancy-regex 原因: {err}");
    }

    #[test]
    fn test_match_range() {
        let re = Regex::new(r"书").unwrap();
        let caps: Vec<(usize, usize)> = re
            .captures_iter("两本书")
            .filter_map(|c| c.get(0).map(|m| (m.start(), m.end())))
            .collect();
        // UTF-8 字节偏移（两=3B 本=3B 书=3B）
        assert_eq!(caps, vec![(6, 9)]);
    }
}
