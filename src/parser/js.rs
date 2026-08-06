//! JS 规则执行（boa_engine 0.19，对齐 legado AnalyzeByJS / js 规则）
//!
//! v2 支持：
//! - 纯 JS 逻辑 + 注入变量（key/page/result/baseUrl/headerMap 简化）
//! - 书源桥接（JsBridge，对齐 legado jsHelp）：
//!   - `java.put(key, val)` / `java.get(key)`：bridge 生命周期内的临时变量
//!   - `java.log(msg)`：tracing 日志
//!   - `java.headerMap.put/get/size`：请求头 Map（eval 后经 `JsBridge::headers()` 读取）
//!   - `java.encodeURI(str, charset)`：URL 百分号编码（gbk/gb2312/utf-8，encoding_rs）
//!   - `java.ajax(urlOrSpec)`：带书源 cookie 的同步请求（支持 `url,{...}` 后缀），返回响应文本
//!   - `java.startBrowserAwait(url, title, isForeground)`：内置浏览器加载页面并等待完成，
//!     返回 `{body: html, cookies: ["name=value",...], status: 200}`（后端接入
//!     `browser::solve_captcha`——CF 质询/Turnstile/滑块统一求解）
//!   - `java.setContent(html)` / `java.getString(rule)` / `java.getElements(rule)`：
//!     设置当前解析文档 + css_chain 规则求值（文本 / outerHTML 数组）
//!   - `java.getWebViewUA()`：固定浏览器 UA（`JS_WEBVIEW_UA`）
//!   - `source.getKey()`（书源 URL）/ `source.getName()`（书源名）
//!   - `source.put(key, val)` / `source.get(key)`：书源级变量，全局共享、按书源 key
//!     隔离（跨搜索/详情调用可见，底层为全局 `Mutex<HashMap>`）
//!
//! boa 0.19 API 注意：
//! - 变量注入需经 JsString 转换（PropertyKey/JsValue 无 From<&str>，有 From<JsString>）
//! - NativeFunction 注册：需捕获状态的闭包走 `from_closure`（捕获数据不得含需 GC
//!   追踪的类型；std Mutex 无 Trace 实现，无法用 from_copy_closure_with_captures）
//! - JsError 含 Rc/NonNull，非 Send/Sync，不能直接进 anyhow，需 map_err 转字符串
//! - JsValue::to_string(&mut Context) 即规范 ToString（数字/布尔按 String() 语义）

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use base64::Engine as _;
use boa_engine::object::builtins::JsArray;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::object::ObjectInitializer;
use boa_engine::property::{Attribute, PropertyKey};
use boa_engine::{
    Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    Source,
};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::model::BookSource;

/// `java.getWebViewUA()` 返回的固定浏览器 UA（与爬虫默认 UA 同系 Chrome 120 内核；
/// 内置浏览器求解（solve_cf_challenge / 待落地的 solve_captcha）返回真实 UA 时
/// 以浏览器为准，此常量作为 JS 侧固定参考值）
pub const JS_WEBVIEW_UA: &str =
    "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Mobile Safari/537.36";

/// source.put/get 书源级变量存储：全局共享（跨搜索/详情调用），
/// 外层 key 为书源 key（URL），内层为该书源的变量表（书源间隔离）
static SOURCE_VARS: LazyLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 书源 JS 桥接：持有书源信息与可被 JS 读写的状态（请求头 / java 临时变量）。
///
/// - 每次搜索/详情流程创建一次（`JsBridge::new(source_key, source_name)`），
///   同流程内多次 `eval_js_with_bridge` 共享 `java.put/get` 与 `java.headerMap`；
/// - `source.put/get` 走全局存储，跨流程/跨 bridge 实例可见（按书源 key 隔离）；
/// - 请求头：`set_headers` 注入初始值，JS 内 `java.headerMap.put` 改写，
///   eval 后 `headers()` 取回用于实际请求。
#[derive(Clone)]
pub struct JsBridge {
    inner: Arc<JsBridgeInner>,
}

struct JsBridgeInner {
    /// 书源 key（URL），`source.getKey()` 返回
    source_key: String,
    /// 书源名称，`source.getName()` 返回
    source_name: String,
    /// 书源登录 URL（JS 代码），`source.loginUrl` 返回（`eval(String(source.loginUrl))` 模式）
    login_url: String,
    /// 书源变量（`source.getVariable()`，legado 书源变量配置）
    source_variable: String,
    /// 书源 header（`source.header`，JSON 文本）
    source_header: String,
    /// 用户命名空间（书源 cookie 按用户隔离；空 = 无 cookie 上下文，
    /// `java.ajax`/`java.startBrowserAwait` 不带书源 cookie）
    ns: String,
    /// 请求头：`java.headerMap` 的底层存储（JS 可改写）
    headers: Mutex<HashMap<String, String>>,
    /// `java.put/get` 临时变量（本 bridge 生命周期内共享）
    java_vars: Mutex<HashMap<String, String>>,
    /// `java.setContent` 设置的当前解析文档（`java.getString/getElements` 的解析源）
    doc: Mutex<Option<String>>,
}

/// 手动 Clone（Mutex 无 Clone——快照当前内容；`Arc::try_unwrap` 多引用兜底路径用）
impl Clone for JsBridgeInner {
    fn clone(&self) -> Self {
        let lock = |m: &Mutex<HashMap<String, String>>| {
            Mutex::new(m.lock().unwrap_or_else(|e| e.into_inner()).clone())
        };
        Self {
            source_key: self.source_key.clone(),
            source_name: self.source_name.clone(),
            login_url: self.login_url.clone(),
            source_variable: self.source_variable.clone(),
            source_header: self.source_header.clone(),
            ns: self.ns.clone(),
            headers: lock(&self.headers),
            java_vars: lock(&self.java_vars),
            doc: Mutex::new(self.doc.lock().unwrap_or_else(|e| e.into_inner()).clone()),
        }
    }
}

