use super::*;
use kaiser_soze::logging::{Format, Level};
use std::path::Path;

#[test]
fn stream_selects_debug_without_an_explicit_level() {
    let args = parse_args_from(
        ["--tap-file", "synthetic.jsonl", "--stream"].map(String::from),
        Sources::new(Format::Human),
    )
    .unwrap();
    assert!(args.logging().stream);
    assert_eq!(args.logging().level, Level::Debug);
}

#[test]
fn explicit_logging_options_override_stream_defaults() {
    let args = parse_args_from(
        [
            "--tap-file",
            "synthetic.jsonl",
            "--stream",
            "--log-level",
            "warn",
            "--log-format",
            "json",
        ]
        .map(String::from),
        Sources::new(Format::Human),
    )
    .unwrap();
    assert_eq!(args.logging().level, Level::Warn);
    assert_eq!(args.logging().format, Format::Json);
}

#[test]
fn invalid_logging_values_and_unknown_flags_are_rejected() {
    for flags in [
        ["--log-level", "verbose"],
        ["--log-format", "xml"],
        ["--unknown", "value"],
    ] {
        assert!(parse_args_from(
            ["--tap-file", "synthetic.jsonl"]
                .into_iter()
                .chain(flags)
                .map(String::from),
            Sources::new(Format::Human)
        )
        .is_err());
    }
}

#[test]
fn debug_file_is_optional_and_can_be_named() {
    let args = parse_args_from(
        ["--tap-file", "synthetic.jsonl", "--log-file", "debug.jsonl"].map(String::from),
        Sources::new(Format::Human),
    )
    .unwrap();
    assert_eq!(args.log_file, Some(PathBuf::from("debug.jsonl")));
    let args = parse_args_from(
        ["--tap-file", "synthetic.jsonl"].map(String::from),
        Sources::new(Format::Human),
    )
    .unwrap();
    assert!(args.log_file.is_none());
}

#[test]
fn a_debug_file_cannot_overwrite_capture_files_or_their_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let tap = dir.path().join("tap.jsonl");
    let alias = dir.path().join("alias.jsonl");
    std::fs::write(&tap, b"synthetic capture\n").unwrap();
    std::fs::hard_link(&tap, &alias).unwrap();
    for log in [&tap, &alias] {
        let args = parse_args_from(
            [
                "--tap-file".into(),
                tap.display().to_string(),
                "--log-file".into(),
                log.display().to_string(),
            ],
            Sources::new(Format::Human),
        );
        assert!(args.is_err());
    }
    assert_eq!(std::fs::metadata(&tap).unwrap().len(), 18);
}

#[test]
fn a_new_debug_file_cannot_share_a_relative_capture_path() {
    let tap = format!("synthetic-absent-{}.jsonl", std::process::id());
    assert!(!Path::new(&tap).exists());
    let args = parse_args_from(
        [
            "--tap-file".into(),
            tap.clone(),
            "--log-file".into(),
            format!("./{tap}"),
        ],
        Sources::new(Format::Human),
    );
    assert!(args.is_err());
}

#[test]
fn dangling_symlinks_cannot_join_debug_and_capture_destinations() {
    for capture_flag in ["--tap-file", "--shape-file"] {
        for link_is_debug in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let capture = dir.path().join("capture.jsonl");
            let debug = dir.path().join("debug.jsonl");
            let (target, link) = if link_is_debug {
                (&capture, &debug)
            } else {
                (&debug, &capture)
            };
            std::os::unix::fs::symlink(target.file_name().unwrap(), link).unwrap();
            let args = parse_args_from(
                [
                    "--tap-file".into(),
                    dir.path().join("tap.jsonl").display().to_string(),
                    capture_flag.into(),
                    capture.display().to_string(),
                    "--log-file".into(),
                    debug.display().to_string(),
                ],
                Sources::new(Format::Human),
            );
            assert!(args.is_err());
            assert!(!target.exists());
        }
    }
}

#[test]
fn clap_accepts_equals_syntax_and_preserves_the_last_repeated_value() {
    let args = parse_args_from(
        ["--tap-file=synthetic.jsonl", "--port=48731", "--port=5000"].map(String::from),
        Sources::new(Format::Human),
    )
    .unwrap();
    assert_eq!(args.port, 5000);
    assert_eq!(args.tap_file, PathBuf::from("synthetic.jsonl"));
}

