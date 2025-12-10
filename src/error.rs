use cpal::{BuildStreamError, DefaultStreamConfigError, DevicesError, PlayStreamError};
use kanal::{ReceiveError, SendError};
use std::fmt::{Display, Formatter};
use std::io;
use std::mem::discriminant;

#[derive(Debug)]
pub(crate) struct Error {
    pub(crate) kind: ErrorKind,
}

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Send(SendError),
    Receive(ReceiveError),
    Devices(DevicesError),
    BuildStream(BuildStreamError),
    PlayStream(PlayStreamError),
    DefaultStreamConfig(DefaultStreamConfigError),
    PluginLoad(vst::host::PluginLoadError),
    BadIcon(tray_icon::BadIcon),
    Menu(tray_icon::menu::Error),
    TrayIcon(tray_icon::Error),
    Json(serde_json::Error),
    Io(io::Error),
    NoOutputDevice,
    InvalidConfiguration(&'static str),
    NoInputDevice,
    EditorMissing,
}

impl PartialEq for ErrorKind {
    fn eq(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }
}

impl Eq for ErrorKind {}

impl From<SendError> for Error {
    fn from(err: SendError) -> Self {
        Error {
            kind: ErrorKind::Send(err),
        }
    }
}

impl From<ReceiveError> for Error {
    fn from(err: ReceiveError) -> Self {
        Error {
            kind: ErrorKind::Receive(err),
        }
    }
}

impl From<DevicesError> for Error {
    fn from(err: DevicesError) -> Self {
        Error {
            kind: ErrorKind::Devices(err),
        }
    }
}

impl From<BuildStreamError> for Error {
    fn from(err: BuildStreamError) -> Self {
        Error {
            kind: ErrorKind::BuildStream(err),
        }
    }
}

impl From<PlayStreamError> for Error {
    fn from(err: PlayStreamError) -> Self {
        Error {
            kind: ErrorKind::PlayStream(err),
        }
    }
}

impl From<vst::host::PluginLoadError> for Error {
    fn from(err: vst::host::PluginLoadError) -> Self {
        Error {
            kind: ErrorKind::PluginLoad(err),
        }
    }
}

impl From<DefaultStreamConfigError> for Error {
    fn from(err: DefaultStreamConfigError) -> Self {
        Error {
            kind: ErrorKind::DefaultStreamConfig(err),
        }
    }
}

impl From<tray_icon::BadIcon> for Error {
    fn from(err: tray_icon::BadIcon) -> Self {
        Error {
            kind: ErrorKind::BadIcon(err),
        }
    }
}

impl From<tray_icon::menu::Error> for Error {
    fn from(err: tray_icon::menu::Error) -> Self {
        Error {
            kind: ErrorKind::Menu(err),
        }
    }
}

impl From<tray_icon::Error> for Error {
    fn from(err: tray_icon::Error) -> Self {
        Error {
            kind: ErrorKind::TrayIcon(err),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error {
            kind: ErrorKind::Json(err),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error {
            kind: ErrorKind::Io(error),
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error { kind }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match &self.kind {
                ErrorKind::Send(error) => format!("send error: {}", error),
                ErrorKind::Receive(error) => format!("receive error: {}", error),
                ErrorKind::Devices(error) => format!("devices error: {}", error),
                ErrorKind::BuildStream(error) => format!("build stream error: {}", error),
                ErrorKind::PlayStream(error) => format!("play stream error: {}", error),
                ErrorKind::DefaultStreamConfig(error) =>
                    format!("default stream config error: {}", error),
                ErrorKind::PluginLoad(error) => format!("plugin load error: {}", error),
                ErrorKind::BadIcon(error) => format!("bad icon: {:?}", error),
                ErrorKind::Menu(error) => format!("menu error: {:?}", error),
                ErrorKind::TrayIcon(error) => format!("tray icon error: {:?}", error),
                ErrorKind::Io(error) => format!("io error: {}", error),
                ErrorKind::Json(error) => format!("json error: {}", error),
                ErrorKind::NoOutputDevice => "output device not found".to_string(),
                ErrorKind::InvalidConfiguration(message) =>
                    format!("invalid configuration: {}", message),
                ErrorKind::NoInputDevice => "input device not found".to_string(),
                ErrorKind::EditorMissing => "editor missing".to_string(),
            }
        )
    }
}
