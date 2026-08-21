use crate::core::config::SerialConfig;
#[cfg(any(windows, target_os = "linux", test))]
use crate::core::config::{SerialParity, StopBits};

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Win32DcbSettings {
    pub byte_size: u8,
    pub parity: u8,
    pub parity_check_enabled: bool,
    pub stop_bits: u8,
}

#[cfg(any(windows, test))]
impl From<&SerialConfig> for Win32DcbSettings {
    fn from(config: &SerialConfig) -> Self {
        let (parity, parity_check_enabled) = match config.parity {
            SerialParity::None => (0, false),
            SerialParity::Odd => (1, true),
            SerialParity::Even => (2, true),
            SerialParity::Mark => (3, true),
            SerialParity::Space => (4, true),
        };
        let stop_bits = match config.stopbits {
            StopBits::One => 0,
            StopBits::OnePointFive => 1,
            StopBits::Two => 2,
        };
        Self {
            byte_size: config.databits.into(),
            parity,
            parity_check_enabled,
            stop_bits,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LinuxTermiosSettings {
    pub character_size: u8,
    pub parity_enabled: bool,
    pub odd_parity: bool,
    pub mark_or_space: bool,
    pub two_stop_bits: bool,
}

#[cfg(any(target_os = "linux", test))]
impl From<&SerialConfig> for LinuxTermiosSettings {
    fn from(config: &SerialConfig) -> Self {
        let (parity_enabled, odd_parity, mark_or_space) = match config.parity {
            SerialParity::None => (false, false, false),
            SerialParity::Even => (true, false, false),
            SerialParity::Odd => (true, true, false),
            SerialParity::Mark => (true, true, true),
            SerialParity::Space => (true, false, true),
        };
        Self {
            character_size: u8::from(config.databits),
            parity_enabled,
            odd_parity,
            mark_or_space,
            // POSIX has only CSTOPB. pySerial maps 1.5 and 2 to CSTOPB.
            two_stop_bits: matches!(config.stopbits, StopBits::OnePointFive | StopBits::Two),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::DataBits;

    fn config(databits: DataBits, parity: SerialParity, stopbits: StopBits) -> SerialConfig {
        SerialConfig {
            port: "TEST".to_owned(),
            baudrate: 110,
            databits,
            parity,
            stopbits,
        }
    }

    #[test]
    fn every_domain_combination_translates_to_win32_dcb() {
        let data_bits = [
            DataBits::Five,
            DataBits::Six,
            DataBits::Seven,
            DataBits::Eight,
        ];
        let parities = [
            SerialParity::None,
            SerialParity::Even,
            SerialParity::Odd,
            SerialParity::Mark,
            SerialParity::Space,
        ];
        let stop_bits = [StopBits::One, StopBits::OnePointFive, StopBits::Two];

        for data in data_bits {
            for parity in parities {
                for stop in stop_bits {
                    let translated = Win32DcbSettings::from(&config(data, parity, stop));
                    assert_eq!(translated.byte_size, u8::from(data));
                    assert_eq!(
                        translated.parity_check_enabled,
                        parity != SerialParity::None
                    );
                    assert_eq!(translated.parity, parity_code(parity));
                    assert_eq!(translated.stop_bits, stop_code(stop));
                }
            }
        }
    }

    #[test]
    fn every_domain_combination_translates_to_linux_termios() {
        let data_bits = [
            DataBits::Five,
            DataBits::Six,
            DataBits::Seven,
            DataBits::Eight,
        ];
        let parities = [
            SerialParity::None,
            SerialParity::Even,
            SerialParity::Odd,
            SerialParity::Mark,
            SerialParity::Space,
        ];
        let stop_bits = [StopBits::One, StopBits::OnePointFive, StopBits::Two];

        for data in data_bits {
            for parity in parities {
                for stop in stop_bits {
                    let translated = LinuxTermiosSettings::from(&config(data, parity, stop));
                    assert_eq!(translated.character_size, u8::from(data));
                    assert_eq!(translated.parity_enabled, parity != SerialParity::None);
                    assert_eq!(
                        translated.odd_parity,
                        matches!(parity, SerialParity::Odd | SerialParity::Mark)
                    );
                    assert_eq!(
                        translated.mark_or_space,
                        matches!(parity, SerialParity::Mark | SerialParity::Space)
                    );
                    assert_eq!(translated.two_stop_bits, stop != StopBits::One);
                }
            }
        }
    }

    #[test]
    fn posix_one_point_five_intentionally_equals_two_stop_bits() {
        let one_point_five = LinuxTermiosSettings::from(&config(
            DataBits::Seven,
            SerialParity::None,
            StopBits::OnePointFive,
        ));
        let two =
            LinuxTermiosSettings::from(&config(DataBits::Seven, SerialParity::None, StopBits::Two));
        assert!(one_point_five.two_stop_bits);
        assert_eq!(one_point_five, two);
    }

    #[test]
    fn linux_mark_and_space_use_cmspar_with_distinct_parodd() {
        let mark =
            LinuxTermiosSettings::from(&config(DataBits::Seven, SerialParity::Mark, StopBits::One));
        let space = LinuxTermiosSettings::from(&config(
            DataBits::Seven,
            SerialParity::Space,
            StopBits::One,
        ));
        assert!(mark.mark_or_space && mark.odd_parity);
        assert!(space.mark_or_space && !space.odd_parity);
    }

    fn parity_code(parity: SerialParity) -> u8 {
        match parity {
            SerialParity::None => 0,
            SerialParity::Odd => 1,
            SerialParity::Even => 2,
            SerialParity::Mark => 3,
            SerialParity::Space => 4,
        }
    }

    fn stop_code(stop_bits: StopBits) -> u8 {
        match stop_bits {
            StopBits::One => 0,
            StopBits::OnePointFive => 1,
            StopBits::Two => 2,
        }
    }
}
