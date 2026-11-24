#!/usr/bin/env node
/**
 * Genera el par de llaves Minisign de Tauri (~/.tauri/atenea.key) e inyecta
 * automáticamente la llave pública en apps/desktop/src-tauri/tauri.conf.json.
 *
 * Uso:
 *   npm --prefix apps/desktop run updater:keygen
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");
const desktopDir = path.join(repoRoot, "apps", "desktop");
const tauriConfPath = path.join(desktopDir, "src-tauri", "tauri.conf.json");

const force = process.argv.includes("--force");
const keyDir = path.join(os.homedir(), ".tauri");
const keyPath = path.join(keyDir, "atenea.key");
const pubKeyPath = `${keyPath}.pub`;

fs.mkdirSync(keyDir, { recursive: true });

if (!fs.existsSync(keyPath) || force) {
  console.log(`[Atenea] Generando llave privada de firma en: ${keyPath}`);
  const npxBin = process.platform === "win32" ? "npx.cmd" : "npx";
  const args = ["tauri", "signer", "generate", "--ci", "-w", keyPath];
  if (force) args.push("-f");
  execFileSync(npxBin, args, {
    cwd: desktopDir,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
} else {
  console.log(`[Atenea] Reutilizando llave existente en: ${keyPath} (usa --force para regenerar)`);
}

if (!fs.existsSync(pubKeyPath)) {
  console.error(`[Atenea] Error: no se encontró ${pubKeyPath}`);
  process.exit(1);
}

const pubKey = fs.readFileSync(pubKeyPath, "utf8").trim();
const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, "utf8"));
tauriConf.plugins = tauriConf.plugins || {};
tauriConf.plugins.updater = tauriConf.plugins.updater || {};
tauriConf.plugins.updater.pubkey = pubKey;

const ghRepo = process.env.ATENEA_GITHUB_REPO?.trim();
if (ghRepo && ghRepo.includes("/")) {
  tauriConf.plugins.updater.endpoints = [
    `https://github.com/${ghRepo}/releases/latest/download/latest.json`,
  ];
}

fs.writeFileSync(tauriConfPath, `${JSON.stringify(tauriConf, null, 2)}\n`, "utf8");

console.log("\n✅ Llave pública actualizada en apps/desktop/src-tauri/tauri.conf.json");
console.log("🔐 Configura este Secret en tu repositorio de GitHub (Settings -> Secrets -> Actions):");
console.log("   - Nombre: TAURI_SIGNING_PRIVATE_KEY");
console.log(`   - Valor:  contenido completo de ${keyPath}`);
console.log("   - (Opcional) TAURI_SIGNING_PRIVATE_KEY_PASSWORD si definiste contraseña.\n");
