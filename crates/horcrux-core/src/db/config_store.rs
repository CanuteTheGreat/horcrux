//! Persistent store for configuration objects owned by the in-memory managers
//! (storage pools, firewall rules, HA resources/groups, templates, alert rules
//! and notification channels, replication jobs, snapshot schedules, cluster
//! nodes, backup jobs and the backup catalog).
//!
//! Every manager keeps its fast in-memory map; this store is the durable copy
//! behind it. A manager built with `with_database(...)` loads its objects from
//! here at startup and writes through on every create/update/delete, so the
//! configuration survives a restart of the API process.
//!
//! Objects are stored as JSON documents in one `config_objects` table keyed by
//! `(kind, id)`. JSON (rather than one column per field) keeps the schema
//! stable while other work adds `#[serde(default)]` fields to these structs:
//! older rows still deserialize, and no migration is needed per new field.
//! Rows are returned in insertion order (SQLite `rowid`), which matters for
//! ordered collections such as firewall rule lists; an update keeps its row's
//! original position.

use super::Database;
use horcrux_common::{Error, Result};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{Row, SqlitePool};

/// Object kinds. One constant per persisted collection.
pub mod kinds {
    pub const STORAGE_POOL: &str = "storage_pool";
    pub const FIREWALL_RULE: &str = "firewall_rule";
    pub const SECURITY_GROUP: &str = "firewall_security_group";
    pub const HA_RESOURCE: &str = "ha_resource";
    pub const HA_GROUP: &str = "ha_group";
    pub const HA_SETTINGS: &str = "ha_settings";
    pub const TEMPLATE: &str = "template";
    pub const ALERT_RULE: &str = "alert_rule";
    pub const NOTIFICATION_CHANNEL: &str = "notification_channel";
    pub const REPLICATION_JOB: &str = "replication_job";
    pub const SNAPSHOT_SCHEDULE: &str = "snapshot_schedule";
    pub const CLUSTER_NODE: &str = "cluster_node";
    pub const CLUSTER_CONFIG: &str = "cluster_config";
    pub const BACKUP_JOB: &str = "backup_job";
    pub const BACKUP: &str = "backup";
}

/// Durable key/value store of JSON configuration documents.
#[derive(Clone)]
pub struct ConfigStore {
    pool: SqlitePool,
}

impl ConfigStore {
    pub fn new(db: &Database) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or replace one object. An update keeps the row's position.
    pub async fn put<T: Serialize + ?Sized>(&self, kind: &str, id: &str, value: &T) -> Result<()> {
        let data = serde_json::to_string(value)
            .map_err(|e| Error::System(format!("Failed to serialize {} {}: {}", kind, id, e)))?;
        sqlx::query(
            "INSERT INTO config_objects (kind, id, data) VALUES (?, ?, ?)
             ON CONFLICT(kind, id) DO UPDATE SET data = excluded.data,
                 updated_at = CURRENT_TIMESTAMP",
        )
        .bind(kind)
        .bind(id)
        .bind(data)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::System(format!("Failed to persist {} {}: {}", kind, id, e)))?;
        Ok(())
    }

    /// Delete one object. Returns whether a row existed.
    pub async fn delete(&self, kind: &str, id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM config_objects WHERE kind = ? AND id = ?")
            .bind(kind)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::System(format!("Failed to delete {} {}: {}", kind, id, e)))?;
        Ok(res.rows_affected() > 0)
    }

    /// Load every object of a kind, in insertion order, as `(id, value)`.
    ///
    /// A row that no longer deserializes (e.g. written by a newer build with
    /// an incompatible change) is skipped with a warning rather than failing
    /// startup; it stays in the table untouched.
    pub async fn load<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<(String, T)>> {
        let rows = sqlx::query("SELECT id, data FROM config_objects WHERE kind = ? ORDER BY rowid")
            .bind(kind)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::System(format!("Failed to load {} objects: {}", kind, e)))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.get("id");
            let data: String = row.get("data");
            match serde_json::from_str::<T>(&data) {
                Ok(v) => out.push((id, v)),
                Err(e) => tracing::warn!("Skipping unreadable stored {} {}: {}", kind, id, e),
            }
        }
        Ok(out)
    }

    /// Load just the values of a kind, in insertion order.
    pub async fn load_values<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
        Ok(self
            .load::<T>(kind)
            .await?
            .into_iter()
            .map(|(_, v)| v)
            .collect())
    }
}

/// Write-through helper for managers whose store is optional (managers built
/// with `new()` stay purely in-memory, which unit tests rely on).
pub async fn put_opt<T: Serialize + ?Sized>(
    store: &Option<ConfigStore>,
    kind: &str,
    id: &str,
    value: &T,
) -> Result<()> {
    match store {
        Some(s) => s.put(kind, id, value).await,
        None => Ok(()),
    }
}

/// Delete helper for managers whose store is optional.
pub async fn delete_opt(store: &Option<ConfigStore>, kind: &str, id: &str) -> Result<()> {
    match store {
        Some(s) => s.delete(kind, id).await.map(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Thing {
        name: String,
        n: u32,
    }

    async fn store() -> (tempfile::TempDir, Database, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("t.db").display());
        let db = Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let s = ConfigStore::new(&db);
        (dir, db, s)
    }

    #[tokio::test]
    async fn put_load_update_delete_roundtrip() {
        let (_dir, _db, s) = store().await;
        let a = Thing {
            name: "a".into(),
            n: 1,
        };
        let b = Thing {
            name: "b".into(),
            n: 2,
        };
        s.put("thing", "a", &a).await.unwrap();
        s.put("thing", "b", &b).await.unwrap();
        s.put("other", "a", &b).await.unwrap();

        // Update keeps insertion order.
        let a2 = Thing {
            name: "a".into(),
            n: 10,
        };
        s.put("thing", "a", &a2).await.unwrap();
        let got: Vec<Thing> = s.load_values("thing").await.unwrap();
        assert_eq!(got, vec![a2.clone(), b.clone()]);

        assert!(s.delete("thing", "a").await.unwrap());
        assert!(!s.delete("thing", "a").await.unwrap());
        let got: Vec<(String, Thing)> = s.load("thing").await.unwrap();
        assert_eq!(got, vec![("b".to_string(), b.clone())]);
        // Other kinds are untouched.
        assert_eq!(s.load_values::<Thing>("other").await.unwrap(), vec![b]);
    }

    #[tokio::test]
    async fn unreadable_rows_are_skipped_not_fatal() {
        let (_dir, _db, s) = store().await;
        s.put("thing", "bad", &serde_json::json!({"unexpected": true}))
            .await
            .unwrap();
        s.put(
            "thing",
            "good",
            &Thing {
                name: "g".into(),
                n: 3,
            },
        )
        .await
        .unwrap();
        let got: Vec<Thing> = s.load_values("thing").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "g");
    }

    #[tokio::test]
    async fn data_survives_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("t.db").display());
        {
            let db = Database::new(&url).await.unwrap();
            db.migrate().await.unwrap();
            ConfigStore::new(&db)
                .put(
                    "thing",
                    "x",
                    &Thing {
                        name: "x".into(),
                        n: 7,
                    },
                )
                .await
                .unwrap();
            db.close().await;
        }
        let db = Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let got: Vec<Thing> = ConfigStore::new(&db).load_values("thing").await.unwrap();
        assert_eq!(got[0].n, 7);
    }
}
