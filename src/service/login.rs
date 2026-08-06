//! 书源登录（loginUrl + loginCheckJs + 验证码）——登录态独立于系统用户（按用户命名空间存库）
//!
//! 三条路径：
//! 1) **HTTP 直连**（默认）：POST（表单）/GET 执行 loginUrl（支持 {user}/{pass}/{captcha}
//!    及 {{...}} 双花括号占位符；带书源既有 cookie）→ 响应 Set-Cookie 合并存库（按用户）→
//!    执行 loginCheckJs（复用 js shim，vars: cookie/result/url）→ true/false。
//! 2) **浏览器自动**（mode=browser 或 HTTP 流检测到点击类验证码后自动切换）：
//!    headless 浏览器（CDP）填表单、滑块自动拖拽（人类轨迹）、图片验证码截图给前端。
//! 3) **图片验证码**：返回 captchaUrl（页面提取 URL 或浏览器截图 data URI）+ captchaId；
//!    前端输入后重新调用 loginBookSource（captcha 参数，HTTP 流）或 submitCaptcha（浏览器流）。
//!
//! 点击类验证码（滑块/点选）处理策略：
//! - 滑块：浏览器自动拖拽（2 次尝试）；失败/超时（30s）→ "需手动 Cookie" 错误
//! - 点选：无法自动识别目标点 → "需手动 Cookie" 错误（请在浏览器登录后粘贴 Cookie）

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::model::BookSource;
use crate::service::{browser, crawler, search};
use crate::storage::Storage;

/// 登录请求参数（均可选）
#[derive(Debug, Clone, Default)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// 图片验证码文本（前端输入后回传）
    pub captcha: String,
}

/// 登录结果（软性结果；硬错误走 Err）
pub enum LoginOutcome {
    /// 登录成功（cookie 已存库，按用户）
    Success { cookie: String },
    /// 需要图片验证码：captcha_url 给前端（页面提取 URL 或浏览器截图 data URI）
    NeedImageCaptcha {
        captcha_url: String,
        captcha_id: String,
        message: String,
    },
    /// 点击类验证码无法自动处理/失败/超时 → 引导手动 Cookie
    NeedManualCookie { message: String },
    /// 登录失败（loginCheckJs 未通过，无验证码）
    Failed { message: String },
}

// ==================== 占位符 / 表单 / loginCheckJs ====================

/// loginUrl/loginBody 占位符替换：{user}/{pass}/{captcha}/{username}/{password} 及双花括号变体。
/// 双花括号优先（避免 `{{user}}` 被 `{user}` 二次替换错位）。
pub fn replace_login_placeholders(
    s: &str,
    username: &str,
    password: &str,
    captcha: &str,
) -> String {
    let mut out = s.to_string();
    out = out
        .replace("{{user}}", username)
        .replace("{{pass}}", password)
        .replace("{{captcha}}", captcha)
        .replace("{{username}}", username)
        .replace("{{password}}", password);
    out = out
        .replace("{user}", username)
        .replace("{pass}", password)
        .replace("{captcha}", captcha)
        .replace("{username}", username)
        .replace("{password}", password);
    out
}

/// 构建登录表单体（application/x-www-form-urlencoded）：
/// loginUi 字段名优先（password 类型→密码；captcha 相关→验证码；首个其余→用户名），
/// 无 loginUi 时缺省 username/password（+captcha，若提供了验证码参数）。
pub fn build_login_form(source: &BookSource, req: &LoginRequest) -> String {
    let fields: Vec<(String, String)> = source
        .login_ui
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .map(|items| {
            items
                .iter()
                .map(|it| {
                    let name = it
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let typ = it
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("text")
                        .to_string();
                    (name, typ)
                })
                .collect()
        })
        .unwrap_or_else(|| {
            let mut d = vec![
                ("username".to_string(), "text".to_string()),
                ("password".to_string(), "password".to_string()),
            ];
            if !req.captcha.is_empty() {
                d.push(("captcha".to_string(), "text".to_string()));
            }
            d
        });
    let mut user_done = false;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (name, typ) in fields {
        if name.is_empty() {
            continue;
        }
        let t = typ.to_lowercase();
        let n = name.to_lowercase();
        let value = if t.contains("password") {
            req.password.clone()
        } else if n.contains("captcha")
            || n.contains("vcode")
            || n.contains("verify")
            || n.contains("checkcode")
            || t.contains("captcha")
            || t.contains("verify")
        {
            req.captcha.clone()
        } else if !user_done {
            user_done = true;
            req.username.clone()
        } else {
            String::new()
        };
        pairs.push((name, value));
    }
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(&k, &v);
    }
    ser.finish()
}