impl JsBridge {
    /// 创建书源桥接
    pub fn new(source_key: impl Into<String>, source_name: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(JsBridgeInner {
                source_key: source_key.into(),
                source_name: source_name.into(),
                login_url: String::new(),
                source_variable: String::new(),
                source_header: String::new(),
                ns: String::new(),
                headers: Mutex::new(HashMap::new()),
                java_vars: Mutex::new(HashMap::new()),
                doc: Mutex::new(None),
            }),
        }
    }

    /// 从书源创建桥接（source.loginUrl/getVariable/header 等扩展字段可用）
    pub fn from_source(source: &BookSource, ns: impl Into<String>) -> Self {
        let bridge = Self::new(&source.book_source_url, &source.book_source_name);
        let mut inner = Arc::try_unwrap(bridge.inner).unwrap_or_else(|arc| (*arc).clone());
        inner.login_url = source.login_url.clone().unwrap_or_default();
        inner.source_variable = source
            .variable
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| source.variable_comment.clone().unwrap_or_default());
        inner.source_header = source.header.clone().unwrap_or_default();
        inner.ns = ns.into();
        Self {
            inner: Arc::new(inner),
        }
    }

    /// 设置用户命名空间（书源 cookie 按用户隔离；搜索/详情流程传入当前用户 ns）
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        // 先取出旧 inner 的可克隆状态，避免借用与赋值冲突
        let source_key = self.inner.source_key.clone();
        let source_name = self.inner.source_name.clone();
        let login_url = self.inner.login_url.clone();
        let source_variable = self.inner.source_variable.clone();
        let source_header = self.inner.source_header.clone();
        let headers = self
            .inner
            .headers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let java_vars = self
            .inner
            .java_vars
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let doc = self
            .inner
            .doc
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.inner = Arc::new(JsBridgeInner {
            source_key,
            source_name,
            login_url,
            source_variable,
            source_header,
            ns: ns.into(),
            headers: Mutex::new(headers),
            java_vars: Mutex::new(java_vars),
            doc: Mutex::new(doc),
        });
        self
    }

    /// 用户命名空间
    pub fn ns(&self) -> &str {
        &self.inner.ns
    }

    /// 书源 key（URL）
    pub fn source_key(&self) -> &str {
        &self.inner.source_key
    }

    /// 书源名称
    pub fn source_name(&self) -> &str {
        &self.inner.source_name
    }

    /// 设置初始请求头（JS 中可通过 `java.headerMap` 改写）
    pub fn set_headers(&self, headers: HashMap<String, String>) {
        *self.inner.headers.lock().unwrap_or_else(|e| e.into_inner()) = headers;
    }

    /// 读取请求头（eval 后取 JS 改写结果，用于实际请求）
    pub fn headers(&self) -> HashMap<String, String> {
        self.inner
            .headers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Default for JsBridge {
    /// 空 bridge（旧 eval_js 兼容路径）：无书源信息、无请求头
    fn default() -> Self {
        Self::new("", "")
    }
}

/// 执行 JS 代码，返回字符串结果（旧签名，内部使用空 bridge）
///
/// 变量以全局属性注入；结果按 String() 语义转字符串
/// （null/undefined → 空串；数字/布尔 → 字面量，如 "42" / "true"）
pub fn eval_js(code: &str, vars: &HashMap<String, String>) -> Result<String> {
    eval_js_with_bridge(code, vars, &JsBridge::default())
}

/// JS 循环迭代上限（GAP #94：boa 死循环防卡死）。
/// boa 0.19 的 RuntimeLimits 无独立“指令数”上限，循环迭代计数（loop_iteration_limit）
/// 是其等价物：每次循环迭代 +1，超限抛 RangeError。10M 足够正常书源规则（一般 <1K 次）
/// 循环，而 `while(true){}` 这类死循环会在毫秒级内触发。
pub const JS_LOOP_ITERATION_LIMIT: u64 = 10_000_000;

/// 构造受限 Context（runtime limits：循环迭代上限，防死循环）
pub fn new_limited_context() -> Context {
    context_with_limit(JS_LOOP_ITERATION_LIMIT)
}

/// 以指定循环迭代上限构造 Context（测试用小上限快速触发）
fn context_with_limit(loop_limit: u64) -> Context {
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(loop_limit);
    context
}

// 最近一次 JS eval 失败的原始错误消息（线程局部——GAP 156：debug SSE 步骤输出用）。
// 每次 `map_js_error`（eval 失败统一出口）记录；debug 流程在步骤结束后
// `take_last_js_error` 取走并附加到步骤输出（错误消息 + JS 片段前 100 字符）。
// 注意：同步代码内取用（debug 步骤的 eval 与读取之间无 await，同线程安全）；
// 其他并发请求的 eval 在线程局部隔离，互不污染。
thread_local! {
    static LAST_JS_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// 取走（并清空）最近一次 JS eval 失败消息；无则 None。
pub fn take_last_js_error() -> Option<String> {
    LAST_JS_ERROR.with(|c| c.borrow_mut().take())
}

/// 将 boa 错误映射为友好文案：超限 → "JS 执行超限"；同时记录原始消息（debug 输出用）
fn map_js_error(e: boa_engine::JsError) -> anyhow::Error {
    let msg = e.to_string();
    LAST_JS_ERROR.with(|c| *c.borrow_mut() = Some(msg.clone()));
    if msg.to_lowercase().contains("loop iteration limit") {
        anyhow!("JS 执行超限")
    } else {
        anyhow!("JS 执行失败: {msg}")
    }
}

/// 执行 JS 代码并注入书源桥接（java.* / source.*，对齐 legado jsHelp）
pub fn eval_js_with_bridge(
    code: &str,
    vars: &HashMap<String, String>,
    bridge: &JsBridge,
) -> Result<String> {
    eval_js_with_bridge_limited(code, vars, bridge, JS_LOOP_ITERATION_LIMIT)
}

/// 指定循环迭代上限的桥接执行（核心；测试用小上限验证超限路径）
fn eval_js_with_bridge_limited(
    code: &str,
    vars: &HashMap<String, String>,
    bridge: &JsBridge,
    loop_limit: u64,
) -> Result<String> {
    // legacy 常见变量默认兜底（调用方未注入时避免 ReferenceError）
    let vars = {
        let mut v = vars.clone();
        for k in [
            "urlSearchSeries",
            "urlSearch",
            "url",
            "baseUrl",
            "headerMap",
            "result",
            "key",
            "page",
        ] {
            v.entry(k.to_string()).or_default();
        }
        v
    };
    let mut context = context_with_limit(loop_limit);
    install_globals(&mut context, bridge)?;
    inject_vars(&mut context, &vars)?;
    install_bridge(&mut context, bridge)?;
    auto_set_content(&vars, bridge);
    let result = context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(map_js_error)?;
    Ok(js_result_to_string(&result, &mut context))
}

/// 执行 JS 并返回结构化结果（serde_json::Value）
///
/// 数组/对象经递归转换（`js_value_to_json`）直接得到 JSON——避免 boa ToString 对
/// 数组元素对象输出 "[object Object]" 导致后续 JSON.parse 为空；若结果为字符串且
/// 可解析为 JSON（如 JS 内 `JSON.stringify(...)` 出口），自动解析为对应结构。
pub fn eval_js_json(code: &str, vars: &HashMap<String, String>) -> Result<JsonValue> {
    eval_js_json_with_bridge(code, vars, &JsBridge::default())
}

/// 带书源桥接的 JSON 版本（同 eval_js_json，注入 java.*/source.*）
pub fn eval_js_json_with_bridge(
    code: &str,
    vars: &HashMap<String, String>,
    bridge: &JsBridge,
) -> Result<JsonValue> {
    let mut context = context_with_limit(JS_LOOP_ITERATION_LIMIT);
    inject_vars(&mut context, vars)?;
    install_bridge(&mut context, bridge)?;
    let result = context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(map_js_error)?;
    let json = js_value_to_json(&result, &mut context)
        .map_err(|e| anyhow!("JS 结果 JSON 转换失败: {e}"))?;
    // 字符串结果：若为 JSON 文本则解析为结构（兼容 JSON.stringify 出口）
    if let JsonValue::String(s) = &json {
        if let Ok(parsed) = serde_json::from_str::<JsonValue>(s) {
            return Ok(parsed);
        }
    }
    Ok(json)
}

/// 指定循环迭代上限的 JSON 桥接执行（测试用小上限验证超限路径）
fn eval_js_json_with_bridge_limited(
    code: &str,
    vars: &HashMap<String, String>,
    bridge: &JsBridge,
    loop_limit: u64,
) -> Result<JsonValue> {
    let mut context = context_with_limit(loop_limit);
    install_globals(&mut context, bridge)?;
    inject_vars(&mut context, vars)?;
    install_bridge(&mut context, bridge)?;
    auto_set_content(vars, bridge);
    let result = context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(map_js_error)?;
    js_value_to_json(&result, &mut context).map_err(|e| anyhow!("JS 结果 JSON 转换失败: {e}"))
}

/// 执行 JS 表达式并返回 JsValue（供内部使用）
pub fn eval_js_value(code: &str, vars: &HashMap<String, String>) -> Result<JsValue> {
    let mut context = context_with_limit(JS_LOOP_ITERATION_LIMIT);
    install_globals(&mut context, &JsBridge::default())?;
    inject_vars(&mut context, vars)?;
    context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(map_js_error)
}

/// 注入变量为全局属性（boa 0.19：key/value 需经 JsString 转换）
fn inject_vars(context: &mut Context, vars: &HashMap<String, String>) -> Result<()> {
    for (k, v) in vars {
        context
            .register_global_property(
                JsString::from(k.as_str()),
                JsValue::from(JsString::from(v.as_str())),
                Attribute::all(),
            )
            .map_err(|e| anyhow!("JS 变量注入失败 [{k}]: {e}"))?;
    }
    Ok(())
}

/// 注册 java / source 全局对象
fn install_bridge(context: &mut Context, bridge: &JsBridge) -> Result<()> {
    let (java, source) = build_bridge_objects(bridge, context);
    context
        .register_global_property(JsString::from("java"), java, Attribute::all())
        .map_err(|e| anyhow!("java 对象注册失败: {e}"))?;
    context
        .register_global_property(JsString::from("source"), source, Attribute::all())
        .map_err(|e| anyhow!("source 对象注册失败: {e}"))?;
    Ok(())
}

/// 规则 JS 求值前自动 setContent（legado AnalyzeByJS 语义：注入 result 的 JS 规则
/// 自动把 result 设为当前解析文档——`java.getString/getElements` 无需先手动 setContent）
fn auto_set_content(vars: &HashMap<String, String>, bridge: &JsBridge) {
    if let Some(result) = vars.get("result") {
        if !result.is_empty() {
            *bridge.inner.doc.lock().unwrap_or_else(|e| e.into_inner()) = Some(result.clone());
        }
    }
}

/// 全局 shim（每个 eval 上下文安装一次，先于 vars 注入——vars 同名覆盖）：
/// - JS prelude：`Map()` 无 new 调用兼容（legado Rhino 允许；boa 0.19 报错）、
///   `cache` 内存对象、`unescape/escape`
/// - 默认全局变量：baseUrl/base_url/src/type/urlIP（缺省值，避免 ReferenceError）
/// - 全局对象/函数：cookie（removeCookie/getCookie/setCookie）、org.jsoup（Jsoup.parse）、
///   xGorgon（stub）、getWbiEnc（bilibili wbi 签名）、Reload（拉取远程 JS 文本）
fn install_globals(context: &mut Context, bridge: &JsBridge) -> Result<()> {
    let prelude = r#"
(function () {
  // Map() 无 new：legado Rhino 允许（boa 0.19 报 TypeError）——包装为可无 new 调用
  var NativeMap = Map;
  function MapShim(iterable) {
    var m = new NativeMap();
    if (iterable != null && typeof iterable[Symbol.iterator] === 'function') {
      var it = iterable[Symbol.iterator]();
      var step;
      while (!(step = it.next()).done) { m.set(step.value[0], step.value[1]); }
    }
    return m;
  }
  MapShim.prototype = NativeMap.prototype;
  try { globalThis.Map = MapShim; } catch (e) {}
  // cache：内存 KV（legado cache 全局）
  if (typeof cache === 'undefined') {
    globalThis.cache = (function () {
      var store = new NativeMap();
      return {
        get: function (k) { return store.get(k); },
        set: function (k, v) { store.set(k, v); },
        getFromMemory: function (k) { return store.get(k); },
        putToMemory: function (k, v) { store.set(k, v); },
        deleteMemory: function (k) { store.delete(k); },
        clear: function () { store.clear(); }
      };
    })();
  }
  if (typeof unescape !== 'function') {
    globalThis.unescape = function (s) { return decodeURIComponent(String(s)); };
    globalThis.escape = function (s) { return encodeURIComponent(String(s)); };
  }
})();
"#;
    context
        .eval(Source::from_bytes(prelude.as_bytes()))
        .map_err(map_js_error)?;

    // 默认全局变量（vars 注入同名覆盖——URL 构造/规则 eval 的显式 baseUrl 优先）
    let key = bridge.inner.source_key.clone();
    let host = url::Url::parse(&key)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
        .unwrap_or_default();
    for (name, value) in [
        ("baseUrl", key.clone()),
        ("base_url", key.clone()),
        ("src", key),
        ("type", String::new()),
        ("urlIP", host),
    ] {
        context
            .register_global_property(
                JsString::from(name),
                JsValue::from(JsString::from(value)),
                Attribute::all(),
            )
            .map_err(|e| anyhow!("JS 全局注入失败 [{name}]: {e}"))?;
    }

    // cookie 对象（书源 cookie 由爬虫层按用户命名空间管理——shim 为无操作接口）
    let cookie = ObjectInitializer::new(context)
        .function(
            unsafe { NativeFunction::from_closure(cookie_remove) },
            JsString::from("removeCookie"),
            1,
        )
        .function(
            unsafe { NativeFunction::from_closure(cookie_get) },
            JsString::from("getCookie"),
            1,
        )
        .function(
            unsafe { NativeFunction::from_closure(cookie_set) },
            JsString::from("setCookie"),
            3,
        )
        .function(
            unsafe { NativeFunction::from_closure(cookie_clear) },
            JsString::from("clearCookie"),
            1,
        )
        .build();
    context
        .register_global_property(JsString::from("cookie"), cookie, Attribute::all())
        .map_err(|e| anyhow!("cookie 对象注册失败: {e}"))?;

    // org.jsoup.Jsoup.parse(html)：Document/Elements shim（scraper 后端）
    let jsoup = ObjectInitializer::new(context)
        .function(
            unsafe { NativeFunction::from_closure(jsoup_parse) },
            JsString::from("parse"),
            1,
        )
        .build();
    let org = ObjectInitializer::new(context)
        .property(JsString::from("jsoup"), jsoup, Attribute::all())
        .build();
    context
        .register_global_property(JsString::from("org"), org, Attribute::all())
        .map_err(|e| anyhow!("org 对象注册失败: {e}"))?;

    // xGorgon：字节系签名 stub（真实算法不可用——返回空串，避免 ReferenceError）
    register_global_fn(context, "xGorgon", xgorgon_stub, 1)?;
    // getWbiEnc：bilibili wbi 签名（真实实现——nav 密钥 + mixinKey + wts/w_rid）
    register_global_fn(context, "getWbiEnc", get_wbi_enc, 1)?;
    // Reload(url)：拉取远程文本（书源远程 JS 加载模式）
    register_global_fn(context, "Reload", reload_fetch, 1)?;
    Ok(())
}

/// 注册全局函数
fn register_global_fn(
    context: &mut Context,
    name: &str,
    f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
    len: usize,
) -> Result<()> {
    let func =
        FunctionObjectBuilder::new(context.realm(), unsafe { NativeFunction::from_closure(f) })
            .name(name)
            .length(len)
            .build();
    context
        .register_global_property(JsString::from(name), func, Attribute::all())
        .map_err(|e| anyhow!("JS 全局函数注册失败 [{name}]: {e}"))
}

/// cookie.removeCookie(url)：书源 cookie 由爬虫层管理，shim 无操作（返回 undefined）
fn cookie_remove(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// cookie.getCookie(url)：返回空串（无 cookie 上下文）
fn cookie_get(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from("")))
}

/// cookie.setCookie(url, key, value)：无操作
fn cookie_set(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// cookie.clearCookie(url)：无操作
fn cookie_clear(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// xGorgon(...)：字节系请求签名 stub（无法复现——返回空串）
fn xgorgon_stub(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from("")))
}

/// Reload(url)：拉取远程内容（书源远程 JS 加载：`eval(String(Reload('...')))`）
fn reload_fetch(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = js_value_to_string(args.get_or_undefined(0), context);
    if url.is_empty() {
        return Err(js_native_error("Reload: url 不能为空"));
    }
    let fut_url = url.clone();
    let fut = async move {
        crate::service::crawler::fetch(&fut_url, &HashMap::new(), 15, "GET", None, None)
            .await
            .map(|r| r.body)
    };
    match block_on_task(fut, Duration::from_secs(60), "Reload") {
        Ok(body) => Ok(JsValue::from(JsString::from(body))),
        Err(e) => Err(js_native_error(format!("Reload 失败（{url}）: {e}"))),
    }
}

// ---- bilibili wbi 签名（getWbiEnc） ----

/// mixinKey 重排表（bilibili wbi 算法公开常量）
const WBI_MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// wbi 密钥缓存（img_key + sub_key + 获取时间戳；1 小时刷新）
static WBI_KEYS: LazyLock<Mutex<Option<(String, String, u64)>>> =
    LazyLock::new(|| Mutex::new(None));

/// 取 wbi 密钥（nav 接口；带缓存；失败 → None）
fn wbi_keys() -> Option<(String, String)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    {
        let cache = WBI_KEYS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((img, sub, ts)) = cache.as_ref() {
            if now - *ts < 3600 {
                return Some((img.clone(), sub.clone()));
            }
        }
    }
    let fut = async {
        let mut headers = HashMap::new();
        headers.insert(
            "User-Agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0".to_string(),
        );
        let resp = crate::service::crawler::fetch(
            "https://api.bilibili.com/x/web-interface/nav",
            &headers,
            10,
            "GET",
            None,
            None,
        )
        .await
        .ok()?;
        let json: JsonValue = serde_json::from_str(&resp.body).ok()?;
        let img = json
            .pointer("/data/wbi_img/img_url")
            .and_then(|v| v.as_str())?;
        let sub = json
            .pointer("/data/wbi_img/sub_url")
            .and_then(|v| v.as_str())?;
        Some((wbi_key_of(img), wbi_key_of(sub)))
    };
    let fetched = block_on_task(
        async move { Ok::<_, anyhow::Error>(fut.await) },
        Duration::from_secs(20),
        "getWbiEnc-nav",
    )
    .ok()
    .flatten();
    if let Some((img, sub)) = fetched {
        *WBI_KEYS.lock().unwrap_or_else(|e| e.into_inner()) = Some((img.clone(), sub.clone(), now));
        return Some((img, sub));
    }
    None
}

