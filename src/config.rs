use std::path::PathBuf;

/// The directory holding soze's own files (the CA today): the flag, else `SOZE_CONFIG_DIR`, else
/// `~/.config/soze`. An empty variable counts as unset.
pub fn config_dir(flag: Option<PathBuf>, env: Option<String>, home: &str) -> PathBuf {
    flag.or_else(|| env.filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(home).join(".config/soze"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_wins_over_the_environment_and_the_default() {
        // Assemble a flag, an environment value and a home folder.
        let flag = Some(PathBuf::from("/flag"));
        let env = Some("/env".to_string());
        // Act
        let dir = config_dir(flag, env, "/home/me");
        // Assert the flag is chosen.
        assert_eq!(dir, PathBuf::from("/flag"));
    }

    #[test]
    fn the_environment_wins_over_the_default() {
        // Assemble an environment value and no flag.
        let env = Some("/env".to_string());
        // Act
        let dir = config_dir(None, env, "/home/me");
        // Assert the environment value is chosen.
        assert_eq!(dir, PathBuf::from("/env"));
    }

    #[test]
    fn neither_given_means_the_soze_folder_under_home() {
        // Act with no flag and no environment value.
        let dir = config_dir(None, None, "/home/me");
        // Assert the default under home is chosen.
        assert_eq!(dir, PathBuf::from("/home/me/.config/soze"));
    }

    #[test]
    fn an_empty_environment_value_is_ignored() {
        // Act with an empty environment value.
        let dir = config_dir(None, Some(String::new()), "/home/me");
        // Assert the default is chosen.
        assert_eq!(dir, PathBuf::from("/home/me/.config/soze"));
    }
}
