use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use atenea_clip::synthetic::{write_clip_file, SyntheticClipSpec};
use atenea_clip::ClipAdapter;
use atenea_core::{AssetAdapter, AteneaStore, IntegrityStatus, LayerSummary, SnapshotOutcome};
use atenea_diff::compute_commit_diff;
use atenea_watcher::{SafeWatcher, WatcherConfig, WatcherEvent};

fn default_store_dir() -> PathBuf {
    atenea_core::resolve_default_store_dir()
}

fn open_store(store_dir: &Path) -> Result<Arc<AteneaStore>> {
    let adapters: Vec<Arc<dyn AssetAdapter>> = vec![Arc::new(ClipAdapter::new())];
    Ok(Arc::new(AteneaStore::open(store_dir, adapters)?))
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn print_usage() {
    eprintln!(
        "Atenea CLI v0.1 — Versionado seguro y Visual Diff para Clip Studio (.clip)

Uso:
  atenea inspect <archivo.clip> [--extract-preview <salida.png>]
  atenea snapshot <archivo.clip> [--label <nota>] [--store <ruta_store>]
  atenea log [--store <ruta_store>]
  atenea diff <commit_base> <commit_nuevo> [--store <ruta_store>]
  atenea restore <commit_id> <destino.clip> [--store <ruta_store>]
  atenea watch <carpeta> [--store <ruta_store>]
  atenea demo-seed <carpeta_destino> [--store <ruta_store>]
"
    );
}

fn parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let store_dir = parse_flag_value(&args, "--store")
        .map(PathBuf::from)
        .unwrap_or_else(default_store_dir);

    match args[0].as_str() {
        "inspect" => {
            let Some(file_arg) = args.get(1) else {
                bail!("Falta la ruta del archivo .clip");
            };
            let path = Path::new(file_arg);
            let adapter = ClipAdapter::new();

            println!("Inspeccionando: {}", path.display());
            match adapter.check_integrity(path)? {
                IntegrityStatus::Valid => println!("Integridad: OK (CSFCHUNK + CHNKSQLi + CHNKFoot válidos)"),
                IntegrityStatus::Corrupted { reason } => {
                    println!("Integridad: CORRUPTO -> {}", reason);
                    return Ok(());
                }
            }

            let container = ClipAdapter::parse_container(path)?;
            let inspection = adapter.inspect(path)?;
            println!(
                "Contenedor: {} | Bloques raster (CHNKExta): {} | SQLite interno: {}",
                format_bytes(container.file_size),
                container.exta_chunk_count,
                format_bytes(container.sqlite_len)
            );
            println!(
                "Lienzo: {:?}x{:?} px @ {:?} DPI",
                inspection.metadata.canvas_width,
                inspection.metadata.canvas_height,
                inspection.metadata.dpi
            );
            println!("Capas detectadas ({}):", inspection.metadata.layers.len());
            for layer in &inspection.metadata.layers {
                println!(
                    "  - [{}] {} (opacidad {}%{})",
                    if layer.visible { "visible" } else { "oculta " },
                    layer.name,
                    layer.opacity,
                    if layer.is_folder { ", carpeta" } else { "" }
                );
            }

            if let Some(out_png) = parse_flag_value(&args, "--extract-preview") {
                if let Some(bytes) = inspection.preview_png {
                    fs::write(&out_png, bytes)?;
                    println!("Vista previa extraída en: {}", out_png);
                }
            }
        }

        "snapshot" => {
            let Some(file_arg) = args.get(1) else {
                bail!("Falta la ruta del archivo .clip");
            };
            let label = parse_flag_value(&args, "--label");
            let store = open_store(&store_dir)?;
            match store.snapshot_file(Path::new(file_arg), "manual", label.as_deref())? {
                SnapshotOutcome::Created { commit } => {
                    let saved_pct = if commit.file_size > 0 {
                        100.0 - ((commit.added_bytes as f64 / commit.file_size as f64) * 100.0)
                    } else {
                        0.0
                    };
                    println!(
                        "Snapshot creado: {} | Archivo: {} | Nuevo en disco: {} ({:.1}% deduplicado)",
                        commit.id,
                        format_bytes(commit.file_size),
                        format_bytes(commit.added_bytes),
                        saved_pct.max(0.0)
                    );
                }
                SnapshotOutcome::Unchanged { commit_id, .. } => {
                    println!("Sin cambios respecto al último snapshot ({})", commit_id);
                }
                SnapshotOutcome::Corrupted { reason, .. } => {
                    println!("ALERTA: Guardado rechazado por corrupción -> {}", reason);
                }
                SnapshotOutcome::MutatedDuringRead { .. } => {
                    println!("El archivo seguía escribiéndose; reintenta en un segundo.");
                }
            }
        }

        "log" => {
            let store = open_store(&store_dir)?;
            let projects = store.list_projects()?;
            if projects.is_empty() {
                println!("No hay proyectos registrados en {}", store_dir.display());
                return Ok(());
            }
            for p in projects {
                println!(
                    "Proyecto: {} ({}) — {} snapshots | Lógico: {} | En disco: {}",
                    p.display_name,
                    p.file_path,
                    p.snapshot_count,
                    format_bytes(p.total_logical_bytes),
                    format_bytes(p.total_stored_bytes)
                );
                for c in store.list_commits(&p.id, "main")? {
                    println!(
                        "  * {} | +{} | {} capas | {}",
                        c.id,
                        format_bytes(c.added_bytes),
                        c.metadata.layers.len(),
                        c.label.as_deref().unwrap_or(c.author_kind.as_str())
                    );
                }
            }
        }

        "diff" => {
            let (Some(base_id), Some(target_id)) = (args.get(1), args.get(2)) else {
                bail!("Uso: atenea diff <commit_base> <commit_nuevo>");
            };
            let store = open_store(&store_dir)?;
            let base = store.get_commit(base_id)?;
            let target = store.get_commit(target_id)?;
            let report = compute_commit_diff(&base, &target)?;
            println!(
                "Visual Diff {} -> {}: {:.2}% píxeles modificados ({} de {} px)",
                report.base_commit_id,
                report.target_commit_id,
                report.changed_ratio * 100.0,
                report.changed_pixels,
                report.total_pixels
            );
            if let Some(bbox) = &report.overall_bbox {
                println!(
                    "Región alterada: x={}, y={}, ancho={}, alto={}",
                    bbox.x, bbox.y, bbox.width, bbox.height
                );
            }
            for lc in &report.layer_changes {
                println!("  Capa [{}] {}: {}", lc.kind, lc.layer_name, lc.detail);
            }
        }

        "restore" => {
            let (Some(commit_id), Some(dest)) = (args.get(1), args.get(2)) else {
                bail!("Uso: atenea restore <commit_id> <destino.clip>");
            };
            let store = open_store(&store_dir)?;
            let out = store.restore_to_copy(commit_id, Path::new(dest))?;
            println!("Restauración verificada por BLAKE3 completada en: {}", out.display());
        }

        "watch" => {
            let Some(folder_arg) = args.get(1) else {
                bail!("Uso: atenea watch <carpeta>");
            };
            let folder = Path::new(folder_arg);
            let store = open_store(&store_dir)?;
            store.add_watched_folder(folder)?;
            let watcher = SafeWatcher::start(
                Arc::clone(&store),
                WatcherConfig::default(),
                |ev| match ev {
                    WatcherEvent::FolderScanned {
                        folder,
                        discovered_files,
                    } => println!("Vigilando {} ({} archivos .clip)", folder, discovered_files),
                    WatcherEvent::SnapshotCreated { commit } => println!(
                        "[OK] Snapshot {} guardado (+{})",
                        commit.id,
                        format_bytes(commit.added_bytes)
                    ),
                    WatcherEvent::CorruptedSaveDetected {
                        file_path, reason, ..
                    } => eprintln!(
                        "[ALERTA CORRUPCIÓN] {} -> {}",
                        file_path, reason
                    ),
                    WatcherEvent::WatcherWarning { message } => eprintln!("[AVISO] {}", message),
                },
            )?;
            watcher.watch_folder(folder)?;
            println!("Atenea SafeWatcher activo. Presiona Ctrl+C para detener.");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }

        "demo-seed" => {
            let Some(folder_arg) = args.get(1) else {
                bail!("Uso: atenea demo-seed <carpeta_destino>");
            };
            let folder = Path::new(folder_arg);
            fs::create_dir_all(folder).context("No se pudo crear la carpeta demo")?;
            let store = open_store(&store_dir)?;
            store.add_watched_folder(folder)?;

            let clip_file = folder.join("ilustracion_atenea_demo.clip");
            let mut spec = SyntheticClipSpec::sample_v1();
            write_clip_file(&clip_file, &spec)?;
            store.snapshot_file(&clip_file, "manual", Some("Boceto y lineart base"))?;

            // Versión 2: cambio de color/sombra en una capa
            spec.layer_blocks[3][0..12].copy_from_slice(b"SHADING_PASS");
            spec.layers[1].visible = false;
            spec.layers.push(LayerSummary {
                id: "layer-color".into(),
                name: "Color base y sombras".into(),
                visible: true,
                opacity: 90,
                is_folder: false,
            });
            spec.accent_rgba = [247, 37, 133, 255];
            spec.stroke_rect = (160, 130, 240, 190);
            write_clip_file(&clip_file, &spec)?;
            store.snapshot_file(&clip_file, "autosave", Some("Color y sombreado"))?;

            // Versión 3: brillos finales
            spec.layer_blocks[7][0..12].copy_from_slice(b"RIM_LIGHTING");
            spec.layers.push(LayerSummary {
                id: "layer-glow".into(),
                name: "Luz de recorte (Rim light)".into(),
                visible: true,
                opacity: 75,
                is_folder: false,
            });
            spec.accent_rgba = [255, 190, 11, 255];
            spec.stroke_rect = (190, 95, 190, 150);
            write_clip_file(&clip_file, &spec)?;
            store.snapshot_file(&clip_file, "autosave", Some("Iluminación final"))?;

            println!(
                "Proyecto demo creado con 3 versiones deduplicadas en: {}",
                clip_file.display()
            );
        }

        _ => print_usage(),
    }

    Ok(())
}
