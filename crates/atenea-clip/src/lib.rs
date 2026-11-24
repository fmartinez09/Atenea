use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use atenea_core::{
    open_shared_read, AssetAdapter, AssetInspection, AssetMetadata, IntegrityStatus, LayerSummary,
};
use rusqlite::{Connection, OpenFlags};

pub const CLIP_MAGIC: &[u8; 8] = b"CSFCHUNK";
pub const CHUNK_HEAD: &[u8; 8] = b"CHNKHead";
pub const CHUNK_EXTA: &[u8; 8] = b"CHNKExta";
pub const CHUNK_SQLI: &[u8; 8] = b"CHNKSQLi";
pub const CHUNK_FOOT: &[u8; 8] = b"CHNKFoot";
pub const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

static SQLITE_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct ClipContainerInfo {
    pub file_size: u64,
    pub exta_chunk_count: u32,
    pub sqlite_offset: u64,
    pub sqlite_len: u64,
    pub has_footer: bool,
}

struct TempSqliteGuard {
    path: PathBuf,
}

impl TempSqliteGuard {
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let seq = SQLITE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "atenea_clip_sqli_{}_{}.sqlite",
            std::process::id(),
            seq
        ));
        let mut f = File::create(&path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(Self { path })
    }
}

impl Drop for TempSqliteGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct ClipAdapter;

