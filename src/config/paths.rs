use super::{Error, Settings};
use kaiser_soze::sink::segment_index;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) fn validate(settings: &Settings, files: &[PathBuf]) -> Result<(), Error> {
    if settings.log_file.as_ref().is_some_and(|log| {
        same_file(log, &settings.tap_file) || settings.shape_file.as_ref().is_some_and(|shape| same_file(log, shape))
    }) {
        return Err(Error::OutputOverlap);
    }
    let outputs = [
        Some(&settings.tap_file),
        settings.shape_file.as_ref(),
        settings.log_file.as_ref(),
    ];
    for output in outputs.into_iter().flatten() {
        if files.iter().any(|file| same_file(output, file)) {
            return Err(Error::ConfigOverlap);
        }
    }
    let rotated = [Some(&settings.tap_file), settings.shape_file.as_ref()];
    if outputs.into_iter().flatten().chain(files).any(|output| {
        rotated
            .into_iter()
            .flatten()
            .any(|active| named_like_segment(output, active))
    }) {
        return Err(Error::SegmentOverlap);
    }
    Ok(())
}

/// Cleanup deletes closed segments, so an output carrying a segment name beside the active file would be lost.
fn named_like_segment(output: &Path, active: &Path) -> bool {
    output
        .file_name()
        .is_some_and(|name| segment_index(active, name).is_some())
        && directory(output).is_some_and(|parent| directory(active) == Some(parent))
}

fn directory(path: &Path) -> Option<PathBuf> {
    resolved(
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || resolved(left)
            .zip(resolved(right))
            .is_some_and(|(left, right)| left == right)
        || std::fs::metadata(left)
            .ok()
            .zip(std::fs::metadata(right).ok())
            .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
}

fn resolved(path: &Path) -> Option<PathBuf> {
    let mut path = path.to_path_buf();
    // Canonicalize existing paths and resolve final links even when their target is not created yet.
    for _ in 0..40 {
        if let Ok(resolved) = path.canonicalize() {
            return Some(resolved);
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Ok(target) = path.read_link() {
            path = parent.join(target);
        } else {
            return Some(parent.canonicalize().ok()?.join(path.file_name()?));
        }
    }
    None
}