/// 执行 loginCheckJs（空脚本 = 默认成功，legacy 语义）。
/// 注入 vars：cookie（合并后 cookie 串）/result（响应体）/url（最终 URL）。
/// 返回 true = 已登录。
pub fn check_login(js: &str, cookie: &str, result: &str, url: &str) -> Result<bool> {
    let js = js.trim();
    if js.is_empty() {
        return Ok(true);
    }
    let mut vars = HashMap::new();
    vars.insert("cookie".to_string(), cookie.to_string());
    vars.insert("result".to_string(), result.to_string());
    vars.insert("url".to_string(), url.to_string());
    let r = crate::parser::js::eval_js(js, &vars)?;
    let r = r.trim();
    Ok(r.eq_ignore_ascii_case("true") || r == "1")
}

/// Set-Cookie 合并（响应多个 Set-Cookie + 用户既有 cookie）：
/// 按 name 合并——新 Set-Cookie 覆盖同名、空值删除、其余保留；顺序稳定（既有为基底 + 新名追加）
pub fn merge_cookie(existing: &str, set_cookies: &[String]) -> String {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, String> = HashMap::new();
    for (k, v) in crawler::parse_cookie_string(existing) {
        if !map.contains_key(&k) {
            order.push(k.clone());
        }
        map.insert(k, v);
    }
    for sc in set_cookies {
        let first = sc.split(';').next().unwrap_or("").trim();
        let Some((k, v)) = first.split_once('=') else {
            continue;
        };
        let k = k.trim().to_string();
        if k.is_empty() {
            continue;
        }
        let v = v.trim();
        if v.is_empty() {
            // 空值 = 删除该 cookie
            map.remove(&k);
            order.retain(|x| x != &k);
            continue;
        }
        if !map.contains_key(&k) {
            order.push(k.clone());
        }
        map.insert(k, v.to_string());
    }
    order
        .into_iter()
        .filter_map(|k| map.get(&k).map(|v| format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join("; ")
}

// ==================== 验证码特征（页面 HTML 启发式） ====================

/// 点击类验证码检测（页面特征匹配）：返回 Some("slider"|"click")。
/// 命中即认为需浏览器/手动处理（不做 OCR、不做 headless 之外的破解）。
pub fn detect_click_captcha(html: &str) -> Option<&'static str> {
    let lower = html.to_lowercase();
    let slider_markers = [
        "geetest",
        "极验",
        "gt.js",
        "gt4",
        "滑块",
        "滑动验证",
        "slide-verify",
        "slider-verify",
        "tcaptcha",
        "nc_1_n1z",
        "aliyun",
        "阿里云验证码",
        "拖动滑块",
        "拼图",
        "jigsaw",
        "dx-captcha",
        "顶象",
        "dragverify",
        "slidercaptcha",
    ];
    if slider_markers.iter().any(|m| lower.contains(m)) {
        return Some("slider");
    }
    let click_markers = [
        "点选",
        "click-verify",
        "clickcaptcha",
        "verify-point",
        "字符点选",
        "语序点选",
        "points-verify",
    ];
    if click_markers.iter().any(|m| lower.contains(m)) {
        return Some("click");
    }
    None
}

/// 图片验证码 URL 提取：页面 `<img>` 中 src/id/class/alt 含验证码特征者取其 src（相对路径拼绝对）。
pub fn extract_image_captcha_url(html: &str, base_url: &str) -> Option<String> {
    let re = regex::Regex::new(r"<img[^>]*>").expect("static regex");
    for cap in re.captures_iter(html) {
        let tag = cap.get(0)?.as_str();
        let ctx = tag.to_lowercase();
        let has_feature = [
            "captcha",
            "vcode",
            "verify",
            "yzm",
            "checkcode",
            "验证码",
            "randimg",
            "kaptcha",
        ]
        .iter()
        .any(|k| ctx.contains(k));
        if !has_feature {
            continue;
        }
        for attr in ["src", "data-src", "data-original"] {
            let attr_re = regex::Regex::new(&format!(r#"{attr}\s*=\s*["']([^"']+)["']"#))
                .expect("static regex");
            if let Some(m) = attr_re.captures(tag) {
                let url = m.get(1)?.as_str();
                if url.starts_with("data:") {
                    return Some(url.to_string());
                }
                return Some(search::to_absolute(url, base_url));
            }
        }
    }
    None
}

// ==================== 验证码会话缓存（内存，5 分钟过期） ====================

struct CaptchaSession {
    ns: String,
    source_url: String,
    kind: String,
    username: String,
    password: String,
    created: Instant,
}

static CAPTCHA_SESSIONS: LazyLock<Mutex<HashMap<String, CaptchaSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const CAPTCHA_TTL: Duration = Duration::from_secs(300);

fn new_captcha_session(ns: &str, source: &BookSource, kind: &str, req: &LoginRequest) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut guard = CAPTCHA_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    // 过期清理
    guard.retain(|_, s| s.created.elapsed() < CAPTCHA_TTL);
    guard.insert(
        id.clone(),
        CaptchaSession {
            ns: ns.to_string(),
            source_url: source.book_source_url.clone(),
            kind: kind.to_string(),
            username: req.username.clone(),
            password: req.password.clone(),
            created: Instant::now(),
        },
    );
    id
}

fn get_captcha_session(id: &str) -> Option<CaptchaSession> {
    let mut guard = CAPTCHA_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|_, s| s.created.elapsed() < CAPTCHA_TTL);
    guard.remove(id)
}

// ==================== HTTP 直连登录流 ====================

/// 书源登录（默认 HTTP 流）。点击类验证码命中且浏览器可用 → 自动切换浏览器流。
pub async fn login_http(
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    req: &LoginRequest,
) -> Result<LoginOutcome> {
    let login_url = source
        .login_url
        .as_deref()
        .ok_or_else(|| anyhow!("书源未配置 loginUrl"))?;
    // `,{...}` 后缀（method/body/charset/headers，对齐搜索链路）
    let (raw_url, suffix) = search::split_url_suffix(login_url);
    let url = replace_login_placeholders(&raw_url, &req.username, &req.password, &req.captcha);

    let mut req_headers = source
        .header
        .as_deref()
        .map(crawler::parse_header)
        .unwrap_or_default();
    if let Some(extra) = &suffix.headers {
        for (k, v) in extra {
            req_headers.insert(k.clone(), v.clone());
        }
    }

    let method = suffix.method.as_deref().unwrap_or("GET").to_string();
    let body = if let Some(b) = &suffix.body {
        Some(replace_login_placeholders(
            b,
            &req.username,
            &req.password,
            &req.captcha,
        ))
    } else if method.eq_ignore_ascii_case("POST") {
        Some(build_login_form(source, req))
    } else {
        None
    };

    let resp = if method.eq_ignore_ascii_case("POST") {
        req_headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        crawler::http_post(
            ns,
            &url,
            &req_headers,
            20,
            body.as_deref(),
            suffix.charset.as_deref(),
        )
        .await?
    } else {
        crawler::http_get(ns, &url, &req_headers, 20).await?
    };

    // Set-Cookie 合并存库（按用户）
    let set_cookies: Vec<String> = resp
        .headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.clone())
        .collect();
    let existing = storage.get_cookie(ns, &source.book_source_url).await?;
    let merged = merge_cookie(existing.as_deref().unwrap_or(""), &set_cookies);
    if !merged.is_empty() {
        storage
            .set_cookie(ns, &source.book_source_url, &merged)
            .await?;
    }

    // loginCheckJs
    let ok = match &source.login_check_js {
        Some(js) => check_login(js, &merged, &resp.body, &resp.url)?,
        None => true,
    };
    if ok {
        return Ok(LoginOutcome::Success { cookie: merged });
    }

    // 失败 → 验证码判定
    if let Some(kind) = detect_click_captcha(&resp.body) {
        // 点击类验证码：浏览器可用 → 自动切换浏览器流（滑块自动拖）；否则手动 Cookie
        if browser::is_browser_available() {
            tracing::info!(
                "书源 [{}] 检测到{kind}验证码——切换浏览器自动登录",
                source.book_source_name
            );
            return login_browser(storage, ns, source, req).await;
        }
        let kind_cn = if kind == "slider" { "滑块" } else { "点选" };
        return Ok(LoginOutcome::NeedManualCookie {
            message: format!(
                "检测到{kind_cn}验证码：请在浏览器登录该书源后，在书源设置粘贴 Cookie（安装/配置 obscura 浏览器后可使用浏览器自动登录）"
            ),
        });
    }
    // 图片验证码：页面含 captcha 图片 → captchaUrl 给前端
    if let Some(captcha_url) = extract_image_captcha_url(&resp.body, &resp.url) {
        let captcha_id = new_captcha_session(ns, source, "image", req);
        return Ok(LoginOutcome::NeedImageCaptcha {
            captcha_url,
            captcha_id,
            message: "需要图片验证码".to_string(),
        });
    }
    // loginUrl 规则含 {captcha} 占位符且首轮未带验证码 → 同样走图片验证码流程
    if raw_url.contains("{captcha}") && req.captcha.is_empty() {
        let captcha_id = new_captcha_session(ns, source, "image", req);
        return Ok(LoginOutcome::NeedImageCaptcha {
            captcha_url: extract_image_captcha_url(&resp.body, &resp.url).unwrap_or_default(),
            captcha_id,
            message: "需要图片验证码（loginUrl 含 {captcha} 占位符）".to_string(),
        });
    }
    Ok(LoginOutcome::Failed {
        message: "登录失败：loginCheckJs 未通过".to_string(),
    })
}

