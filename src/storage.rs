//! JSON persistence. The file location can be overridden with `STUDYPLAN_DATA`.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;

use crate::model::State;

pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("STUDYPLAN_DATA") {
        return Ok(PathBuf::from(p));
    }
    let dirs = ProjectDirs::from("", "", "studyplan")
        .ok_or_else(|| anyhow!("could not determine a data directory"))?;
    Ok(dirs.data_dir().join("state.json"))
}

pub fn load() -> Result<State> {
    let p = path()?;
    match fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text)
            // Never overwrite a file we could not parse.
            .with_context(|| format!("{} is not valid; fix or move it", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

/// Write atomically (temp file + rename) so a crash never leaves half a file.
pub fn save(state: &State) -> Result<()> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &p)?;
    Ok(())
}
