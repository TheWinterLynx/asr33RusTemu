//! Runtime-independent contracts between the application, throttle, and transports.

/// Whether keyboard/application data is sent to a transport or echoed locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunicationMode {
    Line,
    Local,
}

/// Whether byte pacing is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleMode {
    Throttled,
    Unthrottled,
}

/// Independently paced data flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFlow {
    Tx,
    Rx,
    Loopback,
}

/// Commands originating in application/device logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationCommand {
    Transmit(Vec<u8>),
    SetCommunicationMode(CommunicationMode),
    SetThrottleMode(ThrottleMode),
    SetTxRate(i64),
    SetRxRate(i64),
    SetPrinterEnabled(bool),
}

/// Commands a future transport adapter must consume in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportCommand {
    Send(Vec<u8>),
}

/// Events a future transport adapter may produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    Received(Vec<u8>),
    Failed {
        operation: TransportOperation,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportOperation {
    Read,
    Write,
}

/// Observable output after routing and pacing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrottleOutput {
    ToTransport(TransportCommand),
    ToApplication(Vec<u8>),
}
