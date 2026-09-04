//! Current-user-only Unix socket server, optional authenticated
//! loopback HTTP server, frame encoding, and the ordered event
//! stream.

pub mod broker;
pub mod error;
pub mod frame;
pub mod server;

pub use broker::EventBroker;
pub use error::TransportError;
pub use frame::{FrameKind, RequestFrame, ResponseFrame};
pub use server::{SOCKET_FILE_NAME, ServerHandle, SocketServer};
