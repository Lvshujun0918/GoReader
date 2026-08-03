//! 数据模型（兼容 legacy 实体）

pub mod book;
pub mod book_chapter;
pub mod book_group;
pub mod book_source;
pub mod bookmark;
pub mod rss;
pub mod user;

pub use book::Book;
pub use book_chapter::{BookChapter, BookInfo};
pub use book_group::BookGroup;
pub use book_source::BookSource;
pub use bookmark::Bookmark;
pub use rss::{RssArticle, RssSource};
pub use user::User;
