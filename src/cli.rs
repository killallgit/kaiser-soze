use crate::config::{self, Sources};
use clap::Parser;
use kaiser_soze::logging::{Format, Level};
use serde::Serialize;
use std::ffi::OsString;
use std::path::PathBuf;

/// Capture coding-agent model traffic through a local proxy.
#[derive(Parser, Serialize)]
#[command(name = "soze", version, args_override_self = true)]
struct Options {
    /// Load a TOML file instead of the system and user configuration files.
    #[arg(long, value_name = "FILE")]
    #[serde(skip)]
    config: Option<PathBuf>,
    /// Listen on this loopback port (default: OS assigned).
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    /// Append captured traffic to this JSONL file (required after configuration loading).
    #[arg(long, value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    tap_file: Option<PathBuf>,
    /// Write distinct request/reply shapes to this JSONL file.
    #[arg(long, value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    shape_file: Option<PathBuf>,
    /// Reset and write bounded debug metadata to this JSONL file.
    #[arg(long, value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    log_file: Option<PathBuf>,
    /// Terminal severity: off, error, warn, info or debug.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    log_level: Option<Level>,
    /// Terminal output format: human or json.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    log_format: Option<Format>,
    /// Show request/reply summaries; use --live=false to disable a configured value.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    live: Option<bool>,
    /// Show stream metadata; use --stream=false to disable a configured value.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug)]
pub(super) enum Error {
    Arguments(clap::error::ErrorKind),
    Display(String),
    Configuration(config::Error),
}

impl Error {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Arguments(kind) => match kind {
                clap::error::ErrorKind::UnknownArgument => "unknown_flag",
                clap::error::ErrorKind::InvalidValue | clap::error::ErrorKind::ValueValidation => {
                    "invalid_argument_value"
                }
                _ => "invalid_arguments",
            },
            Self::Display(_) => "display_requested",
            Self::Configuration(config::Error::OutputOverlap) => "debug_file_overlaps_capture",
            Self::Configuration(config::Error::ConfigOverlap) => "output_overlaps_configuration",
            Self::Configuration(config::Error::Load(error)) => match error {
                ::config::ConfigError::NotFound(_) => "missing_configuration",
                _ => "invalid_configuration",
            },
        }
    }
}

pub(super) fn parse_args_from<I, T>(args: I, sources: Sources) -> Result<config::Settings, Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args = std::iter::once(OsString::from("soze")).chain(args.into_iter().map(Into::into));
    let options = Options::try_parse_from(args).map_err(|error| {
        if matches!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        ) {
            Error::Display(error.render().to_string())
        } else {
            Error::Arguments(error.kind())
        }
    })?;
    config::load(&options, options.config.as_deref(), sources).map_err(Error::Configuration)
}

pub(super) fn parse_args(format: Format) -> Result<config::Settings, Error> {
    parse_args_from(std::env::args_os().skip(1), Sources::from_environment(format))
}

#[cfg(test)]
mod tests;
