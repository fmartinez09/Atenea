#!/usr/bin/env node
/**
 * Publicador desacoplado de Releases para Atenea (Tauri v2).
 *
 * Implementa el patrón ReleaseStorageAdapter con 3 modos intercambiables por entorno:
 *   - RELEASE_STORAGE=github (default hoy mientras pruebas: 100% GitHub Releases, cero infra externa)
 *   - RELEASE_STORAGE=r2     (100% Cloudflare R2 vía S3 API)
 *   - RELEASE_STORAGE=both   (Binarios en Cloudflare R2 + latest.json y notas espejo en GitHub Releases)
 *
 * Publicación en 2 fases:
 *   1) Sube todos los binarios firmados y SHA256SUMS.txt.
 *   2) Construye y publica atómicamente `latest.json` al final (evitando condiciones de carrera).
 */
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");

function parseArgs(argv) {
  const opts = {
    dryRun: argv.includes("--dry-run"),
    channel: process.env.RELEASE_CHANNEL || "canary",
    storage: (process.env.RELEASE_STORAGE || "github").toLowerCase(),
    version: process.env.RELEASE_VERSION || "",
    notes: process.env.RELEASE_NOTES || "",
    bundleRoot:
      process.env.BUNDLE_ROOT ||
      path.join(repoRoot, "target", "release", "bundle"),
    githubRepo: process.env.GITHUB_REPOSITORY || "fmartinez09/Atenea",
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--channel" && argv[i + 1]) opts.channel = argv[++i];
    else if (arg === "--storage" && argv[i + 1]) opts.storage = argv[++i].toLowerCase();
    else if (arg === "--version" && argv[i + 1]) opts.version = argv[++i];
    else if (arg === "--notes" && argv[i + 1]) opts.notes = argv[++i];
    else if (arg === "--bundle-root" && argv[i + 1]) opts.bundleRoot = path.resolve(argv[++i]);
    else if (arg === "--repo" && argv[i + 1]) opts.githubRepo = argv[++i];
  }

  opts.channel = opts.channel === "stable" ? "stable" : "canary";
  if (!["github", "r2", "both"].includes(opts.storage)) {
    throw new Error(`RELEASE_STORAGE inválido: '${opts.storage}'. Usa github | r2 | both.`);
  }
  return opts;
}

function readWorkspaceVersion() {
  const tauriConfPath = path.join(
    repoRoot,
    "apps",
    "desktop",
    "src-tauri",
    "tauri.conf.json"
  );
  const conf = JSON.parse(fs.readFileSync(tauriConfPath, "utf8"));
  return conf.version;
}

function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(filePath));
  return hash.digest("hex");
}

/**
 * Descubre artefactos firmados de actualización generados por `tauri build` (`createUpdaterArtifacts: true`).
 */
function discoverSignedUpdaterArtifacts(bundleRoot, dryRun = false) {
  const found = [];
  const candidates = [
    {
      platformKey: "windows-x86_64",
      dir: path.join(bundleRoot, "nsis"),
      match: (name) => name.endsWith("-setup.exe") && !name.endsWith(".sig"),
    },
    {
      platformKey: "darwin-aarch64",
      dir: path.join(bundleRoot, "macos"),
      match: (name) => name.endsWith(".app.tar.gz") && name.includes("aarch64"),
    },
    {
      platformKey: "darwin-x86_64",
      dir: path.join(bundleRoot, "macos"),
      match: (name) => name.endsWith(".app.tar.gz") && !name.includes("aarch64"),
    },
    {
      platformKey: "linux-x86_64",
      dir: path.join(bundleRoot, "appimage"),
      match: (name) => name.endsWith(".AppImage") && !name.endsWith(".sig"),
    },
  ];

  for (const spec of candidates) {
    if (!fs.existsSync(spec.dir)) continue;
    for (const entry of fs.readdirSync(spec.dir)) {
      if (!spec.match(entry)) continue;
      const installerPath = path.join(spec.dir, entry);
      const sigPath = `${installerPath}.sig`;
      const sigExists = fs.existsSync(sigPath);
      if (!sigExists && !dryRun) {
        throw new Error(
          `Se encontró ${installerPath} pero falta su firma ${sigPath}. Verifica TAURI_SIGNING_PRIVATE_KEY.`
        );
      }
      found.push({
        platformKey: spec.platformKey,
        fileName: entry,
        installerPath,
        sigPath,
        signature: sigExists
          ? fs.readFileSync(sigPath, "utf8").trim()
          : "DRY_RUN_UNSIGNED_LOCAL_BUNDLE",
        sha256: sha256File(installerPath),
        sizeBytes: fs.statSync(installerPath).size,
      });
    }
  }

  return found;
}

