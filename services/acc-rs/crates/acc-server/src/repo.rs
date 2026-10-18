//! MongoDB 数据访问层：集合初始化、索引与各实体的读写。
//!
//! 约定：Mongo 是事实源；所有时间字段使用 `bson::DateTime`（毫秒），
//! 对外 JSON 一律转成毫秒整数。

use crate::util::{now_dt, now_ms};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, oid::ObjectId, DateTime, Document};
use mongodb::options::{IndexOptions, InsertOneOptions};
use mongodb::{Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_USER: &str = "user";
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DISABLED: &str = "disabled";
pub const STATUS_DELETED: &str = "deleted";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_name: String,
    pub user_name_lower: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_lower: Option<String>,
    #[serde(default)]
    pub email_verified_at: Option<DateTime>,
    pub password_hash: String,
    pub password_algo: String,
    pub role: String,
    pub status: String,
    pub quota_bytes: i64,
    pub used_bytes: i64,
    #[serde(default)]
    pub reserved_bytes: i64,
    #[serde(default)]
    pub must_change_password: bool,
    #[serde(default = "default_locale")]
    pub locale: String,
    pub token_version: i32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    #[serde(default)]
    pub last_login_at: Option<DateTime>,
}

fn default_locale() -> String {
    "zh-CN".into()
}

