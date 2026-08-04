//! GAP 62：multipart 上传超限（axum DefaultBodyLimit 413）→ 明确的 JSON 错误
//!
//! axum 的 DefaultBodyLimit 超限时返回 413 + 纯文本 "length limit exceeded"，
//! 前端难以给出可读提示。此 Layer 拦截 413 响应并替换为统一 JSON：
//! `{"isSuccess":false,"errorMsg":"文件过大：超过上传大小上限（N MB）","data":null}`

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// 上传上限（MB）——用于错误文案
#[derive(Debug, Clone)]
pub struct UploadLimitLayer {
    pub max_mb: i64,
}

impl<S> Layer<S> for UploadLimitLayer {
    type Service = UploadLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        UploadLimitService {
            inner,
            max_mb: self.max_mb,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UploadLimitService<S> {
    inner: S,
    max_mb: i64,
}

impl<S> Service<Request<Body>> for UploadLimitService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let max_mb = self.max_mb;
        Box::pin(async move {
            let resp = inner.call(req).await?;
            if resp.status() == StatusCode::PAYLOAD_TOO_LARGE {
                let msg = format!(
                    "文件过大：超过上传大小上限（{} MB，可用环境变量 READER_UPLOAD_MAX_MB 调整）",
                    max_mb
                );
                let body = serde_json::json!({
                    "isSuccess": false,
                    "errorMsg": msg,
                    "data": null,
                })
                .to_string();
                return Ok(Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .body(Body::from(body))
                    .unwrap_or(resp));
            }
            Ok(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    /// 413 → 明确的 JSON 错误（含上限文案）
    #[tokio::test]
    async fn test_413_rewritten_to_json() {
        let svc = tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(Body::from("length limit exceeded"))
                    .unwrap(),
            )
        });
        let mut layer_svc = UploadLimitLayer { max_mb: 100 }.layer(svc);
        let resp = layer_svc
            .oneshot(
                Request::builder()
                    .uri("/reader3/uploadLocalBook")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["isSuccess"], false);
        assert!(
            json["errorMsg"].as_str().unwrap().contains("100 MB"),
            "错误文案应含上限: {json}"
        );
        assert!(json["errorMsg"].as_str().unwrap().contains("READER_UPLOAD_MAX_MB"));
    }

    /// 非 413 响应原样透传
    #[tokio::test]
    async fn test_non_413_passthrough() {
        let svc = tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from("ok"))
                    .unwrap(),
            )
        });
        let mut layer_svc = UploadLimitLayer { max_mb: 100 }.layer(svc);
        let resp = layer_svc
            .oneshot(Request::builder().body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert_eq!(&bytes[..], b"ok");
    }
}
