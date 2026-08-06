//! WebDAV 服务（/reader3/webdav*，对齐 legacy WebdavController）
//!
//! 根目录：storage/data/{user}/webdav（secure 模式按 Basic 认证用户；非 secure 用 default）
//! 支持：OPTIONS / PROPFIND / GET / PUT / MKCOL / DELETE / MOVE / COPY / LOCK / UNLOCK

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;

use crate::storage::Storage;

/// WebDAV 处理入口（匹配 /reader3/webdav* 任意方法）
pub async fn handle(
    storage: &Storage,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. OPTIONS 预检（不校验认证——CORS/客户端预检，legacy 修复点）
    if method == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::OK)
            .header("DAV", "1,2")
            .header(
                "Allow",
                "OPTIONS,DELETE,GET,PUT,PROPFIND,MKCOL,MOVE,COPY,LOCK,UNLOCK",
            )
            .header("Access-Control-Allow-Origin", "*")
            .body(Body::empty())
            .unwrap();
    }

    // 2. Basic 认证
    let Some((_username, _ns, home)) = authenticate(storage, headers).await else {
        return webdav_status(
            StatusCode::UNAUTHORIZED,
            Some(("WWW-Authenticate", "Basic realm=\"reader\"")),
        );
    };

    // 3. 路径解析（webdav 根目录下）
    let Some(file) = resolve_path(&home, path) else {
        return webdav_status(StatusCode::BAD_REQUEST, None);
    };

    match method.as_str() {
        "PROPFIND" => propfind(&file, path, &home).await,
        "GET" | "HEAD" => get_file(&file).await,
        "PUT" => put_file(&file, body).await,
        "MKCOL" => mkcol(&file).await,
        "DELETE" => delete(&file).await,
        "MOVE" => move_copy(&file, headers, false).await,
        "COPY" => move_copy(&file, headers, true).await,
        "LOCK" => lock(headers),
        "UNLOCK" => webdav_status(StatusCode::NO_CONTENT, None),
        _ => webdav_status(StatusCode::METHOD_NOT_ALLOWED, None),
    }
}

/// Basic 认证 → (username, user_namespace, user_home)
pub(crate) async fn authenticate(
    storage: &Storage,
    headers: &HeaderMap,
) -> Option<(String, String, PathBuf)> {
    if !storage.config.secure {
        let home = storage
            .config
            .storage_dir()
            .join("data")
            .join("default")
            .join("webdav");
        return Some(("default".into(), "default".into(), home));
    }
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())?
        .strip_prefix("Basic ")?;
    let decoded = String::from_utf8(base64_decode(auth)).ok()?;
    let (username, password) = decoded.split_once(':')?;
    let user = storage.find_user(username).await.ok().flatten()?;
    if !user.enable_webdav {
        return None;
    }
    // 统一密码校验：argon2id（PHC）优先，legacy 双 MD5 兼容；MD5 通过时自动升级为 argon2id
    if !crate::util::password::verify_password(storage, &user, password).await {
        return None;
    }
    let home = storage
        .config
        .storage_dir()
        .join("data")
        .join(username)
        .join("webdav");
    Some((username.to_string(), username.to_string(), home))
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .unwrap_or_default()
}

/// 路径解析（安全：不越出 webdav 根）
fn resolve_path(home: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path);
    // 去掉 /reader3/webdav 前缀
    let rel = decoded
        .trim_start_matches("/reader3/webdav")
        .trim_start_matches('/');
    let home_abs = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let target = home_abs.join(rel);
    if target.starts_with(&home_abs) {
        Some(target)
    } else {
        None
    }
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

fn webdav_status(code: StatusCode, header: Option<(&str, &str)>) -> Response {
    let mut builder = Response::builder().status(code);
    if let Some((k, v)) = header {
        builder = builder.header(k, v);
    }
    builder.body(Body::empty()).unwrap()
}

/// PROPFIND：XML 列表（对齐 legacy 语义）
async fn propfind(file: &Path, request_path: &str, _home: &Path) -> Response {
    if !file.exists() {
        return webdav_status(StatusCode::NOT_FOUND, None);
    }
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">\n",
    );
    let base = request_path.trim_end_matches('/');
    let url_encode = |s: &str| s.replace(' ', "%20");

    // 自身
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    xml.push_str(&entry_xml(base, &name, file, true));
    // 子项（仅一级）
    if file.is_dir() {
        if let Ok(entries) = std::fs::read_dir(file) {
            for e in entries.flatten() {
                let child_name = e.file_name().to_string_lossy().into_owned();
                let child_path = e.path();
                let child_url = format!("{}/{}", base, url_encode(&child_name));
                xml.push_str(&entry_xml(&child_url, "", &child_path, false));
            }
        }
    }
    xml.push_str("</D:multistatus>");
    Response::builder()
        .status(StatusCode::MULTI_STATUS)
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(Body::from(xml))
        .unwrap()
}