fn empty_binary() -> mongodb::bson::Binary {
    mongodb::bson::Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: Vec::new(),
    }
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == ROLE_ADMIN
    }

    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    pub fn used_with_reserved(&self) -> i64 {
        self.used_bytes.saturating_add(self.reserved_bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub token_hash: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    #[serde(default)]
    pub last_seen_at: Option<DateTime>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<DateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSession {
    /// uuid v4
    #[serde(rename = "_id")]
    pub id: String,
    pub user_id: ObjectId,
    pub user_name: String,
    pub file_name: String,
    pub file_size: i64,
    pub chunk_size: i64,
    pub total_chunks: i64,
    #[serde(default = "empty_binary")]
    pub uploaded_mask: mongodb::bson::Binary,
    pub upload_token_hash: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub hard_deadline: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDoc {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub file_name: String,
    pub file_name_lower: String,
    #[serde(default)]
    pub current_version_id: Option<ObjectId>,
    pub current_seq: i64,
    pub size_bytes: i64,
    pub mtime_ms: i64,
    #[serde(default)]
    pub sm3: Option<String>,
    /// 下一个可用版本号（`$inc` 后使用）
    pub next_seq: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveVersion {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub save_id: ObjectId,
    pub user_id: ObjectId,
    pub file_name: String,
    pub seq: i64,
    pub size_bytes: i64,
    #[serde(default)]
    pub sm3: Option<String>,
    pub storage_key: String,
    pub created_at: DateTime,
    pub created_by: Option<ObjectId>,
    /// mod | admin | user
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailCode {
    /// HMAC(email|purpose)
    #[serde(rename = "_id")]
    pub id: String,
    pub email_lower: String,
    pub purpose: String,
    pub code_hash: String,
    pub attempts: i32,
    pub last_sent_at: DateTime,
    pub expires_at: DateTime,
    #[serde(default)]
    pub consumed_at: Option<DateTime>,
    pub ip: String,
    pub created_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditLog {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    #[serde(default)]
    pub actor_id: Option<ObjectId>,
    pub actor_name: String,
    pub action: String,
    pub target_type: String,
    #[serde(default)]
    pub target_id: Option<String>,
    /// success | failure
    pub result: String,
    #[serde(default)]
    pub detail: Option<String>,
    pub ip: String,
    #[serde(default)]
    pub user_agent: Option<String>,
    pub created_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(rename = "_id")]
    pub id: String,
    pub default_quota_bytes: i64,
    pub max_file_size: i64,
    pub allowed_extensions: Vec<String>,
    pub updated_at: DateTime,
    #[serde(default)]
    pub updated_by: Option<ObjectId>,
}

/// 用于鉴权与展示的用户快照（**不含密码哈希**，可安全进 Redis）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified_at: Option<DateTime>,
    pub role: String,
    pub status: String,
    pub quota_bytes: i64,
    pub used_bytes: i64,
    pub reserved_bytes: i64,
    pub must_change_password: bool,
    pub locale: String,
    pub created_at: DateTime,
    #[serde(default)]
    pub last_login_at: Option<DateTime>,
}

impl From<&User> for Profile {
    fn from(user: &User) -> Self {
        Self {
            id: user.id,
            user_name: user.user_name.clone(),
            email: user.email.clone(),
            email_verified_at: user.email_verified_at,
            role: user.role.clone(),
            status: user.status.clone(),
            quota_bytes: user.quota_bytes,
            used_bytes: user.used_bytes,
            reserved_bytes: user.reserved_bytes,
            must_change_password: user.must_change_password,
            locale: user.locale.clone(),
            created_at: user.created_at,
            last_login_at: user.last_login_at,
        }
    }
}

impl Profile {
    pub fn is_admin(&self) -> bool {
        self.role == ROLE_ADMIN
    }

    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    pub fn email_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }
}

// ---------------------------------------------------------------------------
// 集合句柄
// ---------------------------------------------------------------------------

pub fn users(db: &Database) -> Collection<User> {
    db.collection("users")
}
pub fn sessions(db: &Database) -> Collection<Session> {
    db.collection("sessions")
}
pub fn upload_sessions(db: &Database) -> Collection<UploadSession> {
    db.collection("upload_sessions")
}
pub fn saves(db: &Database) -> Collection<SaveDoc> {
    db.collection("saves")
}
pub fn versions(db: &Database) -> Collection<SaveVersion> {
    db.collection("save_versions")
}
pub fn email_codes(db: &Database) -> Collection<EmailCode> {
    db.collection("email_codes")
}
pub fn audit_logs(db: &Database) -> Collection<AuditLog> {
    db.collection("audit_logs")
}
pub fn settings(db: &Database) -> Collection<Settings> {
    db.collection("settings")
}

// ---------------------------------------------------------------------------
// 初始化
// ---------------------------------------------------------------------------

/// 幂等建集合（含 JSON Schema 校验）与索引；失败直接返回错误，由启动流程终止。
pub async fn init(db: &Database) -> anyhow::Result<()> {
    create_collections(db).await?;
    create_indexes(db).await?;
    ensure_default_settings(db).await?;
    Ok(())
}

async fn create_collections(db: &Database) -> anyhow::Result<()> {
    let specs = vec![
        (
            "users",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["userName", "userNameLower", "passwordHash", "role", "status",
                                 "quotaBytes", "usedBytes", "reservedBytes", "tokenVersion",
                                 "createdAt", "updatedAt"],
                    "properties": {
                        "userName": { "bsonType": "string", "maxLength": 32 },
                        "userNameLower": { "bsonType": "string" },
                        "email": { "bsonType": ["string", "null"] },
                        "emailLower": { "bsonType": ["string", "null"] },
                        "role": { "enum": ["admin", "user"] },
                        "status": { "enum": ["active", "disabled", "deleted"] },
                        "quotaBytes": { "bsonType": "long", "minimum": 0 },
                        "usedBytes": { "bsonType": "long", "minimum": 0 },
                        "reservedBytes": { "bsonType": "long", "minimum": 0 },
                        "tokenVersion": { "bsonType": "int", "minimum": 0 }
                    }
                }
            },
        ),
        (
            "sessions",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["userId", "tokenHash", "createdAt", "expiresAt"],
                    "properties": {
                        "userId": { "bsonType": "objectId" },
                        "tokenHash": { "bsonType": "string" },
                        "expiresAt": { "bsonType": "date" }
                    }
                }
            },
        ),
        (
            "upload_sessions",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["userId", "fileName", "fileSize", "chunkSize", "totalChunks",
                                 "uploadTokenHash", "createdAt", "expiresAt"],
                    "properties": {
                        "userId": { "bsonType": "objectId" },
                        "fileSize": { "bsonType": "long", "minimum": 0 },
                        "chunkSize": { "bsonType": "long", "minimum": 1 },
                        "totalChunks": { "bsonType": "long", "minimum": 1 }
                    }
                }
            },
        ),
        (
            "saves",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["userId", "fileName", "currentSeq", "sizeBytes", "nextSeq",
                                 "createdAt", "updatedAt"],
                    "properties": {
                        "userId": { "bsonType": "objectId" },
                        "fileName": { "bsonType": "string" },
                        "sizeBytes": { "bsonType": "long", "minimum": 0 }
                    }
                }
            },
        ),
        (
            "save_versions",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["saveId", "userId", "fileName", "seq", "sizeBytes", "storageKey", "createdAt"],
                    "properties": {
                        "saveId": { "bsonType": "objectId" },
                        "userId": { "bsonType": "objectId" },
                        "seq": { "bsonType": "long", "minimum": 1 },
                        "sizeBytes": { "bsonType": "long", "minimum": 0 }
                    }
                }
            },
        ),
        (
            "email_codes",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["emailLower", "purpose", "codeHash", "attempts", "expiresAt"],
                    "properties": {
                        "emailLower": { "bsonType": "string" },
                        "purpose": { "bsonType": "string" },
                        "codeHash": { "bsonType": "string" },
                        "attempts": { "bsonType": "int", "minimum": 0 }
                    }
                }
            },
        ),
        (
            "audit_logs",
            doc! {
                "$jsonSchema": {
                    "bsonType": "object",
                    "required": ["actorName", "action", "targetType", "result", "ip", "createdAt"],
                    "properties": {
                        "actorName": { "bsonType": "string" },
                        "action": { "bsonType": "string" },
                        "result": { "bsonType": "string" }
                    }
                }
            },
        ),
        ("settings", doc! {}),
    ];

    let existing = db.list_collection_names().await?;
    for (name, validator) in specs {
        if existing.iter().any(|n| n == name) {
            continue;
        }
        let command = doc! {
            "create": name,
            "validator": validator,
            "validationLevel": "moderate",
        };
        match db.run_command(command).await {
            Ok(_) => tracing::info!(collection = name, "集合已创建"),
            Err(err) => {
                let text = err.to_string();
                // 并发启动时可能已被其他进程创建
                if !text.contains("already exists") && !text.contains("NamespaceExists") {
                    return Err(anyhow::anyhow!("创建集合 {name} 失败: {err}"));
                }
            }
        }
    }
    Ok(())
}