// ==================== 浏览器自动登录流（CDP） ====================

/// 浏览器自动登录（mode=browser；HTTP 流检测到点击类验证码时自动调用）。
/// 30s 总超时；滑块自动拖拽（2 次尝试）；点选/失败/超时 → "需手动 Cookie"。
pub async fn login_browser(
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    req: &LoginRequest,
) -> Result<LoginOutcome> {
    let login_url = source
        .login_url
        .as_deref()
        .ok_or_else(|| anyhow!("书源未配置 loginUrl"))?;
    let (raw_url, _suffix) = search::split_url_suffix(login_url);
    let url = replace_login_placeholders(&raw_url, &req.username, &req.password, &req.captcha);

    let result = tokio::time::timeout(
        Duration::from_secs(30),
        browser_login_inner(storage, ns, source, &url, req),
    )
    .await;
    match result {
        Ok(r) => r,
        Err(_) => Ok(LoginOutcome::NeedManualCookie {
            message: "浏览器自动登录超时（30s）——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
                .to_string(),
        }),
    }
}

async fn browser_login_inner(
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    url: &str,
    req: &LoginRequest,
) -> Result<LoginOutcome> {
    let mut b = browser::Browser::launch().await?;
    // 注入既有 cookie（保持会话连续性）
    inject_cookies(&mut b, storage, ns, source, url).await?;

    b.navigate(url).await?;

    // ① 验证码处理（滑块自动拖；图片截图；点选降级）
    let mut slider_attempts = 0u32;
    loop {
        let det = b.evaluate(browser::DETECT_CAPTCHA_JS).await?;
        if det.is_null() {
            break;
        }
        let kind = det
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        match kind.as_str() {
            "image" => {
                let (x, y, w, h) = rect_of(&det);
                let png = b.screenshot_clip(x, y, w, h).await?;
                use base64::Engine;
                let data_uri = format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&png)
                );
                let captcha_id = new_captcha_session(ns, source, "image", req);
                return Ok(LoginOutcome::NeedImageCaptcha {
                    captcha_url: data_uri,
                    captcha_id,
                    message: "需要图片验证码（浏览器截图）".to_string(),
                });
            }
            "slider" => {
                if slider_attempts >= 2 {
                    return Ok(LoginOutcome::NeedManualCookie {
                        message: "滑块验证码多次尝试未通过——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
                            .to_string(),
                    });
                }
                slider_attempts += 1;
                let (bx, by, bw, _bh) = rect_of(&det);
                let track_w = det.get("trackW").and_then(|v| v.as_f64()).unwrap_or(300.0);
                let start_x = bx + bw / 2.0;
                let start_y = by + 12.0;
                // 目标距离随机化（轨道 55%~90%），避免固定轨迹被风控
                let dist = (track_w - bw) * (0.55 + rand::random::<f64>() * 0.35);
                let end_x = bx + dist;
                let end_y = start_y + rand::random::<f64>() * 4.0 - 2.0;
                tracing::info!("滑块拖拽尝试 {slider_attempts}（距离 {dist:.0}px）");
                b.mouse_drag(start_x, start_y, end_x, end_y).await?;
                tokio::time::sleep(Duration::from_millis(browser::CAPTCHA_SETTLE_MS)).await;
            }
            "click" => {
                return Ok(LoginOutcome::NeedManualCookie {
                    message: "检测到点选类验证码（无法自动识别目标点）——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
                        .to_string(),
                });
            }
            _ => break,
        }
    }

    // ② 填表单 + 提交
    let fill_js = browser::FILL_FORM_JS
        .replace("'USERNAME'", &serde_json::to_string(&req.username)?)
        .replace("'PASSWORD'", &serde_json::to_string(&req.password)?);
    let fill = b.evaluate(&fill_js).await?;
    if fill.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(LoginOutcome::Failed {
            message: "浏览器登录失败：页面未找到登录表单（可能已登录或页面结构特殊）".to_string(),
        });
    }
    let _ = b.evaluate(browser::SUBMIT_FORM_JS).await?;
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // ③ 提交后可能再现验证码 → 滑块再拖一次 / 图片截图 / 点选降级
    let det2 = b.evaluate(browser::DETECT_CAPTCHA_JS).await?;
    if !det2.is_null() {
        let kind = det2
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        match kind.as_str() {
            "slider" if slider_attempts < 2 => {
                let (bx, by, bw, _bh) = rect_of(&det2);
                let track_w = det2.get("trackW").and_then(|v| v.as_f64()).unwrap_or(300.0);
                let dist = (track_w - bw) * (0.55 + rand::random::<f64>() * 0.35);
                b.mouse_drag(bx + bw / 2.0, by + 12.0, bx + dist, by + 12.0)
                    .await?;
                tokio::time::sleep(Duration::from_millis(browser::CAPTCHA_SETTLE_MS)).await;
            }
            "image" => {
                let (x, y, w, h) = rect_of(&det2);
                let png = b.screenshot_clip(x, y, w, h).await?;
                use base64::Engine;
                let data_uri = format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&png)
                );
                let captcha_id = new_captcha_session(ns, source, "image", req);
                return Ok(LoginOutcome::NeedImageCaptcha {
                    captcha_url: data_uri,
                    captcha_id,
                    message: "需要图片验证码（浏览器截图）".to_string(),
                });
            }
            "click" => {
                return Ok(LoginOutcome::NeedManualCookie {
                    message: "检测到点选类验证码（无法自动识别目标点）——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
                        .to_string(),
                });
            }
            _ => {}
        }
    }

    // ④ 提取结果 → loginCheckJs（vars: cookie/result/url）
    let html = b
        .evaluate("document.documentElement.outerHTML")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let cookie_str = {
        let cookies = b.get_cookies().await?;
        browser::Browser::cookies_to_string(&cookies)
    };
    let page_url = b
        .evaluate("location.href")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let ok = match &source.login_check_js {
        Some(js) => check_login(js, &cookie_str, &html, &page_url)?,
        None => true,
    };
    if ok {
        if !cookie_str.is_empty() {
            storage
                .set_cookie(ns, &source.book_source_url, &cookie_str)
                .await?;
        }
        tracing::info!("书源 [{}] 浏览器自动登录成功", source.book_source_name);
        return Ok(LoginOutcome::Success { cookie: cookie_str });
    }
    if detect_click_captcha(&html).is_some() {
        return Ok(LoginOutcome::NeedManualCookie {
            message: "浏览器自动登录未通过验证——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
                .to_string(),
        });
    }
    Ok(LoginOutcome::Failed {
        message: "浏览器登录失败：loginCheckJs 未通过".to_string(),
    })
}

