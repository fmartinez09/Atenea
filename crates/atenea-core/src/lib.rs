use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_BRANCH: &str = "main";
pub const APP_DATA_IDENTIFIER: &str = "app.atenea.desktop";
pub const CDC_MIN_SIZE: u32 = 16_384;   // 16 KB
pub const CDC_AVG_SIZE: u32 = 65_536;   // 64 KB
pub const CDC_MAX_SIZE: u32 = 262_144;  // 256 KB

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum AteneaError {
    #[error("Archivo no soportado por ningún adaptador registrado: {0}")]
    UnsupportedFormat(String),
    #[error("No se encontró el snapshot solicitado: {0}")]
    CommitNotFound(String),
    #[error("No se encontró la obra solicitada: {0}")]
    ProjectNotFound(String),
    #[error("El archivo destino está abierto o bloqueado por otra aplicación (cierra el lienzo en Clip Studio o restaura como copia): {0}")]
    TargetFileLocked(String),
    #[error("Fallo de verificación de integridad al restaurar (esperado {expected}, obtenido {actual})")]
    RestoreHashMismatch { expected: String, actual: String },
}

/// Abre un archivo en modo lectura permitiendo que otras aplicaciones en Windows
/// (como Clip Studio Paint haciendo write .tmp -> delete -> rename) puedan leer,
/// escribir o reemplazar el archivo sin errores de bloqueo (ERROR_SHARING_VIOLATION).
pub fn open_shared_read(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ (0x1) | FILE_SHARE_WRITE (0x2) | FILE_SHARE_DELETE (0x4)
        opts.share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004);
    }
    opts.open(path)
}

/// Comprueba si otro proceso mantiene algún handle abierto sobre `path`.
pub fn is_file_in_use(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .is_err()
    }
    #[cfg(not(windows))]
    {
        OpenOptions::new().read(true).write(true).open(path).is_err()
    }
}

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn normalize_path_string(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let raw = resolved.to_string_lossy().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
}

pub fn project_id_for_path(path: &Path) -> String {
    let norm = normalize_path_string(path).to_lowercase();
    blake3::hash(norm.as_bytes()).to_hex()[..24].to_string()
}

/// Directorio raíz seguro de datos de usuario fuera de `$INSTDIR` (`%LOCALAPPDATA%\Atenea`),
/// garantizando que los instaladores/actualizadores NSIS jamás borren ni sobrescriban el historial local.
pub fn resolve_app_data_root() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local).join(APP_DATA_IDENTIFIER);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local").join("share").join(APP_DATA_IDENTIFIER);
    }
    PathBuf::from(".atenea-data")
}

/// Resuelve la carpeta del almacén (`store`) y migra automáticamente (una sola vez y con verificación)
/// cualquier historial previo que residiera en la ubicación antigua `%LOCALAPPDATA%\Atenea\store`
/// (la cual colisionaba con `$INSTDIR` del instalador NSIS en Windows).
pub fn resolve_default_store_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("ATENEA_STORE") {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let local_path = PathBuf::from(&local);
        let safe_store = local_path.join(APP_DATA_IDENTIFIER).join("store");
        let legacy_store = local_path.join("Atenea").join("store");
        if let Err(err) = migrate_legacy_store_if_needed(&legacy_store, &safe_store) {
            eprintln!(
                "[Atenea] Aviso: fallo al migrar almacén heredado ({}); usando ubicación previa para no perder historial.",
                err
            );
            if legacy_store.join("index.db").exists() && !safe_store.join("index.db").exists() {
                return legacy_store;
            }
        }
        return safe_store;
    }
    resolve_app_data_root().join("store")
}

/// Migra de forma atómica y verificada un almacén desde `legacy_store` hacia `target_store`
/// sin eliminar los datos originales (los mueve a `backups/legacy-v0.1-store` fuera de `$INSTDIR`).
pub fn migrate_legacy_store_if_needed(legacy_store: &Path, target_store: &Path) -> Result<bool> {
    let legacy_db = legacy_store.join("index.db");
    let target_db = target_store.join("index.db");
    if target_db.exists() || !legacy_db.exists() {
        return Ok(false);
    }

    let target_parent = target_store
        .parent()
        .context("Ruta destino de almacén inválida")?;
    fs::create_dir_all(target_parent)?;

    let staging_dir = target_parent.join(format!("store.migrating.{}", std::process::id()));
    if staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir)?;

    // 1. Copiar blobs y previews direccionados por contenido
    let legacy_blobs = legacy_store.join("blobs");
    if legacy_blobs.exists() {
        copy_dir_recursive(&legacy_blobs, &staging_dir.join("blobs"))?;
    } else {
        fs::create_dir_all(staging_dir.join("blobs"))?;
    }

    let legacy_previews = legacy_store.join("previews");
    if legacy_previews.exists() {
        copy_dir_recursive(&legacy_previews, &staging_dir.join("previews"))?;
    } else {
        fs::create_dir_all(staging_dir.join("previews"))?;
    }

    // 2. Consolidar SQLite + WAL mediante VACUUM INTO y verificar conteos e integridad
    let staged_db = staging_dir.join("index.db");
    let (legacy_commits, legacy_blobs_cnt) = {
        let src_conn = Connection::open(&legacy_db)?;
        src_conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        let _ = src_conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        let c_cnt: i64 = src_conn
            .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
            .unwrap_or(0);
        let b_cnt: i64 = src_conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
            .unwrap_or(0);
        src_conn.execute(
            "VACUUM INTO ?1",
            params![staged_db.to_string_lossy().to_string()],
        )?;
        (c_cnt, b_cnt)
    };

    {
        let dst_conn = Connection::open(&staged_db)?;
        let quick_check: String =
            dst_conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if !quick_check.eq_ignore_ascii_case("ok") {
            let _ = fs::remove_dir_all(&staging_dir);
            bail!("Fallo de integridad en base migrada: {}", quick_check);
        }
        let dst_commits: i64 = dst_conn
            .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
            .unwrap_or(0);
        let dst_blobs: i64 = dst_conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))
            .unwrap_or(0);
        if dst_commits != legacy_commits || dst_blobs != legacy_blobs_cnt {
            let _ = fs::remove_dir_all(&staging_dir);
            bail!(
                "Conteo inconsistente tras migración (commits {} vs {}, blobs {} vs {})",
                legacy_commits,
                dst_commits,
                legacy_blobs_cnt,
                dst_blobs
            );
        }
    }

    // 3. Promover atómicamente el almacén migrado
    fs::rename(&staging_dir, target_store)?;

    // 4. Resguardar el almacén antiguo fuera de $INSTDIR para que una futura desinstalación no lo toque
    let backups_dir = target_parent.join("backups");
    let _ = fs::create_dir_all(&backups_dir);
    let archived_legacy = backups_dir.join(format!("legacy-store-{}", now_unix_ms()));
    if fs::rename(legacy_store, &archived_legacy).is_err() {
        // Si no se pudo mover (p. ej. otro proceso tenía abierto el directorio), renombrar in-situ
        let fallback = legacy_store.with_extension("migrated-backup");
        let _ = fs::rename(legacy_store, fallback);
    }

    Ok(true)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ============================================================================
