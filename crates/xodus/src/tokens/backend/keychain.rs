use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

use base64::prelude::*;

use crate::tokens::store::{TokenBackend, TokenStoreError};

pub struct KeychainBackend;

fn credentials_file_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("xodus").join("tokens.json"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".config").join("xodus").join("tokens.json"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata).join("xodus").join("tokens.json"));
    }
    None
}

fn read_file_tokens() -> HashMap<String, String> {
    let Some(path) = credentials_file_path() else {
        return HashMap::new();
    };
    if let Ok(mut f) = File::open(&path) {
        let mut buf = String::new();
        if f.read_to_string(&mut buf).is_ok() {
            if let Ok(map) = serde_json::from_str(&buf) {
                return map;
            }
        }
    }
    HashMap::new()
}

fn write_file_tokens(map: &HashMap<String, String>) {
    let Some(path) = credentials_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_vec_pretty(map) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
            {
                let _ = f.write_all(&data);
                let _ = f.flush();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = fs::write(&path, &data);
        }
    }
}

impl TokenBackend for KeychainBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
        let mut map = read_file_tokens();
        if let Some(b64) = map.get(key) {
            if let Ok(bytes) = BASE64_STANDARD.decode(b64) {
                if !bytes.is_empty() {
                    return Ok(Some(bytes));
                }
            }
        }

        // Fallback to OS keyring (migrates to file store if present)
        if let Ok(entry) = crate::secrets::get_entry(key) {
            if let Ok(bytes) = entry.get_secret() {
                if !bytes.is_empty() {
                    map.insert(key.to_string(), BASE64_STANDARD.encode(&bytes));
                    write_file_tokens(&map);
                    return Ok(Some(bytes));
                }
            }
        }

        Ok(None)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), TokenStoreError> {
        let mut map = read_file_tokens();
        map.insert(key.to_string(), BASE64_STANDARD.encode(value));
        write_file_tokens(&map);

        if let Ok(entry) = crate::secrets::get_entry(key) {
            let _ = entry.set_secret(value);
        }
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        let mut map = read_file_tokens();
        if map.remove(key).is_some() {
            write_file_tokens(&map);
        }

        if let Ok(entry) = crate::secrets::get_entry(key) {
            let _ = entry.delete_credential();
        }
        Ok(())
    }
}
