//! Owned Windows audio worker and parent/session contracts; native runtime remains unqualified.
pub mod clock;
#[cfg(any(windows, test))]
pub mod endpoints;
pub mod format;
pub mod packet;
pub mod pending;
pub mod protocol;
pub mod wav;
pub mod writer;
pub type Result<T> = std::result::Result<T, String>;

pub mod worker;

#[cfg(windows)]
pub mod native;

pub mod owned_child;
pub mod session;

