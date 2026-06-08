use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::distr::Alphanumeric;
use rand::Rng;
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

// ── API key record ───────────────────────────────────────────────────────────

/// An API key record as stored in (and retrieved from) the database.
/// The `key_hash` is an argon2 hash — never the raw key.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

// ── Key generation utilities ─────────────────────────────────────────────────

/// Generate a new raw API key with format `emr_` + 32 alphanumeric chars.
/// Returns `(raw_key, key_hash, key_prefix)` where `key_prefix` is the first 8 chars.
pub fn generate_api_key() -> Result<(String, String, String), DbError> {
    let suffix: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let raw_key = format!("emr_{}", suffix);
    let key_prefix = raw_key[..8].to_string();
    let key_hash = hash_api_key(&raw_key)?;
    Ok((raw_key, key_hash, key_prefix))
}

/// Hash a raw API key with argon2 using a random salt.
pub fn hash_api_key(raw_key: &str) -> Result<String, DbError> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(raw_key.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| DbError::Query(format!("failed to hash api key: {}", e)))
}

/// Verify a raw API key against a stored argon2 hash.
pub fn verify_api_key(raw_key: &str, key_hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(key_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed_hash)
        .is_ok()
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
                );
                CREATE TABLE IF NOT EXISTS api_keys (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    key_hash TEXT NOT NULL,
                    key_prefix TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    revoked_at TEXT
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

    // ── API key CRUD ─────────────────────────────────────────────────────────

    /// Insert a new API key record.
    pub fn insert_api_key(
        &self,
        id: &str,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<ApiKeyRecord, DbError> {
        self.conn
            .execute(
                "INSERT INTO api_keys (id, name, key_hash, key_prefix)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, name, key_hash, key_prefix],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        self.get_api_key_by_id(id)?
            .ok_or_else(|| DbError::Query("inserted key not found".to_string()))
    }

    /// Retrieve a single API key record by id.
    pub fn get_api_key_by_id(&self, id: &str) -> Result<Option<ApiKeyRecord>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, name, key_hash, key_prefix, created_at, revoked_at
             FROM api_keys WHERE id = ?1",
            params![id],
            row_to_api_key_record,
        );
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Query(e.to_string())),
        }
    }

    /// List all API key records ordered by `created_at`.
    pub fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, key_hash, key_prefix, created_at, revoked_at
                 FROM api_keys ORDER BY created_at ASC",
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        let records = stmt
            .query_map([], row_to_api_key_record)
            .map_err(|e| DbError::Query(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(records)
    }

    /// Revoke an API key by setting `revoked_at` to the current timestamp.
    /// Returns `DbError::NotFound` if the id does not exist.
    pub fn revoke_api_key(&self, id: &str) -> Result<(), DbError> {
        let rows = self
            .conn
            .execute(
                "UPDATE api_keys SET revoked_at = datetime('now') WHERE id = ?1 AND revoked_at IS NULL",
                params![id],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        if rows == 0 {
            Err(DbError::NotFound { name: id.to_string() })
        } else {
            Ok(())
        }
    }

    /// Return `(id, key_hash)` pairs for all non-revoked API keys.
    pub fn get_active_key_hashes(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, key_hash FROM api_keys WHERE revoked_at IS NULL ORDER BY created_at ASC",
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        let pairs = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| DbError::Query(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(pairs)
    }

    /// Revoke the old key and create a new one atomically within a transaction.
    /// Returns the newly created `ApiKeyRecord`.
    /// If the INSERT fails (e.g. duplicate new_id), the revoke is rolled back.
    pub fn rotate_api_key(
        &self,
        old_id: &str,
        new_id: &str,
        new_name: &str,
        new_key_hash: &str,
        new_key_prefix: &str,
    ) -> Result<ApiKeyRecord, DbError> {
        // `unchecked_transaction` works with `&self` (no `&mut self` required).
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| DbError::Query(e.to_string()))?;

        let rows = tx
            .execute(
                "UPDATE api_keys SET revoked_at = datetime('now') WHERE id = ?1 AND revoked_at IS NULL",
                params![old_id],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

        if rows == 0 {
            // Rollback happens automatically when `tx` is dropped without commit.
            return Err(DbError::NotFound { name: old_id.to_string() });
        }

        tx.execute(
            "INSERT INTO api_keys (id, name, key_hash, key_prefix)
             VALUES (?1, ?2, ?3, ?4)",
            params![new_id, new_name, new_key_hash, new_key_prefix],
        )
        .map_err(|e| DbError::Query(e.to_string()))?;

        tx.commit().map_err(|e| DbError::Query(e.to_string()))?;

        self.get_api_key_by_id(new_id)?
            .ok_or_else(|| DbError::Query("rotated key not found".to_string()))
    }

    /// Look up `(id, key_hash)` for the single active key whose `key_prefix` matches.
    /// Returns `None` when no active key has that prefix.
    pub fn get_active_key_hash_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, key_hash FROM api_keys WHERE key_prefix = ?1 AND revoked_at IS NULL",
            params![prefix],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        match result {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Query(e.to_string())),
        }
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

// ── Row mappers ──────────────────────────────────────────────────────────────

fn row_to_api_key_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiKeyRecord> {
    Ok(ApiKeyRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        key_hash: row.get(2)?,
        key_prefix: row.get(3)?,
        created_at: row.get(4)?,
        revoked_at: row.get(5)?,
    })
}

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

    // ── API key tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_create_api_key() {
        let db = Database::open_in_memory().unwrap();
        let record = db
            .insert_api_key("key-id-1", "my-service", "hash-value", "emr_abcd")
            .unwrap();

        assert_eq!(record.id, "key-id-1");
        assert_eq!(record.name, "my-service");
        assert_eq!(record.key_hash, "hash-value");
        assert_eq!(record.key_prefix, "emr_abcd");
        assert!(!record.created_at.is_empty(), "created_at should be set");
        assert!(record.revoked_at.is_none(), "new key should not be revoked");
    }

    #[test]
    fn test_list_api_keys() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("id-1", "service-a", "hash-a", "emr_aaaa").unwrap();
        db.insert_api_key("id-2", "service-b", "hash-b", "emr_bbbb").unwrap();

        let records = db.list_api_keys().unwrap();
        assert_eq!(records.len(), 2);
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"id-1"));
        assert!(ids.contains(&"id-2"));
    }

    #[test]
    fn test_revoke_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("key-id-1", "svc", "hash", "emr_pfx1").unwrap();

        db.revoke_api_key("key-id-1").unwrap();

        let record = db.get_api_key_by_id("key-id-1").unwrap().unwrap();
        assert!(record.revoked_at.is_some(), "revoked_at should be set after revocation");
    }

    #[test]
    fn test_revoked_key_not_in_active() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("key-id-1", "svc", "hash-active", "emr_act1").unwrap();
        db.insert_api_key("key-id-2", "svc2", "hash-revoked", "emr_rev1").unwrap();

        db.revoke_api_key("key-id-2").unwrap();

        let active = db.get_active_key_hashes().unwrap();
        assert_eq!(active.len(), 1, "only one active key expected");
        assert_eq!(active[0].0, "key-id-1");
        assert_eq!(active[0].1, "hash-active");
    }

    #[test]
    fn test_rotate_api_key() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("old-id", "svc", "old-hash", "emr_old1").unwrap();

        let new_record = db
            .rotate_api_key("old-id", "new-id", "svc-rotated", "new-hash", "emr_new1")
            .unwrap();

        assert_eq!(new_record.id, "new-id");
        assert_eq!(new_record.name, "svc-rotated");
        assert_eq!(new_record.key_hash, "new-hash");
        assert!(new_record.revoked_at.is_none(), "new key should not be revoked");

        // Old key must be revoked
        let old_record = db.get_api_key_by_id("old-id").unwrap().unwrap();
        assert!(old_record.revoked_at.is_some(), "old key should be revoked after rotate");

        // Only the new key is active
        let active = db.get_active_key_hashes().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, "new-id");
    }

    #[test]
    fn test_key_generation_format() {
        let (raw_key, _hash, prefix) = generate_api_key().unwrap();
        assert!(raw_key.starts_with("emr_"), "key must start with emr_: {}", raw_key);
        assert_eq!(raw_key.len(), 36, "key must be 36 chars (4 prefix + 32 suffix): {}", raw_key);
        assert_eq!(prefix, &raw_key[..8], "prefix must be first 8 chars");
    }

    #[test]
    fn test_argon2_hash_verify_roundtrip() {
        let raw_key = "emr_testkey12345678901234567890ab";
        let hashed = hash_api_key(raw_key).unwrap();
        assert!(!hashed.is_empty(), "hash should not be empty");
        assert!(verify_api_key(raw_key, &hashed), "valid key should verify successfully");
        assert!(!verify_api_key("emr_wrongkey", &hashed), "wrong key should not verify");
    }

    #[test]
    fn test_get_active_key_hash_by_prefix() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("id-1", "svc", "hash-1", "emr_pref").unwrap();
        db.insert_api_key("id-2", "svc2", "hash-2", "emr_revk").unwrap();
        db.revoke_api_key("id-2").unwrap();

        // Active key found by prefix
        let result = db.get_active_key_hash_by_prefix("emr_pref").unwrap();
        assert!(result.is_some(), "should find active key by prefix");
        let (id, hash) = result.unwrap();
        assert_eq!(id, "id-1");
        assert_eq!(hash, "hash-1");

        // Revoked key not found
        let revoked = db.get_active_key_hash_by_prefix("emr_revk").unwrap();
        assert!(revoked.is_none(), "revoked key must not be returned by prefix lookup");

        // Unknown prefix returns None
        let unknown = db.get_active_key_hash_by_prefix("emr_zzzz").unwrap();
        assert!(unknown.is_none(), "unknown prefix must return None");
    }

    /// If the INSERT of the new key fails (e.g. duplicate id), the old key must
    /// remain active — the revoke UPDATE must be rolled back atomically.
    #[test]
    fn test_rotate_api_key_is_atomic() {
        let db = Database::open_in_memory().unwrap();
        db.insert_api_key("old-id", "svc", "old-hash", "emr_old1").unwrap();
        // Pre-insert a key with the same id as the intended new key to force a
        // constraint violation on INSERT inside rotate_api_key.
        db.insert_api_key("conflict-id", "blocker", "block-hash", "emr_blk1").unwrap();

        // Attempt rotate: the INSERT should fail because "conflict-id" already exists.
        let result = db.rotate_api_key("old-id", "conflict-id", "svc-rotated", "new-hash", "emr_new1");
        assert!(result.is_err(), "rotate with duplicate new_id must fail");

        // The old key must NOT have been revoked — the revoke must be rolled back.
        let old_record = db.get_api_key_by_id("old-id").unwrap().unwrap();
        assert!(
            old_record.revoked_at.is_none(),
            "old key must remain active when rotation fails atomically, but revoked_at = {:?}",
            old_record.revoked_at
        );
    }

    /// rotate_api_key returns NotFound when the old id does not exist.
    #[test]
    fn test_rotate_api_key_old_not_found() {
        let db = Database::open_in_memory().unwrap();
        let result = db.rotate_api_key("ghost-id", "new-id", "svc", "new-hash", "emr_new1");
        assert!(
            matches!(result, Err(DbError::NotFound { .. })),
            "rotate with nonexistent old_id must return NotFound, got: {:?}",
            result
        );
    }
}