// Costura #2: BlobStore Trait (LocalFs hoy, Cloudflare R2 en v0.2)
// ============================================================================

pub trait BlobStore: Send + Sync {
    fn has_blob(&self, hash: &str) -> Result<bool>;
    fn put_blob(&self, hash: &str, data: &[u8]) -> Result<bool>;
    fn get_blob(&self, hash: &str) -> Result<Vec<u8>>;
    fn delete_blob(&self, hash: &str) -> Result<bool>;
}

pub struct LocalFsBlobStore {
    blobs_dir: PathBuf,
}

impl LocalFsBlobStore {
    pub fn new(store_root: &Path) -> Result<Self> {
        let blobs_dir = store_root.join("blobs");
        fs::create_dir_all(&blobs_dir).context("No se pudo crear el directorio de blobs")?;
        Ok(Self { blobs_dir })
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        let p1 = &hash[0..2.min(hash.len())];
        let p2 = &hash[2..4.min(hash.len())];
        self.blobs_dir.join(p1).join(p2).join(hash)
    }
}

impl BlobStore for LocalFsBlobStore {
    fn has_blob(&self, hash: &str) -> Result<bool> {
        Ok(self.blob_path(hash).exists())
    }

    fn put_blob(&self, hash: &str, data: &[u8]) -> Result<bool> {
        let final_path = self.blob_path(hash);
        if final_path.exists() {
            return Ok(false);
        }
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = final_path.with_extension(format!("tmp.{}.{}", std::process::id(), seq));
        {
            let mut f = File::create(&tmp_path)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => Ok(true),
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                if final_path.exists() {
                    Ok(false)
                } else {
                    Err(err.into())
                }
            }
        }
    }

    fn get_blob(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.blob_path(hash);
        fs::read(&path).with_context(|| format!("No se pudo leer el blob {}", hash))
    }

    fn delete_blob(&self, hash: &str) -> Result<bool> {
        let path = self.blob_path(hash);
        if path.exists() {
            fs::remove_file(path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ============================================================================
// Costura #3: AssetAdapter Trait (.clip hoy, .blend / imágenes en v0.3)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayerSummary {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
    pub is_folder: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AssetMetadata {
    pub format: String,
    pub canvas_width: Option<u32>,
    pub canvas_height: Option<u32>,
    pub dpi: Option<f64>,
    pub layers: Vec<LayerSummary>,
}

#[derive(Debug, Clone)]
pub struct AssetInspection {
    pub preview_png: Option<Vec<u8>>,
    pub metadata: AssetMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntegrityStatus {
    Valid,
    Corrupted { reason: String },
}

pub trait AssetAdapter: Send + Sync {
    fn format_id(&self) -> &'static str;
    fn can_handle(&self, path: &Path) -> bool;
    fn check_integrity(&self, path: &Path) -> Result<IntegrityStatus>;
    fn inspect(&self, path: &Path) -> Result<AssetInspection>;
}

// ============================================================================
// Modelos de Dominio y Grafo de Snapshots (DAG)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedFolder {
    pub id: i64,
    pub path: String,
    pub added_at: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub head_commit_id: Option<String>,
    pub commit_count: u64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub file_path: String,
    pub display_name: String,
    pub format: String,
    pub active_branch: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub head_commit_id: Option<String>,
    pub latest_preview_path: Option<String>,
    pub snapshot_count: u64,
    pub branch_count: u64,
    pub latest_file_size: u64,
    pub total_logical_bytes: u64,
    pub total_stored_bytes: u64,
    pub last_integrity_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub branch_name: String,
    pub file_hash: String,
    pub file_size: u64,
    pub added_bytes: u64,
    pub chunk_count: u64,
    pub preview_hash: Option<String>,
    pub preview_path: Option<String>,
    pub metadata: AssetMetadata,
    pub label: Option<String>,
    pub pinned: bool,
    pub author_kind: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectGraph {
    pub project_id: String,
    pub active_branch: String,
    pub branches: Vec<BranchInfo>,
    pub commits: Vec<CommitRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveFolderReport {
    pub removed_projects: u64,
    pub freed_blobs: u64,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub project_count: u64,
    pub snapshot_count: u64,
    pub unique_blob_count: u64,
    pub total_logical_bytes: u64,
    pub total_physical_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirtyProjectStatus {
    pub project_id: String,
    pub display_name: String,
    pub file_path: String,
    pub is_locked_by_app: bool,
    pub has_unsnapshotted_disk_changes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreUpdateWorkspaceAudit {
    pub store_root: String,
    pub project_count: u64,
    pub snapshot_count: u64,
    pub total_physical_bytes: u64,
    pub dirty_or_locked_projects: Vec<DirtyProjectStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SnapshotOutcome {
    Created {
        commit: CommitRecord,
    },
    Unchanged {
        project_id: String,
        commit_id: String,
    },
    Corrupted {
        project_id: String,
        file_path: String,
        reason: String,
    },
    MutatedDuringRead {
        file_path: String,
    },
}

#[derive(Debug, Clone)]
struct ChunkEntry {
    offset: u64,
    length: u64,
    hash: String,
}

// ============================================================================
// Motor Principal: AteneaStore
// ============================================================================

pub struct AteneaStore {
    store_root: PathBuf,
    previews_dir: PathBuf,
    db_path: PathBuf,
    blob_store: Arc<dyn BlobStore>,
    adapters: Vec<Arc<dyn AssetAdapter>>,
}

impl AteneaStore {
    pub fn open(store_root: impl AsRef<Path>, adapters: Vec<Arc<dyn AssetAdapter>>) -> Result<Self> {
        let store_root = store_root.as_ref().to_path_buf();
        let previews_dir = store_root.join("previews");
        fs::create_dir_all(&previews_dir)?;
        let blob_store = Arc::new(LocalFsBlobStore::new(&store_root)?);
        let db_path = store_root.join("index.db");

        let store = Self {
            store_root,
            previews_dir,
            db_path,
            blob_store,
            adapters,
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    pub fn preview_file_path(&self, preview_hash: &str) -> PathBuf {
        self.previews_dir.join(format!("{}.png", preview_hash))
    }

    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS watched_folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT UNIQUE NOT NULL,
                added_at INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                file_path TEXT UNIQUE NOT NULL,
                display_name TEXT NOT NULL,
                format TEXT NOT NULL,
                active_branch TEXT NOT NULL DEFAULT 'main',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_integrity_error TEXT
            );

            CREATE TABLE IF NOT EXISTS branches (
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                head_commit_id TEXT,
                PRIMARY KEY (project_id, name),
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS blobs (
                hash TEXT PRIMARY KEY,
                size_bytes INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS commits (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                parent_id TEXT,
                branch_name TEXT NOT NULL,
                file_hash TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                added_bytes INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL,
                preview_hash TEXT,
                metadata_json TEXT NOT NULL,
                label TEXT,
                pinned INTEGER NOT NULL DEFAULT 0,
                author_kind TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS commit_chunks (
                commit_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                offset_bytes INTEGER NOT NULL,
                length_bytes INTEGER NOT NULL,
                blob_hash TEXT NOT NULL,
                PRIMARY KEY (commit_id, seq),
                FOREIGN KEY (commit_id) REFERENCES commits(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_commits_project_created
                ON commits(project_id, created_at DESC);
            ",
        )?;

        // Migraciones ligeras por si existía un index.db previo
        let _ = conn.execute(
            "ALTER TABLE projects ADD COLUMN active_branch TEXT NOT NULL DEFAULT 'main'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE commits ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            [],
        );

        Ok(())
    }

    pub fn find_adapter(&self, path: &Path) -> Option<&Arc<dyn AssetAdapter>> {
        self.adapters.iter().find(|a| a.can_handle(path))
    }

    // ------------------------------------------------------------------------
    // Gestión de carpetas vigiladas (Workspaces) + Purga y GC
    // ------------------------------------------------------------------------

    pub fn add_watched_folder(&self, folder: &Path) -> Result<WatchedFolder> {
        let normalized = normalize_path_string(folder);
        let now = now_unix_ms();
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO watched_folders (path, added_at, enabled)
             VALUES (?1, ?2, 1)
             ON CONFLICT(path) DO UPDATE SET enabled = 1",
            params![normalized, now],
        )?;
        let row = conn.query_row(
            "SELECT id, path, added_at, enabled FROM watched_folders WHERE path = ?1",
            params![normalized],
            |r| {
                Ok(WatchedFolder {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    added_at: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                })
            },
        )?;
        Ok(row)
    }

    pub fn toggle_watched_folder(&self, folder_id: i64, enabled: bool) -> Result<WatchedFolder> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE watched_folders SET enabled = ?2 WHERE id = ?1",
            params![folder_id, if enabled { 1i64 } else { 0i64 }],
        )?;
        let wf = conn.query_row(
            "SELECT id, path, added_at, enabled FROM watched_folders WHERE id = ?1",
            params![folder_id],
            |r| {
                Ok(WatchedFolder {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    added_at: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                })
            },
        )?;
        Ok(wf)
    }

    pub fn remove_watched_folder(&self, folder_id: i64) -> Result<RemoveFolderReport> {
        self.remove_watched_folder_with_options(folder_id, false)
    }

    pub fn remove_watched_folder_with_options(
        &self,
        folder_id: i64,
        purge_snapshots: bool,
    ) -> Result<RemoveFolderReport> {
        let mut conn = self.connect()?;
        let folder_path: Option<String> = conn
            .query_row(
                "SELECT path FROM watched_folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;

        conn.execute("DELETE FROM watched_folders WHERE id = ?1", params![folder_id])?;

        if !purge_snapshots {
            return Ok(RemoveFolderReport {
                removed_projects: 0,
                freed_blobs: 0,
                freed_bytes: 0,
            });
        }

        let mut removed_projects = 0u64;
        if let Some(root_path) = folder_path {
            let prefix_lower = ensure_trailing_sep(&root_path).to_lowercase();
            let root_lower = root_path.to_lowercase();

            let project_ids: Vec<String> = {
                let mut stmt = conn.prepare("SELECT id, file_path FROM projects")?;
                let rows = stmt.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                rows.flatten()
                    .filter(|(_, fp)| {
                        let fl = fp.to_lowercase();
                        fl == root_lower || fl.starts_with(&prefix_lower)
                    })
                    .map(|(id, _)| id)
                    .collect()
            };

            let tx = conn.transaction()?;
            for pid in &project_ids {
                tx.execute("DELETE FROM projects WHERE id = ?1", params![pid])?;
                removed_projects += 1;
            }
            tx.commit()?;
        }

        let (freed_blobs, freed_bytes) = self.gc_orphan_blobs()?;
        Ok(RemoveFolderReport {
            removed_projects,
            freed_blobs,
            freed_bytes,
        })
    }

    pub fn delete_project(&self, project_id: &str) -> Result<RemoveFolderReport> {
        let conn = self.connect()?;
        let deleted = conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
        drop(conn);
        let (freed_blobs, freed_bytes) = self.gc_orphan_blobs()?;
        Ok(RemoveFolderReport {
            removed_projects: deleted as u64,
            freed_blobs,
            freed_bytes,
        })
    }

    /// Elimina bloques físicos en `blobs/` y miniaturas en `previews/` que ya no estén
    /// referenciados por ningún snapshot activo.
    pub fn gc_orphan_blobs(&self) -> Result<(u64, u64)> {
        let mut conn = self.connect()?;
        let orphans: Vec<(String, u64)> = {
            let mut stmt = conn.prepare(
                "SELECT b.hash, b.size_bytes
                 FROM blobs b
                 WHERE NOT EXISTS (
                     SELECT 1 FROM commit_chunks cc WHERE cc.blob_hash = b.hash
                 )",
            )?;
            let mapped = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
            })?;
            mapped.flatten().collect()
        };

        let mut freed_blobs = 0u64;
        let mut freed_bytes = 0u64;
        let tx = conn.transaction()?;
        for (hash, sz) in &orphans {
            let _ = self.blob_store.delete_blob(hash);
            tx.execute("DELETE FROM blobs WHERE hash = ?1", params![hash])?;
            freed_blobs += 1;
            freed_bytes += *sz;
        }
        tx.commit()?;

        // Limpiar previews huérfanos
        let used_previews: std::collections::HashSet<String> = {
            let mut stmt =
                conn.prepare("SELECT DISTINCT preview_hash FROM commits WHERE preview_hash IS NOT NULL")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.flatten().collect()
        };
        if let Ok(entries) = fs::read_dir(&self.previews_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if !used_previews.contains(stem) {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }

        Ok((freed_blobs, freed_bytes))
    }

    pub fn list_watched_folders(&self) -> Result<Vec<WatchedFolder>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, path, added_at, enabled FROM watched_folders ORDER BY added_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WatchedFolder {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    added_at: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------------
    // Snapshot con FastCDC + BLAKE3 + Variación Activa
    // ------------------------------------------------------------------------

    /// Guarda un snapshot en la variación (`active_branch`) actualmente seleccionada en la obra.
    pub fn snapshot_file(
        &self,
        path: &Path,
        author_kind: &str,
        label: Option<&str>,
    ) -> Result<SnapshotOutcome> {
        let pid = project_id_for_path(path);
        let active_branch = self
            .get_project_active_branch(&pid)?
            .unwrap_or_else(|| DEFAULT_BRANCH.to_string());
        self.snapshot_file_on_branch(path, &active_branch, author_kind, label)
    }

    fn get_project_active_branch(&self, project_id: &str) -> Result<Option<String>> {
        let conn = self.connect()?;
        let branch: Option<String> = conn
            .query_row(
                "SELECT active_branch FROM projects WHERE id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(branch)
    }

    pub fn snapshot_file_on_branch(
        &self,
        path: &Path,
        branch_name: &str,
        author_kind: &str,
        label: Option<&str>,
    ) -> Result<SnapshotOutcome> {
        let adapter = self
            .find_adapter(path)
            .ok_or_else(|| AteneaError::UnsupportedFormat(path.display().to_string()))?;

        let norm_path = normalize_path_string(path);
        let project_id = project_id_for_path(path);
        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sin título")
            .to_string();
        let now = now_unix_ms();

        // 1. Verificar estabilidad inicial
        let meta_before = fs::metadata(path)
            .with_context(|| format!("No se pudo acceder a {}", norm_path))?;
        let size_before = meta_before.len();
        let mtime_before = meta_before.modified().ok();

        // 2. Compuerta de integridad
        match adapter.check_integrity(path)? {
            IntegrityStatus::Valid => {}
            IntegrityStatus::Corrupted { reason } => {
                let conn = self.connect()?;
                conn.execute(
                    "INSERT INTO projects (id, file_path, display_name, format, active_branch, created_at, updated_at, last_integrity_error)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET updated_at = ?6, last_integrity_error = ?7",
                    params![
                        project_id,
                        norm_path,
                        display_name,
                        adapter.format_id(),
                        branch_name,
                        now,
                        reason
                    ],
                )?;
                return Ok(SnapshotOutcome::Corrupted {
                    project_id,
                    file_path: norm_path,
                    reason,
                });
            }
        }

        // 3. Streaming FastCDC + BLAKE3 con handle compartido no bloqueante
        let file = open_shared_read(path)?;
        let chunker =
            fastcdc::v2020::StreamCDC::new(file, CDC_MIN_SIZE, CDC_AVG_SIZE, CDC_MAX_SIZE);

        let mut file_hasher = blake3::Hasher::new();
        let mut chunks: Vec<ChunkEntry> = Vec::new();
        let mut pending_blobs: HashMap<String, Vec<u8>> = HashMap::new();

        for item in chunker {
            let chunk = item.context("Error leyendo stream FastCDC")?;
            file_hasher.update(&chunk.data);
            let chunk_hash = blake3::hash(&chunk.data).to_hex().to_string();
            chunks.push(ChunkEntry {
                offset: chunk.offset,
                length: chunk.length as u64,
                hash: chunk_hash.clone(),
            });
            pending_blobs.entry(chunk_hash).or_insert(chunk.data);
        }

        // 4. Comprobar que Clip Studio no mutó el archivo mientras lo leíamos
        let meta_after = fs::metadata(path)?;
        if meta_after.len() != size_before || meta_after.modified().ok() != mtime_before {
            return Ok(SnapshotOutcome::MutatedDuringRead {
                file_path: norm_path,
            });
        }

        let file_hash = file_hasher.finalize().to_hex().to_string();

        // 5. Evitar commits duplicados respecto al HEAD de la variación actual
        let mut conn = self.connect()?;
        let current_head: Option<(String, String)> = conn
            .query_row(
                "SELECT c.id, c.file_hash
                 FROM branches b
                 JOIN commits c ON c.id = b.head_commit_id
                 WHERE b.project_id = ?1 AND b.name = ?2",
                params![project_id, branch_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        if let Some((head_id, head_file_hash)) = &current_head {
            if head_file_hash == &file_hash {
                conn.execute(
                    "UPDATE projects SET last_integrity_error = NULL, updated_at = ?2 WHERE id = ?1",
                    params![project_id, now],
                )?;
                return Ok(SnapshotOutcome::Unchanged {
                    project_id,
                    commit_id: head_id.clone(),
                });
            }
        }

        // 6. Extraer preview PNG y metadatos semánticos
        let inspection = adapter.inspect(path)?;
        let preview_hash = if let Some(png_bytes) = &inspection.preview_png {
            let phash = blake3::hash(png_bytes).to_hex()[..32].to_string();
            let preview_file = self.preview_file_path(&phash);
            if !preview_file.exists() {
                fs::write(&preview_file, png_bytes)?;
            }
            Some(phash)
        } else {
            None
        };

        // 7. Persistir chunks nuevos en el BlobStore
        let mut added_bytes: u64 = 0;
        for (hash, data) in &pending_blobs {
            if self.blob_store.put_blob(hash, data)? {
                added_bytes += data.len() as u64;
            }
        }

        let parent_id = current_head.map(|(id, _)| id);
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let commit_seed = format!(
            "{}:{}:{}:{}:{}:{}",
            project_id,
            branch_name,
            parent_id.as_deref().unwrap_or("root"),
            file_hash,
            now,
            nonce
        );
        let commit_id = blake3::hash(commit_seed.as_bytes()).to_hex()[..24].to_string();
        let metadata_json = serde_json::to_string(&inspection.metadata)?;

        // 8. Registrar en el grafo SQLite dentro de una transacción atómica
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO projects (id, file_path, display_name, format, active_branch, created_at, updated_at, last_integrity_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, NULL)
             ON CONFLICT(id) DO UPDATE SET
                file_path = excluded.file_path,
                display_name = excluded.display_name,
                active_branch = ?5,
                updated_at = excluded.updated_at,
                last_integrity_error = NULL",
            params![
                project_id,
                norm_path,
                display_name,
                adapter.format_id(),
                branch_name,
                now
            ],
        )?;

        for (hash, data) in &pending_blobs {
            tx.execute(
                "INSERT OR IGNORE INTO blobs (hash, size_bytes, created_at) VALUES (?1, ?2, ?3)",
                params![hash, data.len() as i64, now],
            )?;
        }

        tx.execute(
            "INSERT INTO commits (
                id, project_id, parent_id, branch_name, file_hash, file_size,
                added_bytes, chunk_count, preview_hash, metadata_json, label, pinned, author_kind, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?13)",
            params![
                commit_id,
                project_id,
                parent_id,
                branch_name,
                file_hash,
                size_before as i64,
                added_bytes as i64,
                chunks.len() as i64,
                preview_hash,
                metadata_json,
                label,
                author_kind,
                now
            ],
        )?;

        {
            let mut chunk_stmt = tx.prepare(
                "INSERT INTO commit_chunks (commit_id, seq, offset_bytes, length_bytes, blob_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (seq, ch) in chunks.iter().enumerate() {
                chunk_stmt.execute(params![
                    commit_id,
                    seq as i64,
                    ch.offset as i64,
                    ch.length as i64,
                    ch.hash
                ])?;
            }
        }

        tx.execute(
            "INSERT INTO branches (project_id, name, head_commit_id)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id, name) DO UPDATE SET head_commit_id = excluded.head_commit_id",
            params![project_id, branch_name, commit_id],
        )?;

        tx.commit()?;

        let commit = self.get_commit(&commit_id)?;
        Ok(SnapshotOutcome::Created { commit })
    }

    // ------------------------------------------------------------------------
    // Variaciones (Ramas visuales sin jerga Git) e Hitos Fijados
    // ------------------------------------------------------------------------

    /// Crea una nueva variación apuntando a `from_commit_id` y la marca como activa.
    /// Si `checkout_to_disk` es true, restaura ese snapshot en el archivo `.clip` (verificando que no esté en uso).
    pub fn create_variation(
        &self,
        project_id: &str,
        raw_name: &str,
        from_commit_id: &str,
        checkout_to_disk: bool,
    ) -> Result<BranchInfo> {
        let clean_name = sanitize_branch_name(raw_name)?;
        let target_commit = self.get_commit(from_commit_id)?;
        if target_commit.project_id != project_id {
            bail!("El snapshot elegido no pertenece a esta obra.");
        }

        let mut conn = self.connect()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM branches WHERE project_id = ?1 AND LOWER(name) = LOWER(?2)",
                params![project_id, clean_name],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            bail!("Ya existe una variación llamada '{}'.", clean_name);
        }

        if checkout_to_disk {
            let file_path_str: String = conn
                .query_row(
                    "SELECT file_path FROM projects WHERE id = ?1",
                    params![project_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| AteneaError::ProjectNotFound(project_id.to_string()))?;
            let target_path = PathBuf::from(&file_path_str);
            if is_file_in_use(&target_path) {
                return Err(AteneaError::TargetFileLocked(file_path_str).into());
            }
            if target_path.exists() {
                let _ = self.snapshot_file(
                    &target_path,
                    "pre_restore_backup",
                    Some("Respaldo automático antes de cambiar de variación"),
                );
            }
            self.restore_to_copy(from_commit_id, &target_path)?;
        }

        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO branches (project_id, name, head_commit_id) VALUES (?1, ?2, ?3)",
            params![project_id, clean_name, from_commit_id],
        )?;
        tx.execute(
            "UPDATE projects SET active_branch = ?2, updated_at = ?3 WHERE id = ?1",
            params![project_id, clean_name, now_unix_ms()],
        )?;
        tx.commit()?;

        Ok(BranchInfo {
            name: clean_name,
            head_commit_id: Some(from_commit_id.to_string()),
            commit_count: 0,
            is_active: true,
        })
    }

    /// Cambia la variación activa de la obra y opcionalmente restaura su último snapshot al archivo en disco.
    pub fn switch_variation(
        &self,
        project_id: &str,
        branch_name: &str,
        checkout_to_disk: bool,
    ) -> Result<()> {
        let conn = self.connect()?;
        let head_id: Option<String> = conn
            .query_row(
                "SELECT head_commit_id FROM branches WHERE project_id = ?1 AND name = ?2",
                params![project_id, branch_name],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        let file_path_str: String = conn
            .query_row(
                "SELECT file_path FROM projects WHERE id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| AteneaError::ProjectNotFound(project_id.to_string()))?;
        drop(conn);

        if checkout_to_disk {
            if let Some(ref commit_id) = head_id {
                let target_path = PathBuf::from(&file_path_str);
                if is_file_in_use(&target_path) {
                    return Err(AteneaError::TargetFileLocked(file_path_str).into());
                }
                if target_path.exists() {
                    let _ = self.snapshot_file(
                        &target_path,
                        "pre_restore_backup",
                        Some("Respaldo automático antes de cambiar de variación"),
                    );
                }
                self.restore_to_copy(commit_id, &target_path)?;
            }
        }

        let conn2 = self.connect()?;
        conn2.execute(
            "UPDATE projects SET active_branch = ?2, updated_at = ?3 WHERE id = ?1",
            params![project_id, branch_name, now_unix_ms()],
        )?;
        Ok(())
    }

    pub fn toggle_commit_pin(&self, commit_id: &str, pinned: bool) -> Result<CommitRecord> {
        let conn = self.connect()?;
        let updated = conn.execute(
            "UPDATE commits SET pinned = ?2 WHERE id = ?1",
            params![commit_id, if pinned { 1i64 } else { 0i64 }],
        )?;
        if updated == 0 {
            return Err(AteneaError::CommitNotFound(commit_id.to_string()).into());
        }
        self.get_commit(commit_id)
    }

    // ------------------------------------------------------------------------
    // Consultas del Grafo Completo (estilo GitKraken) y Proyectos
    // ------------------------------------------------------------------------

    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT
                p.id,
                p.file_path,
                p.display_name,
                p.format,
                p.active_branch,
                p.created_at,
                p.updated_at,
                b.head_commit_id,
                hc.preview_hash,
                COALESCE(hc.file_size, 0),
                (SELECT COUNT(*) FROM commits c WHERE c.project_id = p.id),
                (SELECT COUNT(*) FROM branches br WHERE br.project_id = p.id),
                (SELECT COALESCE(SUM(c.file_size), 0) FROM commits c WHERE c.project_id = p.id),
                (SELECT COALESCE(SUM(c.added_bytes), 0) FROM commits c WHERE c.project_id = p.id),
                p.last_integrity_error
             FROM projects p
             LEFT JOIN branches b ON b.project_id = p.id AND b.name = p.active_branch
             LEFT JOIN commits hc ON hc.id = b.head_commit_id
             ORDER BY p.updated_at DESC",
        )?;

        let items = stmt
            .query_map([], |r| {
                let preview_hash: Option<String> = r.get(8)?;
                let latest_preview_path = preview_hash
                    .as_deref()
                    .map(|h| self.preview_file_path(h).to_string_lossy().to_string());
                Ok(ProjectSummary {
                    id: r.get(0)?,
                    file_path: r.get(1)?,
                    display_name: r.get(2)?,
                    format: r.get(3)?,
                    active_branch: r.get(4)?,
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                    head_commit_id: r.get(7)?,
                    latest_preview_path,
                    latest_file_size: r.get::<_, i64>(9)? as u64,
                    snapshot_count: r.get::<_, i64>(10)? as u64,
                    branch_count: r.get::<_, i64>(11)? as u64,
                    total_logical_bytes: r.get::<_, i64>(12)? as u64,
                    total_stored_bytes: r.get::<_, i64>(13)? as u64,
                    last_integrity_error: r.get(14)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(items)
    }

    pub fn get_project_graph(&self, project_id: &str) -> Result<ProjectGraph> {
        let conn = self.connect()?;
        let active_branch: String = conn
            .query_row(
                "SELECT active_branch FROM projects WHERE id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| DEFAULT_BRANCH.to_string());

        let mut b_stmt = conn.prepare(
            "SELECT
                b.name,
                b.head_commit_id,
                (SELECT COUNT(*) FROM commits c WHERE c.project_id = b.project_id AND c.branch_name = b.name)
             FROM branches b
             WHERE b.project_id = ?1
             ORDER BY CASE WHEN b.name = 'main' THEN 0 ELSE 1 END, b.name ASC",
        )?;
        let branches = b_stmt
            .query_map(params![project_id], |r| {
                let name: String = r.get(0)?;
                Ok(BranchInfo {
                    is_active: name == active_branch,
                    name,
                    head_commit_id: r.get(1)?,
                    commit_count: r.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut c_stmt = conn.prepare(
            "SELECT
                id, project_id, parent_id, branch_name, file_hash, file_size,
                added_bytes, chunk_count, preview_hash, metadata_json, label, pinned, author_kind, created_at
             FROM commits
             WHERE project_id = ?1
             ORDER BY created_at DESC, rowid DESC",
        )?;
        let commits = c_stmt
            .query_map(params![project_id], |r| self.row_to_commit(r))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(ProjectGraph {
            project_id: project_id.to_string(),
            active_branch,
            branches,
            commits,
        })
    }

    pub fn list_commits(&self, project_id: &str, branch_name: &str) -> Result<Vec<CommitRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT
                id, project_id, parent_id, branch_name, file_hash, file_size,
                added_bytes, chunk_count, preview_hash, metadata_json, label, pinned, author_kind, created_at
             FROM commits
             WHERE project_id = ?1 AND branch_name = ?2
             ORDER BY created_at DESC, rowid DESC",
        )?;

        let rows = stmt
            .query_map(params![project_id, branch_name], |r| self.row_to_commit(r))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_commit(&self, commit_id: &str) -> Result<CommitRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT
                id, project_id, parent_id, branch_name, file_hash, file_size,
                added_bytes, chunk_count, preview_hash, metadata_json, label, pinned, author_kind, created_at
             FROM commits
             WHERE id = ?1",
            params![commit_id],
            |r| self.row_to_commit(r),
        )
        .optional()?
        .ok_or_else(|| AteneaError::CommitNotFound(commit_id.to_string()).into())
    }

    pub fn update_commit_label(&self, commit_id: &str, label: Option<&str>) -> Result<CommitRecord> {
        let conn = self.connect()?;
        let clean = label.map(str::trim).filter(|s| !s.is_empty());
        let updated = conn.execute(
            "UPDATE commits SET label = ?2 WHERE id = ?1",
            params![commit_id, clean],
        )?;
        if updated == 0 {
            return Err(AteneaError::CommitNotFound(commit_id.to_string()).into());
        }
        self.get_commit(commit_id)
    }

    pub fn storage_stats(&self) -> Result<StorageStats> {
        let conn = self.connect()?;
        let project_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        let snapshot_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))?;
        let unique_blob_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))?;
        let total_logical_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM commits",
            [],
            |r| r.get(0),
        )?;
        let total_physical_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM blobs",
            [],
            |r| r.get(0),
        )?;

        Ok(StorageStats {
            project_count: project_count as u64,
            snapshot_count: snapshot_count as u64,
            unique_blob_count: unique_blob_count as u64,
            total_logical_bytes: total_logical_bytes as u64,
            total_physical_bytes: total_physical_bytes as u64,
        })
    }

    fn row_to_commit(&self, r: &rusqlite::Row<'_>) -> rusqlite::Result<CommitRecord> {
        let preview_hash: Option<String> = r.get(8)?;
        let preview_path = preview_hash
            .as_deref()
            .map(|h| self.preview_file_path(h).to_string_lossy().to_string());
        let meta_str: String = r.get(9)?;
        let metadata: AssetMetadata = serde_json::from_str(&meta_str).unwrap_or_default();

        Ok(CommitRecord {
            id: r.get(0)?,
            project_id: r.get(1)?,
            parent_id: r.get(2)?,
            branch_name: r.get(3)?,
            file_hash: r.get(4)?,
            file_size: r.get::<_, i64>(5)? as u64,
            added_bytes: r.get::<_, i64>(6)? as u64,
            chunk_count: r.get::<_, i64>(7)? as u64,
            preview_hash,
            preview_path,
            metadata,
            label: r.get(10)?,
            pinned: r.get::<_, i64>(11)? != 0,
            author_kind: r.get(12)?,
            created_at: r.get(13)?,
        })
    }

    // ------------------------------------------------------------------------
    // Restauración Segura Verificada bit a bit
    // ------------------------------------------------------------------------

    pub fn restore_to_copy(&self, commit_id: &str, destination: &Path) -> Result<PathBuf> {
        let commit = self.get_commit(commit_id)?;
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT blob_hash FROM commit_chunks WHERE commit_id = ?1 ORDER BY seq ASC",
        )?;
        let chunk_hashes = stmt
            .query_map(params![commit_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }

        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_dest = destination.with_extension(format!(
            "atenea_restore.{}.{}.tmp",
            std::process::id(),
            seq
        ));

        let mut hasher = blake3::Hasher::new();
        {
            let mut out = File::create(&tmp_dest)?;
            for ch_hash in &chunk_hashes {
                let bytes = self.blob_store.get_blob(ch_hash)?;
                hasher.update(&bytes);
                out.write_all(&bytes)?;
            }
            out.sync_all()?;
        }

        let actual_hash = hasher.finalize().to_hex().to_string();
        if actual_hash != commit.file_hash {
            let _ = fs::remove_file(&tmp_dest);
            return Err(AteneaError::RestoreHashMismatch {
                expected: commit.file_hash,
                actual: actual_hash,
            }
            .into());
        }

        fs::rename(&tmp_dest, destination)?;
        Ok(destination.to_path_buf())
    }

    pub fn restore_in_place(&self, commit_id: &str) -> Result<PathBuf> {
        let commit = self.get_commit(commit_id)?;
        let conn = self.connect()?;
        let file_path_str: String = conn.query_row(
            "SELECT file_path FROM projects WHERE id = ?1",
            params![commit.project_id],
            |r| r.get(0),
        )?;
        drop(conn);

        let target = PathBuf::from(&file_path_str);
        if is_file_in_use(&target) {
            return Err(AteneaError::TargetFileLocked(file_path_str).into());
        }

        if target.exists() {
            let _ = self.snapshot_file(
                &target,
                "pre_restore_backup",
                Some("Respaldo automático previo a restaurar"),
            );
        }

        self.restore_to_copy(commit_id, &target)
    }

    pub fn hash_file_blake3(path: &Path) -> Result<String> {
        let mut f = open_shared_read(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = [0u8; 65_536];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    // ------------------------------------------------------------------------
    // Protección Pre-Actualización (Auditoría de Guardado + Respaldo SQLite)
    // ------------------------------------------------------------------------

    /// Audita todas las obras registradas para detectar archivos `.clip` bloqueados/abiertos
    /// en Clip Studio Paint o con cambios guardados en disco que aún no tienen snapshot.
    pub fn inspect_workspace_for_update(&self) -> Result<PreUpdateWorkspaceAudit> {
        let stats = self.storage_stats()?;
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT
                p.id,
                p.display_name,
                p.file_path,
                hc.file_hash,
                hc.file_size
             FROM projects p
             LEFT JOIN branches b ON b.project_id = p.id AND b.name = p.active_branch
             LEFT JOIN commits hc ON hc.id = b.head_commit_id
             ORDER BY p.updated_at DESC",
        )?;

        let rows: Vec<(String, String, String, Option<String>, Option<u64>)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut dirty_or_locked_projects = Vec::new();
        for (pid, display_name, file_path, head_hash, head_size) in rows {
            let path = PathBuf::from(&file_path);
            if !path.exists() {
                continue;
            }
            let is_locked = is_file_in_use(&path);
            let has_unsnapshotted_disk_changes = match (head_hash, head_size) {
                (Some(expected_hash), Some(expected_size)) => {
                    match fs::metadata(&path) {
                        Ok(meta) if meta.len() != expected_size => true,
                        Ok(_) => Self::hash_file_blake3(&path)
                            .map(|actual| actual != expected_hash)
                            .unwrap_or(false),
                        Err(_) => false,
                    }
                }
                _ => true,
            };

            if is_locked || has_unsnapshotted_disk_changes {
                dirty_or_locked_projects.push(DirtyProjectStatus {
                    project_id: pid,
                    display_name,
                    file_path,
                    is_locked_by_app: is_locked,
                    has_unsnapshotted_disk_changes,
                });
            }
        }

        Ok(PreUpdateWorkspaceAudit {
            store_root: self.store_root.to_string_lossy().to_string(),
            project_count: stats.project_count,
            snapshot_count: stats.snapshot_count,
            total_physical_bytes: stats.total_physical_bytes,
            dirty_or_locked_projects,
        })
    }

    /// Escanea las carpetas vigiladas activas y las obras registradas para capturar
    /// cualquier cambio válido pendiente en disco antes de cerrar por actualización.
    pub fn capture_all_pending_snapshots(&self, label: Option<&str>) -> Result<u64> {
        let mut candidate_files: Vec<PathBuf> = Vec::new();

        for proj in self.list_projects()? {
            let p = PathBuf::from(&proj.file_path);
            if p.exists() && self.find_adapter(&p).is_some() {
                candidate_files.push(p);
            }
        }

        for wf in self.list_watched_folders()?.into_iter().filter(|w| w.enabled) {
            let root = PathBuf::from(&wf.path);
            if root.exists() {
                self.collect_adapter_files_recursive(&root, &mut candidate_files);
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut created_count = 0u64;
        for file_path in candidate_files {
            let key = normalize_path_string(&file_path).to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            if let Ok(SnapshotOutcome::Created { .. }) =
                self.snapshot_file(&file_path, "pre_update", label)
            {
                created_count += 1;
            }
        }

        Ok(created_count)
    }

    fn collect_adapter_files_recursive(&self, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if lower.starts_with('.')
                || lower.starts_with("~$")
                || lower.starts_with('~')
                || lower.ends_with(".tmp")
                || lower.contains(".atenea_restore.")
            {
                continue;
            }
            if p.is_dir() {
                self.collect_adapter_files_recursive(&p, out);
            } else if self.find_adapter(&p).is_some() {
                out.push(p);
            }
        }
    }

    /// Crea un respaldo consistente, compactado y verificado de `index.db` antes de instalar una nueva versión,
    /// conservando intactos todos los `blobs/` y `previews/` y manteniendo los últimos 5 respaldos.
    pub fn create_pre_update_backup(
        &self,
        current_version: &str,
        target_version: &str,
    ) -> Result<PathBuf> {
        let backups_dir = self
            .store_root
            .parent()
            .unwrap_or(&self.store_root)
            .join("backups");
        fs::create_dir_all(&backups_dir)?;

        let sanitize_ver = |v: &str| -> String {
            v.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
                .collect()
        };
        let backup_name = format!(
            "index-pre-{}-to-{}-{}.db",
            sanitize_ver(current_version),
            sanitize_ver(target_version),
            now_unix_ms()
        );
        let backup_path = backups_dir.join(backup_name);

        {
            let conn = self.connect()?;
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            conn.execute(
                "VACUUM INTO ?1",
                params![backup_path.to_string_lossy().to_string()],
            )
            .context("No se pudo generar el respaldo previo a la actualización")?;
        }

        // Verificar que el respaldo sea legible e íntegro
        {
            let check_conn = Connection::open(&backup_path)?;
            let status: String =
                check_conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            if !status.eq_ignore_ascii_case("ok") {
                bail!("El respaldo pre-actualización falló quick_check: {}", status);
            }
        }

        // Rotar respaldos antiguos de index-pre-*.db dejando siempre los 5 más recientes
        if let Ok(entries) = fs::read_dir(&backups_dir) {
            let mut db_backups: Vec<(std::time::SystemTime, PathBuf)> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let name = p.file_name()?.to_str()?;
                    if name.starts_with("index-pre-") && name.ends_with(".db") {
                        let mtime = e
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(UNIX_EPOCH);
                        Some((mtime, p))
                    } else {
                        None
                    }
                })
                .collect();
            db_backups.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, old_path) in db_backups.into_iter().skip(5) {
                let _ = fs::remove_file(old_path);
            }
        }

        Ok(backup_path)
    }
}

fn sanitize_branch_name(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("El nombre de la variación no puede estar vacío.");
    }
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        bail!("Nombre de variación inválido.");
    }
    Ok(cleaned)
}

fn ensure_trailing_sep(path: &str) -> String {
    if path.ends_with('\\') || path.ends_with('/') {
        path.to_string()
    } else if path.contains('\\') {
        format!("{}\\", path)
    } else {
        format!("{}/", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct DummyRawAdapter;
    impl AssetAdapter for DummyRawAdapter {
        fn format_id(&self) -> &'static str {
            "clip"
        }
        fn can_handle(&self, path: &Path) -> bool {
            path.extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("clip"))
                .unwrap_or(false)
        }
        fn check_integrity(&self, _path: &Path) -> Result<IntegrityStatus> {
            Ok(IntegrityStatus::Valid)
        }
        fn inspect(&self, _path: &Path) -> Result<AssetInspection> {
            Ok(AssetInspection {
                preview_png: Some(vec![137, 80, 78, 71, 13, 10, 26, 10]),
                metadata: AssetMetadata {
                    format: "clip".into(),
                    canvas_width: Some(1920),
                    canvas_height: Some(1080),
                    dpi: Some(300.0),
                    layers: vec![],
                },
            })
        }
    }

    #[test]
    fn migrates_legacy_instdir_store_preserving_all_commits_and_blobs() -> Result<()> {
        let root = tempdir()?;
        let legacy_store = root.path().join("Atenea").join("store");
        let safe_store = root.path().join(APP_DATA_IDENTIFIER).join("store");
        let art_dir = root.path().join("arte");
        fs::create_dir_all(&art_dir)?;

        let clip_file = art_dir.join("obra_sin_nube.clip");
        let original_payload = vec![42u8; 150_000];
        fs::write(&clip_file, &original_payload)?;

        let c1_id = {
            let store = AteneaStore::open(&legacy_store, vec![Arc::new(DummyRawAdapter)])?;
            store.add_watched_folder(&art_dir)?;
            match store.snapshot_file(&clip_file, "manual", Some("Antes de actualizar"))? {
                SnapshotOutcome::Created { commit } => commit.id,
                other => panic!("Resultado inesperado: {:?}", other),
            }
        };

        let migrated = migrate_legacy_store_if_needed(&legacy_store, &safe_store)?;
        assert!(migrated, "Debió migrar el almacén heredado fuera de $INSTDIR");
        assert!(safe_store.join("index.db").exists());
        assert!(
            !legacy_store.exists(),
            "El almacén antiguo debió moverse a backups para evitar colisiones"
        );

        let reopened = AteneaStore::open(&safe_store, vec![Arc::new(DummyRawAdapter)])?;
        let stats = reopened.storage_stats()?;
        assert_eq!(stats.project_count, 1);
        assert_eq!(stats.snapshot_count, 1);

        let restored = root.path().join("restaurado_post_migracion.clip");
        reopened.restore_to_copy(&c1_id, &restored)?;
        assert_eq!(fs::read(&restored)?, original_payload);

        Ok(())
    }

    #[test]
    fn pre_update_audit_detects_unsaved_disk_changes_and_creates_verified_backup() -> Result<()> {
        let root = tempdir()?;
        let store_dir = root.path().join("store");
        let art_dir = root.path().join("dibujos");
        fs::create_dir_all(&art_dir)?;

        let store = AteneaStore::open(&store_dir, vec![Arc::new(DummyRawAdapter)])?;
        store.add_watched_folder(&art_dir)?;

        let clip_file = art_dir.join("ilustracion.clip");
        fs::write(&clip_file, vec![7u8; 90_000])?;
        store.snapshot_file(&clip_file, "autosave", Some("v1"))?;

        // Sin cambios posteriores en disco -> limpio
        let clean_audit = store.inspect_workspace_for_update()?;
        assert!(clean_audit.dirty_or_locked_projects.is_empty());

        // Simular que el artista guardó cambios nuevos justo antes de actualizar
        fs::write(&clip_file, vec![9u8; 95_000])?;
        let dirty_audit = store.inspect_workspace_for_update()?;
        assert_eq!(dirty_audit.dirty_or_locked_projects.len(), 1);
        assert!(dirty_audit.dirty_or_locked_projects[0].has_unsnapshotted_disk_changes);

        // Capturar pendientes pre-actualización y crear respaldo de index.db
        let captured = store.capture_all_pending_snapshots(Some("Respaldo previo a actualización"))?;
        assert_eq!(captured, 1);
        assert_eq!(store.storage_stats()?.snapshot_count, 2);

        let backup_db = store.create_pre_update_backup("0.1.0", "0.2.0")?;
        assert!(backup_db.exists());

        Ok(())
    }
}
