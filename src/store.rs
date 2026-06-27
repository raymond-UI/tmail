//! Local inbox persistence (DESIGN.md §9).
//!
//! Records live in a single JSON file holding mail.tm tokens and passwords, so
//! it is written **atomically** (temp file → `chmod 0600` → rename) and never
//! left world-readable.

use std::path::{Path, PathBuf};

use crate::error::{AppError, ErrorCode, Result};
use crate::model::InboxRecord;

/// Handle to the on-disk inbox store.
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// Open the store at its default location
    /// (`<data-dir>/tmail/inboxes.json`, per-OS via `directories`).
    pub fn open_default() -> Result<Store> {
        let dirs = directories::ProjectDirs::from("", "", "tmail")
            .ok_or_else(|| AppError::new(ErrorCode::Config, "cannot determine a data directory"))?;
        Ok(Store {
            path: dirs.data_dir().join("inboxes.json"),
        })
    }

    /// Open the store at an explicit path (used by tests).
    pub fn with_path(path: impl Into<PathBuf>) -> Store {
        Store { path: path.into() }
    }

    /// Load all records (newest-first as stored). Missing file → empty.
    pub fn load(&self) -> Result<Vec<InboxRecord>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                AppError::new(ErrorCode::Generic, format!("corrupt inbox store: {e}"))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(AppError::new(
                ErrorCode::Generic,
                format!("cannot read inbox store {}: {e}", self.path.display()),
            )),
        }
    }

    /// Persist all records atomically with `0600` permissions.
    pub fn save(&self, records: &[InboxRecord]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::new(ErrorCode::Generic, format!("cannot create data dir: {e}"))
            })?;
        }
        let json = serde_json::to_vec_pretty(records)?;
        let tmp = self.temp_path();
        std::fs::write(&tmp, &json)?;
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            // Best-effort cleanup so a failed rename doesn't leak the temp file.
            let _ = std::fs::remove_file(&tmp);
            AppError::new(
                ErrorCode::Generic,
                format!("cannot persist inbox store: {e}"),
            )
        })?;
        Ok(())
    }

    /// Insert a record as the most-recent entry.
    pub fn add(&self, record: InboxRecord) -> Result<()> {
        let mut records = self.load()?;
        records.insert(0, record);
        self.save(&records)
    }

    /// Find a record by our short id or by address (case-insensitive).
    pub fn find(&self, target: &str) -> Result<Option<InboxRecord>> {
        Ok(self.load()?.into_iter().find(|r| matches(r, target)))
    }

    /// Remove a record by id or address; returns it if present.
    pub fn remove(&self, target: &str) -> Result<Option<InboxRecord>> {
        let mut records = self.load()?;
        let Some(idx) = records.iter().position(|r| matches(r, target)) else {
            return Ok(None);
        };
        let removed = records.remove(idx);
        self.save(&records)?;
        Ok(Some(removed))
    }

    fn temp_path(&self) -> PathBuf {
        let pid = std::process::id();
        let mut name = self
            .path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "inboxes.json".into());
        name.push(format!(".{pid}.tmp"));
        self.path.with_file_name(name)
    }
}

fn matches(r: &InboxRecord, target: &str) -> bool {
    r.id == target || r.address.eq_ignore_ascii_case(target)
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| AppError::new(ErrorCode::Generic, format!("cannot chmod inbox store: {e}")))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Handle;

    fn rec(id: &str, addr: &str) -> InboxRecord {
        InboxRecord {
            id: id.into(),
            address: addr.into(),
            provider: "mail.tm".into(),
            handle: Handle {
                account_id: "acc".into(),
                address: addr.into(),
                password: "pw".into(),
                token: "tok".into(),
            },
            created_at: "2026-06-27T18:40:00Z".into(),
        }
    }

    fn temp_store() -> (Store, tempdir::TempGuard) {
        let guard = tempdir::TempGuard::new();
        (Store::with_path(guard.path().join("inboxes.json")), guard)
    }

    #[test]
    fn missing_store_loads_empty() {
        let (store, _g) = temp_store();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn add_is_newest_first() {
        let (store, _g) = temp_store();
        store.add(rec("a1", "a@x.com")).unwrap();
        store.add(rec("b2", "b@x.com")).unwrap();
        let recs = store.load().unwrap();
        assert_eq!(recs[0].id, "b2");
        assert_eq!(recs[1].id, "a1");
    }

    #[test]
    fn find_by_id_or_address_case_insensitive() {
        let (store, _g) = temp_store();
        store.add(rec("a1", "Agent@X.com")).unwrap();
        assert!(store.find("a1").unwrap().is_some());
        assert!(store.find("agent@x.com").unwrap().is_some());
        assert!(store.find("nope").unwrap().is_none());
    }

    #[test]
    fn remove_returns_record_and_persists() {
        let (store, _g) = temp_store();
        store.add(rec("a1", "a@x.com")).unwrap();
        assert!(store.remove("a1").unwrap().is_some());
        assert!(store.load().unwrap().is_empty());
        // idempotent: removing again yields None, no error
        assert!(store.remove("a1").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (store, _g) = temp_store();
        store.add(rec("a1", "a@x.com")).unwrap();
        let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[cfg(test)]
mod tempdir {
    //! Minimal scoped temp directory so store tests don't touch real data
    //! dirs and clean up after themselves (avoids a dev-dependency).
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TempGuard {
        path: PathBuf,
    }

    impl TempGuard {
        pub fn new() -> TempGuard {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let unique = format!("tmail-test-{}-{n}", std::process::id());
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempGuard { path }
        }
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