async fn create_indexes(db: &Database) -> anyhow::Result<()> {
    let unique = || IndexOptions::builder().unique(true).build();

    users(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "userNameLower": 1 })
                .options(unique())
                .build(),
        )
        .await?;
    users(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "emailLower": 1 })
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(doc! { "emailLower": { "$type": "string" } })
                        .build(),
                )
                .build(),
        )
        .await?;
    users(db)
        .create_index(IndexModel::builder().keys(doc! { "status": 1, "createdAt": -1 }).build())
        .await?;

    sessions(db)
        .create_index(IndexModel::builder().keys(doc! { "userId": 1 }).build())
        .await?;
    sessions(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expiresAt": 1 })
                .options(IndexOptions::builder().expire_after(std::time::Duration::ZERO).build())
                .build(),
        )
        .await?;
    sessions(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "tokenHash": 1 })
                .options(unique())
                .build(),
        )
        .await?;

    upload_sessions(db)
        .create_index(IndexModel::builder().keys(doc! { "userId": 1 }).build())
        .await?;
    upload_sessions(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expiresAt": 1 })
                .options(IndexOptions::builder().expire_after(std::time::Duration::ZERO).build())
                .build(),
        )
        .await?;

    saves(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "userId": 1, "fileNameLower": 1 })
                .options(unique())
                .build(),
        )
        .await?;
    saves(db)
        .create_index(IndexModel::builder().keys(doc! { "userId": 1, "updatedAt": -1 }).build())
        .await?;

    versions(db)
        .create_index(IndexModel::builder().keys(doc! { "saveId": 1, "seq": -1 }).build())
        .await?;
    versions(db)
        .create_index(IndexModel::builder().keys(doc! { "userId": 1, "createdAt": -1 }).build())
        .await?;

    email_codes(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expiresAt": 1 })
                .options(IndexOptions::builder().expire_after(std::time::Duration::ZERO).build())
                .build(),
        )
        .await?;

    audit_logs(db)
        .create_index(
            IndexModel::builder()
                .keys(doc! { "createdAt": -1 })
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::from_secs(180 * 24 * 3600))
                        .build(),
                )
                .build(),
        )
        .await?;
    audit_logs(db)
        .create_index(IndexModel::builder().keys(doc! { "actorId": 1, "createdAt": -1 }).build())
        .await?;
    audit_logs(db)
        .create_index(IndexModel::builder().keys(doc! { "action": 1, "createdAt": -1 }).build())
        .await?;

    Ok(())
}

