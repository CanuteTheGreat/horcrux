//! Backup catalog and job store: in memory, written through to the
//! database's `config_objects` table (kinds `backup` / `backup_job`, see
//! `db::config_store`) when the manager was built with a database, so both
//! survive restarts.
use super::{Backup, BackupJob};
use crate::db::config_store::{kinds, ConfigStore};
use horcrux_common::Result;
use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct Store {
    backups: RwLock<HashMap<String, Backup>>,
    jobs: RwLock<HashMap<String, BackupJob>>,
    db: Option<ConfigStore>,
}

impl Store {
    pub fn memory() -> Self {
        Self {
            backups: RwLock::new(HashMap::new()),
            jobs: RwLock::new(HashMap::new()),
            db: None,
        }
    }

    /// Load the persisted catalog and jobs.
    pub async fn load(db: ConfigStore) -> Result<Self> {
        let mut backups = HashMap::new();
        for b in db.load_values::<Backup>(kinds::BACKUP).await? {
            if !b.path.exists() {
                tracing::warn!("Backup {} archive {} is missing", b.id, b.path.display());
            }
            backups.insert(b.id.clone(), b);
        }
        let mut jobs = HashMap::new();
        for j in db.load_values::<BackupJob>(kinds::BACKUP_JOB).await? {
            jobs.insert(j.id.clone(), j);
        }
        tracing::info!(
            "Loaded {} backup(s) and {} backup job(s) from database",
            backups.len(),
            jobs.len()
        );
        Ok(Self {
            backups: RwLock::new(backups),
            jobs: RwLock::new(jobs),
            db: Some(db),
        })
    }

    pub async fn put_backup(&self, b: &Backup) -> Result<()> {
        let mut map = self.backups.write().await;
        if let Some(db) = &self.db {
            db.put(kinds::BACKUP, &b.id, b).await?;
        }
        map.insert(b.id.clone(), b.clone());
        Ok(())
    }

    pub async fn get_backup(&self, id: &str) -> Result<Option<Backup>> {
        Ok(self.backups.read().await.get(id).cloned())
    }

    /// All backups, newest first.
    pub async fn list_backups(&self) -> Result<Vec<Backup>> {
        let mut list: Vec<Backup> = self.backups.read().await.values().cloned().collect();
        list.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id)));
        Ok(list)
    }

    pub async fn delete_backup(&self, id: &str) -> Result<()> {
        let mut map = self.backups.write().await;
        if let Some(db) = &self.db {
            db.delete(kinds::BACKUP, id).await?;
        }
        map.remove(id);
        Ok(())
    }

    pub async fn put_job(&self, j: &BackupJob) -> Result<()> {
        let mut map = self.jobs.write().await;
        if let Some(db) = &self.db {
            db.put(kinds::BACKUP_JOB, &j.id, j).await?;
        }
        map.insert(j.id.clone(), j.clone());
        Ok(())
    }

    pub async fn list_jobs(&self) -> Result<Vec<BackupJob>> {
        let mut list: Vec<BackupJob> = self.jobs.read().await.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }

    pub async fn get_job(&self, id: &str) -> Result<Option<BackupJob>> {
        Ok(self.jobs.read().await.get(id).cloned())
    }

    pub async fn delete_job(&self, id: &str) -> Result<bool> {
        let mut map = self.jobs.write().await;
        if !map.contains_key(id) {
            return Ok(false);
        }
        if let Some(db) = &self.db {
            db.delete(kinds::BACKUP_JOB, id).await?;
        }
        map.remove(id);
        Ok(true)
    }
}
