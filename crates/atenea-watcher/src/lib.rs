use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use atenea_core::{
    normalize_path_string, AteneaStore, CommitRecord, SnapshotOutcome,
};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Tiempo continuo de calma sin eventos antes de inspeccionar el archivo (por defecto 2000 ms).
    pub quiescence_duration: Duration,
    /// Pausa entre dos lecturas de metadatos para confirmar que terminó el volcado a disco.
    pub stability_probe_delay: Duration,
    /// Frecuencia de revisión de la cola de archivos pendientes.
    pub tick_interval: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            quiescence_duration: Duration::from_millis(2000),
            stability_probe_delay: Duration::from_millis(250),
            tick_interval: Duration::from_millis(200),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WatcherEvent {
    FolderScanned {
        folder: String,
        discovered_files: usize,
    },
    SnapshotCreated {
        commit: CommitRecord,
    },
    CorruptedSaveDetected {
        project_id: String,
        file_path: String,
        reason: String,
    },
    WatcherWarning {
        message: String,
    },
}

pub struct SafeWatcher {
    store: Arc<AteneaStore>,
    watcher: Mutex<RecommendedWatcher>,
    watched_dirs: Mutex<HashSet<PathBuf>>,
    pending_files: Arc<Mutex<HashMap<PathBuf, Instant>>>,
    stop_flag: Arc<AtomicBool>,
    worker_thread: Mutex<Option<JoinHandle<()>>>,
    callback: Arc<dyn Fn(WatcherEvent) + Send + Sync>,
}

