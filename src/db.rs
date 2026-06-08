use rusqlite::{Connection, params};

use crate::error::DbError;

// ── Provider record ──────────────────────────────────────────────────────────

/// A provider record as stored in (and retrieved from) the database.
/// The `api_key_env_var` field holds the *name* of the env var, never the key value.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRecord {
    pub name: String,
    pub provider_type: String,
    pub api_key_env_var: String,
    pub endpoint: String,
    pub model: String,
    pub enabled: bool,
    pub created_at: String,
}

// ── New-provider request ─────────────────────────────────────────────────────

/// Data needed to create a new provider in the database.
#[derive(Debug, Clone)]
pub struct NewProvider {
    pub name: String,
    pub provider_type: String,
    pub api_key_env_var: String,
    pub endpoint: String,
    pub model: String,
}

// ── Database wrapper ─────────────────────────────────────────────────────────

/// Thin wrapper around a rusqlite `Connection` with schema migration.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path, run migrations, and
    /// enable WAL mode.
    pub fn open(path: &str) -> Result<Self, DbError> {
        let conn =
            Connection::open(path).map_err(|e| DbError::Connection(e.to_string()))?;
        let db = Self { conn };
        db.enable_wal()?;
        db.migrate()?;
        Ok(db)
    }

    /// Open an in-memory SQLite database — useful for tests.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn =
            Connection::open_in_memory().map_err(|e| DbError::Connection(e.to_string()))?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn enable_wal(&self) -> Result<(), DbError> {
        self.conn
            .execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| DbError::Migration(e.to_string()))
    }

    fn migrate(&self) -> Result<(), DbError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS providers (
                    name TEXT PRIMARY KEY,
                    provider_type TEXT NOT NULL,
                    api_key_env_var TEXT NOT NULL,
                    endpoint TEXT NOT NULL,
                    model TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );",
            )
            .map_err(|e| DbError::Migration(e.to_string()))
    }

    // ── CRUD ─────────────────────────────────────────────────────────────────

    /// Insert a new provider. Returns `DbError::AlreadyExists` if the name is taken.
    pub fn insert_provider(&self, p: &NewProvider) -> Result<(), DbError> {
        let result = self.conn.execute(
            "INSERT INTO providers (name, provider_type, api_key_env_var, endpoint, model)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![p.name, p.provider_type, p.api_key_env_var, p.endpoint, p.model],
        );

        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(DbError::AlreadyExists { name: p.name.clone() })
            }
            Err(e) => Err(DbError::Query(e.to_string())),
        }
    }

    /// Retrieve a single provider by name.
    pub fn get_provider(&self, name: &str) -> Result<ProviderRecord, DbError> {
        let result = self.conn.query_row(
            "SELECT name, provider_type, api_key_env_var, endpoint, model, enabled, created_at
             FROM providers WHERE name = ?1",
            params![name],
            row_to_record,
        );

        match result {
            Ok(record) => Ok(record),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(DbError::NotFound { name: name.to_string() })
            }
            Err(e) => Err(DbError::Query(e.to_string())),
        }
    }

    /// List all providers ordered by `created_at`.
    pub fn list_providers(&self) -> Result<Vec<ProviderRecord>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, provider_type, api_key_env_var, endpoint, model, enabled, created_at
                 FROM providers ORDER BY created_at ASC",
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        let records = stmt
            .query_map([], row_to_record)
            .map_err(|e| DbError::Query(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(records)
    }

    /// Delete a provider by name. Returns `DbError::NotFound` if it does not exist.
    pub fn delete_provider(&self, name: &str) -> Result<(), DbError> {
        let rows = self
            .conn
            .execute("DELETE FROM providers WHERE name = ?1", params![name])
            .map_err(|e| DbError::Query(e.to_string()))?;

        if rows == 0 {
            Err(DbError::NotFound { name: name.to_string() })
        } else {
            Ok(())
        }
    }
}

// ── Row mapper ───────────────────────────────────────────────────────────────

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderRecord> {
    Ok(ProviderRecord {
        name: row.get(0)?,
        provider_type: row.get(1)?,
        api_key_env_var: row.get(2)?,
        endpoint: row.get(3)?,
        model: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn new_provider(name: &str) -> NewProvider {
        NewProvider {
            name: name.to_string(),
            provider_type: "voyage".to_string(),
            api_key_env_var: "VOYAGE_API_KEY".to_string(),
            endpoint: "https://api.voyageai.com/v1/embeddings".to_string(),
            model: "voyage-code-3".to_string(),
        }
    }

    #[test]
    fn test_db_new_in_memory() {
        let db = Database::open_in_memory();
        assert!(db.is_ok(), "should open in-memory database: {:?}", db.err());
    }

    #[test]
    fn test_db_insert_provider() {
        let db = Database::open_in_memory().unwrap();
        let p = new_provider("voyage-ai");
        let result = db.insert_provider(&p);
        assert!(result.is_ok(), "insert should succeed: {:?}", result.err());
    }

    #[test]
    fn test_db_get_provider() {
        let db = Database::open_in_memory().unwrap();
        let p = new_provider("voyage-ai");
        db.insert_provider(&p).unwrap();

        let record = db.get_provider("voyage-ai").unwrap();
        assert_eq!(record.name, "voyage-ai");
        assert_eq!(record.provider_type, "voyage");
        assert_eq!(record.api_key_env_var, "VOYAGE_API_KEY");
        assert_eq!(record.endpoint, "https://api.voyageai.com/v1/embeddings");
        assert_eq!(record.model, "voyage-code-3");
        assert!(record.enabled, "should be enabled by default");
        assert!(!record.created_at.is_empty(), "created_at should be set");
    }

    #[test]
    fn test_db_list_providers_empty() {
        let db = Database::open_in_memory().unwrap();
        let records = db.list_providers().unwrap();
        assert!(records.is_empty(), "should have no providers initially");
    }

    #[test]
    fn test_db_list_providers() {
        let db = Database::open_in_memory().unwrap();
        db.insert_provider(&new_provider("provider-a")).unwrap();
        db.insert_provider(&new_provider("provider-b")).unwrap();

        let records = db.list_providers().unwrap();
        assert_eq!(records.len(), 2);
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"provider-a"));
        assert!(names.contains(&"provider-b"));
    }

    #[test]
    fn test_db_delete_provider() {
        let db = Database::open_in_memory().unwrap();
        db.insert_provider(&new_provider("voyage-ai")).unwrap();

        let result = db.delete_provider("voyage-ai");
        assert!(result.is_ok(), "delete should succeed: {:?}", result.err());

        let records = db.list_providers().unwrap();
        assert!(records.is_empty(), "list should be empty after delete");
    }

    #[test]
    fn test_db_duplicate_insert_fails() {
        let db = Database::open_in_memory().unwrap();
        db.insert_provider(&new_provider("voyage-ai")).unwrap();

        let result = db.insert_provider(&new_provider("voyage-ai"));
        assert!(
            matches!(result, Err(DbError::AlreadyExists { .. })),
            "duplicate insert should return AlreadyExists, got: {:?}",
            result
        );
    }

    #[test]
    fn test_db_delete_nonexistent_returns_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = db.delete_provider("nonexistent");
        assert!(
            matches!(result, Err(DbError::NotFound { .. })),
            "deleting nonexistent provider should return NotFound, got: {:?}",
            result
        );
    }

    #[test]
    fn test_db_get_nonexistent_returns_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = db.get_provider("nonexistent");
        assert!(
            matches!(result, Err(DbError::NotFound { .. })),
            "getting nonexistent provider should return NotFound, got: {:?}",
            result
        );
    }
}
