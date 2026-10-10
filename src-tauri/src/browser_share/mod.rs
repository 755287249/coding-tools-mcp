pub mod access;
#[cfg(any(feature="desktop", test))]
mod assets;
#[cfg(feature="desktop")]
pub(crate) mod desktop;
#[cfg(feature="desktop")]
pub use desktop::{init, router};