async fn ensure_default_settings(db: &Database) -> anyhow::Result<()> {
    let collection = settings(db);
    if collection.find_one(doc! { "_id": "default" }).await?.is_none() {
        let doc = Settings {
            id: "default".into(),
            default_quota_bytes: 1024 * 1024 * 1024,
            max_file_size: 2 * 1024 * 1024 * 1024,
            allowed_extensions: vec![
                ".dsv".into(),
                ".moddsv".into(),
                ".server".into(),
            ],
            updated_at: now_dt(),
            updated_by: None,
        };
        collection
            .insert_one(&doc)
            .with_options(InsertOneOptions::builder().bypass_document_validation(true).build())
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// users
// ---------------------------------------------------------------------------

pub struct NewUser<'a> {
    pub user_name: &'a str,
    pub email: Option<&'a str>,
    pub password_hash: &'a str,
    pub role: &'a str,
    pub quota_bytes: i64,
    pub locale: &'a str,
    pub email_verified: bool,
}

pub async fn create_user(db: &Database, new: NewUser<'_>) -> Result<User, mongodb::error::Error> {
    let now = now_dt();
    let user = User {
        id: ObjectId::new(),
        user_name: new.user_name.to_string(),
        user_name_lower: new.user_name.to_lowercase(),
        email: new.email.map(|e| e.to_string()),
        email_lower: new.email.map(|e| e.to_lowercase()),
        email_verified_at: if new.email_verified { Some(now) } else { None },
        password_hash: new.password_hash.to_string(),
        password_algo: "argon2id".into(),
        role: new.role.to_string(),
        status: STATUS_ACTIVE.into(),
        quota_bytes: new.quota_bytes,
        used_bytes: 0,
        reserved_bytes: 0,
        must_change_password: false,
        locale: new.locale.to_string(),
        token_version: 0,
        created_at: now,
        updated_at: now,
        last_login_at: None,
    };
    users(db).insert_one(&user).await?;
    Ok(user)
}

pub async fn find_user_by_id(
    db: &Database,
    id: ObjectId,
) -> Result<Option<User>, mongodb::error::Error> {
    users(db).find_one(doc! { "_id": id }).await
}

pub async fn find_user_by_name(
    db: &Database,
    name: &str,
) -> Result<Option<User>, mongodb::error::Error> {
    users(db)
        .find_one(doc! { "userNameLower": name.to_lowercase() })
        .await
}

pub async fn find_user_by_email(
    db: &Database,
    email: &str,
) -> Result<Option<User>, mongodb::error::Error> {
    users(db)
        .find_one(doc! { "emailLower": email.to_lowercase() })
        .await
}

pub async fn touch_login(db: &Database, id: ObjectId) -> Result<(), mongodb::error::Error> {
    users(db)
        .update_one(
            doc! { "_id": id },
            doc! { "$set": { "lastLoginAt": now_dt(), "updatedAt": now_dt() } },
        )
        .await?;
    Ok(())
}

pub async fn bump_token_version(
    db: &Database,
    id: ObjectId,
    must_change_password: Option<bool>,
) -> Result<i32, mongodb::error::Error> {
    let mut set = doc! { "updatedAt": now_dt() };
    if let Some(flag) = must_change_password {
        set.insert("mustChangePassword", flag);
    }
    let result = users(db)
        .find_one_and_update(doc! { "_id": id }, doc! { "$inc": { "tokenVersion": 1 }, "$set": set })
        .return_document(mongodb::options::ReturnDocument::After)
        .await?;
    Ok(result.map(|u| u.token_version).unwrap_or(0))
}

/// 原子预留配额：`usedBytes + reservedBytes + size <= quotaBytes` 才 `$inc`。
pub async fn reserve_quota(
    db: &Database,
    id: ObjectId,
    size: i64,
) -> Result<bool, mongodb::error::Error> {
    let result = users(db)
        .update_one(
            doc! {
                "_id": id,
                "$expr": {
                    "$lte": [
                        { "$add": ["$usedBytes", "$reservedBytes", size] },
                        "$quotaBytes"
                    ]
                }
            },
            doc! { "$inc": { "reservedBytes": size } },
        )
        .await?;
    Ok(result.matched_count == 1)
}

pub async fn release_reservation(
    db: &Database,
    id: ObjectId,
    size: i64,
) -> Result<(), mongodb::error::Error> {
    users(db)
        .update_one(
            doc! { "_id": id },
            doc! { "$inc": { "reservedBytes": -size } },
        )
        .await?;
    Ok(())
}

pub async fn commit_reservation(
    db: &Database,
    id: ObjectId,
    size: i64,
) -> Result<(), mongodb::error::Error> {
    users(db)
        .update_one(
            doc! { "_id": id },
            doc! { "$inc": { "usedBytes": size, "reservedBytes": -size } },
        )
        .await?;
    Ok(())
}

pub async fn adjust_used(
    db: &Database,
    id: ObjectId,
    delta: i64,
) -> Result<(), mongodb::error::Error> {
    users(db)
        .update_one(doc! { "_id": id }, doc! { "$inc": { "usedBytes": delta } })
        .await?;
    Ok(())
}

pub async fn set_used_bytes(
    db: &Database,
    id: ObjectId,
    used: i64,
) -> Result<(), mongodb::error::Error> {
    users(db)
        .update_one(doc! { "_id": id }, doc! { "$set": { "usedBytes": used.max(0) } })
        .await?;
    Ok(())
}

pub async fn update_user(
    db: &Database,
    id: ObjectId,
    update: Document,
) -> Result<Option<User>, mongodb::error::Error> {
    let mut update = update;
    update.insert("updatedAt", now_dt());
    users(db)
        .find_one_and_update(doc! { "_id": id }, doc! { "$set": update })
        .return_document(mongodb::options::ReturnDocument::After)
        .await
}

pub async fn list_users(
    db: &Database,
    search: Option<&str>,
    status: Option<&str>,
    role: Option<&str>,
    skip: u64,
    limit: u64,
) -> Result<(Vec<User>, u64), mongodb::error::Error> {
    let mut filter = Document::new();
    if let Some(q) = search.filter(|q| !q.trim().is_empty()) {
        let rx = regex_contains(q.trim());
        filter.insert(
            "$or",
            vec![
                doc! { "userName": rx.clone() },
                doc! { "emailLower": rx },
            ],
        );
    }
    if let Some(s) = status.filter(|s| !s.is_empty()) {
        filter.insert("status", s);
    }
    if let Some(r) = role.filter(|r| !r.is_empty()) {
        filter.insert("role", r);
    }

    let total = users(db).count_documents(filter.clone()).await?;
    let cursor = users(db)
        .find(filter)
        .sort(doc! { "createdAt": -1 })
        .skip(skip)
        .limit(limit as i64)
        .await?;
    let list: Vec<User> = cursor.try_collect().await?;
    Ok((list, total))
}

pub async fn count_users(db: &Database, filter: Document) -> Result<u64, mongodb::error::Error> {
    users(db).count_documents(filter).await
}

pub async fn all_users(db: &Database) -> Result<Vec<User>, mongodb::error::Error> {
    let cursor = users(db).find(doc! {}).await?;
    cursor.try_collect().await
}

/// 未过期（可能仍在传）的上传会话，用于对账 reservedBytes。
pub async fn active_upload_sessions(
    db: &Database,
) -> Result<Vec<UploadSession>, mongodb::error::Error> {
    let cursor = upload_sessions(db)
        .find(doc! { "expiresAt": { "$gt": now_dt() } })
        .await?;
    cursor.try_collect().await
}

// ---------------------------------------------------------------------------
// sessions
// ---------------------------------------------------------------------------

pub async fn create_session(
    db: &Database,
    user_id: ObjectId,
    token_hash: &str,
    expires_at: DateTime,
    ip: &str,
    user_agent: Option<&str>,
) -> Result<Session, mongodb::error::Error> {
    let session = Session {
        id: ObjectId::new(),
        user_id,
        token_hash: token_hash.to_string(),
        created_at: now_dt(),
        expires_at,
        last_seen_at: Some(now_dt()),
        ip: Some(ip.to_string()),
        user_agent: user_agent.map(|s| s.to_string()),
        revoked_at: None,
    };
    sessions(db).insert_one(&session).await?;
    Ok(session)
}

pub async fn find_session_by_hash(
    db: &Database,
    token_hash: &str,
) -> Result<Option<Session>, mongodb::error::Error> {
    sessions(db).find_one(doc! { "tokenHash": token_hash }).await
}

pub async fn delete_session_by_hash(
    db: &Database,
    token_hash: &str,
) -> Result<(), mongodb::error::Error> {
    sessions(db)
        .delete_one(doc! { "tokenHash": token_hash })
        .await?;
    Ok(())
}

pub async fn delete_user_sessions(
    db: &Database,
    user_id: ObjectId,
) -> Result<(), mongodb::error::Error> {
    sessions(db).delete_many(doc! { "userId": user_id }).await?;
    Ok(())
}

pub async fn list_user_sessions(
    db: &Database,
    user_id: ObjectId,
) -> Result<Vec<Session>, mongodb::error::Error> {
    let cursor = sessions(db)
        .find(doc! { "userId": user_id, "expiresAt": { "$gt": now_dt() } })
        .sort(doc! { "createdAt": -1 })
        .limit(50)
        .await?;
    cursor.try_collect().await
}

pub async fn count_active_sessions(db: &Database) -> Result<u64, mongodb::error::Error> {
    sessions(db)
        .count_documents(doc! { "expiresAt": { "$gt": now_dt() } })
        .await
}

// ---------------------------------------------------------------------------
// upload_sessions
// ---------------------------------------------------------------------------

pub async fn create_upload_session(
    db: &Database,
    session: &UploadSession,
) -> Result<(), mongodb::error::Error> {
    upload_sessions(db).insert_one(session).await?;
    Ok(())
}

pub async fn find_upload_session(
    db: &Database,
    id: &str,
) -> Result<Option<UploadSession>, mongodb::error::Error> {
    upload_sessions(db).find_one(doc! { "_id": id }).await
}

pub async fn touch_upload_session(
    db: &Database,
    id: &str,
    expires_at: DateTime,
    uploaded_mask: &[u8],
) -> Result<(), mongodb::error::Error> {
    upload_sessions(db)
        .update_one(
            doc! { "_id": id },
            doc! {
                "$set": {
                    "expiresAt": expires_at,
                    "uploadedMask": mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: uploaded_mask.to_vec(),
                    }
                }
            },
        )
        .await?;
    Ok(())
}