// ============================================================================
// Adapter 1: GitHub Releases (activo por defecto mientras pruebas)
// ============================================================================
class GithubReleaseAdapter {
  constructor({ repo, channel, version, dryRun }) {
    this.repo = repo;
    this.channel = channel;
    this.version = version;
    // En modo canary usamos el tag rodante `canary` para que
    // https://github.com/<repo>/releases/download/canary/latest.json sea estable.
    this.releaseTag = channel === "canary" ? "canary" : `v${version}`;
    this.dryRun = dryRun;
  }

  ensureDraftOrRollingRelease(notes) {
    if (this.dryRun) {
      console.log(`[DryRun][GitHub] Asegurando release '${this.releaseTag}' en ${this.repo}`);
      return;
    }

    let exists = true;
    try {
      execFileSync("gh", ["release", "view", this.releaseTag, "--repo", this.repo], {
        stdio: "ignore",
      });
    } catch {
      exists = false;
    }

    if (!exists) {
      const args = [
        "release",
        "create",
        this.releaseTag,
        "--repo",
        this.repo,
        "--title",
        this.channel === "canary"
          ? `Atenea Canary (v${this.version})`
          : `Atenea v${this.version}`,
        "--notes",
        notes,
      ];
      if (this.channel === "canary") {
        args.push("--prerelease");
      } else {
        args.push("--draft");
      }
      execFileSync("gh", args, { stdio: "inherit" });
    }
  }

  uploadVersionedAsset(localPath, publicFileName) {
    // Asegurar nombre limpio si difiere
    const uploadTarget =
      path.basename(localPath) === publicFileName ? localPath : `${localPath}#${publicFileName}`;
    const url = `https://github.com/${this.repo}/releases/download/${this.releaseTag}/${encodeURIComponent(
      publicFileName
    )}`;

    if (this.dryRun) {
      console.log(`[DryRun][GitHub] Upload ${localPath} -> ${url}`);
      return url;
    }

    execFileSync(
      "gh",
      ["release", "upload", this.releaseTag, uploadTarget, "--clobber", "--repo", this.repo],
      { stdio: "inherit" }
    );
    return url;
  }

  publishChannelManifest(manifestPath, notes) {
    if (this.dryRun) {
      console.log(
        `[DryRun][GitHub] Publicando latest.json en release '${this.releaseTag}' y marcando release listo`
      );
      return;
    }

    execFileSync(
      "gh",
      ["release", "upload", this.releaseTag, manifestPath, "--clobber", "--repo", this.repo],
      { stdio: "inherit" }
    );

    const editArgs = [
      "release",
      "edit",
      this.releaseTag,
      "--repo",
      this.repo,
      "--title",
      this.channel === "canary"
        ? `Atenea Canary (v${this.version})`
        : `Atenea v${this.version}`,
      "--notes",
      notes,
    ];
    if (this.channel === "stable") {
      editArgs.push("--draft=false", "--latest");
    } else {
      editArgs.push("--prerelease");
    }
    execFileSync("gh", editArgs, { stdio: "inherit" });
  }
}

