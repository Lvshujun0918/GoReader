//! 请求统计中间件：每个请求记录一次（总数/今日/按接口 Top）
//!
//! 数据落在 `service::monitor::REQUESTS`（内存原子计数器），
//! `GET /reader3/getSystemInfo` / `GET /reader3/getServerStats` 读取聚合。
//! 挂载在最外层（upload_limit 之外）——413/404/静态资源请求同样计入。

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response};
use tower::{Layer, Service};

/// 挂载用 Layer：`app.layer(StatsLayer)`（最外层——所有请求均计数）
#[derive(Debug, Clone, Copy, Default)]
pub struct StatsLayer;

impl<S> Layer<S> for StatsLayer {
    type Service = Stats<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Stats { inner }
    }
}

/// 为每个请求计数一次的 Service 包装
#[derive(Debug, Clone)]
pub struct Stats<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for Stats<S>
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
        // 按路径（不含 query）计数；uri().path() 无分配以外开销可忽略
        let path = req.uri().path().to_string();
        crate::service::monitor::record_request(&path);
        let fut = self.inner.call(req);
        Box::pin(async move { fut.await })
    }
}