/// wbi 图片 URL → 密钥（去目录/扩展名）
fn wbi_key_of(url: &str) -> String {
    url.rsplit('/')
        .next()
        .unwrap_or(url)
        .split('.')
        .next()
        .unwrap_or("")
        .to_string()
}

/// mixinKey = 按重排表取 32 位
fn wbi_mixin_key(orig: &str) -> String {
    let bytes: Vec<char> = orig.chars().collect();
    WBI_MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .filter_map(|&i| bytes.get(i))
        .collect()
}

/// getWbiEnc(paramsObj)：bilibili wbi 签名 → 返回带 wts/w_rid 的 query 串
/// （密钥获取失败 → 返回排序 query + wts，不带 w_rid——不抛异常）
fn get_wbi_enc(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let params = match js_value_to_json(args.get_or_undefined(0), context) {
        Ok(JsonValue::Object(map)) => map,
        _ => return Err(js_native_error("getWbiEnc: 参数须为对象")),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut sorted: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (k, v) in params {
        let s = match v {
            JsonValue::String(s) => s,
            JsonValue::Number(n) => n.to_string(),
            JsonValue::Bool(b) => b.to_string(),
            _ => continue,
        };
        if !s.is_empty() {
            sorted.insert(k, s);
        }
    }
    sorted.insert("wts".to_string(), now.to_string());
    let query = sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    if let Some((img, sub)) = wbi_keys() {
        let mixin = wbi_mixin_key(&format!("{img}{sub}"));
        let w_rid = crate::util::md5::md5_encode(&format!("{query}{mixin}"));
        return Ok(JsValue::from(JsString::from(format!(
            "{query}&w_rid={w_rid}"
        ))));
    }
    Ok(JsValue::from(JsString::from(query)))
}

/// 构建 java / source 对象（ObjectInitializer：function 注册方法、property 挂子对象）
fn build_bridge_objects(bridge: &JsBridge, context: &mut Context) -> (JsObject, JsObject) {
    // java.headerMap：请求头 Map（底层为 bridge.headers，eval 后可读取）
    let mut header_map = ObjectInitializer::new(context);
    header_map
        .function(bind(bridge, header_map_put), JsString::from("put"), 2)
        .function(bind(bridge, header_map_get), JsString::from("get"), 1)
        .function(bind(bridge, header_map_size), JsString::from("size"), 0);
    let header_map = header_map.build();

    // java：put/get（临时变量）、log（tracing）、headerMap（请求头）、
    // encodeURI/ajax/startBrowserAwait/setContent/getString/getElements/getWebViewUA（legacy shim）
    // + 签名/编码/转换 shim：md5Encode/HMacHex/randomUUID/androidId/base64Encode/base64DecodeToString/
    //   hexDecodeToString/t2s/s2t/desEncodeToBase64String/get(url)/connect/head
    let mut java = ObjectInitializer::new(context);
    java.function(bind(bridge, java_put), JsString::from("put"), 2)
        .function(bind(bridge, java_get), JsString::from("get"), 1)
        .function(bind(bridge, java_log), JsString::from("log"), 1)
        .function(bind(bridge, java_toast), JsString::from("toast"), 1)
        .function(bind(bridge, java_toast), JsString::from("longToast"), 1)
        .function(bind(bridge, java_toast), JsString::from("shortToast"), 1)
        .function(
            unsafe { NativeFunction::from_closure(java_aes_decrypt) },
            JsString::from("aesBase64DecodeToString"),
            4,
        )
        .function(
            bind(bridge, java_encode_uri),
            JsString::from("encodeURI"),
            2,
        )
        .function(bind(bridge, java_ajax), JsString::from("ajax"), 2)
        .function(
            bind(bridge, java_start_browser_await),
            JsString::from("startBrowserAwait"),
            3,
        )
        .function(
            bind(bridge, java_set_content),
            JsString::from("setContent"),
            1,
        )
        .function(
            bind(bridge, java_get_string),
            JsString::from("getString"),
            1,
        )
        .function(
            bind(bridge, java_get_elements),
            JsString::from("getElements"),
            1,
        )
        .function(
            bind(bridge, java_get_webview_ua),
            JsString::from("getWebViewUA"),
            0,
        )
        // 签名/编码 shim（legado java.* 常见缺项——按报错消息逐项补齐）
        .function(
            bind(bridge, java_md5_encode),
            JsString::from("md5Encode"),
            1,
        )
        .function(bind(bridge, java_hmac_hex), JsString::from("HMacHex"), 3)
        .function(
            bind(bridge, java_random_uuid),
            JsString::from("randomUUID"),
            0,
        )
        .function(
            bind(bridge, java_android_id),
            JsString::from("androidId"),
            0,
        )
        .function(
            bind(bridge, java_base64_encode),
            JsString::from("base64Encode"),
            1,
        )
        .function(
            bind(bridge, java_base64_decode),
            JsString::from("base64DecodeToString"),
            1,
        )
        .function(
            bind(bridge, java_hex_decode),
            JsString::from("hexDecodeToString"),
            1,
        )
        .function(bind(bridge, java_t2s), JsString::from("t2s"), 1)
        .function(bind(bridge, java_s2t), JsString::from("s2t"), 1)
        .function(
            bind(bridge, java_des_encode),
            JsString::from("desEncodeToBase64String"),
            4,
        )
        .function(bind(bridge, java_connect), JsString::from("connect"), 1)
        .function(bind(bridge, java_head), JsString::from("head"), 2)
        .property(JsString::from("headerMap"), header_map, Attribute::all());
    let java = java.build();

    // source：getKey（书源 URL）/ getName（书源名）/ put/get（书源级变量）
    // + key/url（URL 别名）/ loginUrl（登录 JS）/ header（header 文本）/ getVariable（书源变量）
    let key = JsValue::from(JsString::from(bridge.inner.source_key.as_str()));
    let mut source = ObjectInitializer::new(context);
    source
        .function(bind(bridge, source_get_key), JsString::from("getKey"), 0)
        .function(bind(bridge, source_get_name), JsString::from("getName"), 0)
        .function(bind(bridge, source_put), JsString::from("put"), 2)
        .function(bind(bridge, source_get), JsString::from("get"), 1)
        .function(
            bind(bridge, source_get_variable),
            JsString::from("getVariable"),
            0,
        )
        .property(JsString::from("key"), key.clone(), Attribute::all())
        .property(JsString::from("url"), key.clone(), Attribute::all())
        .property(JsString::from("bookSourceUrl"), key, Attribute::all())
        .property(
            JsString::from("loginUrl"),
            JsValue::from(JsString::from(bridge.inner.login_url.as_str())),
            Attribute::all(),
        )
        .property(
            JsString::from("header"),
            JsValue::from(JsString::from(bridge.inner.source_header.as_str())),
            Attribute::all(),
        );
    let source = source.build();

    (java, source)
}

/// 将 bridge 状态绑定进 NativeFunction 闭包。
///
/// boa 0.19 的 `from_copy_closure_with_captures` 要求捕获类型实现 `Trace`，
/// 而 `std::sync::Mutex` 无 Trace 实现，故走 `from_closure`。
///
/// # Safety
///
/// `from_closure` 的不安全前提是「捕获变量含需 GC 追踪（Trace）的数据」；
/// 此处仅捕获 `Arc<JsBridgeInner>`，内部全是 String / Mutex<HashMap<String, String>>，
/// 不含 JsValue / JsObject / Gc 等需追踪数据，闭包生命周期由 Arc 管理，无 use-after-free。
fn bind<F>(bridge: &JsBridge, f: F) -> NativeFunction
where
    F: Fn(&JsBridgeInner, &[JsValue], &mut Context) -> JsResult<JsValue> + 'static,
{
    let inner = Arc::clone(&bridge.inner);
    unsafe { NativeFunction::from_closure(move |_this, args, ctx| f(&inner, args, ctx)) }
}

// ---- java.* 实现 ----

/// java.put(key, value)：bridge 生命周期内的临时变量
fn java_put(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = js_value_to_string(args.get_or_undefined(1), context);
    inner
        .java_vars
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, value);
    Ok(JsValue::undefined())
}

/// java.get(key)：读取临时变量，缺失返回 undefined。
/// java.get(url, opts) / java.get(httpUrl)：HTTP GET（legado jsHelp 兼容：
/// 无忧书城 `java.get(su,{}).headers('Location')`）——返回响应对象
fn java_get(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    if args.len() > 1 || key.starts_with("http://") || key.starts_with("https://") {
        return java_http_fetch(inner, &key, "GET", context);
    }
    let value = inner
        .java_vars
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    Ok(value.map_or_else(JsValue::undefined, |s| JsValue::from(JsString::from(s))))
}

/// java.log(msg)：tracing 日志（调试书源规则）
/// java.toast/longToast/shortToast：无 UI 环境提示（记日志）
fn java_toast(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let msg = args.first()
        .map(|v| js_value_to_string(v, context))
        .unwrap_or_default();
    tracing::debug!("java.toast: {}", msg);
    Ok(JsValue::undefined())
}

fn java_log(_inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let msg = js_value_to_string(args.get_or_undefined(0), context);
    tracing::info!(target: "reader.js", "[java.log] {}", msg);
    Ok(JsValue::undefined())
}

// ---- legacy shim：encodeURI / ajax / startBrowserAwait / setContent / getString / getElements / getWebViewUA ----

/// 构造 JS 异常（NativeFunction 内 throw，书源规则可 catch）
fn js_native_error(msg: impl Into<String>) -> JsError {
    JsNativeError::error().with_message(msg.into()).into()
}

/// 同步等待异步任务结果（boa 引擎为同步调用环境；`java.ajax`/`java.startBrowserAwait`
/// 需要等待网络/浏览器结果）。
///
/// 阻塞说明：书源 JS 通常在 tokio 工作线程（axum handler）中执行，本函数用专用
/// worker 线程 + 独立 current_thread tokio runtime 执行异步任务，当前线程经
/// std mpsc `recv_timeout` 阻塞等待（最多 `timeout`）。不在 ambient runtime 上直接
/// spawn——避免 current_thread runtime（如 `#[tokio::test]`）下「阻塞唯一 worker →
/// 任务永不被轮询」的死锁；阻塞仅限当前执行 JS 的线程，多线程 runtime 的其他
/// worker 不受影响。
fn block_on_task<T: Send + 'static>(
    fut: impl Future<Output = Result<T>> + Send + 'static,
    timeout: Duration,
    what: &str,
) -> Result<T> {
    let what = what.to_string();
    let what_msg = what.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<T>>();
    std::thread::Builder::new()
        .name(format!("reader-js-{what}"))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(anyhow!("{what_msg} 内部 runtime 创建失败: {e}")));
                    return;
                }
            };
            let _ = tx.send(rt.block_on(fut));
        })
        .map_err(|e| anyhow!("{what} worker 线程创建失败: {e}"))?;
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(anyhow!("{what} 超时（{}s）", timeout.as_secs()))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow!("{what} worker 异常退出"))
        }
    }
}