pub async fn delete_upload_session(
    db: &Database,
    id: &str,
) -> Result<(), mongodb::error::Error> {
    upload_sessions(db).delete_one(doc! { "_id": id }).await?;
    Ok(())
}

pub async fn expired_upload_sessions(
    db: &Database,
    limit: i64,
) -> Result<Vec<UploadSession>, mongodb::error::Error> {
    let cursor = upload_sessions(db)
        .find(doc! { "expiresAt": { "$lte": now_dt() } })
        .limit(limit)
        .await?;
    cursor.try_collect().await
}

// ---------------------------------------------------------------------------
// saves / save_versions
// ---------------------------------------------------------------------------

pub async fn list_saves(
    db: &Database,
    user_id: ObjectId,
) -> Result<Vec<SaveDoc>, mongodb::error::Error> {
    let cursor = saves(db)
        .find(doc! { "userId": user_id })
        .sort(doc! { "fileNameLower": 1 })
        .await?;
    cursor.try_collect().await
}

pub async fn find_save(
    db: &Database,
    user_id: ObjectId,
    file_name: &str,
) -> Result<Option<SaveDoc>, mongodb::error::Error> {
    saves(db)
        .find_one(doc! { "userId": user_id, "fileNameLower": file_name.to_lowercase() })
        .await
}