/// 注入书源既有 cookie（name=value 对 → CDP Network.setCookies）
async fn inject_cookies(
    b: &mut browser::Browser,
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    url: &str,
) -> Result<()> {
    let Some(cookie) = storage.get_cookie(ns, &source.book_source_url).await? else {
        return Ok(());
    };
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("loginUrl 解析失败: {e}"))?;
    let host = parsed.host_str().unwrap_or("").to_string();
    let secure = parsed.scheme() == "https";
    b.set_cookies(&crawler::parse_cookie_string(&cookie), &host, secure)
        .await?;
    Ok(())
}

fn rect_of(det: &Value) -> (f64, f64, f64, f64) {
    (
        det.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
        det.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
        det.get("w").and_then(|v| v.as_f64()).unwrap_or(40.0),
        det.get("h").and_then(|v| v.as_f64()).unwrap_or(40.0),
    )
}

// ==================== getCaptcha / submitCaptcha（浏览器流，图片验证码） ====================

/// POST /reader3/getCaptcha：重新触发登录页 → 检测验证码 → 返回
/// {captchaType: image|slider|click|none, captchaUrl(data URI), captchaId, pageUrl}
pub async fn get_captcha(storage: &Storage, ns: &str, source: &BookSource) -> Result<Value> {
    if !browser::is_browser_available() {
        return Err(anyhow!(
            "未安装浏览器（obscura）——请在书源设置粘贴 Cookie（手动流程；配置 READER_OBSCURA_BIN/READER_OBSCURA_URL 后可使用浏览器自动登录）"
        ));
    }
    let login_url = source
        .login_url
        .as_deref()
        .ok_or_else(|| anyhow!("书源未配置 loginUrl"))?;
    let (raw_url, _suffix) = search::split_url_suffix(login_url);
    let url = replace_login_placeholders(&raw_url, "", "", "");

    let mut b = browser::Browser::launch().await?;
    inject_cookies(&mut b, storage, ns, source, &url).await?;
    b.navigate(&url).await?;

    let det = b.evaluate(browser::DETECT_CAPTCHA_JS).await?;
    if det.is_null() {
        return Ok(json!({ "captchaType": "none", "message": "未检测到验证码" }));
    }
    let kind = det
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .to_string();
    let page_url = b
        .evaluate("location.href")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    match kind.as_str() {
        "image" => {
            let (x, y, w, h) = rect_of(&det);
            let png = b.screenshot_clip(x, y, w, h).await?;
            use base64::Engine;
            let data_uri = format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&png)
            );
            let captcha_id = new_captcha_session(ns, source, "image", &LoginRequest::default());
            Ok(json!({
                "captchaType": "image",
                "captchaUrl": data_uri,
                "captchaId": captcha_id,
                "pageUrl": page_url,
                "message": "需要图片验证码",
            }))
        }
        "slider" => Ok(json!({
            "captchaType": "slider",
            "pageUrl": page_url,
            "message": "检测到滑块验证码——请重新调用登录（浏览器自动处理）",
        })),
        "click" => Ok(json!({
            "captchaType": "click",
            "pageUrl": page_url,
            "message": "检测到点选类验证码（无法自动识别）——请在浏览器登录该书源后粘贴 Cookie",
        })),
        _ => Ok(json!({ "captchaType": "none", "message": "未检测到验证码" })),
    }
}

