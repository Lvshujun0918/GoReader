//! JS 规则执行（boa_engine 0.19，对齐 legado AnalyzeByJS / js 规则）
//!
//! v2 支持：
//! - 纯 JS 逻辑 + 注入变量（key/page/result/baseUrl/headerMap 简化）
//! - 书源桥接（JsBridge，对齐 legado jsHelp）：
//!   - `java.put(key, val)` / `java.get(key)`：bridge 生命周期内的临时变量
//!   - `java.log(msg)`：tracing 日志
//!   - `java.headerMap.put/get/size`：请求头 Map（eval 后经 `JsBridge::headers()` 读取）
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
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{anyhow, Result};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsObject, JsResult, JsString, JsValue, NativeFunction, Source};

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
    /// 请求头：`java.headerMap` 的底层存储（JS 可改写）
    headers: Mutex<HashMap<String, String>>,
    /// `java.put/get` 临时变量（本 bridge 生命周期内共享）
    java_vars: Mutex<HashMap<String, String>>,
}

impl JsBridge {
    /// 创建书源桥接
    pub fn new(source_key: impl Into<String>, source_name: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(JsBridgeInner {
                source_key: source_key.into(),
                source_name: source_name.into(),
                headers: Mutex::new(HashMap::new()),
                java_vars: Mutex::new(HashMap::new()),
            }),
        }
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

/// 执行 JS 代码并注入书源桥接（java.* / source.*，对齐 legado jsHelp）
pub fn eval_js_with_bridge(
    code: &str,
    vars: &HashMap<String, String>,
    bridge: &JsBridge,
) -> Result<String> {
    let mut context = Context::default();
    inject_vars(&mut context, vars)?;
    install_bridge(&mut context, bridge)?;
    let result = context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(|e| anyhow!("JS 执行失败: {e}"))?;
    Ok(js_value_to_string(&result, &mut context))
}

/// 执行 JS 表达式并返回 JsValue（供内部使用）
pub fn eval_js_value(code: &str, vars: &HashMap<String, String>) -> Result<JsValue> {
    let mut context = Context::default();
    inject_vars(&mut context, vars)?;
    context
        .eval(Source::from_bytes(code.as_bytes()))
        .map_err(|e| anyhow!("JS 执行失败: {e}"))
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

/// 构建 java / source 对象（ObjectInitializer：function 注册方法、property 挂子对象）
fn build_bridge_objects(bridge: &JsBridge, context: &mut Context) -> (JsObject, JsObject) {
    // java.headerMap：请求头 Map（底层为 bridge.headers，eval 后可读取）
    let mut header_map = ObjectInitializer::new(context);
    header_map
        .function(bind(bridge, header_map_put), JsString::from("put"), 2)
        .function(bind(bridge, header_map_get), JsString::from("get"), 1)
        .function(bind(bridge, header_map_size), JsString::from("size"), 0);
    let header_map = header_map.build();

    // java：put/get（临时变量）、log（tracing）、headerMap（请求头）
    let mut java = ObjectInitializer::new(context);
    java.function(bind(bridge, java_put), JsString::from("put"), 2)
        .function(bind(bridge, java_get), JsString::from("get"), 1)
        .function(bind(bridge, java_log), JsString::from("log"), 1)
        .function(unsafe { NativeFunction::from_closure(java_aes_decrypt) }, JsString::from("aesBase64DecodeToString"), 4)
        .property(JsString::from("headerMap"), header_map, Attribute::all());
    let java = java.build();

    // source：getKey（书源 URL）/ getName（书源名）/ put/get（书源级变量）
    let mut source = ObjectInitializer::new(context);
    source
        .function(bind(bridge, source_get_key), JsString::from("getKey"), 0)
        .function(bind(bridge, source_get_name), JsString::from("getName"), 0)
        .function(bind(bridge, source_put), JsString::from("put"), 2)
        .function(bind(bridge, source_get), JsString::from("get"), 1);
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

/// java.get(key)：读取临时变量，缺失返回 undefined
fn java_get(inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = js_value_to_string(args.get_or_undefined(0), context);
    let value = inner
        .java_vars
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    Ok(value.map_or_else(JsValue::undefined, |s| JsValue::from(JsString::from(s))))
}

/// java.log(msg)：tracing 日志（调试书源规则）
fn java_log(_inner: &JsBridgeInner, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let msg = js_value_to_string(args.get_or_undefined(0), context);
    tracing::info!(target: "reader.js", "[java.log] {}", msg);
    Ok(JsValue::undefined())
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
        assert_eq!(eval_js("java.log('x'); java.put('a', 'b')", &v).unwrap(), "");
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
        assert_eq!(eval_js_with_bridge("java.get('nope')", &v, &bridge).unwrap(), "");
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
            eval_js_with_bridge(
                "source.getKey() + '|' + source.getName()",
                &v,
                &bridge
            )
            .unwrap(),
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
}

/// java.aesBase64DecodeToString(data, key, mode, iv)：AES/CBC/PKCS5 解密（书源加密 URL 常见）
fn java_aes_decrypt(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = args.get(0).map(|v| js_value_to_string(v, context)).unwrap_or_default();
    let key = args.get(1).map(|v| js_value_to_string(v, context)).unwrap_or_default();
    let iv = args.get(3).map(|v| js_value_to_string(v, context)).unwrap_or_default();
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
    let mut buf = ciphertext;
    match dec.decrypt_padded_vec_mut::<block_padding::Pkcs7>(&mut buf) {
        Ok(pt) => String::from_utf8_lossy(&pt).into_owned(),
        Err(_) => String::new(),
    }
}