pub async fn find_save_by_id(
    db: &Database,
    id: ObjectId,
) -> Result<Option<SaveDoc>, mongodb::error::Error> {
    saves(db).find_one(doc! { "_id": id }).await
}

/// 取得（或创建）存档指针，并原子分配一个新的版本号。
pub async fn next_version_seq(
    db: &Database,
    user_id: ObjectId,
    file_name: &str,
) -> Result<(SaveDoc, i64), mongodb::error::Error> {
    let now = now_dt();
    let filter = doc! { "userId": user_id, "fileNameLower": file_name.to_lowercase() };
    let update = doc! {
        "$inc": { "nextSeq": 1i64 },
        "$setOnInsert": {
            "_id": ObjectId::new(),
            "userId": user_id,
            "fileName": file_name,
            "fileNameLower": file_name.to_lowercase(),
            "currentSeq": 0i64,
            "sizeBytes": 0i64,
            "mtimeMs": now_ms(),
            "createdAt": now,
            "updatedAt": now,
        }
    };
    let save = saves(db)
        .find_one_and_update(filter, update)
        .upsert(true)
        .return_document(mongodb::options::ReturnDocument::After)
        .await?
        .ok_or_else(|| mongodb::error::Error::custom("分配版本号失败"))?;
    let seq = save.next_seq;
    Ok((save, seq))
}

pub async fn update_save_pointer(
    db: &Database,
    save_id: ObjectId,
    version_id: Option<ObjectId>,
    seq: i64,
    size: i64,
    sm3: Option<&str>,
    mtime_ms: i64,
) -> Result<(), mongodb::error::Error> {
    saves(db)
        .update_one(
            doc! { "_id": save_id },
            doc! {
                "$set": {
                    "currentVersionId": version_id,
                    "currentSeq": seq,
                    "sizeBytes": size,
                    "mtimeMs": mtime_ms,
                    "sm3": sm3,
                    "updatedAt": now_dt(),
                }
            },
        )
        .await?;
    Ok(())
}

pub async fn delete_save(db: &Database, save_id: ObjectId) -> Result<(), mongodb::error::Error> {
    saves(db).delete_one(doc! { "_id": save_id }).await?;
    Ok(())
}

