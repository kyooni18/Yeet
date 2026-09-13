pub(crate) const LEGACY_REMOTE_PAGE: &str = include_str!("page.html");

include!(concat!(env!("OUT_DIR"), "/remote_web_assets.rs"));
