//! 存档文件存储：路径布局、文件名白名单、分片合并、版本创建/删除/回退、磁盘保护。
//!
//! ```text
//! saves/
//! ├── temp/<fileId>/<index>.chunk
//! └── users/<userId>/
//!     ├── <fileName>                                  # 当前版本（硬链接）
//!     └── .versions/<fileName>/<seq>_<versionId>.bin  # 历史版本
//! ```

use crate::error::{AppError, AppResult, ErrorCode};
use crate::repo::{self, SaveDoc, SaveVersion, User};
use crate::util::{now_dt, now_ms};
use mongodb::bson::oid::ObjectId;
use mongodb::Database;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug)]
pub struct Storage {
    pub root: PathBuf,
    pub max_file_size: i64,
    pub max_chunk_size: i64,
    pub max_total_chunks: i64,
    pub min_free_bytes: u64,
    pub upload_idle_seconds: u64,
    pub upload_max_hours: u64,
}

const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

impl Storage {
    pub fn new(
        root: PathBuf,
        max_file_size: i64,
        max_chunk_size: i64,
        max_total_chunks: i64,
        min_free_bytes: u64,
        upload_idle_seconds: u64,
        upload_max_hours: u64,
    ) -> Self {
        Self {
            root,
            max_file_size,
            max_chunk_size,
            max_total_chunks,
            min_free_bytes,
            upload_idle_seconds,
            upload_max_hours,
        }
    }

    pub fn temp_root(&self) -> PathBuf {
        self.root.join("temp")
    }

    pub fn user_root(&self, user_id: ObjectId) -> PathBuf {
        self.root.join("users").join(user_id.to_hex())
    }

    pub fn current_path(&self, user_id: ObjectId, file_name: &str) -> PathBuf {
        self.user_root(user_id).join(file_name)
    }

    pub fn versions_dir(&self, user_id: ObjectId, file_name: &str) -> PathBuf {
        self.user_root(user_id).join(".versions").join(file_name)
    }

    pub fn version_path(
        &self,
        user_id: ObjectId,
        file_name: &str,
        seq: i64,
        version_id: ObjectId,
    ) -> PathBuf {
        self.versions_dir(user_id, file_name)
            .join(format!("{seq}_{}.bin", version_id.to_hex()))
    }

    pub fn storage_key(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn temp_dir(&self, file_id: &str) -> PathBuf {
        self.temp_root().join(file_id)
    }

    pub fn chunk_path(&self, file_id: &str, index: i64) -> PathBuf {
        self.temp_dir(file_id).join(format!("{index}.chunk"))
    }

    pub async fn ensure_layout(&self) -> std::io::Result<()> {
        tokio::fs::create_dir_all(self.temp_root()).await?;
        tokio::fs::create_dir_all(self.root.join("users")).await?;
        Ok(())
    }

    /// 剩余磁盘空间（字节）；读取失败时返回 `None`，由调用方决定是否放行。
    pub fn available_space(&self) -> Option<u64> {
        fs4::available_space(&self.root).ok()
    }

    pub fn check_disk(&self) -> AppResult<()> {
        if let Some(free) = self.available_space() {
            if free < self.min_free_bytes {
                return Err(AppError::new(ErrorCode::DiskFull));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 文件名
    // ------------------------------------------------------------------

    /// 校验并归一化文件名；返回安全的名字。
    pub fn validate_file_name(raw: &str) -> AppResult<String> {
        let name = raw.nfc().collect::<String>();
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.len() > 200 {
            return Err(AppError::new(ErrorCode::InvalidFileName));
        }
        if trimmed == "." || trimmed == ".." || trimmed.contains("..") {
            return Err(AppError::new(ErrorCode::InvalidFileName));
        }
        if trimmed.ends_with('.') || trimmed.ends_with(' ') {
            return Err(AppError::new(ErrorCode::InvalidFileName));
        }
        for ch in trimmed.chars() {
            if ch.is_control() || "/\\:*?\"<>|".contains(ch) {
                return Err(AppError::new(ErrorCode::InvalidFileName));
            }
        }
        let stem = trimmed.split('.').next().unwrap_or("").to_ascii_lowercase();
        if RESERVED_NAMES.contains(&stem.as_str()) {
            return Err(AppError::new(ErrorCode::InvalidFileName));
        }
        Ok(trimmed.to_string())
    }

    // ------------------------------------------------------------------
    // 分片
    // ------------------------------------------------------------------

    pub async fn create_temp_dir(&self, file_id: &str) -> std::io::Result<()> {
        tokio::fs::create_dir_all(self.temp_dir(file_id)).await
    }

    pub async fn remove_temp_dir(&self, file_id: &str) {
        let dir = self.temp_dir(file_id);
        if let Err(err) = tokio::fs::remove_dir_all(&dir).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %dir.display(), error = %err, "清理临时分片目录失败");
            }
        }
    }

    /// 检查分片是否齐全，返回缺失的序号（用于 `3006 chunk_incomplete`）。
    pub async fn missing_chunks(&self, file_id: &str, total: i64) -> Vec<i64> {
        let mut missing = Vec::new();
        for index in 0..total {
            let path = self.chunk_path(file_id, index);
            match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() => {}
                _ => missing.push(index),
            }
        }
        missing
    }