pub async fn insert_version(
    db: &Database,
    version: &SaveVersion,
) -> Result<(), mongodb::error::Error> {
    versions(db).insert_one(version).await?;
    Ok(())
}

pub async fn list_versions(
    db: &Database,
    save_id: ObjectId,
    limit: i64,
) -> Result<Vec<SaveVersion>, mongodb::error::Error> {
    let cursor = versions(db)
        .find(doc! { "saveId": save_id })
        .sort(doc! { "seq": -1 })
        .limit(limit)
        .await?;
    cursor.try_collect().await
}

pub async fn find_version(
    db: &Database,
    version_id: ObjectId,
) -> Result<Option<SaveVersion>, mongodb::error::Error> {
    versions(db).find_one(doc! { "_id": version_id }).await
}

pub async fn find_prev_version(
    db: &Database,
    save_id: ObjectId,
    before_seq: i64,
) -> Result<Option<SaveVersion>, mongodb::error::Error> {
    versions(db)
        .find_one(doc! { "saveId": save_id, "seq": { "$lt": before_seq } })
        .sort(doc! { "seq": -1 })
        .await
}

pub async fn delete_version(
    db: &Database,
    version_id: ObjectId,
) -> Result<(), mongodb::error::Error> {
    versions(db).delete_one(doc! { "_id": version_id }).await?;
    Ok(())
}

pub async fn delete_versions_of_save(
    db: &Database,
    save_id: ObjectId,
) -> Result<u64, mongodb::error::Error> {
    let result = versions(db).delete_many(doc! { "saveId": save_id }).await?;
    Ok(result.deleted_count)
}

pub async fn sum_versions_bytes(
    db: &Database,
    user_id: ObjectId,
) -> Result<i64, mongodb::error::Error> {
    let cursor = versions(db).find(doc! { "userId": user_id }).await?;
    let list: Vec<SaveVersion> = cursor.try_collect().await?;
    Ok(list.iter().map(|v| v.size_bytes).sum())
}

pub async fn aggregate_usage(db: &Database) -> Result<Vec<(ObjectId, i64)>, mongodb::error::Error> {
    let cursor = versions(db).find(doc! {}).await?;
    let list: Vec<SaveVersion> = cursor.try_collect().await?;
    let mut map: std::collections::HashMap<ObjectId, i64> = std::collections::HashMap::new();
    for v in list {
        *map.entry(v.user_id).or_insert(0) += v.size_bytes;
    }
    Ok(map.into_iter().collect())
}

pub async fn search_saves(
    db: &Database,
    user_name: Option<&str>,
    file_name: Option<&str>,
    skip: u64,
    limit: u64,
) -> Result<(Vec<(SaveDoc, User)>, u64), mongodb::error::Error> {
    // 用户名先查 users 得到候选 userId
    let mut filter = Document::new();
    if let Some(q) = file_name.filter(|q| !q.trim().is_empty()) {
        filter.insert("fileNameLower", regex_contains(&q.trim().to_lowercase()));
    }
    if let Some(q) = user_name.filter(|q| !q.trim().is_empty()) {
        let rx = regex_contains(q.trim());
        let cursor = users(db)
            .find(doc! { "$or": [ { "userName": rx.clone() }, { "emailLower": rx } ] })
            .limit(500)
            .await?;
        let matched: Vec<User> = cursor.try_collect().await?;
        let ids: Vec<ObjectId> = matched.iter().map(|u| u.id).collect();
        if ids.is_empty() {
            return Ok((vec![], 0));
        }
        filter.insert("userId", doc! { "$in": ids });
    }

    let total = saves(db).count_documents(filter.clone()).await?;
    let cursor = saves(db)
        .find(filter)
        .sort(doc! { "updatedAt": -1 })
        .skip(skip)
        .limit(limit as i64)
        .await?;
    let list: Vec<SaveDoc> = cursor.try_collect().await?;

    let user_ids: Vec<ObjectId> = list.iter().map(|s| s.user_id).collect();
    let user_cursor = users(db).find(doc! { "_id": { "$in": user_ids } }).await?;
    let user_list: Vec<User> = user_cursor.try_collect().await?;
    let by_id: std::collections::HashMap<ObjectId, User> =
        user_list.into_iter().map(|u| (u.id, u)).collect();

    let joined = list
        .into_iter()
        .filter_map(|s| by_id.get(&s.user_id).cloned().map(|u| (s, u)))
        .collect();
    Ok((joined, total))
}

