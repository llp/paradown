use crate::DownloadError;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::repository::repository::DownloadRepository;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

pub struct SqliteRepository {
    pool: Arc<SqlitePool>,
}

impl SqliteRepository {
    pub async fn new(db_path: &PathBuf) -> Result<Self, DownloadError> {
        let cwd = std::env::current_dir()?;
        let db_abs = if db_path.is_absolute() {
            db_path.to_path_buf()
        } else {
            cwd.join(db_path)
        };

        if let Some(parent) = db_abs.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    DownloadError::Other(format!("Failed to create directory {:?}: {}", parent, e))
                })?;
            }
        }

        if !db_abs.exists() {
            fs::File::create(&db_abs).map_err(|e| {
                DownloadError::Other(format!("Failed to create DB file {:?}: {}", db_abs, e))
            })?;
        }

        let conn_str = format!("sqlite://{}", db_abs.display());
        let pool = SqlitePool::connect(&conn_str)
            .await
            .map_err(|e| DownloadError::Other(e.to_string()))?;

        // 创建表
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS download_tasks (
                id INTEGER PRIMARY KEY,
                url TEXT UNIQUE NOT NULL,
                file_name TEXT NOT NULL,
                file_path TEXT,
                status TEXT NOT NULL,
                downloaded_size INTEGER DEFAULT 0,
                total_size INTEGER,
                created_at TEXT DEFAULT (STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT DEFAULT (STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS download_workers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,       -- 保留自增 id 方便管理
                task_id INTEGER NOT NULL,
                "index" INTEGER NOT NULL,                   -- 任务内部 worker 索引
                start INTEGER NOT NULL,
                "end" INTEGER NOT NULL,
                downloaded INTEGER DEFAULT 0,
                status TEXT DEFAULT 'Pending',
                updated_at TEXT DEFAULT (STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')),
                UNIQUE(task_id, "index")                    -- 确保同一任务内部 index 唯一
            );
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS download_checksums (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL,
                    algorithm TEXT NOT NULL,
                    value TEXT NOT NULL,
                    verified BOOLEAN DEFAULT 0,
                    verified_at TEXT,
                    UNIQUE(task_id, algorithm)  -- 确保同一任务同一算法唯一
                );
                "#,
        )
        .execute(&pool)
        .await?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }
}

