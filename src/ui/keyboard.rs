//! Pure conversion from logical keyboard input to ASR-33 bytes.
//!
//! Legacy Python raises `UnicodeEncodeError` for non-ASCII input. Rust returns
//! [`KeyboardEncodingError`] instead; the UI displays it without submitting a
//! partial payload or terminating the application.

use crate::core::config::KeyboardParityMode;
use crate::core::terminal::encode_even_parity;
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyboardInput {
    Text(String),
    Return,
    Backspace,
    Tab,
    Control(char),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardOptions {
    pub uppercase_only: bool,
    pub parity: KeyboardParityMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyboardEncodingError {
    character: char,
}

impl fmt::Display for KeyboardEncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "keyboard character {:?} is not ASCII",
            self.character
        )
    }
}

impl Error for KeyboardEncodingError {}

pub fn encode_input(
    input: &KeyboardInput,
    options: KeyboardOptions,
) -> Result<Vec<u8>, KeyboardEncodingError> {
    match input {
        KeyboardInput::Text(text) => {
            let mut bytes = Vec::new();
            for character in text.chars() {
                bytes.extend(encode_character(character, options)?);
            }
            Ok(bytes)
        }
        KeyboardInput::Return => encode_character('\r', options),
        KeyboardInput::Backspace => encode_character('\x08', options),
        KeyboardInput::Tab => encode_character('\t', options),
        KeyboardInput::Control(character) => {
            let upper = character.to_ascii_uppercase() as u32;
            if (u32::from(b'@')..=u32::from(b'_')).contains(&upper) {
                Ok(encode_bytes(&[(upper as u8) & 0x1f], options.parity))
            } else {
                Ok(Vec::new())
            }
        }
    }
}

fn encode_character(
    character: char,
    options: KeyboardOptions,
) -> Result<Vec<u8>, KeyboardEncodingError> {
    let normalized = if options.uppercase_only {
        character.to_uppercase().collect::<String>()
    } else {
        character.to_string()
    };
    if !normalized.is_ascii() {
        return Err(KeyboardEncodingError { character });
    }
    Ok(encode_bytes(normalized.as_bytes(), options.parity))
}

fn encode_bytes(bytes: &[u8], parity: KeyboardParityMode) -> Vec<u8> {
    match parity {
        KeyboardParityMode::Even => encode_even_parity(bytes),
        // Legacy Tk/Pygame rebuild a one-byte value in these modes. This is
        // observable for Unicode uppercase expansions such as ß -> SS.
        KeyboardParityMode::Mark => bytes.first().map(|byte| byte | 0x80).into_iter().collect(),
        KeyboardParityMode::Space => bytes.first().map(|byte| byte & 0x7f).into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyboardInput, KeyboardOptions, encode_input};
    use crate::core::config::KeyboardParityMode;

    fn options(parity: KeyboardParityMode) -> KeyboardOptions {
        KeyboardOptions {
            uppercase_only: false,
            parity,
        }
    }

    #[test]
    fn maps_printable_and_control_input() {
        let options = options(KeyboardParityMode::Space);
        assert_eq!(
            encode_input(&KeyboardInput::Text("a~".into()), options).expect("ASCII encodes"),
            b"a~"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Return, options).expect("CR encodes"),
            b"\r"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Backspace, options).expect("BS encodes"),
            b"\x08"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Tab, options).expect("tab encodes"),
            b"\t"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Control('C'), options).expect("control encodes"),
            b"\x03"
        );
    }

    #[test]
    fn applies_uppercase_and_all_parity_modes() {
        let mut settings = options(KeyboardParityMode::Space);
        settings.uppercase_only = true;
        assert_eq!(
            encode_input(&KeyboardInput::Text("a".into()), settings).expect("ASCII encodes"),
            b"A"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Text("ß".into()), settings).expect("uppercase is ASCII"),
            b"S"
        );
        settings.parity = KeyboardParityMode::Mark;
        assert_eq!(
            encode_input(&KeyboardInput::Return, settings).expect("CR encodes"),
            b"\x8d",
            "interactive Return still uses keyboard parity"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Text("A".into()), settings).expect("ASCII encodes"),
            b"\xc1"
        );
        settings.parity = KeyboardParityMode::Even;
        assert_eq!(
            encode_input(&KeyboardInput::Text("C".into()), settings).expect("ASCII encodes"),
            b"\xc3"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Text("ß".into()), settings).expect("uppercase is ASCII"),
            b"SS"
        );
    }

    #[test]
    fn return_is_authentic_cr_and_ctrl_j_is_line_feed_with_parity() {
        let space = options(KeyboardParityMode::Space);
        assert_eq!(
            encode_input(&KeyboardInput::Return, space).expect("CR"),
            b"\r"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Control('J'), space).expect("LF"),
            b"\n"
        );
        let mark = options(KeyboardParityMode::Mark);
        assert_eq!(
            encode_input(&KeyboardInput::Return, mark).expect("marked CR"),
            b"\x8d"
        );
        assert_eq!(
            encode_input(&KeyboardInput::Control('J'), mark).expect("marked LF"),
            b"\x8a"
        );
    }

    #[test]
    fn rejects_non_ascii_explicitly() {
        let error = encode_input(
            &KeyboardInput::Text("é".into()),
            options(KeyboardParityMode::Space),
        )
        .expect_err("legacy ASCII-only input is rejected");
        assert_eq!(error.to_string(), "keyboard character 'é' is not ASCII");
        assert!(
            encode_input(
                &KeyboardInput::Text("Aé".into()),
                options(KeyboardParityMode::Space),
            )
            .is_err(),
            "a mixed input returns no partial byte payload"
        );
    }
}
