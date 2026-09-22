//! The driver and the pieces that compose bytes for it. Both the `receipt` CLI
//! and the `receipt-server` HTTP endpoint are thin shells around this.

pub mod art;
pub mod art_data;
pub mod generative;
/// Only the HTTP endpoint talks to Linear, and only it pulls in an HTTP client.
#[cfg(feature = "server")]
pub mod linear;
pub mod printer;
pub mod raster;