#[async_trait]
impl DownloadRepository for SqliteRepository {
    // ---------------- Task ----------------
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, DownloadError> {
        let rows = sqlx::query("SELECT * FROM download_tasks ORDER BY id")
            .fetch_all(&*self.pool)
            .await?;

        let tasks = rows
            .into_iter()
            .map(|row| DBDownloadTask {
                id: row.get("id"),
                url: row.get("url"),
                file_name: row.get("file_name"),
                file_path: row.get("file_path"),
                status: row.get("status"),
                downloaded_size: row.get::<i64, _>("downloaded_size") as u64,
                total_size: row.try_get::<i64, _>("total_size").ok().map(|v| v as u64),
                created_at: row
                    .try_get::<String, _>("created_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
                updated_at: row
                    .try_get::<String, _>("updated_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            })
            .collect();

        Ok(tasks)
    }

    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, DownloadError> {
        let row = sqlx::query("SELECT * FROM download_tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_optional(&*self.pool)
            .await?;

        if let Some(row) = row {
            Ok(Some(DBDownloadTask {
                id: row.get("id"),
                url: row.get("url"),
                file_name: row.get("file_name"),
                file_path: row.get("file_path"),
                status: row.get("status"),
                downloaded_size: row.get::<i64, _>("downloaded_size") as u64,
                total_size: row.try_get::<i64, _>("total_size").ok().map(|v| v as u64),
                created_at: row
                    .try_get::<String, _>("created_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
                updated_at: row
                    .try_get::<String, _>("updated_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            }))
        } else {
            Ok(None)
        }
    }

    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), DownloadError> {
        sqlx::query(
            r#"
            INSERT INTO download_tasks
                (id, url, file_name, file_path, status, downloaded_size, total_size, created_at, updated_at)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')), COALESCE(?9, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')))
            ON CONFLICT(id) DO UPDATE SET
                url=excluded.url,
                file_name=excluded.file_name,
                file_path=excluded.file_path,
                status=excluded.status,
                downloaded_size=excluded.downloaded_size,
                total_size=excluded.total_size,
                updated_at=COALESCE(excluded.updated_at, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now'))
            "#
        )
            .bind(task.id)
            .bind(&task.url)
            .bind(&task.file_name)
            .bind(&task.file_path)
            .bind(&task.status)
            .bind(task.downloaded_size as i64)
            .bind(task.total_size.map(|v| v as i64))
            .bind(task.created_at.map(|dt| dt.to_rfc3339()))
            .bind(task.updated_at.map(|dt| dt.to_rfc3339()))
            .execute(&*self.pool)
            .await?;

        Ok(())
    }

    async fn delete_task(&self, task_id: u32) -> Result<(), DownloadError> {
        sqlx::query("DELETE FROM download_tasks WHERE id = ?1")
            .bind(task_id)
            .execute(&*self.pool)
            .await?;
        Ok(())
    }

    // ---------------- Worker ----------------
    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, DownloadError> {
        let rows =
            sqlx::query("SELECT * FROM download_workers WHERE task_id = ?1 ORDER BY \"index\"")
                .bind(task_id)
                .fetch_all(&*self.pool)
                .await?;

        Ok(rows
            .into_iter()
            .map(|r| DBDownloadWorker {
                id: r.get("id"),
                task_id: r.get("task_id"),
                index: r.get::<i64, _>("index") as u32,
                start: r.get::<i64, _>("start") as u64,
                end: r.get::<i64, _>("end") as u64,
                downloaded: r.get::<i64, _>("downloaded") as u64,
                status: r.get("status"),
                updated_at: r
                    .try_get::<String, _>("updated_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            })
            .collect())
    }

    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, DownloadError> {
        let row = sqlx::query("SELECT * FROM download_workers WHERE id = ?1")
            .bind(worker_id)
            .fetch_optional(&*self.pool)
            .await?;

        if let Some(r) = row {
            Ok(Some(DBDownloadWorker {
                id: r.get("id"),
                task_id: r.get("task_id"),
                index: r.get::<i64, _>("index") as u32,
                start: r.get::<i64, _>("start") as u64,
                end: r.get::<i64, _>("end") as u64,
                downloaded: r.get::<i64, _>("downloaded") as u64,
                status: r.get("status"),
                updated_at: r
                    .try_get::<String, _>("updated_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            }))
        } else {
            Ok(None)
        }
    }

    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), DownloadError> {
        // 假设表中已经对 (task_id, index) 建立 UNIQUE 约束
        sqlx::query(
            r#"
        INSERT INTO download_workers
            (task_id, "index", start, "end", downloaded, status, updated_at)
        VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, COALESCE(?7, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')))
        ON CONFLICT(task_id, "index") DO UPDATE SET
            start=excluded.start,
            "end"=excluded."end",
            downloaded=excluded.downloaded,
            status=excluded.status,
            updated_at=COALESCE(excluded.updated_at, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now'))
        "#,
        )
        .bind(worker.task_id)
        .bind(worker.index)
        .bind(worker.start as i64)
        .bind(worker.end as i64)
        .bind(worker.downloaded as i64)
        .bind(&worker.status)
        .bind(worker.updated_at.map(|dt| dt.to_rfc3339()))
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn delete_workers(&self, task_id: u32) -> Result<(), DownloadError> {
        sqlx::query("DELETE FROM download_workers WHERE task_id = ?1")
            .bind(task_id)
            .execute(&*self.pool)
            .await?;
        Ok(())
    }

    // ---------------- Checksum ----------------
    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, DownloadError> {
        let rows = sqlx::query("SELECT * FROM download_checksums WHERE task_id = ?1")
            .bind(task_id)
            .fetch_all(&*self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| DBDownloadChecksum {
                id: r.get("id"),
                task_id: r.get("task_id"),
                algorithm: r.get("algorithm"),
                value: r.get("value"),
                verified: r.get::<i64, _>("verified") != 0,
                verified_at: r
                    .try_get::<String, _>("verified_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            })
            .collect())
    }

    async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, DownloadError> {
        let row = sqlx::query("SELECT * FROM download_checksums WHERE id = ?1")
            .bind(checksum_id)
            .fetch_optional(&*self.pool)
            .await?;

        if let Some(r) = row {
            Ok(Some(DBDownloadChecksum {
                id: r.get("id"),
                task_id: r.get("task_id"),
                algorithm: r.get("algorithm"),
                value: r.get("value"),
                verified: r.get::<i64, _>("verified") != 0,
                verified_at: r
                    .try_get::<String, _>("verified_at")
                    .ok()
                    .and_then(|s| parse_db_datetime(&s)),
            }))
        } else {
            Ok(None)
        }
    }

    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), DownloadError> {
        sqlx::query(
            r#"
                INSERT INTO download_checksums (task_id, algorithm, value, verified, verified_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(task_id, algorithm) DO UPDATE SET
                    value=excluded.value,
                    verified=excluded.verified,
                    verified_at=excluded.verified_at
                "#,
        )
        .bind(checksum.task_id)
        .bind(&checksum.algorithm)
        .bind(&checksum.value)
        .bind(if checksum.verified { 1 } else { 0 })
        .bind(checksum.verified_at.map(|dt| dt.to_rfc3339()))
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn delete_checksums(&self, task_id: u32) -> Result<(), DownloadError> {
        sqlx::query("DELETE FROM download_checksums WHERE task_id = ?1")
            .bind(task_id)
            .execute(&*self.pool)
            .await?;
        Ok(())
    }
}

fn parse_db_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
        })
}

#[cfg(test)]
mod tests {
    use super::SqliteRepository;
    use crate::repository::models::DBDownloadTask;
    use crate::repository::repository::DownloadRepository;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    #[tokio::test]
    async fn round_trips_task_with_explicit_id_and_timestamps() {
        let tempdir = tempdir().unwrap();
        let db_path = tempdir.path().join("paradown-test.db");
        let repository = SqliteRepository::new(&db_path).await.unwrap();

        let created_at = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        let updated_at = Utc.with_ymd_and_hms(2026, 4, 15, 12, 30, 0).unwrap();
        let task = DBDownloadTask {
            id: 42,
            url: "https://example.com/file.bin".into(),
            file_name: "file.bin".into(),
            file_path: "/tmp/file.bin".into(),
            status: "Paused".into(),
            downloaded_size: 128,
            total_size: Some(1024),
            created_at: Some(created_at),
            updated_at: Some(updated_at),
        };

        repository.save_task(&task).await.unwrap();

        let loaded = repository.load_task(42).await.unwrap().unwrap();
        assert_eq!(loaded.id, 42);
        assert_eq!(loaded.url, task.url);
        assert_eq!(loaded.status, "Paused");
        assert_eq!(loaded.downloaded_size, 128);
        assert_eq!(loaded.total_size, Some(1024));
        assert_eq!(loaded.created_at, Some(created_at));
        assert_eq!(loaded.updated_at, Some(updated_at));
    }

    #[tokio::test]
    async fn updates_task_by_id_without_losing_timestamp_shape() {
        let tempdir = tempdir().unwrap();
        let db_path = tempdir.path().join("paradown-update.db");
        let repository = SqliteRepository::new(&db_path).await.unwrap();

        let created_at = Utc.with_ymd_and_hms(2026, 4, 15, 10, 0, 0).unwrap();
        repository
            .save_task(&DBDownloadTask {
                id: 7,
                url: "https://example.com/archive.tar".into(),
                file_name: "archive.tar".into(),
                file_path: "/tmp/archive.tar".into(),
                status: "Pending".into(),
                downloaded_size: 0,
                total_size: Some(2048),
                created_at: Some(created_at),
                updated_at: Some(created_at),
            })
            .await
            .unwrap();

        let updated_at = Utc.with_ymd_and_hms(2026, 4, 15, 11, 0, 0).unwrap();
        repository
            .save_task(&DBDownloadTask {
                id: 7,
                url: "https://example.com/archive.tar".into(),
                file_name: "archive.tar".into(),
                file_path: "/tmp/archive.tar".into(),
                status: "Running".into(),
                downloaded_size: 512,
                total_size: Some(2048),
                created_at: Some(created_at),
                updated_at: Some(updated_at),
            })
            .await
            .unwrap();

        let loaded = repository.load_task(7).await.unwrap().unwrap();
        assert_eq!(loaded.id, 7);
        assert_eq!(loaded.status, "Running");
        assert_eq!(loaded.downloaded_size, 512);
        assert_eq!(loaded.created_at, Some(created_at));
        assert_eq!(loaded.updated_at, Some(updated_at));
    }
}
