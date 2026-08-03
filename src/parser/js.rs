//! JS 规则执行（boa_engine 0.19，对齐 legado AnalyzeByJS / js 规则）
//!
//! v1 支持：纯 JS 逻辑 + 注入变量（key/page/result/baseUrl/headerMap 简化）
//! Java 扩展（java.*/source.*）逐项 shim（后续迭代）
//!
//! boa 0.19 API 注意：
//! - 变量注入需经 JsString 转换（PropertyKey/JsValue 无 From<&str>，有 From<JsString>）
//! - JsError 含 Rc/NonNull，非 Send/Sync，不能直接进 anyhow，需 map_err 转字符串
//! - JsValue::to_string(&mut Context) 即规范 ToString（数字/布尔按 String() 语义）

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue, Source};

/// 执行 JS 代码，返回字符串结果
///
/// 变量以全局属性注入；结果按 String() 语义转字符串
/// （null/undefined → 空串；数字/布尔 → 字面量，如 "42" / "true"）
pub fn eval_js(code: &str, vars: &HashMap<String, String>) -> Result<String> {
    let mut context = Context::default();
    inject_vars(&mut context, vars)?;
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
}