impl ClipAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Analiza el contenedor `.clip` (`CSFCHUNK`) sin cargar los bloques raster pesados en memoria.
    pub fn parse_container(path: &Path) -> Result<ClipContainerInfo> {
        let mut file = open_shared_read(path)?;
        let file_size = file.metadata()?.len();
        if file_size < 40 {
            bail!("Archivo demasiado pequeño ({} bytes) para ser un .clip válido", file_size);
        }

        let mut envelope = [0u8; 24];
        file.read_exact(&mut envelope)?;
        if &envelope[0..8] != CLIP_MAGIC {
            bail!("Cabecera inválida: no comienza con CSFCHUNK");
        }

        // Comprobar presencia de CHNKFoot cerca del final del archivo (protección contra truncamiento)
        let tail_len = 64u64.min(file_size);
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
        let mut tail = vec![0u8; tail_len as usize];
        file.read_exact(&mut tail)?;
        let has_footer = tail.windows(8).any(|w| w == CHUNK_FOOT);
        if !has_footer {
            bail!("Falta el bloque de cierre CHNKFoot (guardado incompleto o truncado)");
        }

        let raw_first_offset = u64::from_be_bytes(envelope[16..24].try_into().unwrap());
        let first_offset = if (24..file_size).contains(&raw_first_offset) {
            raw_first_offset
        } else {
            24
        };

        let mut pos = first_offset;
        let mut exta_count = 0u32;
        let mut sqlite_region: Option<(u64, u64)> = None;

        while pos + 16 <= file_size {
            file.seek(SeekFrom::Start(pos))?;
            let mut hdr = [0u8; 16];
            if file.read_exact(&mut hdr).is_err() {
                break;
            }
            let tag = &hdr[0..8];
            let chunk_len = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
            let payload_start = pos + 16;

            if tag == CHUNK_FOOT {
                break;
            }
            if payload_start.checked_add(chunk_len).map_or(true, |end| end > file_size) {
                break;
            }

            if tag == CHUNK_EXTA {
                exta_count += 1;
            } else if tag == CHUNK_SQLI {
                sqlite_region = Some((payload_start, chunk_len));
                break;
            } else if tag != CHUNK_HEAD && !tag.starts_with(b"CHNK") {
                break;
            }

            pos = payload_start + chunk_len;
        }

        // Fallback de respaldo: escanear el tramo final buscando CHNKSQLi + cabecera SQLite3
        if sqlite_region.is_none() {
            sqlite_region = Self::scan_tail_for_sqlite(&mut file, file_size)?;
        }

        let (sqlite_offset, sqlite_len) = sqlite_region
            .ok_or_else(|| anyhow::anyhow!("No se encontró el bloque interno CHNKSQLi"))?;

        if sqlite_len < 100 || sqlite_offset + sqlite_len > file_size {
            bail!("Tamaño inválido en bloque CHNKSQLi ({} bytes)", sqlite_len);
        }

        file.seek(SeekFrom::Start(sqlite_offset))?;
        let mut sql_hdr = [0u8; 16];
        file.read_exact(&mut sql_hdr)?;
        if &sql_hdr != SQLITE_MAGIC {
            bail!("El bloque CHNKSQLi no contiene una cabecera SQLite 3 válida");
        }

        Ok(ClipContainerInfo {
            file_size,
            exta_chunk_count: exta_count,
            sqlite_offset,
            sqlite_len,
            has_footer,
        })
    }

    fn scan_tail_for_sqlite(file: &mut File, file_size: u64) -> Result<Option<(u64, u64)>> {
        // CHNKSQLi siempre se ubica al final del contenedor, justo antes de CHNKFoot
        let window = (32 * 1024 * 1024u64).min(file_size);
        let start = file_size - window;
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; window as usize];
        file.read_exact(&mut buf)?;

        for i in (0..buf.len().saturating_sub(32)).rev() {
            if &buf[i..i + 8] == CHUNK_SQLI && &buf[i + 16..i + 32] == SQLITE_MAGIC {
                let len = u64::from_be_bytes(buf[i + 8..i + 16].try_into().unwrap());
                return Ok(Some((start + i as u64 + 16, len)));
            }
        }
        Ok(None)
    }

    fn extract_sqlite_bytes(path: &Path, info: &ClipContainerInfo) -> Result<Vec<u8>> {
        let mut file = open_shared_read(path)?;
        file.seek(SeekFrom::Start(info.sqlite_offset))?;
        let mut buf = vec![0u8; info.sqlite_len as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn table_columns(conn: &Connection, table: &str) -> HashSet<String> {
        let query = format!("PRAGMA table_info({})", table);
        let Ok(mut stmt) = conn.prepare(&query) else {
            return HashSet::new();
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
            return HashSet::new();
        };
        rows.flatten().collect()
    }
}

impl Default for ClipAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetAdapter for ClipAdapter {
    fn format_id(&self) -> &'static str {
        "clip"
    }

    fn can_handle(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("clip"))
            .unwrap_or(false)
    }

    fn check_integrity(&self, path: &Path) -> Result<IntegrityStatus> {
        let info = match Self::parse_container(path) {
            Ok(i) => i,
            Err(err) => {
                return Ok(IntegrityStatus::Corrupted {
                    reason: err.to_string(),
                });
            }
        };

        let sqlite_bytes = match Self::extract_sqlite_bytes(path, &info) {
            Ok(b) => b,
            Err(err) => {
                return Ok(IntegrityStatus::Corrupted {
                    reason: format!("No se pudo extraer CHNKSQLi: {}", err),
                });
            }
        };

        let guard = TempSqliteGuard::from_bytes(&sqlite_bytes)?;
        let conn = match Connection::open_with_flags(
            &guard.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(err) => {
                return Ok(IntegrityStatus::Corrupted {
                    reason: format!("SQLite embebido ilegible: {}", err),
                });
            }
        };

        let quick_check: Result<String, _> =
            conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0));
        match quick_check {
            Ok(status) if status.eq_ignore_ascii_case("ok") => Ok(IntegrityStatus::Valid),
            Ok(status) => Ok(IntegrityStatus::Corrupted {
                reason: format!("Fallo en PRAGMA quick_check del archivo .clip: {}", status),
            }),
            Err(err) => Ok(IntegrityStatus::Corrupted {
                reason: format!("Base SQLite interna corrupta: {}", err),
            }),
        }
    }

    fn inspect(&self, path: &Path) -> Result<AssetInspection> {
        let info = Self::parse_container(path)?;
        let sqlite_bytes = Self::extract_sqlite_bytes(path, &info)
            .context("Error leyendo bloque CHNKSQLi")?;
        let guard = TempSqliteGuard::from_bytes(&sqlite_bytes)?;

        let conn = Connection::open_with_flags(
            &guard.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // 1. Extraer PNG desde CanvasPreview.ImageData
        let preview_cols = Self::table_columns(&conn, "CanvasPreview");
        let preview_png = if preview_cols.contains("ImageData") {
            conn.query_row("SELECT ImageData FROM CanvasPreview LIMIT 1", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .ok()
            .filter(|bytes| !bytes.is_empty())
        } else {
            None
        };

        // 2. Extraer dimensiones de Canvas si existen
        let canvas_cols = Self::table_columns(&conn, "Canvas");
        let mut canvas_width = None;
        let mut canvas_height = None;
        let mut dpi = None;

        if canvas_cols.contains("CanvasWidth") && canvas_cols.contains("CanvasHeight") {
            let dpi_col = if canvas_cols.contains("CanvasResolution") {
                "CanvasResolution"
            } else if canvas_cols.contains("CanvasDpi") {
                "CanvasDpi"
            } else {
                "NULL"
            };
            let sql = format!(
                "SELECT CanvasWidth, CanvasHeight, {} FROM Canvas LIMIT 1",
                dpi_col
            );
            if let Ok((w, h, res)) = conn.query_row(&sql, [], |r| {
                Ok((
                    r.get::<_, Option<f64>>(0)?,
                    r.get::<_, Option<f64>>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                ))
            }) {
                canvas_width = w.map(|v| v.round() as u32);
                canvas_height = h.map(|v| v.round() as u32);
                dpi = res;
            }
        }

        // 3. Extraer inventario de capas desde Layer
        let layer_cols = Self::table_columns(&conn, "Layer");
        let mut layers = Vec::new();
        if layer_cols.contains("LayerName") {
            let id_expr = if layer_cols.contains("LayerUuid") {
                "CAST(LayerUuid AS TEXT)"
            } else if layer_cols.contains("MainId") {
                "CAST(MainId AS TEXT)"
            } else {
                "CAST(_ROWID_ AS TEXT)"
            };
            let vis_expr = if layer_cols.contains("LayerVisibility") {
                "COALESCE(LayerVisibility, 1)"
            } else {
                "1"
            };
            let opa_expr = if layer_cols.contains("LayerOpacity") {
                "COALESCE(LayerOpacity, 256)"
            } else {
                "256"
            };
            let folder_expr = if layer_cols.contains("LayerFolder") {
                "COALESCE(LayerFolder, 0)"
            } else {
                "0"
            };

            let sql = format!(
                "SELECT {}, COALESCE(LayerName, ''), {}, {}, {} FROM Layer ORDER BY _ROWID_ ASC",
                id_expr, vis_expr, opa_expr, folder_expr
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mapped) = stmt.query_map([], |r| {
                    let id: String = r.get(0)?;
                    let name: String = r.get(1)?;
                    let raw_vis: i64 = r.get(2)?;
                    let raw_opa: i64 = r.get(3)?;
                    let raw_folder: i64 = r.get(4)?;

                    // En CSP LayerOpacity suele ir de 0 a 256 (256 = 100%)
                    let opacity = if raw_opa > 100 {
                        ((raw_opa as f64 / 256.0) * 100.0).round().clamp(0.0, 100.0) as u8
                    } else {
                        raw_opa.clamp(0, 100) as u8
                    };

                    Ok(LayerSummary {
                        id,
                        name,
                        visible: (raw_vis & 1) != 0,
                        opacity,
                        is_folder: raw_folder != 0,
                    })
                }) {
                    for item in mapped.flatten() {
                        if !item.name.is_empty() {
                            layers.push(item);
                        }
                    }
                }
            }
        }

        Ok(AssetInspection {
            preview_png,
            metadata: AssetMetadata {
                format: "clip".to_string(),
                canvas_width,
                canvas_height,
                dpi,
                layers,
            },
        })
    }
}