#[test]
fn a_config_file_supplies_required_settings_and_flags_override_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "tap_file = 'synthetic.jsonl'\nport = 48731\nstream = true\nlog_level = 'warn'\n",
    )
    .unwrap();
    let args = parse_args_from(
        [
            "--config".to_string(),
            path.display().to_string(),
            "--port=5000".to_string(),
        ],
        Sources::new(Format::Human),
    )
    .unwrap();
    assert_eq!(args.tap_file, PathBuf::from("synthetic.jsonl"));
    assert_eq!(args.port, 5000);
    assert!(args.logging().stream);
    assert_eq!(args.logging().level, Level::Warn);
}

#[test]
fn optional_files_layer_in_order_without_erasing_unspecified_settings() {
    let dir = tempfile::tempdir().unwrap();
    let system = dir.path().join("system.toml");
    let user = dir.path().join("user.toml");
    std::fs::write(&system, "tap_file = 'synthetic.jsonl'\nport = 3001\nlive = true\n").unwrap();
    std::fs::write(&user, "port = 3002\nstream = true\n").unwrap();
    let mut sources = Sources::new(Format::Json);
    sources.files = vec![dir.path().join("absent.toml"), system, user];
    let args = parse_args_from(Vec::<String>::new(), sources).unwrap();
    assert_eq!(args.port, 3002);
    assert_eq!(args.tap_file, PathBuf::from("synthetic.jsonl"));
    assert!(args.logging().live);
    assert!(args.logging().stream);
    assert_eq!(args.logging().level, Level::Debug);
    assert_eq!(args.logging().format, Format::Json);
}

#[test]
fn flags_override_environment_and_environment_overrides_file_values() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");
    std::fs::write(
        &file,
        "tap_file = 'synthetic.jsonl'\nlog_level = 'warn'\nlog_format = 'human'\nlive = true\nstream = true\n",
    )
    .unwrap();
    let mut sources = Sources::new(Format::Human);
    sources.environment.extend([
        ("SOZE_LOG".into(), "debug".into()),
        ("SOZE_LOG_FORMAT".into(), "json".into()),
    ]);
    let args = parse_args_from(
        [
            "--config".to_string(),
            file.display().to_string(),
            "--log-level=error".into(),
            "--stream=false".into(),
        ],
        sources,
    )
    .unwrap();
    assert_eq!(args.logging().level, Level::Error);
    assert_eq!(args.logging().format, Format::Json);
    assert!(args.logging().live);
    assert!(!args.logging().stream);
}

#[test]
fn valid_flags_can_override_invalid_lower_priority_environment_values() {
    let mut sources = Sources::new(Format::Human);
    sources.environment.insert("SOZE_LOG".into(), "invalid".into());
    let args = parse_args_from(["--tap-file=synthetic.jsonl", "--log-level=warn"], sources).unwrap();
    assert_eq!(args.logging().level, Level::Warn);
}

#[test]
fn undeclared_environment_variables_do_not_become_binary_settings() {
    let mut sources = Sources::new(Format::Human);
    sources.environment.insert("SOZE_PORT".into(), "invalid".into());
    let args = parse_args_from(["--tap-file=synthetic.jsonl"], sources).unwrap();
    assert_eq!(args.port, 0);
}

#[test]
fn malformed_unknown_and_invalid_file_settings_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    for contents in [
        "tap_file = [",
        "tap_file = 'synthetic.jsonl'\nport = -1\n",
        "tap_file = 'synthetic.jsonl'\nlog_level = 'trace'\n",
        "tap_file = 'synthetic.jsonl'\ntap_flie = 'typo'\n",
    ] {
        std::fs::write(&path, contents).unwrap();
        let result = parse_args_from(
            ["--config".to_string(), path.display().to_string()],
            Sources::new(Format::Human),
        );
        assert!(matches!(result, Err(Error::Configuration(config::Error::Load(_)))));
        assert_eq!(result.err().unwrap().kind(), "invalid_configuration");
    }
}

#[test]
fn missing_explicit_configuration_has_the_missing_configuration_code() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let error = parse_args_from(
        ["--config".to_string(), missing.display().to_string()],
        Sources::new(Format::Human),
    )
    .err()
    .unwrap();
    assert_eq!(error.kind(), "missing_configuration");
}

#[test]
fn missing_explicit_configuration_does_not_load_a_suffixed_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("config.toml");
    std::fs::write(dir.path().join("config.toml.toml"), "tap_file = 'synthetic.jsonl'\n").unwrap();

    let result = parse_args_from(
        ["--config".to_string(), missing.display().to_string()],
        Sources::new(Format::Human),
    );

    assert!(matches!(
        result,
        Err(Error::Configuration(config::Error::Load(::config::ConfigError::Foreign(error))))
            if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    ));
}

