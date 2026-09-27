#![forbid(unsafe_code)]

//! `SQLite` archive: the [`Dialect`] the archive capability's `SqlArchive`
//! keeps every retained item in, as one row of a single `SQLite` 3 database
//! file, restored by row id — `SqlArchive::<Sqlite>`, made by [`at`].
//!
//! A xmip-core-archive **technology** (repository-model.md). The store, the
//! row, the INSERT and SELECT and the receipt are the capability's
//! (`archive::sql`, ADR-0044), as they are for `PostgreSQL`, SQL Server and
//! `MySQL`; until 2026-09-24 this crate carried its own copy of each. Only
//! the dialect is this crate's: `"…"` identifiers and `'…'` literals as
//! ISO writes them (`codec::sql`), the bytes as an `X'…'` literal in and
//! `hex(bytes)` out, the new row's id through `RETURNING`, and the engine,
//! opened on the file the store's address names. No server: this is the
//! archive a single node keeps beside itself, so the file, its directory
//! and the table are created on first use, and an operator opens it with
//! any `SQLite` client. The table:
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS archive (
//!     id          INTEGER PRIMARY KEY,
//!     data_type   TEXT NOT NULL,
//!     identifier  TEXT NOT NULL,
//!     bytes       BLOB NOT NULL,
//!     metadata    TEXT NOT NULL,
//!     archived_at TEXT NOT NULL
//! );
//! ```
//!
//! The receipt is `sqlite://<file>/main/<table>?id=<n>`, the file as the
//! path of a URI (`net::uri`) and `main` the name `SQLite` gives the
//! database a file holds: `sqlite:///C:/xmip/archive.sqlite/main/archive?id=1`.

use std::path::Path;

use archive::ArchiveError;
use archive::sql::{Dialect, Row, Server, SqlArchive, table_name};
use codec::hex;
use codec::sql::Delimiter;
use rusqlite::Connection;
use rusqlite::types::ValueRef;

/// The name `SQLite` gives the database a file holds.
pub const DATABASE: &str = "main";

/// An archive in the database file at `path`; the file, its directory and
/// the table are created on first use.
#[must_use]
pub fn at(path: impl AsRef<Path>) -> SqlArchive<Sqlite> {
    SqlArchive::new(net::uri::path_of(path.as_ref()), DATABASE, "")
}

/// What `SQLite` does its own way.
pub struct Sqlite;

impl Dialect for Sqlite {
    const SCHEME: &'static str = "sqlite";
    const BYTES_EXPRESSION: &'static str = "hex(bytes)";
    const ID_AFTER_VALUES: &'static str = " RETURNING id";
    type Connection = Connection;

    fn quote_identifier(name: &str) -> String {
        Delimiter::IDENTIFIER.quote(name)
    }

    fn quote_literal(text: &str) -> String {
        Delimiter::STRING.quote(text)
    }

    fn bytes_literal(bytes: &[u8]) -> String {
        format!("X'{}'", hex::encode(bytes))
    }

    /// `hex(bytes)` answered as its digits.
    fn column_bytes(text: &str) -> Option<Vec<u8>> {
        hex::decode(text).ok()
    }

    /// The file the address names, its directory created, waiting out a
    /// writer that holds it for as long as the store's timeout.
    fn connect(server: &Server) -> Result<Connection, ArchiveError> {
        let path = net::uri::file_of(&server.address);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ArchiveError::caused_by)?;
        }
        let connection = Connection::open(&path).map_err(ArchiveError::caused_by)?;
        if let Some(timeout) = server.timeout {
            connection
                .busy_timeout(timeout)
                .map_err(ArchiveError::caused_by)?;
        }
        Ok(connection)
    }

    fn prepare(connection: &mut Connection, table: &str) -> Result<(), ArchiveError> {
        let create = format!(
            "CREATE TABLE IF NOT EXISTS {} (\
             id INTEGER PRIMARY KEY, \
             data_type TEXT NOT NULL, \
             identifier TEXT NOT NULL, \
             bytes BLOB NOT NULL, \
             metadata TEXT NOT NULL, \
             archived_at TEXT NOT NULL)",
            table_name::<Self>(table)
        );
        connection
            .execute(&create, [])
            .map(|_| ())
            .map_err(ArchiveError::caused_by)
    }

    /// Every row, every cell as text: an integer or a real as its digits,
    /// a BLOB lossily.
    fn select(connection: &mut Connection, sql: &str) -> Result<Vec<Row>, ArchiveError> {
        let mut statement = connection.prepare(sql).map_err(ArchiveError::caused_by)?;
        let width = statement.column_count();
        let mut rows = statement.query([]).map_err(ArchiveError::caused_by)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(ArchiveError::caused_by)? {
            let mut cells = Vec::with_capacity(width);
            for index in 0..width {
                let cell = row.get_ref(index).map_err(ArchiveError::caused_by)?;
                cells.push(match cell {
                    ValueRef::Null => None,
                    ValueRef::Integer(value) => Some(value.to_string()),
                    ValueRef::Real(value) => Some(value.to_string()),
                    ValueRef::Text(text) | ValueRef::Blob(text) => {
                        Some(String::from_utf8_lossy(text).into_owned())
                    }
                });
            }
            out.push(cells);
        }
        Ok(out)
    }

    fn close(connection: Connection) -> Result<(), ArchiveError> {
        connection
            .close()
            .map_err(|(_, error)| ArchiveError::caused_by(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archive::fixture::item;
    use archive::{ArchiveReceipt, ArchiveStore};
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xmip-sqlite-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn an_archived_item_restores_from_its_row() {
        let root = scratch("roundtrip");
        let store = at(root.join("archive.sqlite"));
        let original = item("json#1");
        let receipt = store.archive(original.clone()).expect("archive");
        assert!(
            receipt.location.starts_with("sqlite:///"),
            "{}",
            receipt.location
        );
        assert!(
            receipt
                .location
                .ends_with("archive.sqlite/main/archive?id=1"),
            "{}",
            receipt.location
        );
        assert_eq!(receipt.checksum, None);
        let restored = store.restore(&receipt).expect("restore");
        assert_eq!(restored, original, "a real SQLite read gives the item back");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_items_get_two_row_ids_and_every_byte_comes_back() {
        let root = scratch("two");
        let store = at(root.join("archive.sqlite")).with_table("audit");
        let first = store.archive(item("it's #1")).expect("first");
        let mut every = item("json#2");
        every.bytes = (0..=255).collect();
        let second = store.archive(every.clone()).expect("second");
        assert!(
            first.location.ends_with("/main/audit?id=1"),
            "{}",
            first.location
        );
        assert!(
            second.location.ends_with("/main/audit?id=2"),
            "{}",
            second.location
        );
        assert_eq!(store.restore(&second).expect("restore"), every);
        assert_eq!(
            store.restore(&first).expect("restore").identifier,
            "it's #1"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_receipt_for_a_missing_row_or_another_file_is_refused() {
        let root = scratch("missing");
        let store = at(root.join("archive.sqlite"));
        let kept = store.archive(item("json#1")).expect("archive");
        let receipt = ArchiveReceipt {
            location: kept.location.replace("?id=1", "?id=99"),
            checksum: None,
        };
        let refused = store.restore(&receipt).expect_err("no such row");
        assert!(refused.message.contains(&receipt.location), "{refused}");
        let elsewhere = ArchiveReceipt {
            location: "sqlite:///elsewhere/archive.sqlite/main/archive?id=1".to_string(),
            checksum: None,
        };
        let refused = store.restore(&elsewhere).expect_err("another file");
        assert!(refused.message.contains("not a receipt"), "{refused}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_reopened_archive_restores_what_an_earlier_one_kept() {
        let root = scratch("reopen");
        let path = root.join("archive.sqlite");
        let original = item("json#7");
        let receipt = at(&path).archive(original.clone()).expect("archive");
        let restored = at(&path)
            .restore(&receipt)
            .expect("restore from the file on disk");
        assert_eq!(restored, original);
        std::fs::remove_dir_all(&root).ok();
    }
}
