//! Runtime-independent transport adapters.

use std::error::Error;

use crate::core::events::{TransportCommand, TransportEvent};

pub mod serial;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSendFailure {
    Backpressure,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportSendError {
    pub failure: TransportSendFailure,
    pub command: TransportCommand,
    pub message: String,
}

/// Narrow lifecycle and routing boundary shared by real and fake transports.
pub trait Transport {
    type Error: Error + Send + Sync + 'static;

    fn start(&mut self) -> Result<(), Self::Error>;

    fn send(&mut self, command: TransportCommand) -> Result<(), TransportSendError>;

    fn try_recv(&mut self) -> Result<Option<TransportEvent>, Self::Error>;

    fn shutdown(&mut self) -> Result<(), Self::Error>;

    fn join(&mut self) -> Result<(), Self::Error>;
}
