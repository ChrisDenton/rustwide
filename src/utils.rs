use failure::Error;
use fs2::FileExt;
use log::warn;
use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf, Prefix, PrefixComponent};

pub(crate) fn file_lock<T>(
    path: &Path,
    msg: &str,
    f: impl FnOnce() -> Result<T, Error> + std::panic::UnwindSafe,
) -> Result<T, Error> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;

    let mut message_displayed = false;
    while let Err(err) = file.try_lock_exclusive() {
        if !message_displayed && err.kind() == fs2::lock_contended_error().kind() {
            warn!("blocking on other processes finishing to {}", msg);
            message_displayed = true;
        }
        file.lock_exclusive()?;
    }

    let res = std::panic::catch_unwind(f);
    let _ = file.unlock();

    match res {
        Ok(res) => res,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// If a prefix uses the extended-length syntax (`\\?\`), return the equivalent version without it.
///
/// Returns `None` if `prefix.kind().is_verbatim()` is `false`.
fn strip_verbatim_from_prefix(prefix: &PrefixComponent<'_>) -> Option<PathBuf> {
    let ret = match prefix.kind() {
        Prefix::Verbatim(s) => Path::new(s).to_owned(),

        Prefix::VerbatimDisk(drive) => [format!(r"{}:\", drive as char)].iter().collect(),

        Prefix::VerbatimUNC(_, _) => unimplemented!(),

        _ => return None,
    };

    Some(ret)
}

#[derive(Debug)]
struct DirRemoveError {
    kind: std::io::ErrorKind,
    path: String,
}

impl std::error::Error for DirRemoveError {}

impl std::fmt::Display for DirRemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "failed to remove directory '{}' : {:?}",
            self.path, self.kind
        ))
    }
}

pub(crate) fn improve_remove_dir_error(error: std::io::Error, path: &Path) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        DirRemoveError {
            kind: error.kind(),
            path: path.display().to_string(),
        },
    )
}

#[test]
fn custom_remove_dir_error() {
    let path = "test/path".as_ref();

    let expected = "failed to remove directory 'test/path' : PermissionDenied";
    let tested = format!(
        "{}",
        improve_remove_dir_error(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            path
        )
    );
    assert_eq!(expected, tested);

    let expected = "Custom { kind: PermissionDenied, error: DirRemoveError { kind: PermissionDenied, path: \"test/path\" } }";
    let tested = format!(
        "{:?}",
        improve_remove_dir_error(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            path
        )
    );
    assert_eq!(expected, tested);
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // `fs::canonicalize` returns an extended-length path on Windows. Such paths not supported by
    // many programs, including rustup. We strip the `\\?\` prefix of the canonicalized path, but
    // this changes the meaning of some path components, and imposes a length of around 260
    // characters.
    if cfg!(windows) {
        // A conservative estimate for the maximum length of a path on Windows.
        //
        // The additional 12 byte restriction is applied when creating directories. It ensures that
        // files can always be created inside that directory without exceeding the path limit.
        const MAX_PATH_LEN: usize = 260 - 12;

        let mut components = p.components();
        let first_component = components.next().unwrap();

        if let Component::Prefix(prefix) = first_component {
            if let Some(mut modified_path) = strip_verbatim_from_prefix(&prefix) {
                modified_path.push(components.as_path());
                p = modified_path;
            }
        }

        if p.as_os_str().len() >= MAX_PATH_LEN {
            warn!(
                "Canonicalized path is too long for Windows: {:?}",
                p.as_os_str(),
            );
        }
    }

    p
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn strip_verbatim() {
        let suite = vec![
            (r"C:\Users\carl", None),
            (r"\Users\carl", None),
            (r"\\?\C:\Users\carl", Some(r"C:\")),
            (r"\\?\Users\carl", Some(r"Users")),
        ];

        for (input, output) in suite {
            let p = Path::new(input);
            let first_component = p.components().next().unwrap();

            if let Component::Prefix(prefix) = &first_component {
                let stripped = strip_verbatim_from_prefix(&prefix);
                assert_eq!(stripped.as_ref().map(|p| p.to_str().unwrap()), output);
            } else {
                assert!(output.is_none());
            }
        }
    }
}
