use crate::model::Vault;
use directories::ProjectDirs;
use std::fs;
use std::io;
use std::path::PathBuf;

const QUALIFIER: &str = "dev";
const ORG: &str = "keyvault";
const APP: &str = "keyvault";
const FILE_NAME: &str = "data.json";

pub fn data_path() -> PathBuf {
    if let Some(dirs) = ProjectDirs::from(QUALIFIER, ORG, APP) {
        let dir = dirs.data_dir().to_path_buf();
        let _ = fs::create_dir_all(&dir);
        return dir.join(FILE_NAME);
    }
    PathBuf::from(FILE_NAME)
}

pub fn load() -> Vault {
    let path = data_path();
    match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => match serde_json::from_str::<Vault>(&s) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[keyvault] failed to parse {}: {e}. Starting fresh.", path.display());
                Vault::default()
            }
        },
        _ => Vault::default(),
    }
}

pub fn save(vault: &Vault) -> io::Result<()> {
    let path = data_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(vault)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}
