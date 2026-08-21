#[cfg(windows)]
mod implementation {
    use std::io;
    use std::mem::{self, MaybeUninit};
    use std::os::windows::io::AsRawHandle;
    use std::time::Duration;

    use serialport::{DataBits, FlowControl, Parity, StopBits};
    use windows_sys::Win32::Devices::Communication::{DCB, GetCommState, SetCommState};
    use windows_sys::Win32::Foundation::HANDLE;

    use super::super::settings::Win32DcbSettings;
    use super::super::{PortIo, SerialAdapterError};
    use crate::core::config::{DataBits as DomainDataBits, SerialConfig, SerialParity};

    pub(crate) fn open(
        config: &SerialConfig,
        timeout: Duration,
    ) -> Result<Box<dyn PortIo>, SerialAdapterError> {
        let port = serialport::new(&config.port, config.baudrate)
            .data_bits(data_bits(config.databits))
            .flow_control(FlowControl::None)
            .parity(base_parity(config.parity))
            .stop_bits(base_stop_bits(config.stopbits))
            .timeout(timeout)
            .open_native()
            .map_err(SerialAdapterError::Open)?;
        apply_dcb(&port, config)?;
        Ok(Box::new(port))
    }

    fn apply_dcb(
        port: &serialport::COMPort,
        config: &SerialConfig,
    ) -> Result<(), SerialAdapterError> {
        let handle = port.as_raw_handle() as HANDLE;
        let mut dcb = MaybeUninit::<DCB>::zeroed();
        // SAFETY: `handle` remains owned by `port`; the initialized DCB has the
        // required length and Win32 writes only within that structure.
        let mut dcb = unsafe {
            (*dcb.as_mut_ptr()).DCBlength = mem::size_of::<DCB>() as u32;
            if GetCommState(handle, dcb.as_mut_ptr()) == 0 {
                return Err(SerialAdapterError::Platform(io::Error::last_os_error()));
            }
            dcb.assume_init()
        };
        let settings = Win32DcbSettings::from(config);
        dcb.ByteSize = settings.byte_size;
        dcb.Parity = settings.parity;
        dcb.StopBits = settings.stop_bits;
        const F_PARITY_BIT: u32 = 1 << 1;
        if settings.parity_check_enabled {
            dcb._bitfield |= F_PARITY_BIT;
        } else {
            dcb._bitfield &= !F_PARITY_BIT;
        }
        // SAFETY: the handle and DCB are valid for the duration of the call.
        if unsafe { SetCommState(handle, &dcb) } == 0 {
            return Err(SerialAdapterError::Platform(io::Error::last_os_error()));
        }
        Ok(())
    }

    fn data_bits(value: DomainDataBits) -> DataBits {
        match value {
            DomainDataBits::Five => DataBits::Five,
            DomainDataBits::Six => DataBits::Six,
            DomainDataBits::Seven => DataBits::Seven,
            DomainDataBits::Eight => DataBits::Eight,
        }
    }

    fn base_parity(value: SerialParity) -> Parity {
        match value {
            SerialParity::None | SerialParity::Mark | SerialParity::Space => Parity::None,
            SerialParity::Even => Parity::Even,
            SerialParity::Odd => Parity::Odd,
        }
    }

    fn base_stop_bits(value: crate::core::config::StopBits) -> StopBits {
        match value {
            crate::core::config::StopBits::One => StopBits::One,
            crate::core::config::StopBits::OnePointFive | crate::core::config::StopBits::Two => {
                StopBits::Two
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod implementation {
    use std::io;
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    use serialport::{DataBits, FlowControl, Parity, StopBits};

    use super::super::settings::LinuxTermiosSettings;
    use super::super::{PortIo, SerialAdapterError};
    use crate::core::config::{DataBits as DomainDataBits, SerialConfig, SerialParity};

    pub(crate) fn open(
        config: &SerialConfig,
        timeout: Duration,
    ) -> Result<Box<dyn PortIo>, SerialAdapterError> {
        let port = serialport::new(&config.port, config.baudrate)
            .data_bits(data_bits(config.databits))
            .flow_control(FlowControl::None)
            .parity(base_parity(config.parity))
            .stop_bits(base_stop_bits(config.stopbits))
            .timeout(timeout)
            .open_native()
            .map_err(SerialAdapterError::Open)?;
        apply_termios(&port, config)?;
        Ok(Box::new(port))
    }

    fn apply_termios(
        port: &serialport::TTYPort,
        config: &SerialConfig,
    ) -> Result<(), SerialAdapterError> {
        let fd = port.as_raw_fd();
        let mut termios = std::mem::MaybeUninit::<libc::termios>::zeroed();
        // SAFETY: `fd` is valid while `port` lives and tcgetattr initializes
        // the supplied termios structure on success.
        let mut termios = unsafe {
            if libc::tcgetattr(fd, termios.as_mut_ptr()) != 0 {
                return Err(SerialAdapterError::Platform(io::Error::last_os_error()));
            }
            termios.assume_init()
        };
        let settings = LinuxTermiosSettings::from(config);
        termios.c_cflag &=
            !(libc::CSIZE | libc::PARENB | libc::PARODD | libc::CMSPAR | libc::CSTOPB);
        termios.c_cflag |= match settings.character_size {
            5 => libc::CS5,
            6 => libc::CS6,
            7 => libc::CS7,
            8 => libc::CS8,
            value => {
                return Err(SerialAdapterError::Unsupported(format!(
                    "Linux does not support {value} data bits"
                )));
            }
        };
        if settings.parity_enabled {
            termios.c_cflag |= libc::PARENB;
        }
        if settings.odd_parity {
            termios.c_cflag |= libc::PARODD;
        }
        if settings.mark_or_space {
            termios.c_cflag |= libc::CMSPAR;
        }
        if settings.two_stop_bits {
            termios.c_cflag |= libc::CSTOPB;
        }
        // SAFETY: `fd` and the termios pointer are valid for this call.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            return Err(SerialAdapterError::Platform(io::Error::last_os_error()));
        }
        Ok(())
    }

    fn data_bits(value: DomainDataBits) -> DataBits {
        match value {
            DomainDataBits::Five => DataBits::Five,
            DomainDataBits::Six => DataBits::Six,
            DomainDataBits::Seven => DataBits::Seven,
            DomainDataBits::Eight => DataBits::Eight,
        }
    }

    fn base_parity(value: SerialParity) -> Parity {
        match value {
            SerialParity::None | SerialParity::Mark | SerialParity::Space => Parity::None,
            SerialParity::Even => Parity::Even,
            SerialParity::Odd => Parity::Odd,
        }
    }

    fn base_stop_bits(value: crate::core::config::StopBits) -> StopBits {
        match value {
            crate::core::config::StopBits::One => StopBits::One,
            crate::core::config::StopBits::OnePointFive | crate::core::config::StopBits::Two => {
                StopBits::Two
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod implementation {
    use std::time::Duration;

    use super::super::{PortIo, SerialAdapterError};
    use crate::core::config::SerialConfig;

    pub(crate) fn open(
        _config: &SerialConfig,
        _timeout: Duration,
    ) -> Result<Box<dyn PortIo>, SerialAdapterError> {
        Err(SerialAdapterError::Unsupported(
            "native serial configuration is implemented only for Windows and Linux".to_owned(),
        ))
    }
}

pub(super) use implementation::open;