// ============================================================================
// Generador de archivos `.clip` sintéticos realistas (para pruebas y demos)
// ============================================================================

pub mod synthetic {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    #[derive(Debug, Clone)]
    pub struct SyntheticClipSpec {
        pub width: u32,
        pub height: u32,
        pub dpi: f64,
        pub layers: Vec<LayerSummary>,
        /// Bloques binarios de capas: cambiar solo 1 bloque permite verificar deduplicación FastCDC real
        pub layer_blocks: Vec<Vec<u8>>,
        /// Color y rectángulo pintado sobre el preview PNG para probar Visual Diff
        pub accent_rgba: [u8; 4],
        pub stroke_rect: (u32, u32, u32, u32),
    }

    impl SyntheticClipSpec {
        pub fn sample_v1() -> Self {
            let mut blocks = Vec::new();
            for idx in 0..16u32 {
                // ~80 KB por bloque pseudo-aleatorio determinista -> ~1.28 MB total
                let mut raw = Vec::with_capacity(80_000);
                let mut state = 0x9E37_79B9u64.wrapping_mul((idx as u64) + 1);
                for _ in 0..10_000 {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    raw.extend_from_slice(&state.to_le_bytes());
                }
                let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
                enc.write_all(&raw).unwrap();
                blocks.push(enc.finish().unwrap());
            }

            Self {
                width: 640,
                height: 480,
                dpi: 300.0,
                layers: vec![
                    LayerSummary {
                        id: "layer-bg".into(),
                        name: "Fondo papel".into(),
                        visible: true,
                        opacity: 100,
                        is_folder: false,
                    },
                    LayerSummary {
                        id: "layer-sketch".into(),
                        name: "Boceto".into(),
                        visible: true,
                        opacity: 65,
                        is_folder: false,
                    },
                    LayerSummary {
                        id: "layer-lineart".into(),
                        name: "Lineart".into(),
                        visible: true,
                        opacity: 100,
                        is_folder: false,
                    },
                ],
                layer_blocks: blocks,
                accent_rgba: [72, 149, 239, 255],
                stroke_rect: (140, 110, 220, 180),
            }
        }
    }

