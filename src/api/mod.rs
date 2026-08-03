//! HTTP API（/reader3/*，兼容 legacy）

pub mod router;
pub mod webdav;

pub use router::router;
