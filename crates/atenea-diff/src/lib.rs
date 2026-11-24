use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use atenea_core::{AssetMetadata, CommitRecord, LayerSummary};
use base64::Engine;
use image::{ImageBuffer, ImageFormat, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerChange {
    pub kind: String, // "added" | "removed" | "modified"
    pub layer_name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualDiffReport {
    pub base_commit_id: String,
    pub target_commit_id: String,
    pub preview_width: u32,
    pub preview_height: u32,
    pub changed_pixels: u64,
    pub total_pixels: u64,
    pub changed_ratio: f64,
    pub overall_bbox: Option<BoundingBox>,
    /// Data URL (`data:image/png;base64,...`) de la imagen anterior para render directo en UI
    pub base_image_data_url: Option<String>,
    /// Data URL (`data:image/png;base64,...`) de la imagen nueva
    pub target_image_data_url: Option<String>,
    /// Data URL (`data:image/png;base64,...`) del mapa de calor de píxeles alterados
    pub heatmap_data_url: Option<String>,
    pub layer_changes: Vec<LayerChange>,
}

pub fn compute_commit_diff(base: &CommitRecord, target: &CommitRecord) -> Result<VisualDiffReport> {
    let layer_changes = diff_layers(&base.metadata, &target.metadata);

    let base_bytes = base
        .preview_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.exists())
        .map(std::fs::read)
        .transpose()?;
    let target_bytes = target
        .preview_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.exists())
        .map(std::fs::read)
        .transpose()?;

    let base_data_url = base_bytes.as_deref().map(png_to_data_url);
    let target_data_url = target_bytes.as_deref().map(png_to_data_url);

    if let (Some(b_raw), Some(t_raw)) = (&base_bytes, &target_bytes) {
        let img_a = image::load_from_memory(b_raw).context("Error decodificando preview base")?;
        let img_b = image::load_from_memory(t_raw).context("Error decodificando preview nuevo")?;

        let width = img_a.width().max(img_b.width());
        let height = img_a.height().max(img_b.height());
        let rgba_a = img_a.to_rgba8();
        let rgba_b = img_b.to_rgba8();

        let mut changed_pixels = 0u64;
        let total_pixels = (width as u64) * (height as u64);
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0u32;
        let mut max_y = 0u32;

        let heatmap = ImageBuffer::from_fn(width, height, |x, y| {
            let pa = if x < rgba_a.width() && y < rgba_a.height() {
                *rgba_a.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 0])
            };
            let pb = if x < rgba_b.width() && y < rgba_b.height() {
                *rgba_b.get_pixel(x, y)
            } else {
                Rgba([0, 0, 0, 0])
            };

            let dr = (pa[0] as i32 - pb[0] as i32).unsigned_abs();
            let dg = (pa[1] as i32 - pb[1] as i32).unsigned_abs();
            let db = (pa[2] as i32 - pb[2] as i32).unsigned_abs();
            let da = (pa[3] as i32 - pb[3] as i32).unsigned_abs();
            let max_delta = dr.max(dg).max(db).max(da);

            if max_delta > 8 {
                changed_pixels += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                let intensity = (max_delta * 2).clamp(120, 255) as u8;
                // Magenta de alto contraste sobre versión nueva atenuada
                Rgba([255, 36, 148, intensity])
            } else {
                // Fondo atenuado en escala de grises suave para ubicar visualmente el cambio
                let luma = ((pb[0] as u32 * 299 + pb[1] as u32 * 587 + pb[2] as u32 * 114) / 1000) as u8;
                Rgba([luma / 2 + 24, luma / 2 + 24, luma / 2 + 28, 210])
            }
        });

        let mut cursor = Cursor::new(Vec::new());
        heatmap.write_to(&mut cursor, ImageFormat::Png)?;
        let heatmap_png = cursor.into_inner();

        let overall_bbox = if changed_pixels > 0 {
            Some(BoundingBox {
                x: min_x,
                y: min_y,
                width: max_x - min_x + 1,
                height: max_y - min_y + 1,
            })
        } else {
            None
        };

        let changed_ratio = if total_pixels > 0 {
            changed_pixels as f64 / total_pixels as f64
        } else {
            0.0
        };

        return Ok(VisualDiffReport {
            base_commit_id: base.id.clone(),
            target_commit_id: target.id.clone(),
            preview_width: width,
            preview_height: height,
            changed_pixels,
            total_pixels,
            changed_ratio,
            overall_bbox,
            base_image_data_url: base_data_url,
            target_image_data_url: target_data_url,
            heatmap_data_url: Some(png_to_data_url(&heatmap_png)),
            layer_changes,
        });
    }

    Ok(VisualDiffReport {
        base_commit_id: base.id.clone(),
        target_commit_id: target.id.clone(),
        preview_width: 0,
        preview_height: 0,
        changed_pixels: 0,
        total_pixels: 0,
        changed_ratio: 0.0,
        overall_bbox: None,
        base_image_data_url: base_data_url,
        target_image_data_url: target_data_url,
        heatmap_data_url: None,
        layer_changes,
    })
}

pub fn diff_layers(base: &AssetMetadata, target: &AssetMetadata) -> Vec<LayerChange> {
    let mut changes = Vec::new();
    let base_map: HashMap<&str, &LayerSummary> =
        base.layers.iter().map(|l| (l.id.as_str(), l)).collect();
    let target_map: HashMap<&str, &LayerSummary> =
        target.layers.iter().map(|l| (l.id.as_str(), l)).collect();

    for tl in &target.layers {
        match base_map.get(tl.id.as_str()) {
            None => changes.push(LayerChange {
                kind: "added".into(),
                layer_name: tl.name.clone(),
                detail: format!("Nueva capa ({}% opacidad)", tl.opacity),
            }),
            Some(bl) => {
                let mut notes = Vec::new();
                if bl.name != tl.name {
                    notes.push(format!("renombrada de '{}'", bl.name));
                }
                if bl.visible != tl.visible {
                    notes.push(if tl.visible {
                        "mostrada".into()
                    } else {
                        "ocultada".into()
                    });
                }
                if bl.opacity != tl.opacity {
                    notes.push(format!("opacidad {}% → {}%", bl.opacity, tl.opacity));
                }
                if !notes.is_empty() {
                    changes.push(LayerChange {
                        kind: "modified".into(),
                        layer_name: tl.name.clone(),
                        detail: notes.join(", "),
                    });
                }
            }
        }
    }

    for bl in &base.layers {
        if !target_map.contains_key(bl.id.as_str()) {
            changes.push(LayerChange {
                kind: "removed".into(),
                layer_name: bl.name.clone(),
                detail: "Capa eliminada".into(),
            });
        }
    }

    changes
}

fn png_to_data_url(bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:image/png;base64,{}", encoded)
}