/// 内置浏览器可用性（`java.startBrowserAwait` 前置检查；测试钩子可强制覆盖）。
/// GAP 175：camoufox 后端启用（默认）时视为可用——求解链会在 CDP 失败/不可用时
/// 自动走 camoufox（HTTP 后端），前置检查不再因缺浏览器直接拦截。
fn js_browser_available() -> bool {
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering;
        match JS_BROWSER_AVAIL_OVERRIDE.load(Ordering::Relaxed) {
            1 => return true,
            -1 => return false,
            _ => {}
        }
    }
    crate::service::browser::is_browser_available() || crate::service::camoufox::enabled()
}

/// 测试钩子：强制浏览器可用性（Some(true)/Some(false) 强制；None 恢复自动探测）
#[cfg(test)]
fn force_js_browser_available(v: Option<bool>) {
    use std::sync::atomic::Ordering;
    JS_BROWSER_AVAIL_OVERRIDE.store(
        match v {
            Some(true) => 1,
            Some(false) => -1,
            None => 0,
        },
        Ordering::Relaxed,
    );
}

#[cfg(test)]
static JS_BROWSER_AVAIL_OVERRIDE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);

/// 测试注入：页面求解钩子（返回 (html, cookies, ua)）；None = 走真实浏览器
#[cfg(test)]
type SolveHook = dyn Fn(String, Vec<(String, String)>) -> Result<(String, Vec<(String, String)>, String)>
    + Send
    + Sync;

#[cfg(test)]
static JS_SOLVE_HOOK: LazyLock<Mutex<Option<Arc<SolveHook>>>> = LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
fn register_js_solve_hook(hook: Option<Arc<SolveHook>>) {
    *JS_SOLVE_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = hook;
}

/// 页面求解（`java.startBrowserAwait` 后端）：带书源 cookie 启动内置浏览器加载页面并
/// 等待加载完成，返回 (html, cookies, user_agent)。
///
/// 接入 `browser::solve_captcha`（统一验证码求解入口：CF JS 质询 / Turnstile / 滑块
/// 自动处理；进程内 CDP，会话级浏览器实例惰性启动/复用/异常自动重启）。
async fn solve_page(
    ns: String,
    url: String,
    cookies: Vec<(String, String)>,
    _title: String,
) -> Result<(String, Vec<(String, String)>, String)> {
    #[cfg(test)]
    {
        let hook = JS_SOLVE_HOOK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(hook) = hook {
            return hook(url, cookies);
        }
        // 测试环境：未注册求解钩子时直接报错——禁止测试路径启动真实浏览器
        Err(anyhow!("测试环境未注册页面求解钩子（不会启动真实浏览器）"))
    }
    #[cfg(not(test))]
    {
        let sol = crate::service::browser::solve_captcha(&ns, &url, &cookies, 60_000)
            .await
            .map_err(|e| anyhow!("浏览器加载失败（{url}）: {e:#}"))?;
        Ok((sol.html, sol.cookies, sol.user_agent))
    }
}

/// `java.startBrowserAwait(url, title, isForeground)`：内置浏览器打开 url（vUrl 已含
/// query）并等待加载完成，返回 JS 对象 `{body: 页面 html, cookies: ["name=value",...],
/// status: 200}`（boa 对象构造）。
/// - 带书源既有 cookie（按用户命名空间 + url base 匹配，与 crawler 一致）
/// - headless 环境无前台概念，isForeground 仅兼容占位
/// - 浏览器不可用 / 加载失败 / 超时（60s）→ 抛 JS 异常（书源规则可 catch）
fn java_start_browser_await(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = js_value_to_string(args.get_or_undefined(0), context);
    let title = js_value_to_string(args.get_or_undefined(1), context);
    let _is_foreground = args.get_or_undefined(2).to_boolean();
    if url.is_empty() {
        return Err(js_native_error("java.startBrowserAwait: url 不能为空"));
    }
    if !js_browser_available() {
        return Err(js_native_error("java.startBrowserAwait 失败：obscura 浏览器不可用（唯一浏览器后端）——请下载 obscura stealth 构建并设置 READER_OBSCURA_BIN（或配置 READER_OBSCURA_URL 连接既有 CDP 服务，或配置 FLARESOLVERR_URL 走 FlareSolverr 路径）".to_string()));
    }
    let ns = inner.ns.clone();
    let fut_url = url.clone();
    let fut = async move {
        // 书源既有 cookie（按用户命名空间；无注册/未命中 → 空）
        let cookie_str = crate::service::crawler::cookie_for(&ns, &fut_url)
            .await
            .unwrap_or_default();
        let cookies = crate::service::crawler::parse_cookie_string(&cookie_str);
        solve_page(ns, fut_url, cookies, title).await
    };
    let (html, cookies, _user_agent) =
        match block_on_task(fut, Duration::from_secs(60), "java.startBrowserAwait") {
            Ok(v) => v,
            Err(e) => {
                return Err(js_native_error(format!(
                    "java.startBrowserAwait 失败（{url}）: {e}"
                )))
            }
        };
    // JS 对象：{body: html, cookies: ["a=1", ...], status: 200}（先构造数组再建对象，
    // 避免 ObjectInitializer 与 from_iter 同时可变借用 context）
    let cookies_arr = JsArray::from_iter(
        cookies
            .into_iter()
            .map(|(k, v)| JsValue::from(JsString::from(format!("{k}={v}")))),
        context,
    );
    let mut obj = ObjectInitializer::new(context);
    obj.property(
        JsString::from("body"),
        JsValue::from(JsString::from(html)),
        Attribute::all(),
    )
    .property(JsString::from("cookies"), cookies_arr, Attribute::all())
    .property(
        JsString::from("status"),
        JsValue::from(200),
        Attribute::all(),
    );
    Ok(obj.build().into())
}

/// `java.encodeURI(str, charset)`：按 charset（默认 utf-8；gbk/gb2312 走 encoding_rs）
/// 对字符串做 URL 百分号编码。encodeURI 语义：ASCII 字母数字与 `-_.!~*'()` 及
/// `;/?:@&=+$,#` 保留不编码；其余字节逐个 `%XX`（大写）。
fn java_encode_uri(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    let charset = args
        .get(1)
        .map(|v| js_value_to_string(v, context))
        .unwrap_or_default();
    let encoding =
        encoding_rs::Encoding::for_label(charset.trim().as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (bytes, _, _) = encoding.encode(&s);
    Ok(JsValue::from(JsString::from(percent_encode_uri(&bytes))))
}

/// encodeURI 语义百分号编码（保留 unreserved + `;/?:@&=+$,#`）
fn percent_encode_uri(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'!'
                    | b'~'
                    | b'*'
                    | b'\''
                    | b'('
                    | b')'
                    | b','
                    | b';'
                    | b'/'
                    | b'?'
                    | b':'
                    | b'@'
                    | b'&'
                    | b'='
                    | b'+'
                    | b'$'
                    | b'#'
            )
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `url,{...}` 后缀（同 crawler/search 解析：method/body/charset/headers）
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AjaxSuffix {
    method: Option<String>,
    body: Option<String>,
    charset: Option<String>,
    headers: Option<HashMap<String, String>>,
}

/// 切分 `url,{...}` 后缀：从最后一个「逗号后整段为合法 JSON」的位置切分
/// （对齐 search::split_url_suffix）
fn parse_ajax_suffix(url: &str) -> (String, AjaxSuffix) {
    let mut split: Option<(usize, AjaxSuffix)> = None;
    for (i, ch) in url.char_indices() {
        if ch != ',' {
            continue;
        }
        let rest = url[i + 1..].trim_start();
        if !rest.starts_with('{') {
            continue;
        }
        if let Ok(suffix) = serde_json::from_str::<AjaxSuffix>(rest) {
            split = Some((i, suffix));
        }
    }
    match split {
        Some((i, suffix)) => (url[..i].to_string(), suffix),
        None => (url.to_string(), AjaxSuffix::default()),
    }
}

/// `java.ajax(urlOrSpec)`：带书源 cookie 的同步请求（阻塞等待结果），返回响应体文本。
/// - 支持 `url,{...}` 后缀（method/body/charset/headers，同 crawler 解析）
/// - url 为空 → 用书源 URL（baseUrl）兜底（legado 语义；兼容 `java.ajax(source.key)` 类写法）
/// - 请求头基底为 `java.headerMap`（书源 header）+ 后缀 headers + 书源 cookie
/// - 可选第 2 参：超时秒数（legado callTimeout 兼容；默认 15s，上限 60s）
/// - 失败/超时 → 抛 JS 异常
fn java_ajax(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url_spec = js_value_to_string(args.get_or_undefined(0), context);
    let timeout_secs = args
        .get(1)
        .and_then(|v| v.as_number())
        .map(|n| (n as u64).clamp(1, 60))
        .unwrap_or(15);
    let (mut url, suffix) = parse_ajax_suffix(&url_spec);
    if url.is_empty() {
        // 空 url 兜底：书源 URL（legado：java.ajax 空参/undefined → 书源地址）
        url = inner.source_key.clone();
    }
    let ns = inner.ns.clone();
    // 请求头基底：java.headerMap（书源 header，JS 可改写）——async 块前克隆（'static）
    let headers_base = inner
        .headers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let fut_url = url.clone();
    let fut = async move {
        let mut headers = headers_base;
        if let Some(extra) = &suffix.headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }
        // 书源 cookie（按用户命名空间；无注册/未命中 → 不带）
        if let Some(cookie_str) = crate::service::crawler::cookie_for(&ns, &fut_url).await {
            if !cookie_str.is_empty() {
                headers.insert("Cookie".to_string(), cookie_str);
            }
        }
        let method = suffix.method.as_deref().unwrap_or("GET");
        let resp = crate::service::crawler::fetch(
            &fut_url,
            &headers,
            timeout_secs,
            method,
            suffix.body.as_deref(),
            suffix.charset.as_deref(),
        )
        .await?;
        Ok::<_, anyhow::Error>(resp.body)
    };
    let body = match block_on_task(fut, Duration::from_secs(60), "java.ajax") {
        Ok(b) => b,
        Err(e) => return Err(js_native_error(format!("java.ajax 失败（{url}）: {e}"))),
    };
    Ok(JsValue::from(JsString::from(body)))
}

/// `java.setContent(html)`：设置当前解析文档（后续 getString/getElements 的解析源）
fn java_set_content(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let html = js_value_to_string(args.get_or_undefined(0), context);
    *inner.doc.lock().unwrap_or_else(|e| e.into_inner()) = Some(html);
    Ok(JsValue::undefined())
}

/// 当前文档（未 setContent → 明确错误提示）
fn current_doc(inner: &JsBridgeInner) -> JsResult<String> {
    inner
        .doc
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| js_native_error("尚未设置文档内容：请先调用 java.setContent(html)"))
}

/// 规则末段是否为属性/文本提取器（与 css_chain::is_attr_extractor 对齐）
fn rule_ends_with_extractor(rule: &str) -> bool {
    rule.split('@')
        .next_back()
        .map(|s| {
            matches!(
                s.trim(),
                "text"
                    | "textNodes"
                    | "ownText"
                    | "html"
                    | "all"
                    | "href"
                    | "src"
                    | "value"
                    | "data-src"
                    | "data-original"
                    | "data-url"
            )
        })
        .unwrap_or(false)
}

/// `java.getString(rule)`：对已存文档用 css_chain 规则求值，返回首个结果文本。
/// 规则末段是 `@text/@href` 等提取器时返回提取值；否则结果是元素 outerHTML，
/// 转文本（对齐 legado getString 的 getText 语义）；无匹配返回空串。
fn java_get_string(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let rule = js_value_to_string(args.get_or_undefined(0), context);
    let doc = current_doc(inner)?;
    let results = crate::parser::css_chain::css_chain(&rule, &doc);
    let text = match results.first() {
        Some(first) => {
            if rule_ends_with_extractor(&rule) {
                first.clone()
            } else {
                // 元素 HTML → 文本（对齐 search::field 语义）
                let f = scraper::Html::parse_fragment(first);
                let t = f
                    .root_element()
                    .text()
                    .collect::<String>()
                    .trim()
                    .to_string();
                if t.is_empty() {
                    first.clone()
                } else {
                    t
                }
            }
        }
        None => String::new(),
    };
    Ok(JsValue::from(JsString::from(text)))
}

