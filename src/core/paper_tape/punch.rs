//! In-memory paper-tape punch state and append/overwrite semantics.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PunchMode {
    Append,
    Overwrite,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PunchState {
    #[default]
    Unloaded,
    Stopped,
    Running,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TapePunch {
    mode: PunchMode,
    state: PunchState,
    bytes: Option<Vec<u8>>,
}

impl TapePunch {
    #[must_use]
    pub fn new(mode: PunchMode) -> Self {
        Self {
            mode,
            state: PunchState::Unloaded,
            bytes: None,
        }
    }

    #[must_use]
    pub fn mode(&self) -> PunchMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: PunchMode) {
        self.mode = mode;
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            PunchMode::Append => PunchMode::Overwrite,
            PunchMode::Overwrite => PunchMode::Append,
        };
    }

    #[must_use]
    pub fn state(&self) -> PunchState {
        self.state
    }

    /// Load the current storage contents according to the selected write mode.
    pub fn load(&mut self, existing: &[u8]) {
        self.bytes = Some(match self.mode {
            PunchMode::Append => existing.to_vec(),
            PunchMode::Overwrite => Vec::new(),
        });
        self.state = PunchState::Stopped;
    }

    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    pub fn start(&mut self) -> bool {
        if self.bytes.is_none() {
            return false;
        }
        self.state = PunchState::Running;
        true
    }

    pub fn stop(&mut self) {
        if self.bytes.is_some() {
            self.state = PunchState::Stopped;
        }
    }

    /// Add bytes when the punch is loaded and running.
    pub fn punch(&mut self, data: &[u8]) -> bool {
        if self.state != PunchState::Running {
            return false;
        }
        let Some(bytes) = self.bytes.as_mut() else {
            self.state = PunchState::Unloaded;
            return false;
        };
        bytes.extend_from_slice(data);
        true
    }

    /// Unload and return the complete bytes that storage should persist.
    pub fn unload(&mut self) -> Option<Vec<u8>> {
        self.state = PunchState::Unloaded;
        self.bytes.take()
    }
}