/// 构造不区分大小写的“包含”正则，并转义元字符，避免用户输入被当作正则执行。
pub fn regex_contains(input: &str) -> Document {
    let mut escaped = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    doc! { "$regex": escaped, "$options": "i" }
}

// ---------------------------------------------------------------------------
// email_codes
// ---------------------------------------------------------------------------

pub async fn upsert_email_code(
    db: &Database,
    code: &EmailCode,
) -> Result<(), mongodb::error::Error> {
    email_codes(db)
        .replace_one(doc! { "_id": &code.id }, code)
        .upsert(true)
        .await?;
    Ok(())
}

pub async fn find_email_code(
    db: &Database,
    id: &str,
) -> Result<Option<EmailCode>, mongodb::error::Error> {
    email_codes(db).find_one(doc! { "_id": id }).await
}

pub async fn incr_code_attempts(
    db: &Database,
    id: &str,
) -> Result<Option<EmailCode>, mongodb::error::Error> {
    email_codes(db)
        .find_one_and_update(
            doc! { "_id": id },
            doc! { "$inc": { "attempts": 1i32 } },
        )
        .return_document(mongodb::options::ReturnDocument::After)
        .await
}

pub async fn consume_email_code(
    db: &Database,
    id: &str,
) -> Result<(), mongodb::error::Error> {
    email_codes(db)
        .update_one(
            doc! { "_id": id },
            doc! { "$set": { "consumedAt": now_dt() } },
        )
        .await?;
    Ok(())
}

pub async fn delete_email_code(db: &Database, id: &str) -> Result<(), mongodb::error::Error> {
    email_codes(db).delete_one(doc! { "_id": id }).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

pub struct AuditEntry<'a> {
    pub actor_id: Option<ObjectId>,
    pub actor_name: &'a str,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<String>,
    pub result: &'a str,
    pub detail: Option<String>,
    pub ip: &'a str,
    pub user_agent: Option<String>,
}

pub async fn write_audit(db: &Database, entry: AuditEntry<'_>) {
    let log = AuditLog {
        id: ObjectId::new(),
        actor_id: entry.actor_id,
        actor_name: entry.actor_name.to_string(),
        action: entry.action.to_string(),
        target_type: entry.target_type.to_string(),
        target_id: entry.target_id,
        result: entry.result.to_string(),
        detail: entry.detail,
        ip: entry.ip.to_string(),
        user_agent: entry.user_agent,
        created_at: now_dt(),
    };
    if let Err(err) = audit_logs(db).insert_one(&log).await {
        tracing::error!(error = %err, "写审计日志失败");
    }
}

pub struct AuditFilter<'a> {
    pub actor: Option<&'a str>,
    pub action: Option<&'a str>,
    pub result: Option<&'a str>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

pub async fn list_audit(
    db: &Database,
    filter: AuditFilter<'_>,
    skip: u64,
    limit: u64,
) -> Result<(Vec<AuditLog>, u64), mongodb::error::Error> {
    let mut query = Document::new();
    if let Some(actor) = filter.actor.filter(|s| !s.trim().is_empty()) {
        query.insert("actorName", regex_contains(actor.trim()));
    }
    if let Some(action) = filter.action.filter(|s| !s.is_empty()) {
        query.insert("action", action);
    }
    if let Some(result) = filter.result.filter(|s| !s.is_empty()) {
        query.insert("result", result);
    }
    if filter.from_ms.is_some() || filter.to_ms.is_some() {
        let mut range = Document::new();
        if let Some(from) = filter.from_ms {
            range.insert("$gte", DateTime::from_millis(from));
        }
        if let Some(to) = filter.to_ms {
            range.insert("$lte", DateTime::from_millis(to));
        }
        query.insert("createdAt", range);
    }

    let total = audit_logs(db).count_documents(query.clone()).await?;
    let cursor = audit_logs(db)
        .find(query)
        .sort(doc! { "createdAt": -1 })
        .skip(skip)
        .limit(limit as i64)
        .await?;
    let list: Vec<AuditLog> = cursor.try_collect().await?;
    Ok((list, total))
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

pub async fn get_settings(db: &Database) -> Result<Option<Settings>, mongodb::error::Error> {
    settings(db).find_one(doc! { "_id": "default" }).await
}

pub async fn put_settings(
    db: &Database,
    value: &Settings,
) -> Result<(), mongodb::error::Error> {
    settings(db)
        .replace_one(doc! { "_id": "default" }, value)
        .upsert(true)
        .await?;
    Ok(())
}
