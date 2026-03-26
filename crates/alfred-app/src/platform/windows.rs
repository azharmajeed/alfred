//! Windows-specific helpers.

use std::path::PathBuf;

/// Detect the best available shell in priority order:
///   PowerShell Core → PowerShell 5 → %COMSPEC% (cmd.exe)
///
/// Both PowerShell variants work because EventProxy now forwards VT PtyWrite
/// responses (e.g. DSR cursor-position reply) back to the shell, preventing
/// the startup hang that was caused by unanswered \x1b[6n queries.
pub fn default_shell() -> PathBuf {
    // PowerShell Core (pwsh.exe) — installed via winget / Store / manual
    if let Some(pwsh) = find_on_path("pwsh.exe") {
        return pwsh;
    }

    // Legacy PowerShell (powershell.exe) — ships with Windows
    if let Some(ps) = find_on_path("powershell.exe") {
        return ps;
    }

    // Fallback: %COMSPEC% (usually C:\Windows\System32\cmd.exe)
    if let Ok(comspec) = std::env::var("COMSPEC") {
        let p = PathBuf::from(comspec);
        if p.exists() {
            return p;
        }
    }

    PathBuf::from("cmd.exe")
}

#[allow(dead_code)]
fn find_on_path(exe: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(exe);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}
