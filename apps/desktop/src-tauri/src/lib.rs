use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

mod updater;

use atenea_clip::synthetic::{write_clip_file, SyntheticClipSpec};
use atenea_clip::ClipAdapter;
use atenea_core::{
    AssetAdapter, AteneaStore, BranchInfo, CommitRecord, LayerSummary, ProjectSummary,
    RemoveFolderReport, SnapshotOutcome, StorageStats, WatchedFolder,
};
use atenea_diff::{compute_commit_diff, VisualDiffReport};
use atenea_watcher::{SafeWatcher, WatcherConfig, WatcherEvent};
use base64::Engine;
use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;

pub struct AppState {
    pub store: Arc<AteneaStore>,
    pub watcher: Arc<SafeWatcher>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectCardDto {
    #[serde(flatten)]
    pub summary: ProjectSummary,
    pub latest_preview_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitDto {
    #[serde(flatten)]
    pub commit: CommitRecord,
    pub preview_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectGraphDto {
    pub project_id: String,
    pub active_branch: String,
    pub branches: Vec<BranchInfo>,
    pub commits: Vec<CommitDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardDto {
    pub store_root: String,
    pub watched_folders: Vec<WatchedFolder>,
    pub projects: Vec<ProjectCardDto>,
    pub stats: StorageStats,
}

fn file_to_png_data_url(path_opt: Option<&str>) -> Option<String> {
    let p = Path::new(path_opt?);
    let bytes = fs::read(p).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:image/png;base64,{}", b64))
}

fn enrich_commit(commit: CommitRecord) -> CommitDto {
    let preview_data_url = file_to_png_data_url(commit.preview_path.as_deref());
    CommitDto {
        commit,
        preview_data_url,
    }
}

#[tauri::command]
fn get_dashboard_state(state: State<'_, AppState>) -> Result<DashboardDto, String> {
    let watched_folders = state
        .store
        .list_watched_folders()
        .map_err(|e| e.to_string())?;
    let raw_projects = state.store.list_projects().map_err(|e| e.to_string())?;
    let stats = state.store.storage_stats().map_err(|e| e.to_string())?;

    let projects = raw_projects
        .into_iter()
        .map(|summary| {
            let latest_preview_data_url =
                file_to_png_data_url(summary.latest_preview_path.as_deref());
            ProjectCardDto {
                summary,
                latest_preview_data_url,
            }
        })
        .collect();

    Ok(DashboardDto {
        store_root: state.store.store_root().to_string_lossy().to_string(),
        watched_folders,
        projects,
        stats,
    })
}

#[tauri::command]
fn add_watched_folder(path: String, state: State<'_, AppState>) -> Result<WatchedFolder, String> {
    let trimmed = path.trim();
    let p = Path::new(trimmed);
    if !p.exists() || !p.is_dir() {
        return Err(format!("La carpeta no existe o no es válida: {}", trimmed));
    }
    let wf = state
        .store
        .add_watched_folder(p)
        .map_err(|e| e.to_string())?;
    state
        .watcher
        .watch_folder(p)
        .map_err(|e| e.to_string())?;
    Ok(wf)
}

#[tauri::command]
fn toggle_watched_folder(
    folder_id: i64,
    path: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<WatchedFolder, String> {
    let wf = state
        .store
        .toggle_watched_folder(folder_id, enabled)
        .map_err(|e| e.to_string())?;
    let p = Path::new(&path);
    if enabled {
        if p.exists() {
            let _ = state.watcher.watch_folder(p);
        }
    } else {
        let _ = state.watcher.unwatch_folder(p);
    }
    Ok(wf)
}

#[tauri::command]
fn remove_watched_folder(
    folder_id: i64,
    path: String,
    purge_snapshots: bool,
    state: State<'_, AppState>,
) -> Result<RemoveFolderReport, String> {
    let _ = state.watcher.unwatch_folder(Path::new(&path));
    state
        .store
        .remove_watched_folder_with_options(folder_id, purge_snapshots)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_project(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<RemoveFolderReport, String> {
    state
        .store
        .delete_project(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_project_graph(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<ProjectGraphDto, String> {
    let graph = state
        .store
        .get_project_graph(&project_id)
        .map_err(|e| e.to_string())?;
    Ok(ProjectGraphDto {
        project_id: graph.project_id,
        active_branch: graph.active_branch,
        branches: graph.branches,
        commits: graph.commits.into_iter().map(enrich_commit).collect(),
    })
}

#[tauri::command]
fn create_variation(
    project_id: String,
    name: String,
    from_commit_id: String,
    checkout_to_disk: bool,
    state: State<'_, AppState>,
) -> Result<BranchInfo, String> {
    state
        .store
        .create_variation(&project_id, &name, &from_commit_id, checkout_to_disk)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn switch_variation(
    project_id: String,
    branch_name: String,
    checkout_to_disk: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .store
        .switch_variation(&project_id, &branch_name, checkout_to_disk)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn snapshot_project_now(
    file_path: String,
    label: Option<String>,
    state: State<'_, AppState>,
) -> Result<SnapshotOutcome, String> {
    state
        .store
        .snapshot_file(Path::new(&file_path), "manual", label.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_commit_label(
    commit_id: String,
    label: Option<String>,
    state: State<'_, AppState>,
) -> Result<CommitDto, String> {
    let updated = state
        .store
        .update_commit_label(&commit_id, label.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(enrich_commit(updated))
}

#[tauri::command]
fn toggle_commit_pin(
    commit_id: String,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<CommitDto, String> {
    let updated = state
        .store
        .toggle_commit_pin(&commit_id, pinned)
        .map_err(|e| e.to_string())?;
    Ok(enrich_commit(updated))
}

#[tauri::command]
fn compute_visual_diff(
    base_commit_id: String,
    target_commit_id: String,
    state: State<'_, AppState>,
) -> Result<VisualDiffReport, String> {
    let base = state
        .store
        .get_commit(&base_commit_id)
        .map_err(|e| e.to_string())?;
    let target = state
        .store
        .get_commit(&target_commit_id)
        .map_err(|e| e.to_string())?;
    compute_commit_diff(&base, &target).map_err(|e| e.to_string())
}

#[tauri::command]
fn restore_commit_copy(
    commit_id: String,
    destination_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let out = state
        .store
        .restore_to_copy(&commit_id, Path::new(&destination_path))
        .map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
fn restore_commit_in_place(
    commit_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let out = state
        .store
        .restore_in_place(&commit_id)
        .map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().to_string())
}

#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let p = Path::new(&path);
        if p.is_dir() {
            Command::new("explorer.exe")
                .arg(&path)
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            Command::new("explorer.exe")
                .arg(format!("/select,{}", path))
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn seed_demo_project(state: State<'_, AppState>) -> Result<String, String> {
    let demo_dir = state
        .store
        .store_root()
        .parent()
        .unwrap_or_else(|| state.store.store_root())
        .join("demo_clip_studio");
    fs::create_dir_all(&demo_dir).map_err(|e| e.to_string())?;

    state
        .store
        .add_watched_folder(&demo_dir)
        .map_err(|e| e.to_string())?;

    let clip_path = demo_dir.join("personaje_principal.clip");
    let pid = atenea_core::project_id_for_path(&clip_path);
    // Limpiar demo previa si existía para generar un grafo limpio y reproducible
    let _ = state.store.delete_project(&pid);

    let mut base_spec = SyntheticClipSpec::sample_v1();

    // Commit 1 en `main`: Boceto inicial (Fijado ⭐)
    write_clip_file(&clip_path, &base_spec).map_err(|e| e.to_string())?;
    if let SnapshotOutcome::Created { commit } = state
        .store
        .snapshot_file_on_branch(&clip_path, "main", "manual", Some("Boceto estructural"))
        .map_err(|e| e.to_string())?
    {
        let _ = state.store.toggle_commit_pin(&commit.id, true);
    }
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Commit 2 en `main`: Lineart limpio (Punto de bifurcación + Fijado ⭐)
    base_spec.layer_blocks[2][0..12].copy_from_slice(b"CLEAN_INK_V2");
    base_spec.layers[1].opacity = 30;
    base_spec.accent_rgba = [99, 102, 241, 255];
    base_spec.stroke_rect = (145, 108, 225, 182);
    write_clip_file(&clip_path, &base_spec).map_err(|e| e.to_string())?;
    let c2_outcome = state
        .store
        .snapshot_file_on_branch(&clip_path, "main", "autosave", Some("Lineart entintado"))
        .map_err(|e| e.to_string())?;
    let fork_commit_id = match c2_outcome {
        SnapshotOutcome::Created { commit } => {
            let _ = state.store.toggle_commit_pin(&commit.id, true);
            commit.id
        }
        SnapshotOutcome::Unchanged { commit_id, .. } => commit_id,
        _ => return Err("No se pudo crear el commit base de demo".into()),
    };
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Variación paralela `Paleta nocturna` naciendo desde Commit 2 (como en GitKraken)
    let mut night_spec = base_spec.clone();
    state
        .store
        .create_variation(&pid, "Paleta nocturna", &fork_commit_id, false)
        .map_err(|e| e.to_string())?;

    night_spec.layer_blocks[6][0..12].copy_from_slice(b"NIGHT_SHADE1");
    night_spec.layers[1].visible = false;
    night_spec.layers.push(LayerSummary {
        id: "layer-night-base".into(),
        name: "Ambiente azul nocturno".into(),
        visible: true,
        opacity: 85,
        is_folder: false,
    });
    night_spec.accent_rgba = [14, 165, 233, 255];
    night_spec.stroke_rect = (130, 95, 260, 205);
    write_clip_file(&clip_path, &night_spec).map_err(|e| e.to_string())?;
    state
        .store
        .snapshot_file_on_branch(
            &clip_path,
            "Paleta nocturna",
            "autosave",
            Some("Base fría azulada"),
        )
        .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(15));

    night_spec.layer_blocks[10][0..12].copy_from_slice(b"NEON_CYAN_GL");
    night_spec.layers.push(LayerSummary {
        id: "layer-neon-rim".into(),
        name: "Contraluz neón cian".into(),
        visible: true,
        opacity: 92,
        is_folder: false,
    });
    night_spec.accent_rgba = [34, 211, 238, 255];
    night_spec.stroke_rect = (175, 85, 210, 175);
    write_clip_file(&clip_path, &night_spec).map_err(|e| e.to_string())?;
    state
        .store
        .snapshot_file_on_branch(
            &clip_path,
            "Paleta nocturna",
            "autosave",
            Some("Contraluz cian intenso"),
        )
        .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(15));

    // Volver a `main` y avanzar 2 snapshots cálidos para mostrar ambas ramas vivas en paralelo
    let mut warm_spec = base_spec;
    warm_spec.layer_blocks[4][0..12].copy_from_slice(b"WARM_SHADING");
    warm_spec.layers[1].visible = false;
    warm_spec.layers.push(LayerSummary {
        id: "layer-warm-color".into(),
        name: "Color base y sombras cálidas".into(),
        visible: true,
        opacity: 90,
        is_folder: false,
    });
    warm_spec.accent_rgba = [236, 72, 153, 255];
    warm_spec.stroke_rect = (155, 120, 235, 190);
    write_clip_file(&clip_path, &warm_spec).map_err(|e| e.to_string())?;
    state
        .store
        .snapshot_file_on_branch(
            &clip_path,
            "main",
            "autosave",
            Some("Sombreado cálido principal"),
        )
        .map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(15));

    warm_spec.layer_blocks[9][0..12].copy_from_slice(b"GOLD_RIM_FIN");
    warm_spec.layers.push(LayerSummary {
        id: "layer-gold-rim".into(),
        name: "Luz dorada de atardecer".into(),
        visible: true,
        opacity: 80,
        is_folder: false,
    });
    warm_spec.accent_rgba = [250, 204, 21, 255];
    warm_spec.stroke_rect = (185, 92, 200, 155);
    write_clip_file(&clip_path, &warm_spec).map_err(|e| e.to_string())?;
    state
        .store
        .snapshot_file_on_branch(
            &clip_path,
            "main",
            "autosave",
            Some("Iluminación dorada final"),
        )
        .map_err(|e| e.to_string())?;

    state
        .watcher
        .watch_folder(&demo_dir)
        .map_err(|e| e.to_string())?;

    Ok(pid)
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_system_tray(app: &AppHandle) -> anyhow::Result<()> {
    let open_item = MenuItemBuilder::with_id("open", "Abrir Atenea").build(app)?;
    let update_item =
        MenuItemBuilder::with_id("check_updates", "Buscar actualizaciones…").build(app)?;
    let status_item = MenuItemBuilder::with_id("status", "Protección Zero-Corruption activa")
        .enabled(false)
        .build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Salir completamente").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[&open_item, &update_item, &status_item, &quit_item])
        .build()?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Atenea — Protegiendo tus archivos .clip en segundo plano")
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            "open" => show_main_window(app_handle),
            "check_updates" => {
                show_main_window(app_handle);
                let _ = app_handle.emit("atenea://open-updater-modal", ());
            }
            "quit" => {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    state.watcher.shutdown();
                }
                app_handle.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store_dir = atenea_core::resolve_default_store_dir();
    let adapters: Vec<Arc<dyn AssetAdapter>> = vec![Arc::new(ClipAdapter::new())];
    let store = Arc::new(
        AteneaStore::open(&store_dir, adapters)
            .expect("No se pudo inicializar el almacén local de Atenea"),
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let watcher = SafeWatcher::start(
                Arc::clone(&store),
                WatcherConfig::default(),
                move |event| {
                    if let WatcherEvent::CorruptedSaveDetected {
                        ref file_path,
                        ref reason,
                        ..
                    } = event
                    {
                        let file_name = Path::new(file_path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(file_path.as_str());
                        let _ = app_handle
                            .notification()
                            .builder()
                            .title("Atenea detectó un guardado corrupto")
                            .body(format!(
                                "{} quedó incompleto ({}). Tus versiones anteriores están a salvo.",
                                file_name, reason
                            ))
                            .show();
                    }
                    let _ = app_handle.emit("atenea://watcher-event", &event);
                },
            )?;

            app.manage(AppState { store, watcher });
            setup_system_tray(app.handle())?;

            // Sondeo silencioso de actualizaciones al iniciar (si auto_check está activo)
            let bg_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if updater::load_settings().auto_check {
                    tauri::async_runtime::spawn(async move {
                        let _ = updater::perform_update_check(&bg_handle, None, true).await;
                    });
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    if !updater::is_exiting_for_update() {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard_state,
            add_watched_folder,
            toggle_watched_folder,
            remove_watched_folder,
            delete_project,
            get_project_graph,
            create_variation,
            switch_variation,
            snapshot_project_now,
            set_commit_label,
            toggle_commit_pin,
            compute_visual_diff,
            restore_commit_copy,
            restore_commit_in_place,
            reveal_in_explorer,
            seed_demo_project,
            updater::get_updater_settings,
            updater::save_updater_settings,
            updater::check_app_update,
            updater::preflight_update_check,
            updater::flush_snapshots_before_update,
            updater::install_app_update,
        ])
        .run(tauri::generate_context!())
        .expect("Error al ejecutar la aplicación de escritorio Atenea");
}
