#![forbid(unsafe_code)]

//! `SQLite` archive: an [`ArchiveStore`] that keeps every retained item as one
//! row of a single `SQLite` 3 database file, and restores it by row id.
//!
//! A xmip-core-archive **technology** (repository-model.md): it depends on the
//! archive capability for the [`ArchiveStore`] trait and its item, receipt and
//! error types, never the reverse. The file holds one table, `archive`, with
//! the four item columns every archive technology shares — `data_type`,
//! `identifier`, `bytes`, `metadata` — plus `id`, the row, and `archived_at`,
//! the moment. No server: this is the archive a single node keeps beside
//! itself, and an operator opens it with any `SQLite` client. The metadata text
//! and `archived_at` come from the capability, `archive::metadata` and
//! `archive::timestamp` (ADR-0044).

use std::fmt::Display;
use std::path::{Path, PathBuf};

use archive::{ArchiveError, ArchiveItem, ArchiveReceipt, ArchiveStore, metadata, timestamp};
use rusqlite::{Connection, OptionalExtension, params};

/// The one table, created the first time the file is opened.
const CREATE: &str = "CREATE TABLE IF NOT EXISTS archive (\
    id INTEGER PRIMARY KEY, \
    data_type TEXT NOT NULL, \
    identifier TEXT NOT NULL, \
    bytes BLOB NOT NULL, \
    metadata TEXT NOT NULL, \
    archived_at TEXT NOT NULL)";
const INSERT: &str = "INSERT INTO archive \
    (data_type, identifier, bytes, metadata, archived_at) VALUES (?1, ?2, ?3, ?4, ?5)";
const SELECT: &str = "SELECT data_type, identifier, bytes, metadata FROM archive WHERE id = ?1";

/// An archive that persists items as rows of one `SQLite` database file.
pub struct SqliteArchive {
    path: PathBuf,
}

impl SqliteArchive {
    /// An archive in the database file at `path`; the file, its directory and
    /// the table are created on first use.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// What every receipt of this file shares, up to the row id:
    /// `sqlite:///<path>#`.
    fn prefix(&self) -> String {
        format!("sqlite://{}#", uri_path(&self.path))
    }

    /// The database, opened with its table in place.
    fn open(&self) -> Result<Connection, ArchiveError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(error)?;
        }
        let connection = Connection::open(&self.path).map_err(error)?;
        connection.execute(CREATE, []).map_err(error)?;
        Ok(connection)
    }

    /// The row a receipt names, refusing a receipt of another file: a row id
    /// means nothing outside the file that issued it.
    fn rowid_of(&self, location: &str) -> Result<i64, ArchiveError> {
        let fragment = location
            .strip_prefix(&self.prefix())
            .ok_or_else(|| ArchiveError {
                message: format!(
                    "{location} is not a receipt of the archive at {}",
                    self.path.display()
                ),
            })?;
        fragment.parse().map_err(|_| ArchiveError {
            message: format!("{location} does not name a row"),
        })
    }
}

impl ArchiveStore for SqliteArchive {
    fn archive(&self, item: ArchiveItem) -> Result<ArchiveReceipt, ArchiveError> {
        let connection = self.open()?;
        connection
            .execute(
                INSERT,
                params![
                    item.data_type,
                    item.identifier,
                    item.bytes,
                    metadata::encode(&item.metadata),
                    timestamp::now()
                ],
            )
            .map_err(error)?;
        let rowid = connection.last_insert_rowid();
        Ok(ArchiveReceipt {
            location: format!("{}{rowid}", self.prefix()),
            checksum: None,
        })
    }

    fn restore(&self, receipt: &ArchiveReceipt) -> Result<ArchiveItem, ArchiveError> {
        let rowid = self.rowid_of(&receipt.location)?;
        let connection = self.open()?;
        let row = connection
            .query_row(SELECT, [rowid], |row| {
                Ok(ArchiveItem {
                    data_type: row.get(0)?,
                    identifier: row.get(1)?,
                    bytes: row.get(2)?,
                    metadata: metadata::decode(&row.get::<_, String>(3)?),
                })
            })
            .optional()
            .map_err(error)?;
        row.ok_or_else(|| ArchiveError {
            message: format!("no row {rowid} in {}", receipt.location),
        })
    }
}

/// `path` as the path part of a URI: forward slashes, and a leading slash so a
/// Windows drive reads `/C:/...` after the `sqlite://` authority.
fn uri_path(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    }
}

fn error(cause: impl Display) -> ArchiveError {
    ArchiveError {
        message: cause.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xmip-sqlite-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    fn item(id: &str) -> ArchiveItem {
        ArchiveItem {
            data_type: "json".to_string(),
            identifier: id.to_string(),
            bytes: b"{\"kept\":true}".to_vec(),
            metadata: vec![("source".to_string(), "playground".to_string())],
        }
    }

    #[test]
    fn an_archived_item_restores_from_its_row() {
        let root = scratch("roundtrip");
        let store = SqliteArchive::new(root.join("archive.sqlite"));
        let original = item("json#1");
        let receipt = store.archive(original.clone()).expect("archive");
        assert!(
            receipt.location.starts_with("sqlite:///"),
            "{}",
            receipt.location
        );
        assert!(
            receipt.location.ends_with("archive.sqlite#1"),
            "{}",
            receipt.location
        );
        assert_eq!(receipt.checksum, None);
        let restored = store.restore(&receipt).expect("restore");
        assert_eq!(restored, original, "a real SQLite read gives the item back");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_items_get_two_row_ids() {
        let root = scratch("two");
        let store = SqliteArchive::new(root.join("archive.sqlite"));
        let first = store.archive(item("json#1")).expect("first");
        let second = store.archive(item("json#2")).expect("second");
        assert_ne!(first.location, second.location);
        assert!(first.location.ends_with("#1"));
        assert!(second.location.ends_with("#2"));
        assert_eq!(
            store.restore(&second).expect("restore").identifier,
            "json#2"
        );
        assert_eq!(store.restore(&first).expect("restore").identifier, "json#1");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_receipt_for_a_missing_row_is_refused_naming_the_location() {
        let root = scratch("missing");
        let store = SqliteArchive::new(root.join("archive.sqlite"));
        store.archive(item("json#1")).expect("archive");
        let receipt = ArchiveReceipt {
            location: format!("{}99", store.prefix()),
            checksum: None,
        };
        let refused = store.restore(&receipt).expect_err("no such row");
        assert!(refused.message.contains(&receipt.location), "{refused}");
        assert!(refused.message.contains("99"), "{refused}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_receipt_of_another_file_is_refused() {
        let root = scratch("other");
        let store = SqliteArchive::new(root.join("archive.sqlite"));
        let receipt = ArchiveReceipt {
            location: "sqlite:///elsewhere/archive.sqlite#1".to_string(),
            checksum: None,
        };
        let refused = store.restore(&receipt).expect_err("another file");
        assert!(refused.message.contains("not a receipt"), "{refused}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_reopened_archive_restores_what_an_earlier_one_kept() {
        let root = scratch("reopen");
        let path = root.join("archive.sqlite");
        let original = item("json#7");
        let receipt = {
            let first = SqliteArchive::new(&path);
            first.archive(original.clone()).expect("archive")
        };
        let second = SqliteArchive::new(&path);
        let restored = second
            .restore(&receipt)
            .expect("restore from the file on disk");
        assert_eq!(restored, original);
        std::fs::remove_dir_all(&root).ok();
    }
}
