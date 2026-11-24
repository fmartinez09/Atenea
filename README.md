# Atenea (v0.1 — Hito 1 MVP)

**Time Machine visual y blindado contra corrupción para Clip Studio Paint (`.clip`)**, construido en **Rust + Tauri v2**.

## Qué incluye esta versión (`v0.1`)

- **Protocolo Zero-Corruption en Windows (`atenea-watcher`):** Vigila carpetas `.clip` ignorando archivos `.tmp`, esperando ventana de enfriamiento (*quiescence debounce*) y abriendo siempre con `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` para jamás bloquear a Clip Studio Paint.
- **Compuerta de Integridad (`atenea-clip`):** Verifica cabeceras `CSFCHUNK`, pie `CHNKFoot` y salud interna de la base SQLite `CHNKSQLi` (`PRAGMA quick_check`). Si un guardado queda truncado por un cierre inesperado, Atenea bloquea la ingesta corrupta y protege las versiones previas.
- **Deduplicación por Bloques (`atenea-core`):** Almacén direccionado por contenido (CAS) con **FastCDC (`v2020`) + BLAKE3** e índice SQLite preparado para ramas (`DAG`).
- **Inspección Semántica y Visual Diff (`atenea-diff` + `apps/desktop`):** Extrae en solo lectura `CanvasPreview` (PNG) y el árbol de capas desde `CHNKSQLi`, permitiendo comparar versiones con **Cortina A/B**, **Papel Cebolla** y **Mapa de Diferencias de Píxeles**, además de restaurar copias verificadas bit a bit.

## Estructura del Repositorio

- `crates/atenea-core` — CAS local (FastCDC + BLAKE3), grafo SQLite (`commits` + `branches`), traits `BlobStore` y `AssetAdapter`, motor de restauración verificada.
- `crates/atenea-clip` — Parser de contenedores `CSFCHUNK`, inspector *read-only* de `CHNKSQLi` y generador sintético para pruebas.
- `crates/atenea-watcher` — Monitor no intrusivo para Windows.
- `crates/atenea-diff` — Motor de diferencias visuales y semánticas de capas.
- `crates/atenea-cli` — Herramienta de línea de comandos (`atenea`).
- `apps/desktop` — Aplicación de escritorio en **Tauri v2** (React + TypeScript + Tailwind CSS + System Tray).

## Comandos Rápidos

### Ejecutar la App de Escritorio (Modo Desarrollo)
```powershell
cd apps/desktop
npm run tauri dev
```

### Usar la CLI directamente
```powershell
cargo run -p atenea-cli -- demo-seed .atenea-dev/arte --store .atenea-dev/store
cargo run -p atenea-cli -- inspect .atenea-dev/arte/ilustracion_atenea_demo.clip
cargo run -p atenea-cli -- log --store .atenea-dev/store
```

### Ejecutar Tests Automáticos
```powershell
cargo test --workspace
```

## Releases y Auto-Actualización Segura (`GitHub Releases` → `Cloudflare R2`)

- **Historial local blindado al actualizar:** El almacén vive en `%LOCALAPPDATA%\app.atenea.desktop\store` (aislado de `%LOCALAPPDATA%\Atenea` donde instala NSIS), migra automáticamente almacenes previos y genera respaldos verificados (`VACUUM INTO`) en `backups/` antes de instalar una nueva versión.
- **Protección de trabajo en curso:** Antes de actualizar, Atenea detecta si `CLIPStudioPaint.exe` está abierto o si hay cambios en disco sin snapshot, permite capturarlos con un clic y exige confirmación explícita de guardado.
- **Generar llaves de firma Minisign (una sola vez):**
  ```powershell
  npm --prefix apps/desktop run updater:keygen
  ```
  Guarda el contenido de `~/.tauri/atenea.key` en el secret `TAURI_SIGNING_PRIVATE_KEY` de GitHub.
- **Publicar desde GitHub Actions (`.github/workflows/release.yml`):** Lanza `Release Atenea Desktop` (`workflow_dispatch`) eligiendo canal (`canary` o `stable`) y `storage_adapter` (`github` hoy mientras pruebas; `r2` o `both` cuando actives Cloudflare R2).
