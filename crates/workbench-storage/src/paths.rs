use std::path::PathBuf;
use workbench_core::{BusinessError, ErrorKind, RetryAdvice};

/// Uses OS application data only; never falls back to the project or working directory.
pub fn default_data_directory() -> Result<PathBuf, BusinessError> {
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("AgentWorkbench"));
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Library/Application Support/AgentWorkbench"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .map(|p| p.join("agent-workbench"));
    root.filter(|p| p.is_absolute()).ok_or_else(|| {
        BusinessError::new(
            ErrorKind::Storage,
            "OS application data directory is unavailable or not absolute",
            RetryAdvice::AfterCorrection,
        )
    })
}

pub(crate) fn project_key(path: &std::path::Path) -> Result<String, BusinessError> {
    let text = path
        .to_str()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| {
            BusinessError::new(
                ErrorKind::InvalidInput,
                "project directory must be an absolute Unicode path",
                RetryAdvice::AfterCorrection,
            )
        })?;
    #[cfg(target_os = "windows")]
    let key = text
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    #[cfg(not(target_os = "windows"))]
    let key = text.to_owned();
    Ok(key)
}
