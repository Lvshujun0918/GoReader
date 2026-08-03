//! HTTP API（/reader3/*，兼容 legacy）

pub mod opds;
pub mod router;
pub mod webdav;

pub use router::router;
