pub mod api;
pub mod extract;
pub mod mcp;
pub mod store;

pub const MAX_UPLOAD: usize = 10 * 1024 * 1024;
pub const MAX_SECTIONS: usize = 10_000;
