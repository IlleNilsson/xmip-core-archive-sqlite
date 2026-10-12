# xmip-core-archive-sqlite

SQLite archive target: one file, no server, the archive a single node keeps
beside itself. It is the SQLite `archive::sql::Database` of the capability's
`archive::sql::SqlArchive`, the same store PostgreSQL, SQL Server and MariaDB
archive through, its SQL `sql::SQLITE` from `xmip-core-library-sql`: `xmip_core_archive_sqlite::at(path)` is
`SqlArchive<Sqlite>` on that file, and a receipt reads
`sqlite:///<file>/main/<table>?id=<n>`. A technology of
[xmip-core-archive](https://github.com/IlleNilsson/xmip-core-archive).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