/// `java.getElements(rule)`：对已存文档用 css_chain 规则求值。
/// - 规则带 `@` 提取器（@text/@href 等）→ 返回提取值字符串数组（原语义）
/// - 纯选择器规则 → 返回元素对象数组（jsoup 语义：`el.select(css)`/`el.text()`/`el.attr(name)`）
fn java_get_elements(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let rule = js_value_to_string(args.get_or_undefined(0), context);
    let doc = current_doc(inner)?;
    let results = crate::parser::css_chain::css_chain(&rule, &doc);
    if rule_ends_with_extractor(&rule) {
        let arr = JsArray::from_iter(
            results
                .into_iter()
                .map(|s| JsValue::from(JsString::from(s))),
            context,
        );
        return Ok(arr.into());
    }
    // 元素对象数组（jsoup Elements：select/text/attr/first/get/size/eachAttr/eachText/html/val）
    jsoup_elements_from_htmls(results, context)
}

/// `java.getWebViewUA()`：固定浏览器 UA（见 `JS_WEBVIEW_UA`）
fn java_get_webview_ua(
    _inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(JS_WEBVIEW_UA)))
}

// ---- 签名/编码/转换 shim（legado java.* 常见缺项） ----

/// java.md5Encode(str)：md5 十六进制（小写）
fn java_md5_encode(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    Ok(JsValue::from(JsString::from(crate::util::md5::md5_encode(
        &s,
    ))))
}

/// java.HMacHex(data, algo, key)：HMAC 十六进制（HmacMD5/HmacSHA1/HmacSHA256/HmacSHA512）
fn java_hmac_hex(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = js_value_to_string(args.get_or_undefined(0), context);
    let algo = js_value_to_string(args.get_or_undefined(1), context);
    let key = js_value_to_string(args.get_or_undefined(2), context);
    use hmac::{Mac, SimpleHmac};
    let out: String = match algo.to_lowercase().as_str() {
        "hmacmd5" => hex::encode(
            SimpleHmac::<md5::Md5>::new_from_slice(key.as_bytes())
                .map_err(|_| js_native_error("HMacHex: 密钥长度非法"))?
                .chain_update(data.as_bytes())
                .finalize()
                .into_bytes(),
        ),
        "hmacsha1" => hex::encode(
            SimpleHmac::<sha1::Sha1>::new_from_slice(key.as_bytes())
                .map_err(|_| js_native_error("HMacHex: 密钥长度非法"))?
                .chain_update(data.as_bytes())
                .finalize()
                .into_bytes(),
        ),
        "hmacsha256" => hex::encode(
            SimpleHmac::<sha2::Sha256>::new_from_slice(key.as_bytes())
                .map_err(|_| js_native_error("HMacHex: 密钥长度非法"))?
                .chain_update(data.as_bytes())
                .finalize()
                .into_bytes(),
        ),
        "hmacsha512" => hex::encode(
            SimpleHmac::<sha2::Sha512>::new_from_slice(key.as_bytes())
                .map_err(|_| js_native_error("HMacHex: 密钥长度非法"))?
                .chain_update(data.as_bytes())
                .finalize()
                .into_bytes(),
        ),
        other => return Err(js_native_error(format!("HMacHex: 不支持的算法 {other}"))),
    };
    Ok(JsValue::from(JsString::from(out)))
}

/// java.randomUUID()：UUID v4 对象（toString() → 连字符小写）
fn java_random_uuid(
    _inner: &JsBridgeInner,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let u = uuid::Uuid::new_v4().to_string();
    let mut obj = ObjectInitializer::new(context);
    obj.function(
        unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                Ok(JsValue::from(JsString::from(u.clone())))
            })
        },
        JsString::from("toString"),
        0,
    );
    Ok(obj.build().into())
}

/// java.androidId()：稳定设备 ID（legado Android 环境常量）
fn java_android_id(
    _inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from("9774d56d682e549c")))
}

/// java.base64Encode(str)
fn java_base64_encode(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    Ok(JsValue::from(JsString::from(
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes()),
    )))
}

/// java.base64DecodeToString(str)
fn java_base64_decode(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    match base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) {
        Ok(bytes) => Ok(JsValue::from(JsString::from(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Err(_) => Err(js_native_error(
            "java.base64DecodeToString: base64 解码失败",
        )),
    }
}

/// java.hexDecodeToString(str)：十六进制 → 文本
fn java_hex_decode(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    match hex::decode(s.trim()) {
        Ok(bytes) => Ok(JsValue::from(JsString::from(
            String::from_utf8_lossy(&bytes).into_owned(),
        ))),
        Err(_) => Err(js_native_error("java.hexDecodeToString: 十六进制解码失败")),
    }
}

/// java.t2s(str)：繁体 → 简体（zhconv 表）
fn java_t2s(_inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    Ok(JsValue::from(JsString::from(zhconv::zhconv(
        &s,
        zhconv::Variant::ZhHans,
    ))))
}

/// java.s2t(str)：简体 → 繁体
fn java_s2t(_inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let s = js_value_to_string(args.get_or_undefined(0), context);
    Ok(JsValue::from(JsString::from(zhconv::zhconv(
        &s,
        zhconv::Variant::ZhHant,
    ))))
}

/// java.desEncodeToBase64String(data, key, mode, iv)：DES/ECB/PKCS5 加密 → base64
/// （仅支持 ECB 模式；key 取前 8 字节）
fn java_des_encode(
    _inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = js_value_to_string(args.get_or_undefined(0), context);
    let key = js_value_to_string(args.get_or_undefined(1), context);
    let mode = js_value_to_string(args.get_or_undefined(2), context);
    if !mode.to_uppercase().contains("ECB") {
        return Err(js_native_error(
            "java.desEncodeToBase64String: 仅支持 DES/ECB 模式",
        ));
    }
    let mut key_bytes = key.as_bytes().to_vec();
    key_bytes.resize(8, 0);
    use des::cipher::{Block, BlockCipherEncrypt, KeyInit};
    let cipher = des::Des::new_from_slice(&key_bytes[..8])
        .map_err(|e| js_native_error(format!("java.desEncodeToBase64String: {e}")))?;
    // PKCS5/7 padding
    let mut padded = data.as_bytes().to_vec();
    let pad = 8 - (padded.len() % 8);
    padded.extend(std::iter::repeat_n(pad as u8, pad));
    let mut out = Vec::with_capacity(padded.len());
    for chunk in padded.chunks(8) {
        let mut block = Block::<des::Des>::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    Ok(JsValue::from(JsString::from(
        base64::engine::general_purpose::STANDARD.encode(out),
    )))
}

/// 同步 HTTP 请求（java.get(url)/connect(url)/head(url) 共用）：返回响应对象
fn java_http_fetch(
    inner: &JsBridgeInner,
    url: &str,
    method: &str,
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = url.to_string();
    if url.is_empty() {
        return Err(js_native_error("java HTTP 请求: url 不能为空"));
    }
    let ns = inner.ns.clone();
    let headers_base = inner
        .headers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let method = method.to_string();
    let fut_method = method.clone();
    let fut_url = url.clone();
    let fut = async move {
        let mut headers = headers_base;
        if let Some(cookie_str) = crate::service::crawler::cookie_for(&ns, &fut_url).await {
            if !cookie_str.is_empty() {
                headers.insert("Cookie".to_string(), cookie_str);
            }
        }
        crate::service::crawler::fetch(&fut_url, &headers, 15, &fut_method, None, None).await
    };
    let resp = match block_on_task(fut, Duration::from_secs(60), "java-http") {
        Ok(r) => r,
        Err(e) => return Err(js_native_error(format!("java {method} 失败（{url}）: {e}"))),
    };
    http_response_object(resp, context)
}

/// 构造 HTTP 响应 JS 对象（legado okhttp 语义子集）：
/// raw() → {request(){url()}, code()}；header(name)；headers(name)→数组；body()/html()；
/// json()；code()；url()；cookies()→{}；toString() → body
fn http_response_object(
    resp: crate::service::crawler::FetchResponse,
    context: &mut Context,
) -> JsResult<JsValue> {
    let body = resp.body.clone();
    let final_url = resp.url.clone();
    let status = resp.status;
    let headers: Vec<(String, String)> = resp.headers.clone();
    let headers_arc = Arc::new(headers);

    let mut raw = ObjectInitializer::new(context);
    {
        let url = final_url.clone();
        let code = status;
        raw.function(
            unsafe {
                NativeFunction::from_closure(move |_this, _args, ctx| {
                    let mut req = ObjectInitializer::new(ctx);
                    let u = url.clone();
                    req.function(
                        unsafe {
                            NativeFunction::from_closure(move |_t, _a, _c| {
                                Ok(JsValue::from(JsString::from(u.clone())))
                            })
                        },
                        JsString::from("url"),
                        0,
                    );
                    let c = code;
                    req.function(
                        unsafe {
                            NativeFunction::from_closure(move |_t, _a, _c| Ok(JsValue::from(c)))
                        },
                        JsString::from("code"),
                        0,
                    );
                    Ok(req.build().into())
                })
            },
            JsString::from("request"),
            0,
        );
    }
    raw.function(
        unsafe { NativeFunction::from_closure(move |_t, _a, _c| Ok(JsValue::from(status))) },
        JsString::from("code"),
        0,
    );
    let raw = raw.build();

    let mut obj = ObjectInitializer::new(context);
    obj.property(JsString::from("raw"), raw, Attribute::all());
    {
        let headers = Arc::clone(&headers_arc);
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let name = js_value_to_string(args.get_or_undefined(0), ctx).to_lowercase();
                    let v = headers
                        .iter()
                        .find(|(k, _)| k == &name)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(v)))
                })
            },
            JsString::from("header"),
            1,
        );
    }
    {
        let headers = Arc::clone(&headers_arc);
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let name = js_value_to_string(args.get_or_undefined(0), ctx).to_lowercase();
                    let vals: Vec<JsValue> = headers
                        .iter()
                        .filter(|(k, _)| k == &name)
                        .map(|(_, v)| JsValue::from(JsString::from(v.clone())))
                        .collect();
                    Ok(JsArray::from_iter(vals, ctx).into())
                })
            },
            JsString::from("headers"),
            1,
        );
    }
    {
        let body = body.clone();
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(body.clone())))
                })
            },
            JsString::from("body"),
            0,
        );
    }
    {
        let body = body.clone();
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(body.clone())))
                })
            },
            JsString::from("html"),
            0,
        );
    }
    {
        let body = body.clone();
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, ctx| {
                    match serde_json::from_str::<JsonValue>(&body) {
                        Ok(json) => JsValue::from_json(&json, ctx)
                            .map_err(|e| js_native_error(format!("json(): {e}"))),
                        Err(_) => Err(js_native_error("json(): 响应体不是合法 JSON")),
                    }
                })
            },
            JsString::from("json"),
            0,
        );
    }
    {
        let url = final_url.clone();
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(url.clone())))
                })
            },
            JsString::from("url"),
            0,
        );
    }
    obj.function(
        unsafe {
            NativeFunction::from_closure(move |_t, _a, _c| {
                // cookies()：空对象（cookie 由爬虫层管理）
                Ok(ObjectInitializer::new(_c).build().into())
            })
        },
        JsString::from("cookies"),
        0,
    );
    {
        let body = body.clone();
        obj.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(body.clone())))
                })
            },
            JsString::from("toString"),
            0,
        );
    }
    Ok(obj.build().into())
}

/// java.connect(url)：HTTP GET（legado okhttp 连接对象——raw().request().url() 链）
fn java_connect(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = js_value_to_string(args.get_or_undefined(0), context);
    java_http_fetch(inner, &url, "GET", context)
}

/// java.head(url, headers)：HTTP HEAD（cookies() 兼容）
fn java_head(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = js_value_to_string(args.get_or_undefined(0), context);
    java_http_fetch(inner, &url, "HEAD", context)
}

