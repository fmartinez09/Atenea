use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use atenea_core::PreUpdateWorkspaceAudit;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

use crate::AppState;

/// Llave pública placeholder de desarrollo incluida por defecto en `tauri.conf.json`.
/// Se reemplaza con `npm run updater:keygen` antes de publicar releases firmados reales.
pub const DEV_PLACEHOLDER_PUBKEY: &str =
    "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY4NjEyRURFQkIwREFDRDUKUldUVnJBMjczaTVoK0ZFQTlaOU1VVTVseFJ1RVdBK2JRakM1bFBUNnNVRkRxM1VOMEtiL0phYXcK";

pub const DEFAULT_GITHUB_REPO: &str = "fmartinez09/Atenea";

static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static EXITING_FOR_UPDATE: AtomicBool = AtomicBool::new(false);

struct UpdateGuardReset;
impl Drop for UpdateGuardReset {
    fn drop(&mut self) {
        UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

pub fn is_exiting_for_update() -> bool {
    EXITING_FOR_UPDATE.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdaterSettings {
    pub channel: String,
    pub github_repo: String,
    pub r2_public_url: Option<String>,
    pub custom_pubkey: Option<String>,
    pub auto_check: bool,
}

impl Default for UpdaterSettings {
    fn default() -> Self {
        let env_repo = env::var("ATENEA_GITHUB_REPO")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_GITHUB_REPO.to_string());

        let env_r2 = env::var("ATENEA_R2_PUBLIC_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty());

        let env_pubkey = env::var("ATENEA_UPDATER_PUBKEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Self {
            channel: "stable".to_string(),
            github_repo: env_repo,
            r2_public_url: env_r2,
            custom_pubkey: env_pubkey,
            auto_check: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveUpdaterSettingsInput {
    pub channel: String,
    pub github_repo: String,
    pub r2_public_url: Option<String>,
    pub custom_pubkey: Option<String>,
    pub auto_check: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdaterSettingsDto {
    pub current_version: String,
    pub channel: String,
    pub github_repo: String,
    pub r2_public_url: Option<String>,
    pub custom_pubkey: Option<String>,
    pub auto_check: bool,
    pub active_adapter_mode: String,
    pub resolved_endpoints: Vec<String>,
    pub using_dev_placeholder_key: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailableUpdateDto {
    pub version: String,
    pub current_version: String,
    pub channel: String,
    pub pub_date: Option<String>,
    pub notes: Option<String>,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheckResponse {
    pub current_version: String,
    pub channel: String,
    pub active_adapter_mode: String,
    pub endpoints: Vec<String>,
    /// "available" | "up_to_date" | "no_published_release" | "error"
    pub status: String,
    pub update: Option<AvailableUpdateDto>,
    pub message: Option<String>,
    pub using_dev_placeholder_key: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatePreflightReport {
    pub clip_studio_running: bool,
    pub pending_watcher_files: Vec<String>,
    pub audit: PreUpdateWorkspaceAudit,
    pub update_in_progress: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateProgressPayload {
    /// "downloading" | "capturing_snapshots" | "stopping_watcher" | "backing_up_db" | "installing" | "error"
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: String,
}

fn settings_file_path() -> PathBuf {
    atenea_core::resolve_app_data_root().join("updater-settings.json")
}

pub fn load_settings() -> UpdaterSettings {
    let path = settings_file_path();
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(mut parsed) = serde_json::from_str::<UpdaterSettings>(&raw) {
            parsed.channel = normalize_channel(&parsed.channel);
            parsed.github_repo = parsed.github_repo.trim().trim_matches('/').to_string();
            parsed.r2_public_url = parsed
                .r2_public_url
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty());
            parsed.custom_pubkey = parsed
                .custom_pubkey
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return parsed;
        }
    }
    UpdaterSettings::default()
}

fn persist_settings(settings: &UpdaterSettings) -> Result<(), String> {
    let path = settings_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

fn normalize_channel(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "canary" | "beta" | "nightly" => "canary".to_string(),
        _ => "stable".to_string(),
    }
}

/// Adapter de resolución de endpoints:
/// - Mientras pruebas sin R2: consulta GitHub Releases (`latest` para `stable`, tag `canary` para pruebas).
/// - Al configurar `r2_public_url`: Cloudflare R2 pasa a ser primario y GitHub Releases actúa como fallback automático.
pub fn resolve_endpoint_urls(settings: &UpdaterSettings, channel_override: Option<&str>) -> (String, Vec<Url>) {
    let channel = channel_override
        .map(normalize_channel)
        .unwrap_or_else(|| normalize_channel(&settings.channel));

    let mut urls: Vec<Url> = Vec::new();
    let has_r2 = if let Some(ref r2_base) = settings.r2_public_url {
        let candidate = format!("{}/channels/{}/latest.json", r2_base.trim_end_matches('/'), channel);
        if let Ok(u) = Url::parse(&candidate) {
            urls.push(u);
            true
        } else {
            false
        }
    } else {
        false
    };

    let repo = settings.github_repo.trim().trim_matches('/');
    let has_github = if !repo.is_empty() && repo.contains('/') {
        let gh_url = if channel == "canary" {
            format!("https://github.com/{}/releases/download/canary/latest.json", repo)
        } else {
            format!("https://github.com/{}/releases/latest/download/latest.json", repo)
        };
        if let Ok(u) = Url::parse(&gh_url) {
            urls.push(u);
            true
        } else {
            false
        }
    } else {
        false
    };

    let mode = match (has_r2, has_github) {
        (true, true) => "r2_with_github_fallback",
        (true, false) => "r2_only",
        (false, true) => "github",
        (false, false) => "unconfigured",
    };

    (mode.to_string(), urls)
}

fn is_using_placeholder_key(app: &AppHandle, settings: &UpdaterSettings) -> bool {
    if let Some(ref custom) = settings.custom_pubkey {
        return custom.trim() == DEV_PLACEHOLDER_PUBKEY.trim();
    }
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .map(|pk| pk.trim() == DEV_PLACEHOLDER_PUBKEY.trim())
        .unwrap_or(true)
}

fn build_settings_dto(app: &AppHandle, settings: &UpdaterSettings) -> UpdaterSettingsDto {
    let (mode, urls) = resolve_endpoint_urls(settings, None);
    UpdaterSettingsDto {
        current_version: app.package_info().version.to_string(),
        channel: settings.channel.clone(),
        github_repo: settings.github_repo.clone(),
        r2_public_url: settings.r2_public_url.clone(),
        custom_pubkey: settings.custom_pubkey.clone(),
        auto_check: settings.auto_check,
        active_adapter_mode: mode,
        resolved_endpoints: urls.into_iter().map(|u| u.to_string()).collect(),
        using_dev_placeholder_key: is_using_placeholder_key(app, settings),
    }
}

pub fn detect_clip_studio_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq CLIPStudioPaint.exe", "/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
            return stdout.contains("clipstudiopaint.exe");
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub async fn perform_update_check(
    app: &AppHandle,
    channel_override: Option<String>,
    emit_if_available: bool,
) -> UpdateCheckResponse {
    let settings = load_settings();
    let channel = channel_override
        .as_deref()
        .map(normalize_channel)
        .unwrap_or_else(|| normalize_channel(&settings.channel));
    let (mode, urls) = resolve_endpoint_urls(&settings, Some(&channel));
    let endpoint_strings: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
    let current_version = app.package_info().version.to_string();
    let using_placeholder = is_using_placeholder_key(app, &settings);

    if urls.is_empty() {
        return UpdateCheckResponse {
            current_version,
            channel,
            active_adapter_mode: mode,
            endpoints: endpoint_strings,
            status: "error".into(),
            update: None,
            message: Some("Configura un repositorio GitHub (owner/repo) o una URL pública de Cloudflare R2.".into()),
            using_dev_placeholder_key: using_placeholder,
        };
    }

    let mut builder = app.updater_builder();
    if let Some(ref custom_pk) = settings.custom_pubkey {
        builder = builder.pubkey(custom_pk.clone());
    }
    let builder = match builder.endpoints(urls) {
        Ok(b) => b,
        Err(err) => {
            return UpdateCheckResponse {
                current_version,
                channel,
                active_adapter_mode: mode,
                endpoints: endpoint_strings,
                status: "error".into(),
                update: None,
                message: Some(format!("Endpoints de actualización inválidos: {}", err)),
                using_dev_placeholder_key: using_placeholder,
            };
        }
    };

    let updater = match builder.build() {
        Ok(u) => u,
        Err(err) => {
            return UpdateCheckResponse {
                current_version,
                channel,
                active_adapter_mode: mode,
                endpoints: endpoint_strings,
                status: "error".into(),
                update: None,
                message: Some(format!("No se pudo inicializar el actualizador: {}", err)),
                using_dev_placeholder_key: using_placeholder,
            };
        }
    };

    match updater.check().await {
        Ok(Some(upd)) => {
            let dto = AvailableUpdateDto {
                version: upd.version.clone(),
                current_version: upd.current_version.clone(),
                channel: channel.clone(),
                pub_date: upd.date.map(|d| d.to_string()),
                notes: upd.body.clone(),
                download_url: upd.download_url.to_string(),
            };
            if emit_if_available {
                let _ = app.emit("atenea://update-available", &dto);
            }
            UpdateCheckResponse {
                current_version,
                channel,
                active_adapter_mode: mode,
                endpoints: endpoint_strings,
                status: "available".into(),
                update: Some(dto),
                message: None,
                using_dev_placeholder_key: using_placeholder,
            }
        }
        Ok(None) => UpdateCheckResponse {
            current_version,
            channel,
            active_adapter_mode: mode,
            endpoints: endpoint_strings,
            status: "up_to_date".into(),
            update: None,
            message: Some("Ya tienes la versión más reciente instalada.".into()),
            using_dev_placeholder_key: using_placeholder,
        },
        Err(err) => {
            let raw = err.to_string();
            let lower = raw.to_ascii_lowercase();
            let is_missing_release = lower.contains("404")
                || lower.contains("not found")
                || lower.contains("release")
                || lower.contains("valid json");

            if is_missing_release {
                UpdateCheckResponse {
                    current_version,
                    channel: channel.clone(),
                    active_adapter_mode: mode,
                    endpoints: endpoint_strings,
                    status: "no_published_release".into(),
                    update: None,
                    message: Some(format!(
                        "Aún no hay un release publicado con latest.json en el canal '{}'.",
                        channel
                    )),
                    using_dev_placeholder_key: using_placeholder,
                }
            } else {
                UpdateCheckResponse {
                    current_version,
                    channel,
                    active_adapter_mode: mode,
                    endpoints: endpoint_strings,
                    status: "error".into(),
                    update: None,
                    message: Some(format!("No se pudo consultar actualizaciones: {}", raw)),
                    using_dev_placeholder_key: using_placeholder,
                }
            }
        }
    }
}

fn emit_progress(
    app: &AppHandle,
    phase: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: impl Into<String>,
) {
    let _ = app.emit(
        "atenea://update-progress",
        UpdateProgressPayload {
            phase: phase.to_string(),
            downloaded_bytes,
            total_bytes,
            message: message.into(),
        },
    );
}

#[tauri::command]
pub fn get_updater_settings(app: AppHandle) -> UpdaterSettingsDto {
    let settings = load_settings();
    build_settings_dto(&app, &settings)
}

#[tauri::command]
pub fn save_updater_settings(
    input: SaveUpdaterSettingsInput,
    app: AppHandle,
) -> Result<UpdaterSettingsDto, String> {
    let updated = UpdaterSettings {
        channel: normalize_channel(&input.channel),
        github_repo: input.github_repo.trim().trim_matches('/').to_string(),
        r2_public_url: input
            .r2_public_url
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty()),
        custom_pubkey: input
            .custom_pubkey
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        auto_check: input.auto_check,
    };
    persist_settings(&updated)?;
    Ok(build_settings_dto(&app, &updated))
}

#[tauri::command]
pub async fn check_app_update(
    channel_override: Option<String>,
    app: AppHandle,
) -> Result<UpdateCheckResponse, String> {
    Ok(perform_update_check(&app, channel_override, false).await)
}

#[tauri::command]
pub fn preflight_update_check(state: State<'_, AppState>) -> Result<UpdatePreflightReport, String> {
    let clip_studio_running = detect_clip_studio_running();
    let pending_watcher_files = state.watcher.pending_file_paths();
    let audit = state
        .store
        .inspect_workspace_for_update()
        .map_err(|e| e.to_string())?;
    Ok(UpdatePreflightReport {
        clip_studio_running,
        pending_watcher_files,
        audit,
        update_in_progress: UPDATE_IN_PROGRESS.load(Ordering::SeqCst),
    })
}

#[tauri::command]
pub fn flush_snapshots_before_update(
    state: State<'_, AppState>,
) -> Result<UpdatePreflightReport, String> {
    state
        .store
        .capture_all_pending_snapshots(Some("Snapshot preventivo previo a actualizar Atenea"))
        .map_err(|e| e.to_string())?;
    preflight_update_check(state)
}

#[tauri::command]
pub async fn install_app_update(
    user_confirmed_saved: bool,
    expected_version: Option<String>,
    channel_override: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !user_confirmed_saved {
        return Err(
            "Por seguridad, confirma primero que guardaste tus cambios en Clip Studio Paint."
                .into(),
        );
    }

    if UPDATE_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Ya hay una actualización en curso.".into());
    }
    let _reset_guard = UpdateGuardReset;

    let settings = load_settings();
    let channel = channel_override
        .as_deref()
        .map(normalize_channel)
        .unwrap_or_else(|| normalize_channel(&settings.channel));
    let (_, urls) = resolve_endpoint_urls(&settings, Some(&channel));
    if urls.is_empty() {
        return Err("No hay endpoints de actualización configurados.".into());
    }

    let mut builder = app.updater_builder();
    if let Some(ref custom_pk) = settings.custom_pubkey {
        builder = builder.pubkey(custom_pk.clone());
    }
    let updater = builder
        .endpoints(urls)
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;

    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Err("No hay actualizaciones pendientes en este momento.".into());
    };

    if let Some(ref expected) = expected_version {
        if &update.version != expected {
            return Err(format!(
                "La versión publicada cambió ({} -> {}). Vuelve a revisar las notas antes de instalar.",
                expected, update.version
            ));
        }
    }

    // 1. Descargar y verificar firma criptográfica Minisign ANTES de detener SafeWatcher.
    //    Si falla la red o la firma es inválida, la protección Zero-Corruption sigue activa sin interrupción.
    emit_progress(
        &app,
        "downloading",
        0,
        None,
        format!("Descargando y verificando firma de Atenea v{}…", update.version),
    );

    let mut accumulated: u64 = 0;
    let mut last_emit = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let app_for_dl = app.clone();
    let app_for_done = app.clone();
    let ver_label = update.version.clone();

    let verified_bytes = update
        .download(
            move |chunk_len, total_opt| {
                accumulated += chunk_len as u64;
                if last_emit.elapsed() >= Duration::from_millis(80)
                    || total_opt.map(|t| accumulated >= t).unwrap_or(false)
                {
                    last_emit = Instant::now();
                    emit_progress(
                        &app_for_dl,
                        "downloading",
                        accumulated,
                        total_opt,
                        format!("Descargando actualización v{}…", ver_label),
                    );
                }
            },
            move || {
                emit_progress(
                    &app_for_done,
                    "downloading",
                    0,
                    None,
                    "Firma criptográfica verificada correctamente.",
                );
            },
        )
        .await
        .map_err(|err| {
            emit_progress(
                &app,
                "error",
                0,
                None,
                format!("Descarga o firma rechazada: {}", err),
            );
            format!("No se pudo descargar/verificar el paquete: {}", err)
        })?;

    // 2. Capturar cualquier guardado .clip de último segundo en disco
    emit_progress(
        &app,
        "capturing_snapshots",
        0,
        None,
        "Asegurando últimos guardados .clip en tu historial local…",
    );
    let _ = state
        .store
        .capture_all_pending_snapshots(Some("Respaldo automático antes de actualizar Atenea"));

    // 3. Detener limpiamente SafeWatcher y liberar handles de carpetas
    emit_progress(
        &app,
        "stopping_watcher",
        0,
        None,
        "Pausando monitor de archivos de forma segura…",
    );
    state.watcher.shutdown();

    // 4. Crear respaldo verificado de index.db (blobs/ y previews/ quedan intactos fuera de $INSTDIR)
    emit_progress(
        &app,
        "backing_up_db",
        0,
        None,
        "Creando respaldo verificado del índice local (tu historial permanece intacto)…",
    );
    let current_ver = app.package_info().version.to_string();
    state
        .store
        .create_pre_update_backup(&current_ver, &update.version)
        .map_err(|e| {
            format!(
                "Se abortó la actualización porque no se pudo respaldar index.db: {}",
                e
            )
        })?;

    // 5. Permitir cierre limpio sin bloqueo de System Tray y aplicar instalador NSIS
    EXITING_FOR_UPDATE.store(true, Ordering::SeqCst);
    emit_progress(
        &app,
        "installing",
        0,
        None,
        "Aplicando actualización y reiniciando Atenea…",
    );

    if let Err(err) = update.install(verified_bytes) {
        EXITING_FOR_UPDATE.store(false, Ordering::SeqCst);
        emit_progress(
            &app,
            "error",
            0,
            None,
            format!("Fallo al ejecutar el instalador: {}", err),
        );
        return Err(format!("No se pudo instalar la actualización: {}", err));
    }

    app.restart();
}