// ============================================================================
// Adapter 2: Cloudflare R2 (S3-compatible, $0 egress)
// ============================================================================
class R2StorageAdapter {
  constructor({ channel, version, dryRun }) {
    this.channel = channel;
    this.version = version;
    this.dryRun = dryRun;

    const accountId = process.env.R2_ACCOUNT_ID || "";
    this.endpoint =
      process.env.R2_S3_ENDPOINT ||
      (accountId ? `https://${accountId}.r2.cloudflarestorage.com` : "");
    this.bucket = process.env.R2_BUCKET || "atenea-releases";
    this.publicBaseUrl = (process.env.R2_PUBLIC_URL || "").replace(/\/+$/, "");

    if (!dryRun && (!this.endpoint || !this.publicBaseUrl)) {
      throw new Error(
        "Para usar RELEASE_STORAGE=r2 o both debes definir R2_PUBLIC_URL y R2_ACCOUNT_ID (o R2_S3_ENDPOINT), además de AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY."
      );
    }
  }

  uploadVersionedAsset(localPath, publicFileName) {
    const objectKey = `versions/v${this.version}/${publicFileName}`;
    const publicBase = this.publicBaseUrl || "https://releases.example.com";
    const publicUrl = `${publicBase}/${objectKey}`;

    if (this.dryRun) {
      console.log(`[DryRun][R2] aws s3 cp ${localPath} -> s3://${this.bucket}/${objectKey} (immutable)`);
      return publicUrl;
    }

    execFileSync(
      "aws",
      [
        "s3",
        "cp",
        localPath,
        `s3://${this.bucket}/${objectKey}`,
        "--endpoint-url",
        this.endpoint,
        "--cache-control",
        "public, max-age=31536000, immutable",
      ],
      {
        stdio: "inherit",
        env: { ...process.env, AWS_DEFAULT_REGION: "auto" },
      }
    );
    return publicUrl;
  }

  publishChannelManifest(manifestPath) {
    const objectKey = `channels/${this.channel}/latest.json`;
    if (this.dryRun) {
      console.log(
        `[DryRun][R2] aws s3 cp ${manifestPath} -> s3://${this.bucket}/${objectKey} (no-cache)`
      );
      return;
    }

    execFileSync(
      "aws",
      [
        "s3",
        "cp",
        manifestPath,
        `s3://${this.bucket}/${objectKey}`,
        "--endpoint-url",
        this.endpoint,
        "--content-type",
        "application/json",
        "--cache-control",
        "no-cache, no-store, must-revalidate",
      ],
      {
        stdio: "inherit",
        env: { ...process.env, AWS_DEFAULT_REGION: "auto" },
      }
    );
  }
}