// ---- org.jsoup shim（scraper 后端） ----

/// org.jsoup.Jsoup.parse(html)：Document 对象（select/text/title/html/toString）
fn jsoup_parse(_this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let html = js_value_to_string(_args.get_or_undefined(0), context);
    jsoup_document(&html, context)
}

/// Document 对象
fn jsoup_document(html: &str, context: &mut Context) -> JsResult<JsValue> {
    let html = html.to_string();
    let mut doc = ObjectInitializer::new(context);
    {
        let h = html.clone();
        doc.function(
            unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let css = js_value_to_string(args.get_or_undefined(0), ctx);
                    jsoup_select(&h, &css, ctx)
                })
            },
            JsString::from("select"),
            1,
        );
    }
    {
        let h = html.clone();
        doc.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _ctx| {
                    let parsed = scraper::Html::parse_document(&h);
                    let txt = parsed.root_element().text().collect::<String>();
                    Ok(JsValue::from(JsString::from(txt)))
                })
            },
            JsString::from("text"),
            0,
        );
    }
    {
        let h = html.clone();
        doc.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _ctx| {
                    let parsed = scraper::Html::parse_document(&h);
                    let title = scraper::Selector::parse("title")
                        .ok()
                        .and_then(|sel| parsed.select(&sel).next())
                        .map(|e| e.text().collect::<String>())
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(title)))
                })
            },
            JsString::from("title"),
            0,
        );
    }
    {
        let h = html.clone();
        doc.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(h.clone())))
                })
            },
            JsString::from("toString"),
            0,
        );
    }
    Ok(doc.build().into())
}

/// Elements 选择（css 选择器 → 元素对象数组 + jsoup 方法）
fn jsoup_select(html: &str, css: &str, context: &mut Context) -> JsResult<JsValue> {
    let parsed = scraper::Html::parse_fragment(html);
    let Ok(selector) = scraper::Selector::parse(css) else {
        return Ok(JsArray::new(context).into()); // 非法选择器 → 空 Elements（不抛错）
    };
    let htmls: Vec<String> = parsed.select(&selector).map(|e| e.html()).collect();
    jsoup_elements_from_htmls(htmls, context)
}

/// 由元素 HTML 列表构建 jsoup Elements（数组 + attr/text/first/get/size/eachAttr/eachText/html/val）
fn jsoup_elements_from_htmls(htmls: Vec<String>, context: &mut Context) -> JsResult<JsValue> {
    let arr = JsArray::new(context);
    for h in &htmls {
        let el = jsoup_element(h, context)?;
        arr.push(el, context)?;
    }
    let htmls = Arc::new(htmls);
    // attr(name)：首元素属性
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("attr"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let name = js_value_to_string(args.get_or_undefined(0), ctx);
                    Ok(JsValue::from(JsString::from(first_attr(&htmls, &name))))
                })
            })
            .name("attr")
            .length(1)
            .build(),
            true,
            context,
        )?;
    }
    // text()：首元素文本
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("text"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, _ctx| {
                    Ok(JsValue::from(JsString::from(first_text(&htmls))))
                })
            })
            .name("text")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // first()：首元素对象（无 → null）
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("first"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, ctx| match htmls.first() {
                    Some(h) => jsoup_element(h, ctx),
                    None => Ok(JsValue::null()),
                })
            })
            .name("first")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // get(i)：第 i 个元素对象（越界 → null）
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("get"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let i = args.get_or_undefined(0).to_i32(ctx).unwrap_or(0);
                    match htmls.get(i.max(0) as usize) {
                        Some(h) => jsoup_element(h, ctx),
                        None => Ok(JsValue::null()),
                    }
                })
            })
            .name("get")
            .length(1)
            .build(),
            true,
            context,
        )?;
    }
    // size()：数量
    {
        let n = htmls.len() as i32;
        arr.set(
            JsString::from("size"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| Ok(JsValue::from(n)))
            })
            .name("size")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // eachAttr(name)：各元素属性值数组
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("eachAttr"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let name = js_value_to_string(args.get_or_undefined(0), ctx);
                    let vals: Vec<JsValue> = htmls
                        .iter()
                        .map(|h| JsValue::from(JsString::from(attr_of(h, &name))))
                        .collect();
                    Ok(JsArray::from_iter(vals, ctx).into())
                })
            })
            .name("eachAttr")
            .length(1)
            .build(),
            true,
            context,
        )?;
    }
    // eachText()：各元素文本数组
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("eachText"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, ctx| {
                    let vals: Vec<JsValue> = htmls
                        .iter()
                        .map(|h| JsValue::from(JsString::from(text_of(h))))
                        .collect();
                    Ok(JsArray::from_iter(vals, ctx).into())
                })
            })
            .name("eachText")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // html()：首元素 innerHTML
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("html"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, _ctx| {
                    Ok(JsValue::from(JsString::from(inner_html_of(
                        htmls.first().map(String::as_str).unwrap_or(""),
                    ))))
                })
            })
            .name("html")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // val()：首元素 value 属性/文本
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("val"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, _ctx| {
                    let v = htmls
                        .first()
                        .map(|h| {
                            let a = attr_of(h, "value");
                            if a.is_empty() {
                                text_of(h)
                            } else {
                                a
                            }
                        })
                        .unwrap_or_default();
                    Ok(JsValue::from(JsString::from(v)))
                })
            })
            .name("val")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    // toString()/outerHtml()：全部元素 HTML 拼接
    {
        let htmls = Arc::clone(&htmls);
        arr.set(
            JsString::from("toString"),
            FunctionObjectBuilder::new(context.realm(), unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(htmls.join(""))))
                })
            })
            .name("toString")
            .length(0)
            .build(),
            true,
            context,
        )?;
    }
    Ok(arr.into())
}

/// jsoup 元素对象（select/attr/text/html/val/ownText/toString）
fn jsoup_element(html: &str, context: &mut Context) -> JsResult<JsValue> {
    let html = html.to_string();
    let mut el = ObjectInitializer::new(context);
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let css = js_value_to_string(args.get_or_undefined(0), ctx);
                    jsoup_select(&h, &css, ctx)
                })
            },
            JsString::from("select"),
            1,
        );
    }
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, args, ctx| {
                    let name = js_value_to_string(args.get_or_undefined(0), ctx);
                    Ok(JsValue::from(JsString::from(attr_of(&h, &name))))
                })
            },
            JsString::from("attr"),
            1,
        );
    }
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(text_of(&h))))
                })
            },
            JsString::from("text"),
            0,
        );
    }
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(inner_html_of(&h))))
                })
            },
            JsString::from("html"),
            0,
        );
    }
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    let v = attr_of(&h, "value");
                    Ok(JsValue::from(JsString::from(if v.is_empty() {
                        text_of(&h)
                    } else {
                        v
                    })))
                })
            },
            JsString::from("val"),
            0,
        );
    }
    {
        let h = html.clone();
        el.function(
            unsafe {
                NativeFunction::from_closure(move |_t, _a, _c| {
                    Ok(JsValue::from(JsString::from(h.clone())))
                })
            },
            JsString::from("toString"),
            0,
        );
    }
    Ok(el.build().into())
}

/// 元素 HTML → 首个元素属性值
fn attr_of(html: &str, name: &str) -> String {
    let parsed = scraper::Html::parse_fragment(html);
    parsed.root_element().attr(name).unwrap_or("").to_string()
}

/// 元素 HTML → 文本
fn text_of(html: &str) -> String {
    let parsed = scraper::Html::parse_fragment(html);
    parsed
        .root_element()
        .text()
        .collect::<String>()
        .trim()
        .to_string()
}

/// 元素 HTML → innerHTML
fn inner_html_of(html: &str) -> String {
    let parsed = scraper::Html::parse_fragment(html);
    parsed.root_element().inner_html()
}

/// 首个元素属性值（Elements.attr）
fn first_attr(htmls: &[String], name: &str) -> String {
    htmls.first().map(|h| attr_of(h, name)).unwrap_or_default()
}

/// 首个元素文本（Elements.text）
fn first_text(htmls: &[String]) -> String {
    htmls.first().map(|h| text_of(h)).unwrap_or_default()
}

// ---- java.headerMap.* 实现（请求头 Map）----

/// java.headerMap.put(key, value)
fn header_map_put(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = js_value_to_string(args.get_or_undefined(1), context);
    inner
        .headers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, value);
    Ok(JsValue::undefined())
}

/// java.headerMap.get(key)
fn header_map_get(
    inner: &JsBridgeInner,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = inner
        .headers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    Ok(value.map_or_else(JsValue::undefined, |s| JsValue::from(JsString::from(s))))
}

/// java.headerMap.size()
fn header_map_size(
    inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let size = inner
        .headers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .len();
    Ok(JsValue::from(size as i32))
}

// ---- source.* 实现 ----

/// source.getKey()：书源 key（URL）
fn source_get_key(
    inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(inner.source_key.as_str())))
}

/// source.getName()：书源名称
fn source_get_name(
    inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(inner.source_name.as_str())))
}

/// source.put(key, value)：书源级变量（全局存储，按书源 key 隔离，
/// 跨搜索/详情调用可见）
fn source_put(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = js_value_to_string(args.get_or_undefined(1), context);
    SOURCE_VARS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(inner.source_key.clone())
        .or_default()
        .insert(key, value);
    Ok(JsValue::undefined())
}

/// source.get(key)：读取书源级变量，缺失返回 undefined
fn source_get(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = SOURCE_VARS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&inner.source_key)
        .and_then(|m| m.get(&key).cloned());
    Ok(value.map_or_else(JsValue::undefined, |s| JsValue::from(JsString::from(s))))
}

/// source.getVariable()：书源变量（legado 书源变量配置；无 → 空串）
fn source_get_variable(
    inner: &JsBridgeInner,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(
        inner.source_variable.as_str(),
    )))
}

/// JsValue → 字符串（对齐 String() 语义：数字/布尔 → 字面量；
/// null/undefined → 空串，对齐 legado 空结果语义；对象 → toString()）
fn js_value_to_string(v: &JsValue, context: &mut Context) -> String {
    match v {
        JsValue::Null | JsValue::Undefined => String::new(),
        _ => v
            .to_string(context)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default(),
    }
}

/// eval 结果出口：数组/对象 → JSON 文本（避免 ToString 的 "[object Object]"
/// 使下游 JSON 解析为空）；其余按 String() 语义（数字/布尔字面量、null/undefined 空串）
fn js_result_to_string(v: &JsValue, context: &mut Context) -> String {
    match v {
        JsValue::Null | JsValue::Undefined => String::new(),
        JsValue::Object(_) => js_value_to_json(v, context)
            .map(|j| j.to_string())
            .unwrap_or_default(),
        _ => js_value_to_string(v, context),
    }
}

/// JsValue → serde_json::Value 递归转换（数组/对象/基本类型全支持）
///
/// 背景：boa ToString 对数组输出元素 Join（对象元素为 "[object Object]"），
/// 经 JSON.parse 必然解析为空。此处对齐 JSON.stringify 语义（Undefined/BigInt/
/// Symbol → null，不 panic——区别于 boa `JsValue::to_json` 对 Undefined 的 todo!）。
pub fn js_value_to_json(v: &JsValue, context: &mut Context) -> JsResult<JsonValue> {
    match v {
        JsValue::Null | JsValue::Undefined => Ok(JsonValue::Null),
        JsValue::Boolean(b) => Ok(JsonValue::Bool(*b)),
        JsValue::String(s) => Ok(JsonValue::String(s.to_std_string_escaped())),
        JsValue::Rational(r) => Ok(serde_json::json!(*r)),
        JsValue::Integer(i) => Ok(serde_json::json!(*i)),
        // BigInt/Symbol：JSON.stringify 语义（BigInt 抛错、Symbol 忽略）——此处收敛为 null
        JsValue::BigInt(_) | JsValue::Symbol(_) => Ok(JsonValue::Null),
        JsValue::Object(obj) => {
            if obj.is_array() {
                // 数组：按 length 逐元素（对齐 JSON.stringify 语义）
                let len = obj
                    .get(JsString::from("length"), context)?
                    .to_u32(context)?;
                let mut arr = Vec::with_capacity(len as usize);
                for k in 0..len {
                    let val = obj.get(k, context)?;
                    arr.push(js_value_to_json(&val, context)?);
                }
                Ok(JsonValue::Array(arr))
            } else {
                // 对象：own_property_keys 遍历（Symbol 键跳过）
                let mut map = JsonMap::new();
                for key in obj.own_property_keys(context)? {
                    let k = match &key {
                        PropertyKey::String(s) => s.to_std_string_escaped(),
                        PropertyKey::Index(i) => i.get().to_string(),
                        PropertyKey::Symbol(_) => continue,
                    };
                    let val = obj.get(key, context)?;
                    map.insert(k, js_value_to_json(&val, context)?);
                }
                Ok(JsonValue::Object(map))
            }
        }
    }
}