impl SafeWatcher {
    pub fn start<F>(store: Arc<AteneaStore>, config: WatcherConfig, on_event: F) -> Result<Arc<Self>>
    where
        F: Fn(WatcherEvent) + Send + Sync + 'static,
    {
        let pending_files: Arc<Mutex<HashMap<PathBuf, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let callback: Arc<dyn Fn(WatcherEvent) + Send + Sync> = Arc::new(on_event);

        let pending_for_notify = Arc::clone(&pending_files);
        let store_for_notify = Arc::clone(&store);
        let cb_for_notify = Arc::clone(&callback);

        let raw_watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Access(_) | EventKind::Remove(_)) {
                        return;
                    }
                    let now = Instant::now();
                    for path in event.paths {
                        if should_ignore_path(&path) {
                            continue;
                        }
                        if store_for_notify.find_adapter(&path).is_some() {
                            if let Ok(mut map) = pending_for_notify.lock() {
                                map.insert(path, now);
                            }
                        }
                    }
                }
                Err(err) => {
                    (cb_for_notify)(WatcherEvent::WatcherWarning {
                        message: format!("Aviso del monitor de archivos: {}", err),
                    });
                }
            },
            Config::default(),
        )
        .context("No se pudo inicializar el monitor nativo de archivos")?;

        let instance = Arc::new(Self {
            store: Arc::clone(&store),
            watcher: Mutex::new(raw_watcher),
            watched_dirs: Mutex::new(HashSet::new()),
            pending_files: Arc::clone(&pending_files),
            stop_flag: Arc::clone(&stop_flag),
            worker_thread: Mutex::new(None),
            callback: Arc::clone(&callback),
        });

        // Hilo dedicado de enfriamiento (quiescence) e ingesta no bloqueante
        let worker_pending = Arc::clone(&pending_files);
        let worker_stop = Arc::clone(&stop_flag);
        let worker_store = Arc::clone(&store);
        let worker_cb = Arc::clone(&callback);

        let handle = thread::Builder::new()
            .name("atenea-safe-watcher".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    thread::sleep(config.tick_interval);
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    let ready_paths: Vec<PathBuf> = {
                        let mut guard = match worker_pending.lock() {
                            Ok(g) => g,
                            Err(_) => continue,
                        };
                        let now = Instant::now();
                        let ready: Vec<PathBuf> = guard
                            .iter()
                            .filter(|(_, ts)| now.duration_since(**ts) >= config.quiescence_duration)
                            .map(|(p, _)| p.clone())
                            .collect();
                        for p in &ready {
                            guard.remove(p);
                        }
                        ready
                    };

                    for path in ready_paths {
                        if !path.exists() {
                            continue;
                        }

                        // Sondeo pasivo de estabilidad de tamaño y fecha de modificación
                        let Ok(m1) = fs::metadata(&path) else {
                            continue;
                        };
                        thread::sleep(config.stability_probe_delay);

                        // Si llegó otro evento durante la pausa, posponer para no competir con CSP
                        let had_recent_activity = worker_pending
                            .lock()
                            .map(|g| g.contains_key(&path))
                            .unwrap_or(false);
                        if had_recent_activity {
                            continue;
                        }

                        let Ok(m2) = fs::metadata(&path) else {
                            continue;
                        };
                        if m1.len() != m2.len() || m1.modified().ok() != m2.modified().ok() {
                            if let Ok(mut g) = worker_pending.lock() {
                                g.insert(path, Instant::now());
                            }
                            continue;
                        }

                        match worker_store.snapshot_file(&path, "autosave", None) {
                            Ok(SnapshotOutcome::Created { commit }) => {
                                (worker_cb)(WatcherEvent::SnapshotCreated { commit });
                            }
                            Ok(SnapshotOutcome::Corrupted {
                                project_id,
                                file_path,
                                reason,
                            }) => {
                                (worker_cb)(WatcherEvent::CorruptedSaveDetected {
                                    project_id,
                                    file_path,
                                    reason,
                                });
                            }
                            Ok(SnapshotOutcome::MutatedDuringRead { .. }) => {
                                // El artista volvió a guardar mientras leíamos: reencolar limpiamente
                                if let Ok(mut g) = worker_pending.lock() {
                                    g.insert(path, Instant::now());
                                }
                            }
                            Ok(SnapshotOutcome::Unchanged { .. }) => {}
                            Err(err) => {
                                (worker_cb)(WatcherEvent::WatcherWarning {
                                    message: format!(
                                        "Error procesando {}: {}",
                                        path.display(),
                                        err
                                    ),
                                });
                            }
                        }
                    }
                }
            })?;

        if let Ok(mut slot) = instance.worker_thread.lock() {
            *slot = Some(handle);
        }

        // Cargar carpetas previamente registradas en SQLite
        if let Ok(existing_folders) = instance.store.list_watched_folders() {
            for wf in existing_folders.into_iter().filter(|f| f.enabled) {
                let p = PathBuf::from(&wf.path);
                if p.exists() {
                    let _ = instance.watch_folder(&p);
                }
            }
        }

        Ok(instance)
    }

    pub fn watch_folder(&self, folder: &Path) -> Result<()> {
        let canonical = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        {
            let mut dirs = self.watched_dirs.lock().unwrap();
            if !dirs.contains(&canonical) {
                let mut watcher = self.watcher.lock().unwrap();
                watcher
                    .watch(&canonical, RecursiveMode::Recursive)
                    .with_context(|| {
                        format!("No se pudo vigilar la carpeta {}", canonical.display())
                    })?;
                dirs.insert(canonical.clone());
            }
        }

        // Escaneo inicial no intrusivo para descubrir archivos .clip ya presentes
        let mut discovered = Vec::new();
        collect_supported_files(&canonical, &self.store, &mut discovered);
        let count = discovered.len();
        {
            let mut map = self.pending_files.lock().unwrap();
            // Les damos timestamp antiguo para que el worker los procese tras una breve verificación
            let past = Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now);
            for file_path in discovered {
                map.entry(file_path).or_insert(past);
            }
        }

        (self.callback)(WatcherEvent::FolderScanned {
            folder: normalize_path_string(&canonical),
            discovered_files: count,
        });
        Ok(())
    }

    pub fn unwatch_folder(&self, folder: &Path) -> Result<()> {
        let canonical = fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        let mut dirs = self.watched_dirs.lock().unwrap();
        if dirs.remove(&canonical) {
            let mut watcher = self.watcher.lock().unwrap();
            let _ = watcher.unwatch(&canonical);
        }
        Ok(())
    }

    pub fn pending_file_paths(&self) -> Vec<String> {
        let Ok(guard) = self.pending_files.lock() else {
            return Vec::new();
        };
        guard
            .keys()
            .map(|p| normalize_path_string(p))
            .collect()
    }

    pub fn shutdown(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut dirs) = self.watched_dirs.lock() {
            if let Ok(mut watcher) = self.watcher.lock() {
                for dir in dirs.drain() {
                    let _ = watcher.unwatch(&dir);
                }
            }
        }
        if let Ok(mut slot) = self.worker_thread.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for SafeWatcher {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

pub fn should_ignore_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    let lower = file_name.to_ascii_lowercase();
    // Ignorar archivos temporales de Clip Studio Paint, copias intermedias de Atenea y ocultos
    lower.starts_with('.')
        || lower.starts_with("~$")
        || lower.starts_with('~')
        || lower.ends_with(".tmp")
        || lower.contains(".atenea_restore.")
}

