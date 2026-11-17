//! SQLite projection store: WAL mode and `sqlite-vec` (docs/implementation/01, section 3.2).
//!
//! The journal owns durability. This store is a rebuildable projection, so `synchronous = NORMAL` is intentional.

use std::path::Path;
use std::sync::Once;

use eon_primitives::{EonError, EonResult};
use rusqlite::{Connection, params};

static REGISTER_VEC: Once = Once::new();

type AutoExtensionInit = unsafe extern "C" fn(
    *mut rusqlite::ffi::sqlite3,
    *mut *mut std::ffi::c_char,
    *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int;

fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| unsafe {
        let init: AutoExtensionInit = std::mem::transmute::<*const (), AutoExtensionInit>(
            sqlite_vec::sqlite3_vec_init as *const (),
        );
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}

pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    pub fn open(path: impl AsRef<Path>) -> EonResult<Self> {
        register_sqlite_vec();
        let conn = Connection::open(path).map_err(sql_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 134217728;
             CREATE TABLE IF NOT EXISTS projection_checkpoint (
                 name TEXT PRIMARY KEY,
                 journal_seq INTEGER NOT NULL
             );",
        )
        .map_err(sql_err)?;
        Ok(Self { conn })
    }

    pub fn journal_mode(&self) -> EonResult<String> {
        self.conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(sql_err)
    }

    pub fn vec_version(&self) -> EonResult<String> {
        self.conn
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .map_err(sql_err)
    }

    /// Records how far a named projection has applied the journal. Only moves forward.
    pub fn advance_checkpoint(&self, name: &str, journal_seq: u64) -> EonResult<()> {
        self.conn
            .execute(
                "INSERT INTO projection_checkpoint (name, journal_seq) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET journal_seq = excluded.journal_seq
                 WHERE excluded.journal_seq > projection_checkpoint.journal_seq",
                params![name, journal_seq as i64],
            )
            .map_err(sql_err)?;
        Ok(())
    }

    pub fn checkpoint(&self, name: &str) -> EonResult<Option<u64>> {
        match self.conn.query_row(
            "SELECT journal_seq FROM projection_checkpoint WHERE name = ?1",
            params![name],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(seq) => Ok(Some(seq as u64)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sql_err(e)),
        }
    }
}

fn sql_err(error: rusqlite::Error) -> EonError {
    EonError::OperationFailed {
        operation: "state_store".to_string(),
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_wal_mode_with_vec_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path().join("state.db")).unwrap();
        assert_eq!(store.journal_mode().unwrap(), "wal");
        assert!(store.vec_version().unwrap().starts_with('v'));
    }

    #[test]
    fn checkpoint_only_moves_forward() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path().join("state.db")).unwrap();
        store.advance_checkpoint("ui", 10).unwrap();
        store.advance_checkpoint("ui", 4).unwrap();
        assert_eq!(store.checkpoint("ui").unwrap(), Some(10));
        assert_eq!(store.checkpoint("absent").unwrap(), None);
    }
}