/// POST /reader3/submitCaptcha：图片验证码文本回填（浏览器流）→ 提交 → cookie 存库 →
/// loginCheckJs → {isLogin}
pub async fn submit_captcha(
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    captcha_id: &str,
    captcha_text: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<Value> {
    let session = get_captcha_session(captcha_id)
        .ok_or_else(|| anyhow!("验证码会话已过期（5 分钟），请重新获取"))?;
    if session.ns != ns || session.source_url != source.book_source_url {
        return Err(anyhow!("验证码会话与书源不匹配"));
    }
    if session.kind != "image" {
        return Err(anyhow!("该验证码会话不是图片验证码，无法提交文本"));
    }
    if captcha_text.trim().is_empty() {
        return Err(anyhow!("请输入验证码"));
    }
    let req = LoginRequest {
        username: username.unwrap_or(&session.username).to_string(),
        password: password.unwrap_or(&session.password).to_string(),
        captcha: captcha_text.trim().to_string(),
    };
    // 浏览器：导航 → 填验证码 + 用户/密码 → 提交 → cookie → loginCheckJs
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        submit_captcha_inner(storage, ns, source, &req),
    )
    .await;
    match result {
        Ok(Ok(outcome)) => Ok(match outcome {
            LoginOutcome::Success { cookie } => {
                json!({ "isLogin": true, "cookie": cookie, "needCaptcha": false })
            }
            LoginOutcome::NeedImageCaptcha {
                captcha_url,
                captcha_id,
                message,
            } => json!({
                "isLogin": false, "needCaptcha": true, "captchaUrl": captcha_url,
                "captchaId": captcha_id, "message": message
            }),
            LoginOutcome::NeedManualCookie { message } => json!({
                "isLogin": false, "needManualCaptcha": true, "message": message
            }),
            LoginOutcome::Failed { message } => json!({
                "isLogin": false, "message": message
            }),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(json!({
            "isLogin": false, "needManualCaptcha": true,
            "message": "验证码提交超时（30s）——请在浏览器登录该书源后，在书源设置粘贴 Cookie"
        })),
    }
}

async fn submit_captcha_inner(
    storage: &Storage,
    ns: &str,
    source: &BookSource,
    req: &LoginRequest,
) -> Result<LoginOutcome> {
    let login_url = source
        .login_url
        .as_deref()
        .ok_or_else(|| anyhow!("书源未配置 loginUrl"))?;
    let (raw_url, _suffix) = search::split_url_suffix(login_url);
    let url = replace_login_placeholders(&raw_url, &req.username, &req.password, &req.captcha);

    let mut b = browser::Browser::launch().await?;
    inject_cookies(&mut b, storage, ns, source, &url).await?;
    b.navigate(&url).await?;

    // 填验证码输入框
    let fill_captcha_js =
        browser::FILL_CAPTCHA_JS.replace("'CAPTCHA'", &serde_json::to_string(&req.captcha)?);
    let fc = b.evaluate(&fill_captcha_js).await?;
    if fc.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(LoginOutcome::Failed {
            message: "页面未找到验证码输入框".to_string(),
        });
    }
    // 填用户名/密码并提交
    let fill_js = browser::FILL_FORM_JS
        .replace("'USERNAME'", &serde_json::to_string(&req.username)?)
        .replace("'PASSWORD'", &serde_json::to_string(&req.password)?);
    let fill = b.evaluate(&fill_js).await?;
    if fill.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(LoginOutcome::Failed {
            message: "页面未找到登录表单".to_string(),
        });
    }
    let _ = b.evaluate(browser::SUBMIT_FORM_JS).await?;
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // 提交后仍出验证码 → 新截图
    let det = b.evaluate(browser::DETECT_CAPTCHA_JS).await?;
    if !det.is_null() && det.get("kind").and_then(|k| k.as_str()) == Some("image") {
        let (x, y, w, h) = rect_of(&det);
        let png = b.screenshot_clip(x, y, w, h).await?;
        use base64::Engine;
        let data_uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        let captcha_id = new_captcha_session(ns, source, "image", req);
        return Ok(LoginOutcome::NeedImageCaptcha {
            captcha_url: data_uri,
            captcha_id,
            message: "验证码不正确，请重试".to_string(),
        });
    }

    // 结果判定
    let html = b
        .evaluate("document.documentElement.outerHTML")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let cookie_str = {
        let cookies = b.get_cookies().await?;
        browser::Browser::cookies_to_string(&cookies)
    };
    let page_url = b
        .evaluate("location.href")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let ok = match &source.login_check_js {
        Some(js) => check_login(js, &cookie_str, &html, &page_url)?,
        None => true,
    };
    if ok {
        if !cookie_str.is_empty() {
            storage
                .set_cookie(ns, &source.book_source_url, &cookie_str)
                .await?;
        }
        return Ok(LoginOutcome::Success { cookie: cookie_str });
    }
    if detect_click_captcha(&html).is_some() {
        return Ok(LoginOutcome::NeedManualCookie {
            message: "验证后仍未通过——请在浏览器登录该书源后，在书源设置粘贴 Cookie".to_string(),
        });
    }
    Ok(LoginOutcome::Failed {
        message: "登录失败：loginCheckJs 未通过".to_string(),
    })
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn source_with_login(login_url: &str, login_check_js: &str) -> BookSource {
        BookSource {
            book_source_url: "https://src.test".to_string(),
            book_source_name: "测试源".to_string(),
            login_url: Some(login_url.to_string()),
            login_check_js: if login_check_js.is_empty() {
                None
            } else {
                Some(login_check_js.to_string())
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_replace_placeholders() {
        // 双花括号优先 + 单花括号 + 各字段
        assert_eq!(
            replace_login_placeholders(
                "https://a.com/login?u={{user}}&p={{pass}}&c={{captcha}}",
                "u1",
                "p1",
                "c1"
            ),
            "https://a.com/login?u=u1&p=p1&c=c1"
        );
        assert_eq!(
            replace_login_placeholders(
                "https://a.com/login?u={user}&p={pass}&c={captcha}",
                "u1",
                "p1",
                "c1"
            ),
            "https://a.com/login?u=u1&p=p1&c=c1"
        );
        // 未提供字段 → 空串
        assert_eq!(
            replace_login_placeholders("https://a.com/login?c={captcha}", "", "", ""),
            "https://a.com/login?c="
        );
        // username/password 别名
        assert_eq!(
            replace_login_placeholders(
                "https://a.com/{{username}}/{{password}}",
                "alice",
                "pw",
                ""
            ),
            "https://a.com/alice/pw"
        );
    }

    #[test]
    fn test_check_login() {
        // 空脚本 = 成功（legacy 语义）
        assert!(check_login("", "a=1", "body", "https://a.com").unwrap());
        // true/1 → 成功
        assert!(check_login(
            "result.indexOf('ok') >= 0",
            "a=1",
            "ok body",
            "https://a.com"
        )
        .unwrap());
        assert!(!check_login(
            "result.indexOf('ok') >= 0",
            "a=1",
            "bad body",
            "https://a.com"
        )
        .unwrap());
        assert!(check_login(
            "cookie.indexOf('sid') >= 0",
            "sid=1; a=2",
            "x",
            "https://a.com"
        )
        .unwrap());
        assert!(!check_login("cookie.indexOf('sid') >= 0", "a=2", "x", "https://a.com").unwrap());
        // 布尔表达式直返
        assert!(check_login("true", "", "", "").unwrap());
        assert!(!check_login("false", "", "", "").unwrap());
    }

    #[test]
    fn test_merge_cookie() {
        // 新 Set-Cookie 覆盖同名、不同名保留、空值删除、顺序稳定
        let merged = merge_cookie(
            "sid=old; theme=dark",
            &[
                "sid=new; Path=/; HttpOnly".to_string(),
                "token=abc".to_string(),
            ],
        );
        assert_eq!(merged, "sid=new; theme=dark; token=abc");
        // 空值删除
        let merged = merge_cookie(
            "sid=old; theme=dark",
            &["sid=; Expires=Thu, 01 Jan 1970".to_string()],
        );
        assert_eq!(merged, "theme=dark");
        // 无既有 + 无 Set-Cookie
        assert_eq!(merge_cookie("", &[]), "");
        // 仅既有
        assert_eq!(merge_cookie("a=1", &[]), "a=1");
    }

    #[test]
    fn test_build_login_form() {
        let src = source_with_login("https://a.com/login", "");
        let req = LoginRequest {
            username: "u1".into(),
            password: "p1".into(),
            captcha: "".into(),
        };
        assert_eq!(build_login_form(&src, &req), "username=u1&password=p1");
        // 带验证码 → 追加 captcha 字段
        let req = LoginRequest {
            username: "u1".into(),
            password: "p1".into(),
            captcha: "c1".into(),
        };
        assert_eq!(
            build_login_form(&src, &req),
            "username=u1&password=p1&captcha=c1"
        );
        // loginUi 字段名优先
        let mut src2 = src.clone();
        src2.login_ui = Some(r#"[{"name":"loginName","type":"text"},{"name":"loginPassword","type":"password"},{"name":"vcode","type":"text"}]"#.into());
        let req = LoginRequest {
            username: "u2".into(),
            password: "p2".into(),
            captcha: "v2".into(),
        };
        assert_eq!(
            build_login_form(&src2, &req),
            "loginName=u2&loginPassword=p2&vcode=v2"
        );
    }

    #[test]
    fn test_detect_click_captcha() {
        assert_eq!(
            detect_click_captcha("<html>geetest slider</html>"),
            Some("slider")
        );
        assert_eq!(
            detect_click_captcha("<html>滑动验证</html>"),
            Some("slider")
        );
        assert_eq!(detect_click_captcha("<html>点选验证</html>"), Some("click"));
        assert_eq!(detect_click_captcha("<html>normal page</html>"), None);
        // 图片验证码页（img captcha）不算点击类
        assert_eq!(
            detect_click_captcha(r#"<img src="/captcha.png" alt="验证码">"#),
            None
        );
    }

    #[test]
    fn test_extract_image_captcha_url() {
        let html =
            r#"<html><img src="/captcha.png"><img id="vcode" src="https://a.com/c.png"></html>"#;
        assert_eq!(
            extract_image_captcha_url(html, "https://a.com/login").as_deref(),
            Some("https://a.com/captcha.png")
        );
        // 相对路径拼绝对
        let html = r#"<img class="captcha-img" data-src="/api/code?t=1">"#;
        assert_eq!(
            extract_image_captcha_url(html, "https://a.com/login").as_deref(),
            Some("https://a.com/api/code?t=1")
        );
        // 无验证码图 → None
        assert_eq!(
            extract_image_captcha_url("<img src='/logo.png'>", "https://a.com"),
            None
        );
    }

    #[test]
    fn test_captcha_session_ttl_and_match() {
        let src = source_with_login("https://a.com/login", "");
        let req = LoginRequest {
            username: "u".into(),
            password: "p".into(),
            captcha: "".into(),
        };
        let id = new_captcha_session("default", &src, "image", &req);
        let s = get_captcha_session(&id).unwrap();
        assert_eq!(s.ns, "default");
        assert_eq!(s.source_url, "https://src.test");
        assert_eq!(s.kind, "image");
        // 二次获取（已移除）→ None
        assert!(get_captcha_session(&id).is_none());
        // 未知 id → None
        assert!(get_captcha_session("nope").is_none());
    }
}

/// 定位非 Send 类型：tokio::spawn 要求 future Send（axum Handler 同约束）
#[cfg(test)]
mod send_tests {
    use super::*;

    async fn test_storage() -> Storage {
        let dir = std::env::temp_dir().join(format!("reader-login-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = crate::AppConfig::from_env();
        config.work_dir = dir.to_string_lossy().into_owned();
        crate::storage::init(&config).await.unwrap()
    }

    #[tokio::test]
    async fn test_login_futures_are_send() {
        let storage = test_storage().await;
        let src = BookSource {
            book_source_url: "https://a.com".into(),
            book_source_name: "A".into(),
            login_url: Some("https://a.com/login".into()),
            ..Default::default()
        };
        let s2 = storage.clone();
        let src2 = src.clone();
        tokio::spawn(async move {
            let _ = login_http(&s2, "default", &src2, &LoginRequest::default()).await;
        });
        let s3 = storage.clone();
        let src3 = src.clone();
        tokio::spawn(async move {
            let _ = login_browser(&s3, "default", &src3, &LoginRequest::default()).await;
        });
        storage.pool.close().await;
        let dir = std::env::temp_dir().join(format!("reader-login-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