#[test]
fn explicit_configuration_preserves_non_file_io_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "synthetic").unwrap();

    for (path, kind) in [
        (dir.path().to_path_buf(), std::io::ErrorKind::IsADirectory),
        (file.join("config.toml"), std::io::ErrorKind::NotADirectory),
    ] {
        let result = parse_args_from(
            ["--config".to_string(), path.display().to_string()],
            Sources::new(Format::Human),
        );

        assert!(matches!(
            result,
            Err(Error::Configuration(config::Error::Load(::config::ConfigError::Foreign(error))))
                if error.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == kind)
        ));
    }
}

#[test]
fn other_configuration_io_failures_remain_invalid_configuration() {
    let error = Error::Configuration(config::Error::Load(::config::ConfigError::Foreign(Box::new(
        std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    ))));
    assert_eq!(error.kind(), "invalid_configuration");
}

#[test]
fn explicit_configuration_replaces_implicit_discovery_and_must_exist() {
    let dir = tempfile::tempdir().unwrap();
    let implicit = dir.path().join("implicit.toml");
    let explicit = dir.path().join("explicit.toml");
    std::fs::write(&implicit, "malformed = [").unwrap();
    std::fs::write(&explicit, "tap_file = 'synthetic.jsonl'\n").unwrap();
    let mut sources = Sources::new(Format::Human);
    sources.files.push(implicit);
    let args = parse_args_from(["--config".to_string(), explicit.display().to_string()], sources).unwrap();
    assert_eq!(args.tap_file, PathBuf::from("synthetic.jsonl"));
    std::fs::remove_file(&explicit).unwrap();
    let missing = parse_args_from(
        ["--config".to_string(), explicit.display().to_string()],
        Sources::new(Format::Human),
    );
    assert!(matches!(missing, Err(Error::Configuration(config::Error::Load(_)))));
}

#[test]
fn output_alias_validation_uses_the_merged_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("tap.jsonl");
    let alias = dir.path().join("alias.jsonl");
    let file = dir.path().join("config.toml");
    std::fs::write(&capture, b"synthetic capture\n").unwrap();
    std::fs::hard_link(&capture, &alias).unwrap();
    std::fs::write(&file, format!("tap_file = {capture:?}\nlog_file = {alias:?}\n")).unwrap();
    let result = parse_args_from(
        ["--config".to_string(), file.display().to_string()],
        Sources::new(Format::Human),
    );
    assert!(matches!(
        result,
        Err(Error::Configuration(config::Error::OutputOverlap))
    ));
    assert_eq!(std::fs::metadata(capture).unwrap().len(), 18);
}

#[test]
fn missing_required_capture_path_is_a_configuration_error() {
    let result = parse_args_from(Vec::<String>::new(), Sources::new(Format::Human));
    assert!(matches!(result, Err(Error::Configuration(config::Error::Load(_)))));
    assert_eq!(result.err().unwrap().kind(), "missing_configuration");
}

#[test]
fn configuration_files_cannot_be_used_as_output_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");
    let alias = dir.path().join("alias.toml");
    std::fs::write(&file, "tap_file = 'synthetic.jsonl'\n").unwrap();
    std::fs::hard_link(&file, &alias).unwrap();
    for flag in ["--tap-file", "--shape-file", "--log-file"] {
        let result = parse_args_from(
            [
                "--config".to_string(),
                file.display().to_string(),
                flag.to_string(),
                alias.display().to_string(),
            ],
            Sources::new(Format::Human),
        );
        assert!(result.is_err());
    }
}

#[test]
fn optional_config_paths_report_io_errors_instead_of_ignoring_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut sources = Sources::new(Format::Human);
    sources.files.push(dir.path().to_path_buf());
    let result = parse_args_from(["--tap-file=synthetic.jsonl"], sources);
    assert!(matches!(result, Err(Error::Configuration(config::Error::Load(_)))));
}

fn config_file(dir: &tempfile::TempDir, text: &str) -> PathBuf {
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("tap_file = 'synthetic.jsonl'\n{text}")).unwrap();
    path
}

fn started(path: &Path, flags: &[&str]) -> (config::Settings, Reload) {
    let args = ["--config".to_string(), path.display().to_string()]
        .into_iter()
        .chain(flags.iter().map(|flag| flag.to_string()));
    parse_with_reload(args, Sources::new(Format::Human)).ok().unwrap()
}

#[test]
fn a_file_may_omit_its_version_or_declare_1_and_nothing_newer() {
    let dir = tempfile::tempdir().unwrap();
    for text in ["", "version = 1\n"] {
        let path = config_file(&dir, text);
        assert!(parse_args_from(
            ["--config".into(), path.display().to_string()],
            Sources::new(Format::Human)
        )
        .is_ok());
    }
    for (text, newer) in [
        ("version = 2\n", true),
        ("version = 'one'\n", false),
        ("version = '1'\n", false),
    ] {
        let path = config_file(&dir, text);
        let result = parse_args_from(
            ["--config".into(), path.display().to_string()],
            Sources::new(Format::Human),
        );
        assert_eq!(
            matches!(result, Err(Error::Configuration(config::Error::UnsupportedVersion))),
            newer
        );
        assert!(result.is_err());
    }
}