function buildReleaseNotesMarkdown(version, channel, artifacts, downloadUrlsByPlatform, userNotes) {
  const header =
    userNotes && userNotes.trim()
      ? userNotes.trim()
      : `Actualización de Atenea v${version} (${channel}). Tu historial local de snapshots permanece intacto.`;

  const rows = artifacts.map((art) => {
    const mb = (art.sizeBytes / (1024 * 1024)).toFixed(2);
    const url = downloadUrlsByPlatform[art.platformKey];
    return `| \`${art.platformKey}\` | [${art.fileName}](${url}) | ${mb} MB | \`${art.sha256.slice(
      0,
      16
    )}…\` |`;
  });

  return [
    header,
    "",
    "### Descargas verificadas",
    "| Plataforma | Instalador | Tamaño | SHA-256 |",
    "| :--- | :--- | :--- | :--- |",
    ...rows,
  ].join("\n");
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const version = (opts.version || readWorkspaceVersion()).replace(/^v/, "");

  let artifacts = discoverSignedUpdaterArtifacts(opts.bundleRoot, opts.dryRun);
  if (artifacts.length === 0 && opts.dryRun) {
    console.log(
      `[DryRun] No hay binarios compilados en ${opts.bundleRoot}; simulando artefacto Windows NSIS.`
    );
    artifacts = [
      {
        platformKey: "windows-x86_64",
        fileName: `Atenea_${version}_x64-setup.exe`,
        installerPath: path.join(opts.bundleRoot, "nsis", `Atenea_${version}_x64-setup.exe`),
        sigPath: path.join(opts.bundleRoot, "nsis", `Atenea_${version}_x64-setup.exe.sig`),
        signature: "DRY_RUN_MINISIGN_SIGNATURE",
        sha256: "0".repeat(64),
        sizeBytes: 11_500_000,
      },
    ];
  } else if (artifacts.length === 0) {
    throw new Error(`No se encontraron bundles firmados dentro de ${opts.bundleRoot}`);
  }

  const outTempDir = fs.mkdtempSync(path.join(os.tmpdir(), "atenea-release-manifest-"));
  try {
    const ghAdapter =
      opts.storage === "github" || opts.storage === "both"
        ? new GithubReleaseAdapter({
            repo: opts.githubRepo,
            channel: opts.channel,
            version,
            dryRun: opts.dryRun,
          })
        : null;

    const r2Adapter =
      opts.storage === "r2" || opts.storage === "both"
        ? new R2StorageAdapter({
            channel: opts.channel,
            version,
            dryRun: opts.dryRun,
          })
        : null;

    // En modo "both" los binarios viven en R2 (cero costo de salida) y GitHub actúa como espejo de latest.json y notas
    const binaryTargetAdapter = r2Adapter || ghAdapter;

    if (ghAdapter) {
      ghAdapter.ensureDraftOrRollingRelease(opts.notes || `Preparando Atenea v${version}…`);
    }

    const sumsLines = [];
    const platformsManifest = {};
    const downloadUrlsByPlatform = {};

    for (const art of artifacts) {
      // Para el tag rodante canary evitamos colisiones incluyendo versión si hace falta, o mantenemos nombre limpio
      const url = binaryTargetAdapter.uploadVersionedAsset(art.installerPath, art.fileName);
      if (!opts.dryRun && fs.existsSync(art.sigPath)) {
        binaryTargetAdapter.uploadVersionedAsset(art.sigPath, `${art.fileName}.sig`);
      }
      downloadUrlsByPlatform[art.platformKey] = url;
      platformsManifest[art.platformKey] = {
        signature: art.signature,
        url,
      };
      sumsLines.push(`${art.sha256}  ${art.fileName}`);
    }

    const sumsPath = path.join(outTempDir, "SHA256SUMS.txt");
    fs.writeFileSync(sumsPath, `${sumsLines.join("\n")}\n`, "utf8");
    binaryTargetAdapter.uploadVersionedAsset(sumsPath, "SHA256SUMS.txt");

    const markdownNotes = buildReleaseNotesMarkdown(
      version,
      opts.channel,
      artifacts,
      downloadUrlsByPlatform,
      opts.notes
    );

    const latestManifest = {
      version,
      notes:
        opts.notes && opts.notes.trim()
          ? opts.notes.trim()
          : `Atenea v${version} (${opts.channel})`,
      pub_date: new Date().toISOString(),
      platforms: platformsManifest,
    };

    const manifestPath = path.join(outTempDir, "latest.json");
    fs.writeFileSync(manifestPath, `${JSON.stringify(latestManifest, null, 2)}\n`, "utf8");

    // Fase 2 (Atómica): Publicar latest.json al final cuando todos los binarios ya están arriba
    if (r2Adapter) {
      r2Adapter.publishChannelManifest(manifestPath);
    }
    if (ghAdapter) {
      ghAdapter.publishChannelManifest(manifestPath, markdownNotes);
    }

    console.log(`\n✅ Release v${version} (${opts.channel}) listo vía adapter '${opts.storage}'.`);
    if (opts.dryRun) {
      console.log("📄 Vista previa de latest.json generado:\n", JSON.stringify(latestManifest, null, 2));
    }
  } finally {
    fs.rmSync(outTempDir, { recursive: true, force: true });
  }
}

main();
