#[cfg(feature = "ghost-cell")]
pub mod input_taking;
#[cfg(not(feature = "ghost-cell"))]
pub mod input_taking_refmut;
mod poller;
pub mod resumable;