#[test]
fn a_reload_takes_the_live_keys_and_names_the_restart_only_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_file(&dir, "port = 1000\nlog_level = 'warn'\n");
    let (mut running, reload) = started(&path, &[]);

    config_file(&dir, "port = 2000\nlog_level = 'debug'\nlive = true\n");
    let restart = running.reload_into(reload.run().ok().unwrap());

    assert_eq!(restart, ["port"]);
    assert_eq!(running.port, 1000);
    assert_eq!(running.logging().level, Level::Debug);
    assert!(running.logging().live);
}

#[test]
fn a_reload_that_changes_nothing_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_file(&dir, "log_level = 'warn'\n");
    let (mut running, reload) = started(&path, &[]);
    let before = running.logging();

    let restart = running.reload_into(reload.run().ok().unwrap());

    assert!(restart.is_empty());
    assert_eq!(running.logging(), before);
}

#[test]
fn a_failed_reload_returns_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_file(&dir, "log_level = 'warn'\n");
    let (_, reload) = started(&path, &[]);

    for text in ["port = ", "unknown_key = 1\n", "version = 2\n", "log_level = 'loud'\n"] {
        config_file(&dir, text);
        assert!(reload.run().is_err());
    }
    std::fs::remove_file(&path).unwrap();
    assert!(reload.run().is_err());
}

#[test]
fn flags_keep_their_precedence_over_an_edited_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_file(&dir, "log_level = 'warn'\n");
    let (mut running, reload) = started(&path, &["--log-level=error"]);

    config_file(&dir, "log_level = 'debug'\n");
    running.reload_into(reload.run().ok().unwrap());

    assert_eq!(running.logging().level, Level::Error);
}

#[test]
fn each_restart_only_key_is_named_when_a_reload_changes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    for (key, value) in [
        ("port", "2000"),
        ("tap_file", "'other.jsonl'"),
        ("shape_file", "'shape.jsonl'"),
        ("log_file", "'debug.jsonl'"),
        ("log_format", "'json'"),
        ("retention", "{ segment_mib = 32 }"),
    ] {
        std::fs::write(&path, "tap_file = 'synthetic.jsonl'\n").unwrap();
        let (mut running, reload) = started(&path, &[]);

        let tap = if key == "tap_file" {
            String::new()
        } else {
            "tap_file = 'synthetic.jsonl'\n".into()
        };
        std::fs::write(&path, format!("{tap}{key} = {value}\n")).unwrap();

        assert_eq!(running.reload_into(reload.run().ok().unwrap()), [key]);
    }
}

fn retention_from(text: &str) -> Result<config::Retention, Error> {
    let dir = tempfile::tempdir().unwrap();
    let path = config_file(&dir, text);
    parse_args_from(
        ["--config".into(), path.display().to_string()],
        Sources::new(Format::Human),
    )
    .map(|settings| settings.retention)
}

#[test]
fn retention_defaults_apply_to_whatever_the_file_omits() {
    let defaults = config::Retention {
        segment_mib: 64,
        capacity_mib: 1024,
        max_age_days: 14,
    };
    assert_eq!(retention_from("").ok(), Some(defaults));
    assert_eq!(
        retention_from("[retention]\nsegment_mib = 32\n").ok(),
        Some(config::Retention {
            segment_mib: 32,
            ..defaults
        })
    );
}

#[test]
fn retention_accepts_a_capacity_equal_to_the_segment_size() {
    let retention = retention_from("[retention]\nsegment_mib = 8\ncapacity_mib = 8\n").ok();
    assert_eq!(retention.map(|retention| retention.capacity_mib), Some(8));
}

#[test]
fn invalid_retention_is_an_invalid_configuration() {
    for text in [
        "[retention]\nsegment_mib = 0\n",
        "[retention]\ncapacity_mib = 0\n",
        "[retention]\nmax_age_days = 0\n",
        "[retention]\nmax_age_days = -1\n",
        "[retention]\nsegment_mib = 100\ncapacity_mib = 99\n",
        "[retention]\nunknown_key = 1\n",
    ] {
        let result = retention_from(text);
        assert!(matches!(result, Err(Error::Configuration(config::Error::Load(_)))));
        assert_eq!(result.unwrap_err().kind(), "invalid_configuration");
    }
}