fn collect_supported_files(dir: &Path, store: &AteneaStore, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if should_ignore_path(&path) {
            continue;
        }
        if path.is_dir() {
            collect_supported_files(&path, store, out);
        } else if store.find_adapter(&path).is_some() {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atenea_clip::synthetic::{write_clip_file, SyntheticClipSpec};
    use atenea_clip::ClipAdapter;
    use std::sync::mpsc;
    use tempfile::tempdir;

    #[test]
    fn watches_csp_style_temp_rename_and_catches_corrupt_save() -> Result<()> {
        let dir = tempdir()?;
        let store_dir = dir.path().join("store");
        let art_dir = dir.path().join("dibujos");
        fs::create_dir_all(&art_dir)?;

        let store = Arc::new(AteneaStore::open(
            &store_dir,
            vec![Arc::new(ClipAdapter::new())],
        )?);

        let (tx, rx) = mpsc::channel();
        let watcher = SafeWatcher::start(
            Arc::clone(&store),
            WatcherConfig {
                quiescence_duration: Duration::from_millis(300),
                stability_probe_delay: Duration::from_millis(80),
                tick_interval: Duration::from_millis(60),
            },
            move |ev| {
                let _ = tx.send(ev);
            },
        )?;

        watcher.watch_folder(&art_dir)?;

        // Simular el mecanismo exacto de guardado de Clip Studio Paint:
        // 1) Escribir a un archivo .tmp (debe ser ignorado por Atenea)
        // 2) Renombrar atómicamente a .clip
        let tmp_save = art_dir.join("csp_saving_8812.tmp");
        let final_clip = art_dir.join("retrato.clip");
        let spec = SyntheticClipSpec::sample_v1();
        write_clip_file(&tmp_save, &spec)?;
        fs::rename(&tmp_save, &final_clip)?;

        // Esperar evento SnapshotCreated
        let mut created_commit = None;
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(250)) {
                if let WatcherEvent::SnapshotCreated { commit } = ev {
                    created_commit = Some(commit);
                    break;
                }
            }
        }
        let c1 = created_commit.expect("SafeWatcher debió capturar el guardado .tmp -> .clip");
        assert_eq!(c1.display_branch_check(), "main");

        // Ahora simular un crash a mitad de guardado dejando un .clip truncado
        fs::write(&final_clip, b"CSFCHUNK_TRUNCATED_CRASH")?;

        let mut corrupt_detected = false;
        let deadline2 = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline2 {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(250)) {
                if let WatcherEvent::CorruptedSaveDetected { .. } = ev {
                    corrupt_detected = true;
                    break;
                }
            }
        }
        assert!(
            corrupt_detected,
            "SafeWatcher debió detectar y bloquear el guardado corrupto"
        );

        // Confirmar que el historial sano conserva exactamente 1 commit válido
        let commits = store.list_commits(&c1.project_id, "main")?;
        assert_eq!(commits.len(), 1);

        watcher.shutdown();
        Ok(())
    }

    trait CommitTestExt {
        fn display_branch_check(&self) -> &str;
    }
    impl CommitTestExt for CommitRecord {
        fn display_branch_check(&self) -> &str {
            &self.branch_name
        }
    }
}