    /// 顺序合并分片到目标文件，同时流式计算 SM3；返回 `(大小, sm3)`。
    pub async fn merge_chunks(
        &self,
        file_id: &str,
        total: i64,
        dest: &Path,
    ) -> AppResult<(i64, String)> {
        use sm3::Digest as _;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut out = tokio::fs::File::create(dest).await?;
        let mut hasher = sm3::Sm3::new();
        let mut total_size: i64 = 0;
        let mut buf = vec![0u8; 256 * 1024];

        for index in 0..total {
            let chunk_path = self.chunk_path(file_id, index);
            let mut chunk = tokio::fs::File::open(&chunk_path).await.map_err(|err| {
                warn!(path = %chunk_path.display(), error = %err, "读取分片失败");
                AppError::new(ErrorCode::ChunkIncomplete).with_payload(
                    serde_json::json!({ "nots": [index] }),
                )
            })?;
            loop {
                let read = chunk.read(&mut buf).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buf[..read]);
                out.write_all(&buf[..read]).await?;
                total_size += read as i64;
            }
        }

        out.flush().await?;
        out.sync_all().await?;
        drop(out);
        Ok((total_size, hex::encode(hasher.finalize())))
    }

    // ------------------------------------------------------------------
    // 版本
    // ------------------------------------------------------------------

    /// 把 `staged` 文件提交为新版本；返回更新后的存档指针与版本记录。
    ///
    /// 顺序：写版本文件 → 插版本记录 → 切换当前硬链接 → 更新指针 → 计入配额。
    #[allow(clippy::too_many_arguments)]
    pub async fn commit_version(
        &self,
        db: &Database,
        user: &User,
        file_name: &str,
        staged: &Path,
        size: i64,
        sm3: &str,
        created_by: Option<ObjectId>,
        source: &str,
    ) -> AppResult<(SaveDoc, SaveVersion)> {
        let (save, seq) = repo::next_version_seq(db, user.id, file_name).await?;
        // 大小写冲突时统一使用最初登记的文件名，避免产生两份实体文件
        let canonical = save.file_name.clone();

        let version_id = ObjectId::new();
        let version_path = self.version_path(user.id, &canonical, seq, version_id);
        if let Some(parent) = version_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // rename 跨目录但同一磁盘，原子；失败则退回拷贝
        if let Err(err) = tokio::fs::rename(staged, &version_path).await {
            warn!(error = %err, "rename 失败，改为拷贝版本文件");
            tokio::fs::copy(staged, &version_path).await?;
            let _ = tokio::fs::remove_file(staged).await;
        }

        let version = SaveVersion {
            id: version_id,
            save_id: save.id,
            user_id: user.id,
            file_name: canonical.clone(),
            seq,
            size_bytes: size,
            sm3: Some(sm3.to_string()),
            storage_key: self.storage_key(&version_path),
            created_at: now_dt(),
            created_by,
            source: source.to_string(),
        };

        if let Err(err) = repo::insert_version(db, &version).await {
            let _ = tokio::fs::remove_file(&version_path).await;
            return Err(err.into());
        }

        if let Err(err) = self.switch_current(user.id, &canonical, &version_path).await {
            // 版本文件与记录保留，reconcile 会修正指针
            warn!(error = %err, "切换当前版本失败");
            return Err(err);
        }

        let mtime = now_ms();
        repo::update_save_pointer(
            db,
            save.id,
            Some(version.id),
            seq,
            size,
            Some(sm3),
            mtime,
        )
        .await?;
        repo::commit_reservation(db, user.id, size).await?;

        let mut updated = save;
        updated.current_version_id = Some(version.id);
        updated.current_seq = seq;
        updated.size_bytes = size;
        updated.mtime_ms = mtime;
        updated.sm3 = Some(sm3.to_string());
        Ok((updated, version))
    }

    /// 用 `target` 替换当前硬链接（保持内容只有一份）。
    async fn switch_current(
        &self,
        user_id: ObjectId,
        file_name: &str,
        target: &Path,
    ) -> AppResult<()> {
        let current = self.current_path(user_id, file_name);
        if let Some(parent) = current.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::remove_file(&current).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        if let Err(err) = tokio::fs::hard_link(target, &current).await {
            // 某些文件系统（如跨卷、FAT）不支持硬链接，退化为拷贝
            warn!(error = %err, "硬链接失败，改为拷贝当前文件");
            tokio::fs::copy(target, &current).await?;
        }
        Ok(())
    }

    /// 删除当前版本（默认回退上一版）或彻底删除所有版本。
    pub async fn delete_save(
        &self,
        db: &Database,
        user: &User,
        save: &SaveDoc,
        delete_versions: bool,
    ) -> AppResult<DeleteOutcome> {
        let current = self.current_path(user.id, &save.file_name);

        if delete_versions {
            let versions = repo::list_versions(db, save.id, 100_000).await?;
            let mut freed = 0i64;
            for version in &versions {
                if let Some(path) = self.resolve_key(&version.storage_key) {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                freed += version.size_bytes;
            }
            let _ = tokio::fs::remove_file(&current).await;
            repo::delete_versions_of_save(db, save.id).await?;
            repo::delete_save(db, save.id).await?;
            if freed > 0 {
                repo::adjust_used(db, user.id, -freed).await?;
            }
            let versions_dir = self.versions_dir(user.id, &save.file_name);
            let _ = tokio::fs::remove_dir_all(&versions_dir).await;
            return Ok(DeleteOutcome {
                deleted_all: true,
                freed_bytes: freed,
                current_seq: 0,
            });
        }

        // 只删当前：找上一版
        let prev = repo::find_prev_version(db, save.id, save.current_seq).await?;
        let mut freed = save.size_bytes;

        if let Some(version_id) = save.current_version_id {
            if let Some(version) = repo::find_version(db, version_id).await? {
                if let Some(path) = self.resolve_key(&version.storage_key) {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                freed = version.size_bytes;
            }
            repo::delete_version(db, version_id).await?;
        }

        match prev {
            Some(prev) => {
                let prev_path = self
                    .resolve_key(&prev.storage_key)
                    .ok_or_else(|| AppError::internal("版本文件路径非法"))?;
                self.switch_current(user.id, &save.file_name, &prev_path).await?;
                repo::update_save_pointer(
                    db,
                    save.id,
                    Some(prev.id),
                    prev.seq,
                    prev.size_bytes,
                    prev.sm3.as_deref(),
                    now_ms(),
                )
                .await?;
                // usedBytes 的口径是“全部版本字节数之和”：回退后当前版本被移除、
                // 上一版本重新计入，差额要立刻修正，否则要等 10 分钟对账才自愈
                if prev.size_bytes != freed {
                    repo::adjust_used(db, user.id, prev.size_bytes - freed).await?;
                }
                Ok(DeleteOutcome {
                    deleted_all: false,
                    freed_bytes: freed,
                    current_seq: prev.seq,
                })
            }
            None => {
                let _ = tokio::fs::remove_file(&current).await;
                // “无更早版本”意味着文件彻底消失：较新的版本记录（如果存在）
                // 也必须一并清除，否则会成为孤儿版本，虚增 usedBytes 统计
                repo::delete_versions_of_save(db, save.id).await?;
                repo::delete_save(db, save.id).await?;
                if freed > 0 {
                    repo::adjust_used(db, user.id, -freed).await?;
                }
                let versions_dir = self.versions_dir(user.id, &save.file_name);
                let _ = tokio::fs::remove_dir_all(&versions_dir).await;
                Ok(DeleteOutcome {
                    deleted_all: false,
                    freed_bytes: freed,
                    current_seq: 0,
                })
            }
        }
    }

    /// 回退到指定版本；不删除更新的版本。
    pub async fn rollback(
        &self,
        db: &Database,
        user: &User,
        save: &SaveDoc,
        version: &SaveVersion,
    ) -> AppResult<SaveDoc> {
        let path = self
            .resolve_key(&version.storage_key)
            .ok_or_else(|| AppError::internal("版本文件路径非法"))?;
        if tokio::fs::metadata(&path).await.is_err() {
            return Err(AppError::new(ErrorCode::VersionNotFound));
        }
        self.switch_current(user.id, &save.file_name, &path).await?;
        repo::update_save_pointer(
            db,
            save.id,
            Some(version.id),
            version.seq,
            version.size_bytes,
            version.sm3.as_deref(),
            now_ms(),
        )
        .await?;

        let mut updated = save.clone();
        updated.current_version_id = Some(version.id);
        updated.current_seq = version.seq;
        updated.size_bytes = version.size_bytes;
        updated.sm3 = version.sm3.clone();
        updated.mtime_ms = now_ms();
        Ok(updated)
    }

    pub fn resolve_key(&self, storage_key: &str) -> Option<PathBuf> {
        let candidate = self.root.join(storage_key.replace('/', std::path::MAIN_SEPARATOR_STR));
        // 防目录穿越：解析后必须仍在根目录内
        let root = self.root.canonicalize().ok()?;
        let parent = candidate.parent()?.canonicalize().ok()?;
        if parent.starts_with(&root) {
            Some(candidate)
        } else {
            None
        }
    }

    /// 清理由崩溃残留的临时目录（DB 中不存在的 fileId）。
    pub async fn clean_orphan_temp(&self, db: &Database) -> usize {
        let mut removed = 0usize;
        let mut entries = match tokio::fs::read_dir(self.temp_root()).await {
            Ok(e) => e,
            Err(_) => return 0,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            match repo::find_upload_session(db, &name).await {
                Ok(Some(_)) => {}
                _ => {
                    let _ = tokio::fs::remove_dir_all(entry.path()).await;
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            info!(removed, "已清理孤儿上传目录");
        }
        removed
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub deleted_all: bool,
    pub freed_bytes: i64,
    pub current_seq: i64,
}
