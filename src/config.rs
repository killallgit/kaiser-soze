mod paths;

use kaiser_soze::logging::{Format, Level, Settings as LogSettings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub port: u16,
    pub tap_file: PathBuf,
    pub shape_file: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
    log_level: Option<Level>,
    log_format: Format,
    live: bool,
    stream: bool,
}

impl Settings {
    pub fn logging(&self) -> LogSettings {
        LogSettings {
            level: self
                .log_level
                .unwrap_or(if self.stream { Level::Debug } else { Level::Info }),
            format: self.log_format,
            live: self.live,
            stream: self.stream,
        }
    }
}

/// Inputs are captured once so tests and startup resolve the same sources without mutating process state.
pub(super) struct Sources {
    pub files: Vec<PathBuf>,
    pub environment: HashMap<String, String>,
    pub format: Format,
}

impl Sources {
    pub fn new(format: Format) -> Self {
        Self {
            files: Vec::new(),
            environment: HashMap::new(),
            format,
        }
    }

    pub fn from_environment(format: Format) -> Self {
        let mut sources = Self::new(format);
        sources.files.push(PathBuf::from("/etc/soze/config.toml"));
        if let Some(home) = std::env::var_os("HOME") {
            sources.files.push(PathBuf::from(home).join(".config/soze/config.toml"));
        }
        for key in ["SOZE_LOG", "SOZE_LOG_FORMAT"] {
            if let Ok(value) = std::env::var(key) {
                sources.environment.insert(key.into(), value);
            }
        }
        sources
    }
}

#[derive(Debug)]
pub(super) enum Error {
    Load(::config::ConfigError),
    OutputOverlap,
    ConfigOverlap,
}

pub(super) fn load(overrides: &impl Serialize, explicit: Option<&Path>, sources: Sources) -> Result<Settings, Error> {
    let mut builder = ::config::Config::builder()
        .set_default("port", 0)
        .map_err(Error::Load)?
        .set_default("live", false)
        .map_err(Error::Load)?
        .set_default("stream", false)
        .map_err(Error::Load)?
        .set_default(
            "log_format",
            match sources.format {
                Format::Human => "human",
                Format::Json => "json",
            },
        )
        .map_err(Error::Load)?;
    let files = explicit.map_or(sources.files, |path| vec![path.to_path_buf()]);
    for path in &files {
        if explicit.is_none()
            && !path
                .try_exists()
                .map_err(|error| Error::Load(::config::ConfigError::Foreign(Box::new(error))))?
        {
            continue;
        }
        builder = builder.add_source(::config::File::from(path.as_path()).format(::config::FileFormat::Toml));
    }
    let environment = sources
        .environment
        .into_iter()
        .filter_map(|(key, value)| {
            let key = match key.as_str() {
                "SOZE_LOG" => "log_level",
                "SOZE_LOG_FORMAT" => "log_format",
                _ => return None,
            };
            Some((key.to_string(), value))
        })
        .collect();
    let settings: Settings = builder
        .add_source(::config::Environment::default().source(Some(environment)))
        .add_source(::config::Config::try_from(overrides).map_err(Error::Load)?)
        .build()
        .map_err(Error::Load)?
        .try_deserialize()
        .map_err(Error::Load)?;
    paths::validate(&settings, &files)?;
    Ok(settings)
}
