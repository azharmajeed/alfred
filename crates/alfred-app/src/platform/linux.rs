//! Linux-specific helpers.

use std::path::PathBuf;

/// Return the user's default shell from `$SHELL`, falling back to `/bin/bash`.
pub fn default_shell() -> PathBuf {
    if let Ok(shell) = std::env::var("SHELL") {
        let p = PathBuf::from(shell);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("/bin/bash")
}