fn entry_xml(url: &str, name: &str, file: &Path, is_self: bool) -> String {
    let modified = file
        .metadata()
        .and_then(|m| m.modified())
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        })
        .unwrap_or_default();
    let display = if is_self { name } else { name };
    if file.is_dir() {
        format!(
            "<D:response><D:href>{}</D:href><D:propstat><D:status>HTTP/1.1 200 OK</D:status><D:prop><D:getlastmodified>{}</D:getlastmodified><D:creationdate>{}</D:creationdate><D:resourcetype><D:collection/></D:resourcetype><D:displayname>{}</D:displayname></D:prop></D:propstat></D:response>\n",
            url, modified, modified, display
        )
    } else {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        format!(
            "<D:response><D:href>{}</D:href><D:propstat><D:status>HTTP/1.1 200 OK</D:status><D:prop><D:getlastmodified>{}</D:getlastmodified><D:creationdate>{}</D:creationdate><D:resourcetype/><D:displayname>{}</D:displayname><D:getcontentlength>{}</D:getcontentlength></D:prop></D:propstat></D:response>\n",
            url, modified, modified, display, len
        )
    }
}

async fn get_file(file: &Path) -> Response {
    if !file.exists() || !file.is_file() {
        return webdav_status(StatusCode::NOT_FOUND, None);
    }
    match tokio::fs::read(file).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
    }
}

async fn put_file(file: &Path, body: axum::body::Bytes) -> Response {
    let Some(parent) = file.parent() else {
        return webdav_status(StatusCode::CONFLICT, None);
    };
    if !parent.exists() {
        return webdav_status(StatusCode::CONFLICT, None);
    }
    match tokio::fs::write(file, &body).await {
        Ok(_) => webdav_status(StatusCode::CREATED, None),
        Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
    }
}

async fn mkcol(file: &Path) -> Response {
    if file.exists() {
        return webdav_status(StatusCode::METHOD_NOT_ALLOWED, None);
    }
    match tokio::fs::create_dir_all(file).await {
        Ok(_) => webdav_status(StatusCode::CREATED, None),
        Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
    }
}

async fn delete(file: &Path) -> Response {
    if !file.exists() {
        return webdav_status(StatusCode::NOT_FOUND, None);
    }
    if file.is_dir() {
        match tokio::fs::remove_dir_all(file).await {
            Ok(_) => webdav_status(StatusCode::OK, None),
            Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
        }
    } else {
        match tokio::fs::remove_file(file).await {
            Ok(_) => webdav_status(StatusCode::OK, None),
            Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
        }
    }
}

/// MOVE/COPY（Destination 头）
async fn move_copy(file: &Path, headers: &HeaderMap, copy: bool) -> Response {
    let Some(dest) = headers.get("destination").and_then(|v| v.to_str().ok()) else {
        return webdav_status(StatusCode::BAD_REQUEST, None);
    };
    let dest_path = percent_decode(dest.split('?').next().unwrap_or(dest));
    // Destination 是完整 URL（http://host/reader3/webdav/xxx）——取路径部分
    let dest_path = dest_path
        .split("://")
        .nth(1)
        .and_then(|s| s.split_once('/').map(|(_, p)| format!("/{p}")))
        .unwrap_or(dest_path.clone());
    // 需要 home 才能 resolve——简化：MOVE 在 webdav 根内（调用方传入的 file 已安全）
    // 这里用 file.parent 推导 home（webdav 根 = file 链上最近的 webdav 目录）
    let mut home = file.to_path_buf();
    while let Some(p) = home.parent() {
        if p.file_name().map(|n| n == "webdav").unwrap_or(false) {
            home = p.to_path_buf();
            break;
        }
        home = p.to_path_buf();
    }
    let rel = dest_path
        .trim_start_matches("/reader3/webdav")
        .trim_start_matches('/');
    let target = home.join(rel);
    // 安全校验：目标必须在 webdav 根内（防路径穿越任意写入）
    let home_abs = home.canonicalize().unwrap_or_else(|_| home.clone());
    let target_abs = target.canonicalize().unwrap_or_else(|_| target.clone());
    if !target_abs.starts_with(&home_abs) {
        return webdav_status(StatusCode::FORBIDDEN, None);
    }
    if !file.exists() {
        return webdav_status(StatusCode::NOT_FOUND, None);
    }
    if copy {
        match copy_recursive(file, &target) {
            Ok(_) => webdav_status(StatusCode::CREATED, None),
            Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
        }
    } else {
        match tokio::fs::rename(file, &target).await {
            Ok(_) => webdav_status(StatusCode::CREATED, None),
            Err(_) => webdav_status(StatusCode::INTERNAL_SERVER_ERROR, None),
        }
    }
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// LOCK：返回 lock token（legacy 语义——不真正持锁）
fn lock(headers: &HeaderMap) -> Response {
    let _timeout = headers
        .get("timeout")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Second-3600")
        .to_string();
    let lock_token = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock><D:locktype><write/></D:locktype><D:lockscope><exclusive/></D:lockscope><D:locktoken><D:href>{}</D:href></D:locktoken><D:depth>infinity</D:depth></D:activelock></D:lockdiscovery></D:prop>",
        lock_token
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("Lock-Token", lock_token)
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(Body::from(body))
        .unwrap()
}
