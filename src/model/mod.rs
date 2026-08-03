//! 数据模型（兼容 legacy 实体）

pub mod book;
pub mod book_chapter;
pub mod book_group;
pub mod book_source;
pub mod bookmark;
pub mod http_tts;
pub mod replace_rule;
pub mod rss;
pub mod source_sub;
pub mod txt_toc_rule;
pub mod user;

pub use book::Book;
pub use book_chapter::{BookChapter, BookInfo};
pub use book_group::{BookGroup, BookGroupWithCount};
pub use book_source::BookSource;
pub use bookmark::Bookmark;
pub use http_tts::HttpTts;
pub use replace_rule::ReplaceRule;
pub use rss::{RssArticle, RssSource};
pub use source_sub::SourceSub;
pub use txt_toc_rule::TxtTocRule;
pub use user::User;
