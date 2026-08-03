//! HTTP API（/reader3/*，兼容 legacy）

pub mod files;
pub mod opds;
pub mod router;
pub mod webdav;

pub use router::router;
