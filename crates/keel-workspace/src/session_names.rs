//! Provider-owned session names, with writes restricted to naming metadata in guarded sessions.
use camino::{Utf8Path, Utf8PathBuf};

mod antigravity;
mod antigravity_proto;
mod claude;
mod codex;
mod files;

/// Provider roots are injected so naming tests never touch the person's conversations.
#[derive(Clone)]
pub struct SessionNames {
    pub claude: Utf8PathBuf,
    pub codex: Utf8PathBuf,
    pub antigravity: Utf8PathBuf,
}

/// A native name plus its change fingerprint, without conversation content.
#[derive(Clone, Debug)]
pub struct NativeName {
    pub title: Option<String>,
    pub explicit: bool,
    pub fingerprint: String,
    pub last_active: Option<String>,
    pub messages: usize,
}

impl SessionNames {
    /// Use the same configured roots as transcript following.
    pub fn local() -> Option<Self> {
        Some(Self {
            claude: crate::claude_home()?,
            codex: crate::codex_home()?,
            antigravity: crate::home()?.join(".gemini/antigravity-cli"),
        })
    }

    /// Read a session only when its recorded working directory matches the registered checkout.
    pub fn read(
        &self,
        provider: &str,
        cwd: &Utf8Path,
        id: &str,
    ) -> Result<Option<NativeName>, String> {
        files::valid_id(id)?;
        match provider {
            "claude" => Ok(claude::read(&self.claude, cwd, id)),
            "codex" => codex::read(&self.codex, cwd, id),
            "antigravity" => antigravity::read(&self.antigravity, cwd, id),
            _ => Err("Unknown session provider".into()),
        }
    }

    /// Rename existing native metadata; Codex writes must go through its App Server API.
    pub fn rename(
        &self,
        provider: &str,
        cwd: &Utf8Path,
        id: &str,
        name: &str,
    ) -> Result<(), String> {
        files::valid_id(id)?;
        match provider {
            "claude" => claude::rename(&self.claude, cwd, id, name),
            "antigravity" => antigravity::rename(&self.antigravity, cwd, id, name),
            _ => Err("This provider requires its naming API".into()),
        }
    }
}

#[cfg(test)]
mod tests;