    pub fn render_preview_png(spec: &SyntheticClipSpec) -> Result<Vec<u8>> {
        let pw = 320u32;
        let ph = 240u32;
        let sx = pw as f32 / spec.width.max(1) as f32;
        let sy = ph as f32 / spec.height.max(1) as f32;
        let (rx, ry, rw, rh) = (
            (spec.stroke_rect.0 as f32 * sx) as u32,
            (spec.stroke_rect.1 as f32 * sy) as u32,
            (spec.stroke_rect.2 as f32 * sx).max(8.0) as u32,
            (spec.stroke_rect.3 as f32 * sy).max(8.0) as u32,
        );

        let img = ImageBuffer::from_fn(pw, ph, |x, y| {
            // Fondo cálido estilo lienzo de ilustración + silueta base constante
            let mut px = [245u8, 241u8, 234u8, 255u8];
            let dx = x as i32 - 160;
            let dy = y as i32 - 120;
            if dx * dx + dy * dy < 75 * 75 {
                px = [58, 62, 74, 255];
            }
            if x >= rx && x < (rx + rw).min(pw) && y >= ry && y < (ry + rh).min(ph) {
                px = spec.accent_rgba;
            }
            Rgba(px)
        });

        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Png)?;
        Ok(cursor.into_inner())
    }

    pub fn build_clip_bytes(spec: &SyntheticClipSpec) -> Result<Vec<u8>> {
        let preview_png = render_preview_png(spec)?;

        // Construir SQLite real en archivo temporal para volcar sus bytes exactos en CHNKSQLi
        let seq = SQLITE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_db = std::env::temp_dir().join(format!(
            "atenea_synth_build_{}_{}.sqlite",
            std::process::id(),
            seq
        ));
        {
            let conn = Connection::open(&tmp_db)?;
            conn.execute_batch(
                "
                PRAGMA journal_mode = OFF;
                PRAGMA synchronous = OFF;
                CREATE TABLE Canvas (
                    MainId INTEGER PRIMARY KEY,
                    CanvasWidth REAL,
                    CanvasHeight REAL,
                    CanvasResolution REAL
                );
                CREATE TABLE CanvasPreview (
                    MainId INTEGER PRIMARY KEY,
                    ImageData BLOB
                );
                CREATE TABLE Layer (
                    MainId INTEGER PRIMARY KEY AUTOINCREMENT,
                    LayerUuid TEXT,
                    LayerName TEXT,
                    LayerVisibility INTEGER,
                    LayerOpacity INTEGER,
                    LayerFolder INTEGER
                );
                ",
            )?;

            conn.execute(
                "INSERT INTO Canvas (MainId, CanvasWidth, CanvasHeight, CanvasResolution) VALUES (1, ?1, ?2, ?3)",
                rusqlite::params![spec.width as f64, spec.height as f64, spec.dpi],
            )?;
            conn.execute(
                "INSERT INTO CanvasPreview (MainId, ImageData) VALUES (1, ?1)",
                rusqlite::params![preview_png],
            )?;

            for layer in &spec.layers {
                let vis = if layer.visible { 1i64 } else { 0i64 };
                let opa = ((layer.opacity as f64 / 100.0) * 256.0).round() as i64;
                let folder = if layer.is_folder { 1i64 } else { 0i64 };
                conn.execute(
                    "INSERT INTO Layer (LayerUuid, LayerName, LayerVisibility, LayerOpacity, LayerFolder)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![layer.id, layer.name, vis, opa, folder],
                )?;
            }
        }
        let sqlite_bytes = fs::read(&tmp_db)?;
        let _ = fs::remove_file(&tmp_db);

        // Ensamblar chunks con enteros de 64 bits Big-Endian
        let mut body = Vec::new();

        // 1. CHNKHead
        let head_payload = [0u8; 40];
        body.extend_from_slice(CHUNK_HEAD);
        body.extend_from_slice(&(head_payload.len() as u64).to_be_bytes());
        body.extend_from_slice(&head_payload);

        // 2. Bloques CHNKExta (datos raster comprimidos por capa)
        for block in &spec.layer_blocks {
            body.extend_from_slice(CHUNK_EXTA);
            body.extend_from_slice(&(block.len() as u64).to_be_bytes());
            body.extend_from_slice(block);
        }

        // 3. CHNKSQLi
        body.extend_from_slice(CHUNK_SQLI);
        body.extend_from_slice(&(sqlite_bytes.len() as u64).to_be_bytes());
        body.extend_from_slice(&sqlite_bytes);

        // 4. CHNKFoot
        body.extend_from_slice(CHUNK_FOOT);
        body.extend_from_slice(&0u64.to_be_bytes());

        let total_size = 24u64 + body.len() as u64;
        let mut out = Vec::with_capacity(total_size as usize);
        out.extend_from_slice(CLIP_MAGIC);
        out.extend_from_slice(&total_size.to_be_bytes());
        out.extend_from_slice(&24u64.to_be_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub fn write_clip_file(path: &Path, spec: &SyntheticClipSpec) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = build_clip_bytes(spec)?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::synthetic::*;
    use super::*;
    use atenea_core::{AteneaStore, SnapshotOutcome};
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn parses_synthetic_clip_and_deduplicates_incremental_saves() -> Result<()> {
        let dir = tempdir()?;
        let store_dir = dir.path().join("store");
        let work_dir = dir.path().join("art");
        fs::create_dir_all(&work_dir)?;

        let clip_path = work_dir.join("personaje_principal.clip");
        let mut spec = SyntheticClipSpec::sample_v1();
        write_clip_file(&clip_path, &spec)?;

        let adapter = Arc::new(ClipAdapter::new());
        assert_eq!(adapter.check_integrity(&clip_path)?, IntegrityStatus::Valid);

        let store = AteneaStore::open(&store_dir, vec![adapter])?;

        // Snapshot 1 (completo)
        let s1 = store.snapshot_file(&clip_path, "autosave", Some("Boceto inicial"))?;
        let c1 = match s1 {
            SnapshotOutcome::Created { commit } => commit,
            other => panic!("Se esperaba Created, llegó {:?}", other),
        };
        assert_eq!(c1.metadata.canvas_width, Some(640));
        assert_eq!(c1.metadata.layers.len(), 3);
        assert!(c1.preview_path.is_some());
        let original_v1_hash = AteneaStore::hash_file_blake3(&clip_path)?;
        assert_eq!(c1.file_hash, original_v1_hash);

        // Snapshot repetido sin cambios -> debe detectarse como Unchanged
        let s_dup = store.snapshot_file(&clip_path, "autosave", None)?;
        assert!(matches!(s_dup, SnapshotOutcome::Unchanged { .. }));

        // Modificar SOLO 1 de los 16 bloques de capas + agregar 1 capa nueva ("Sombras")
        spec.layer_blocks[5][10..18].copy_from_slice(b"MODIFIED");
        spec.layers[1].visible = false; // Ocultar Boceto
        spec.layers.push(LayerSummary {
            id: "layer-shadows".into(),
            name: "Sombras".into(),
            visible: true,
            opacity: 80,
            is_folder: false,
        });
        spec.accent_rgba = [247, 37, 133, 255];
        write_clip_file(&clip_path, &spec)?;

        // Snapshot 2 (incremental con FastCDC)
        let s2 = store.snapshot_file(&clip_path, "autosave", None)?;
        let c2 = match s2 {
            SnapshotOutcome::Created { commit } => commit,
            other => panic!("Se esperaba Created en v2, llegó {:?}", other),
        };
        assert_eq!(c2.parent_id.as_deref(), Some(c1.id.as_str()));
        assert_eq!(c2.metadata.layers.len(), 4);
        // Verificar deduplicación real: debe escribir menos del 35% del tamaño total del archivo
        assert!(
            c2.added_bytes < (c2.file_size * 35) / 100,
            "Se esperaba alta deduplicación FastCDC, pero added_bytes={} de file_size={}",
            c2.added_bytes,
            c2.file_size
        );

        // Simular corrupción truncando el archivo (como fallo de guardado o sync roto)
        let raw_bytes = fs::read(&clip_path)?;
        fs::write(&clip_path, &raw_bytes[..raw_bytes.len() / 2])?;
        let s_corrupt = store.snapshot_file(&clip_path, "autosave", None)?;
        assert!(matches!(s_corrupt, SnapshotOutcome::Corrupted { .. }));

        // El historial sano sigue intacto (2 snapshots) y podemos restaurar V1 bit a bit
        let commits = store.list_commits(&c1.project_id, "main")?;
        assert_eq!(commits.len(), 2);

        let restored_copy = work_dir.join("personaje_restaurado_v1.clip");
        store.restore_to_copy(&c1.id, &restored_copy)?;
        assert_eq!(AteneaStore::hash_file_blake3(&restored_copy)?, original_v1_hash);

        Ok(())
    }
}