/// java.aesBase64DecodeToString(data, key, mode, iv)：AES/CBC/PKCS5 解密（书源加密 URL 常见）
fn java_aes_decrypt(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = args.first()
        .map(|v| js_value_to_string(v, context))
        .unwrap_or_default();
    let key = args
        .get(1)
        .map(|v| js_value_to_string(v, context))
        .unwrap_or_default();
    let iv = args
        .get(3)
        .map(|v| js_value_to_string(v, context))
        .unwrap_or_default();
    let decrypted = aes_base64_decode_to_string(&data, &key, &iv);
    Ok(JsValue::from(JsString::from(decrypted)))
}

/// AES-128-CBC/PKCS7 解密（key/iv 取前 16 字节）
fn aes_base64_decode_to_string(data: &str, key: &str, iv: &str) -> String {
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    use base64::Engine;
    let ciphertext = match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    if ciphertext.is_empty() {
        return String::new();
    }
    let key_bytes: Vec<u8> = key.as_bytes().iter().take(16).copied().collect();
    let iv_bytes: Vec<u8> = iv.as_bytes().iter().take(16).copied().collect();
    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
    let Ok(dec) = Aes128CbcDec::new_from_slices(&key_bytes, &iv_bytes) else {
        return String::new();
    };
    let buf = ciphertext;
    match dec.decrypt_padded_vec_mut::<block_padding::Pkcs7>(&buf) {
        Ok(pt) => String::from_utf8_lossy(&pt).into_owned(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ---- 旧行为兼容 ----

    #[test]
    fn eval_js_string_concat() {
        // key 注入 + 字符串拼接
        let v = vars(&[("key", "a")]);
        assert_eq!(eval_js("key + 'x'", &v).unwrap(), "ax");
    }

    #[test]
    fn eval_js_number_boolean_to_string() {
        let v = vars(&[]);
        // 数字 → 字符串
        assert_eq!(eval_js("1 + 2", &v).unwrap(), "3");
        assert_eq!(eval_js("6 * 7", &v).unwrap(), "42");
        assert_eq!(eval_js("3.14", &v).unwrap(), "3.14");
        // 布尔 → 字符串
        assert_eq!(eval_js("true", &v).unwrap(), "true");
        assert_eq!(eval_js("1 > 2", &v).unwrap(), "false");
    }

    #[test]
    fn eval_js_injected_vars() {
        let v = vars(&[
            ("result", "hello"),
            ("page", "2"),
            ("baseUrl", "https://a.com"),
        ]);
        assert_eq!(eval_js("result + page", &v).unwrap(), "hello2");
        assert_eq!(eval_js("baseUrl.length", &v).unwrap(), "13");
    }

    #[test]
    fn eval_js_null_undefined_to_empty() {
        let v = vars(&[]);
        assert_eq!(eval_js("undefined", &v).unwrap(), "");
        assert_eq!(eval_js("null", &v).unwrap(), "");
    }

    #[test]
    fn eval_js_error_returns_err() {
        let v = vars(&[]);
        assert!(eval_js("throw new Error('boom')", &v).is_err());
        assert!(eval_js("let let = 1", &v).is_err());
    }

    #[test]
    fn eval_js_backward_compat_default_bridge() {
        // 旧签名内部走空 bridge：java/source 可用但不跨调用保留
        let v = vars(&[]);
        assert_eq!(
            eval_js("java.log('x'); java.put('a', 'b')", &v).unwrap(),
            ""
        );
        assert_eq!(eval_js("java.get('a')", &v).unwrap(), "");
        assert_eq!(eval_js("source.getKey()", &v).unwrap(), "");
    }

    // ---- java.* shim ----

    #[test]
    fn bridge_java_put_get_roundtrip() {
        let bridge = JsBridge::new("https://src.test", "测试源");
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge("java.put('k1', 'v1'); java.get('k1')", &v, &bridge).unwrap(),
            "v1"
        );
        // 同 bridge 跨调用共享
        assert_eq!(
            eval_js_with_bridge("java.get('k1')", &v, &bridge).unwrap(),
            "v1"
        );
    }

    #[test]
    fn bridge_java_get_missing_is_undefined() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge("java.get('nope')", &v, &bridge).unwrap(),
            ""
        );
    }

    #[test]
    fn bridge_java_vars_isolated_per_bridge() {
        let b1 = JsBridge::new("", "");
        let b2 = JsBridge::new("", "");
        let v = vars(&[]);
        eval_js_with_bridge("java.put('k', 'from-b1')", &v, &b1).unwrap();
        assert_eq!(eval_js_with_bridge("java.get('k')", &v, &b2).unwrap(), "");
    }

    #[test]
    fn bridge_java_log_no_error() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge("java.log('hello ' + 1)", &v, &bridge).unwrap(),
            ""
        );
    }

    // ---- source.* shim ----

    #[test]
    fn bridge_source_put_get_cross_call() {
        // 跨 eval 调用/跨 bridge 实例：同书源 key 共享（全局存储）
        let b1 = JsBridge::new("https://src-x.test/book", "源A");
        let b2 = JsBridge::new("https://src-x.test/book", "源A");
        let v = vars(&[]);
        eval_js_with_bridge("source.put('page', '2')", &v, &b1).unwrap();
        assert_eq!(
            eval_js_with_bridge("source.get('page')", &v, &b2).unwrap(),
            "2"
        );
    }

    #[test]
    fn bridge_source_vars_isolated_by_source_key() {
        let a = JsBridge::new("https://a.test", "A");
        let b = JsBridge::new("https://b.test", "B");
        let v = vars(&[]);
        eval_js_with_bridge("source.put('x', '1')", &v, &a).unwrap();
        assert_eq!(eval_js_with_bridge("source.get('x')", &v, &b).unwrap(), "");
        assert_eq!(eval_js_with_bridge("source.get('x')", &v, &a).unwrap(), "1");
    }

    #[test]
    fn bridge_source_key_and_name() {
        let bridge = JsBridge::new("https://src.test", "测试源");
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge("source.getKey() + '|' + source.getName()", &v, &bridge).unwrap(),
            "https://src.test|测试源"
        );
    }

    // ---- java.headerMap shim ----

    #[test]
    fn bridge_header_map_put_get() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge(
                "java.headerMap.put('User-Agent', 'ua/1'); java.headerMap.get('User-Agent')",
                &v,
                &bridge
            )
            .unwrap(),
            "ua/1"
        );
        assert_eq!(
            eval_js_with_bridge("java.headerMap.size()", &v, &bridge).unwrap(),
            "1"
        );
        // eval 后 Rust 侧可读取改写后的请求头
        assert_eq!(
            bridge.headers().get("User-Agent").map(String::as_str),
            Some("ua/1")
        );
    }

    #[test]
    fn bridge_initial_headers_visible_in_js() {
        let bridge = JsBridge::new("", "");
        bridge.set_headers(vars(&[("Referer", "https://r.test")]));
        let v = vars(&[]);
        assert_eq!(
            eval_js_with_bridge("java.headerMap.get('Referer')", &v, &bridge).unwrap(),
            "https://r.test"
        );
    }

    // ---- 纯 JS 兼容 ----

    #[test]
    fn bridge_pure_js_still_works() {
        let bridge = JsBridge::new("https://src.test", "源");
        let v = vars(&[("key", "a")]);
        assert_eq!(eval_js_with_bridge("key + 'x'", &v, &bridge).unwrap(), "ax");
        assert_eq!(eval_js_with_bridge("1 + 2", &v, &bridge).unwrap(), "3");
        assert!(eval_js_with_bridge("throw new Error('x')", &v, &bridge).is_err());
    }

    // ---- 数组/对象序列化（bookList 修复） ----

    #[test]
    fn eval_js_array_to_json_string() {
        // 数组结果：eval 字符串出口应输出 JSON 文本而非 "[object Object]"
        let v = vars(&[]);
        assert_eq!(
            eval_js("[{a:1},{b:2}]", &v).unwrap(),
            r#"[{"a":1},{"b":2}]"#
        );
        // 对象结果同样 JSON 化
        assert_eq!(
            eval_js("({name:'A',url:'u'})", &v).unwrap(),
            r#"{"name":"A","url":"u"}"#
        );
        // 字符串/数字/布尔/null 语义不变
        assert_eq!(eval_js("JSON.stringify([1,2])", &v).unwrap(), "[1,2]");
        assert_eq!(eval_js("1+2", &v).unwrap(), "3");
        assert_eq!(eval_js("null", &v).unwrap(), "");
    }

    #[test]
    fn eval_js_json_array_from_parse() {
        // JSON.parse(result).data 数组 → 直接结构化返回（bookList 核心修复场景）
        let v = vars(&[(
            "result",
            r#"{"data":[{"name":"书A","url":"/a"},{"name":"书B","url":"/b"}]}"#,
        )]);
        let json = eval_js_json("JSON.parse(result).data", &v).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "书A");
        assert_eq!(arr[1]["url"], "/b");
    }

    #[test]
    fn eval_js_json_object_and_scalars() {
        let v = vars(&[]);
        // 对象
        let json = eval_js_json("({x:1,y:'s'})", &v).unwrap();
        assert_eq!(json["x"], 1);
        assert_eq!(json["y"], "s");
        // 字符串内 JSON 自动解析（JSON.stringify 出口）
        let json = eval_js_json("JSON.stringify([{n:1}])", &v).unwrap();
        assert_eq!(json.as_array().unwrap()[0]["n"], 1);
        // 标量
        assert_eq!(eval_js_json("42", &v).unwrap(), serde_json::json!(42));
        assert_eq!(eval_js_json("3.14", &v).unwrap(), serde_json::json!(3.14));
        assert_eq!(eval_js_json("true", &v).unwrap(), serde_json::json!(true));
        assert_eq!(eval_js_json("'str'", &v).unwrap(), serde_json::json!("str"));
        // undefined/bigint → null（不 panic，区别于 boa to_json 的 todo!）
        assert_eq!(
            eval_js_json("undefined", &v).unwrap(),
            serde_json::json!(null)
        );
        assert_eq!(eval_js_json("1n", &v).unwrap(), serde_json::json!(null));
    }

    #[test]
    fn eval_js_json_with_bridge_roundtrip() {
        let bridge = JsBridge::new("https://src.test", "源");
        let v = vars(&[]);
        let json = eval_js_json_with_bridge(
            "java.put('k','v'); JSON.parse('{\"list\":[{\"n\":1}]}').list",
            &v,
            &bridge,
        )
        .unwrap();
        assert_eq!(json.as_array().unwrap()[0]["n"], 1);
    }

    /// GAP #94：死循环 JS 触发循环迭代上限 → 报“JS 执行超限”（而非卡死）
    #[test]
    fn eval_js_infinite_loop_hits_limit() {
        let v = vars(&[]);
        // 小上限快速触发（1K 次迭代）；正式入口用 10M 上限，同一映射路径
        for code in ["while(true){}", "for(;;){}"] {
            let err =
                eval_js_with_bridge_limited(code, &v, &JsBridge::default(), 1_000).unwrap_err();
            assert!(
                err.to_string().contains("JS 执行超限"),
                "死循环应报超限: {}（实际: {err}）",
                code
            );
        }
        // 正常循环不受影响
        assert_eq!(
            eval_js("let s=0; for(let i=0;i<100;i++){s+=i} s", &v).unwrap(),
            "4950"
        );
        // 超限同样作用于 JSON 出口
        let err =
            eval_js_json_with_bridge_limited("while(true){}", &v, &JsBridge::default(), 1_000)
                .unwrap_err();
        assert!(err.to_string().contains("JS 执行超限"));
    }

    /// 受限 Context：单独设置小上限可快速触发（10M 上限下死循环也能在合理时间内触发）
    #[test]
    fn limited_context_small_limit_quickly_triggers() {
        let mut ctx = context_with_limit(1_000);
        let err = ctx
            .eval(boa_engine::Source::from_bytes(b"while(true){}".as_slice()))
            .unwrap_err();
        assert!(err
            .to_string()
            .to_lowercase()
            .contains("loop iteration limit"));
    }

    // ---- legacy shim：java.encodeURI ----

    #[test]
    fn bridge_java_encode_uri_gbk_and_utf8() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        // GBK：中文 → GBK 字节百分号编码（中=D6D0 文=CEC4）
        assert_eq!(
            eval_js_with_bridge("java.encodeURI('中文', 'GBK')", &v, &bridge).unwrap(),
            "%D6%D0%CE%C4"
        );
        // gb2312 别名等价
        assert_eq!(
            eval_js_with_bridge("java.encodeURI('中文', 'gb2312')", &v, &bridge).unwrap(),
            "%D6%D0%CE%C4"
        );
        // 默认 utf-8：中=E4B8AD 文=E69687
        assert_eq!(
            eval_js_with_bridge("java.encodeURI('中文')", &v, &bridge).unwrap(),
            "%E4%B8%AD%E6%96%87"
        );
        // 显式 utf-8 与空 charset 均回退默认
        assert_eq!(
            eval_js_with_bridge("java.encodeURI('中', 'utf-8')", &v, &bridge).unwrap(),
            "%E4%B8%AD"
        );
        assert_eq!(
            eval_js_with_bridge("java.encodeURI('中', '')", &v, &bridge).unwrap(),
            "%E4%B8%AD"
        );
        // encodeURI 语义：ASCII 字母数字与保留集不编码；空格 → %20
        assert_eq!(
            eval_js_with_bridge("java.encodeURI(\"a b-c_.!~*'()/?:@&=+$,#\")", &v, &bridge)
                .unwrap(),
            "a%20b-c_.!~*'()/?:@&=+$,#"
        );
    }

    // ---- legacy shim：java.setContent / getString / getElements ----

    #[test]
    fn bridge_java_set_content_get_string() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        let html = r#"<div class="book"><h2>书名A</h2><a href="/b/1">详情</a></div><div class="book"><h2>书名B</h2></div>"#;
        let js_html = serde_json::to_string(html).unwrap();
        // @text 提取器
        assert_eq!(
            eval_js_with_bridge(
                &format!("java.setContent({js_html}); java.getString('div.book@h2@text')"),
                &v,
                &bridge
            )
            .unwrap(),
            "书名A"
        );
        // 无 @ 提取器的选择器规则：元素 HTML → 文本
        assert_eq!(
            eval_js_with_bridge(
                &format!("java.setContent({js_html}); java.getString('div.book@h2')"),
                &v,
                &bridge
            )
            .unwrap(),
            "书名A"
        );
        // 索引选择器（.1 → 第 2 个）
        assert_eq!(
            eval_js_with_bridge(
                &format!("java.setContent({js_html}); java.getString('div.book.1@h2@text')"),
                &v,
                &bridge
            )
            .unwrap(),
            "书名B"
        );
        // 无匹配 → 空串
        assert_eq!(
            eval_js_with_bridge(
                &format!("java.setContent({js_html}); java.getString('div.none@h2@text')"),
                &v,
                &bridge
            )
            .unwrap(),
            ""
        );
    }

    #[test]
    fn bridge_java_get_elements_returns_outer_html_array() {
        let bridge = JsBridge::new("", "");
        let html =
            r#"<div class="book"><h2>书名A</h2></div><div class="book"><h2>书名B</h2></div>"#;
        let js_html = serde_json::to_string(html).unwrap();
        let json = eval_js_json_with_bridge(
            &format!("java.setContent({js_html}); java.getElements('div.book')"),
            &vars(&[]),
            &bridge,
        )
        .unwrap();
        let arr = json.as_array().expect("getElements 应返回数组");
        assert_eq!(arr.len(), 2, "应匹配 2 个 div.book");
        // 新语义：无提取器时返回元素对象（jsoup Elements 风格——可调 .html()/.text()）
        assert!(
            arr[0].is_object(),
            "元素应为对象（含 html/text 等方法）: {}",
            arr[0]
        );
        // 带 @html 提取器 → 字符串数组（outerHTML）
        let js_html2 = serde_json::to_string(html).unwrap();
        let json2 = eval_js_json_with_bridge(
            &format!("java.setContent({js_html2}); java.getElements('div.book@html')"),
            &vars(&[]),
            &bridge,
        )
        .unwrap();
        let arr2 = json2.as_array().expect("应返回数组");
        assert_eq!(arr2.len(), 2);
        assert!(
            arr2[0]
                .as_str()
                .unwrap()
                .contains("<div class=\"book\"><h2>书名A</h2></div>"),
            "元素应返回 outerHTML: {}",
            arr2[0]
        );
        assert!(arr2[1].as_str().unwrap().contains("书名B"));
    }

    #[test]
    fn bridge_java_doc_requires_set_content() {
        let bridge = JsBridge::new("", "");
        let v = vars(&[]);
        let err = eval_js_with_bridge("java.getString('div@text')", &v, &bridge).unwrap_err();
        assert!(
            err.to_string().contains("setContent"),
            "应提示先调用 setContent: {err}"
        );
        let err = eval_js_with_bridge("java.getElements('div')", &v, &bridge).unwrap_err();
        assert!(err.to_string().contains("setContent"), "{err}");
    }

    #[test]
    fn bridge_java_get_webview_ua() {
        let bridge = JsBridge::new("", "");
        let ua = eval_js_with_bridge("java.getWebViewUA()", &vars(&[]), &bridge).unwrap();
        assert_eq!(ua, JS_WEBVIEW_UA);
        assert!(ua.contains("Chrome/120"), "UA 应含 Chrome 标识: {ua}");
    }

    // ---- legacy shim：java.startBrowserAwait（缺浏览器明确报错 / 成功路径 / 求解失败）----

    /// startBrowserAwait 测试串行化（全局求解钩子/浏览器可用性覆盖为共享状态，
    /// 并行测试会互相踩踏——见 solve_error_propagates 钩子泄漏到 returns_js_object）
    static SOLVE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 浏览器不可用：返回明确错误（不启动浏览器、不发请求）
    #[test]
    fn bridge_java_start_browser_await_browser_unavailable() {
        let _guard = SOLVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_js_browser_available(Some(false));
        let bridge = JsBridge::new("https://src.test", "源").with_namespace("default");
        let err = eval_js_with_bridge(
            "java.startBrowserAwait('https://a.test/book/1', '标题', true)",
            &vars(&[]),
            &bridge,
        )
        .unwrap_err();
        force_js_browser_available(None);
        assert!(
            err.to_string().contains("浏览器"),
            "应提示浏览器不可用: {err}"
        );
    }

    /// 成功路径（求解钩子注入）：返回 {body, cookies, status} JS 对象
    #[test]
    fn bridge_java_start_browser_await_returns_js_object() {
        let _guard = SOLVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_js_browser_available(Some(true));
        register_js_solve_hook(Some(Arc::new(|url, cookies| {
            assert_eq!(url, "https://a.test/book/1?q=1");
            assert!(cookies.is_empty(), "无 cookie 存储时应为空");
            Ok((
                "<html><body>hello-page</body></html>".to_string(),
                vec![
                    ("cf_clearance".to_string(), "xyz".to_string()),
                    ("sid".to_string(), "1".to_string()),
                ],
                "Mozilla/5.0 test UA".to_string(),
            ))
        })));
        let bridge = JsBridge::new("https://src.test", "源").with_namespace("default");
        let r = eval_js_with_bridge(
            "var w = java.startBrowserAwait('https://a.test/book/1?q=1', '标题', false); w.body + '|' + w.status + '|' + w.cookies.length + '|' + w.cookies[0]",
            &vars(&[]),
            &bridge,
        );
        register_js_solve_hook(None);
        force_js_browser_available(None);
        assert_eq!(
            r.unwrap(),
            "<html><body>hello-page</body></html>|200|2|cf_clearance=xyz"
        );
    }

    /// 求解失败：错误信息透传（含 url 上下文）
    #[test]
    fn bridge_java_start_browser_await_solve_error_propagates() {
        let _guard = SOLVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_js_browser_available(Some(true));
        register_js_solve_hook(Some(Arc::new(|_, _| Err(anyhow!("模拟浏览器求解失败")))));
        let bridge = JsBridge::new("", "").with_namespace("default");
        let err = eval_js_with_bridge(
            "java.startBrowserAwait('https://a.test/x', 't', true)",
            &vars(&[]),
            &bridge,
        )
        .unwrap_err();
        register_js_solve_hook(None);
        force_js_browser_available(None);
        assert!(
            err.to_string().contains("模拟浏览器求解失败"),
            "求解错误应透传: {err}"
        );
    }

    // ---- legacy shim：java.ajax（本地 HTTP 服务器端到端）----

    /// 微型 HTTP 服务器：返回固定响应体；捕获收到的请求（方法/路径/body）
    async fn serve_echo(
        body: Vec<u8>,
        captured: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for _ in 0..10 {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                captured.lock().unwrap().push(req);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let mut resp = head.into_bytes();
                resp.extend_from_slice(&body);
                let _ = sock.write_all(&resp).await;
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn bridge_java_ajax_get_returns_body() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let url = rt.block_on(serve_echo(b"hello-ajax".to_vec(), captured.clone()));
        let bridge = JsBridge::new("", "").with_namespace("default");
        let r = eval_js_with_bridge(&format!("java.ajax('{url}/x?a=1')"), &vars(&[]), &bridge);
        assert_eq!(r.unwrap(), "hello-ajax");
        // 无 cookie 存储注册：请求不应带 Cookie 头
        let req = captured.lock().unwrap()[0].to_lowercase();
        assert!(!req.contains("cookie:"), "不应带 Cookie 头: {req}");
    }

    #[test]
    fn bridge_java_ajax_post_suffix_gbk() {
        // POST + `,{...}` 后缀（method/body/charset）：GBK 字节响应正确解码
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let gbk = {
            let (bytes, _, _) = encoding_rs::GBK.encode("中文响应");
            bytes.into_owned()
        };
        let url = rt.block_on(serve_echo(gbk, captured.clone()));
        let bridge = JsBridge::new("", "").with_namespace("default");
        let code = format!(
            "java.ajax('{url}/p,{{\"method\":\"POST\",\"body\":\"k=v\",\"charset\":\"GBK\"}}')"
        );
        let r = eval_js_with_bridge(&code, &vars(&[]), &bridge);
        assert_eq!(r.unwrap(), "中文响应", "GBK 响应应按 charset 解码");
        let req = captured.lock().unwrap()[0].clone();
        assert!(req.starts_with("POST /p"), "应 POST 到 /p: {req}");
        assert!(req.contains("k=v"), "应携带 body: {req}");
    }

    /// 连接失败：明确报错（抛 JS 异常）
    #[test]
    fn bridge_java_ajax_error_propagates() {
        let bridge = JsBridge::new("", "").with_namespace("default");
        // 127.0.0.1:1 大概率无服务 → 快速连接失败
        let err = eval_js_with_bridge("java.ajax('http://127.0.0.1:1/nope')", &vars(&[]), &bridge)
            .unwrap_err();
        assert!(
            err.to_string().contains("java.ajax 失败"),
            "应报 ajax 失败: {err}"
        );
    }
}
