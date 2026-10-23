//! optimizar_glb — reescribe los GLB del mod 3D de GDani31 para el visor VR.
//!
//! Los modelos originales pesan demasiado para el APK (la carpeta de la
//! ensambladora son 21 MB) y traen texturas embebidas que el pipeline del
//! visor (vertices con color, `DioramaVertex`) no sabe usar.
//!
//! Que hace este:
//!   1. aplana la jerarquia de nodos (hornea las transformaciones),
//!   2. **hornea el color del material y de la textura en los vertices**
//!      (COLOR_0), asi el GLB optimizado no lleva texturas,
//!   3. **saca las texturas embebidas** a ficheros sueltos PNG en
//!      `texturas/` (por si el dia se quiere el shader PBR),
//!   4. redondea posiciones/normales/UV a 3 decimales,
//!   5. **decima** la malla agrupando vertices en una rejilla (una malla de
//!      90.000 vertices baja a unos miles: en la mesa la maqueta mide 12 cm),
//!   6. escribe un GLB valido y mucho mas pequeño.
//!
//! Uso:
//!   optimizar_glb <entrada.glb> <salida.glb> [--grid <u>] [--texturas <dir>]
//!   optimizar_glb --lote <tabla.json> <raiz_modelos> <dir_salida>
//!
//! El modo `--lote` recorre una tabla de fuentes (JSON), optimiza cada
//! modelo y escribe el `manifest.json` que consume `src/modelos3d.rs`, de
//! modo que el APK y el codigo nunca se desincronizan.
//!
//! Sin `rm` en ningun sitio: los ficheros se sobrescriben o se dejan.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Un vertice ya horneado (posicion en espacio mundo, en unidades del modelo).
#[derive(Clone, Copy, Debug)]
struct Vrt {
    p: [f32; 3],
    n: [f32; 3],
    uv: [f32; 2],
    c: [f32; 4],
}

#[derive(Default, Debug, Clone)]
struct Resumen {
    origen: u64,
    destino: u64,
    verts_origen: usize,
    verts_destino: usize,
    tri_origen: usize,
    tri_destino: usize,
    texturas: usize,
    bytes_texturas: u64,
    bbox: [f32; 3],
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("[optimizar_glb] ERROR: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "uso:\n  \
             optimizar_glb <entrada.glb> <salida.glb> [--grid <u>] [--texturas <dir>]\n  \
             optimizar_glb --lote <tabla.json> <raiz_modelos> <dir_salida>"
        );
        return Ok(());
    }
    if args[0] == "--lote" {
        if args.len() < 4 {
            return Err(anyhow!("--lote necesita <tabla.json> <raiz_modelos> <dir_salida>"));
        }
        let con_texturas = args.iter().any(|a| a == "--texturas");
        return modo_lote(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            con_texturas,
        );
    }
    if args.len() < 2 {
        return Err(anyhow!("faltan <entrada.glb> <salida.glb>"));
    }
    let entrada = PathBuf::from(&args[0]);
    let salida = PathBuf::from(&args[1]);
    let mut grid: Option<f32> = None;
    let mut dir_texturas: Option<PathBuf> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--grid" => {
                i += 1;
                grid = args.get(i).and_then(|v| v.parse().ok());
            }
            "--texturas" => {
                i += 1;
                dir_texturas = args.get(i).map(PathBuf::from);
            }
            otro => return Err(anyhow!("opcion desconocida: {otro}")),
        }
        i += 1;
    }
    let dir_texturas = dir_texturas.unwrap_or_else(|| {
        salida
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("texturas")
    });
    let r = optimizar_fichero(&entrada, &salida, grid, Some(dir_texturas))?;
    println!("{}", resumen_linea(&entrada, &r));
    Ok(())
}

// ---------------------------------------------------------------------------
// Modo lote: tabla de fuentes -> modelos optimizados + manifest.json
// ---------------------------------------------------------------------------

fn modo_lote(tabla: &Path, raiz: &Path, salida: &Path, con_texturas: bool) -> Result<()> {
    let texto = std::fs::read_to_string(tabla)
        .with_context(|| format!("no se lee la tabla {}", tabla.display()))?;
    let t: Value = serde_json::from_str(&texto).context("tabla JSON invalida")?;
    let escala_tile_m: f32 = t
        .get("escala_tile_m")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.04) as f32;

    std::fs::create_dir_all(salida)?;
    let dir_texturas = salida.join("texturas");
    if con_texturas {
        std::fs::create_dir_all(&dir_texturas)?;
    }

    let modelos = t
        .get("modelos")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("la tabla no tiene 'modelos'"))?;

    let mut manifiesto_modelos = serde_json::Map::new();
    let mut tipos = serde_json::Map::new();
    // fuente -> fichero ya escrito (para no duplicar el mismo GLB)
    let mut por_fuente: HashMap<String, String> = HashMap::new();
    let mut lineas = Vec::new();
    let mut total_orig = 0u64;
    let mut total_dest = 0u64;

    for (proto, def) in modelos.iter() {
        let fuentes: Vec<String> = def
            .get("fuentes")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if fuentes.is_empty() {
            continue;
        }
        let huella: f32 = def
            .get("huella_tiles")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let giros: Vec<String> = def
            .get("giros")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or("direccion").to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut partes = Vec::new();
        let mut ancho_m = 0.0f32;
        let mut alto_m = 0.0f32;
        let mut any = false;
        for (i, f) in fuentes.iter().enumerate() {
            let src = raiz.join(f);
            if !src.exists() {
                eprintln!("[optimizar_glb] falta {src:?}, se salta");
                continue;
            }
            let nombre = if fuentes.len() == 1 {
                format!("{proto}.glb")
            } else {
                format!("{proto}.{i}.glb")
            };
            // si ese mismo GLB ya se optimizo para otro prototipo, se reutiliza
            if let Some(prev) = por_fuente.get(f) {
                any = true;
                let meta = leer_meta(&salida.join(prev))?;
                let escala = (huella * escala_tile_m) / meta[0].max(meta[2]).max(1e-6);
                ancho_m = ancho_m.max(huella * escala_tile_m);
                alto_m = alto_m.max(meta[1] * escala);
                partes.push(json!({
                    "archivo": prev,
                    "giro": giros.get(i).cloned().unwrap_or_else(|| "direccion".to_string()),
                }));
                continue;
            }
            let dst = salida.join(&nombre);
            let texturas = if con_texturas {
                Some(dir_texturas.join(proto))
            } else {
                None
            };
            let r = optimizar_fichero(&src, &dst, None, texturas)?;
            total_orig += r.origen;
            total_dest += r.destino;
            any = true;
            // tamano mostrado: la huella del prototipo en tiles * escala
            let escala = (huella * escala_tile_m) / r.bbox[0].max(r.bbox[2]).max(1e-6);
            let aw = huella * escala_tile_m;
            let al = r.bbox[1] * escala;
            ancho_m = ancho_m.max(aw);
            alto_m = alto_m.max(al);
            por_fuente.insert(f.clone(), nombre.clone());
            partes.push(json!({
                "archivo": nombre,
                "giro": giros.get(i).cloned().unwrap_or_else(|| "direccion".to_string()),
                "verts": r.verts_destino,
                "triangulos": r.tri_destino,
            }));
            lineas.push(resumen_linea(&src, &r));
        }
        if !any {
            continue;
        }
        manifiesto_modelos.insert(
            proto.clone(),
            json!({
                "huella_tiles": huella,
                "ancho_m": ancho_m,
                "alto_m": alto_m,
                "partes": partes,
            }),
        );
    }

    if let Some(obj) = t.get("tipos").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                tipos.insert(k.clone(), json!(s));
            }
        }
    }

    let manifiesto = json!({
        "version": 1,
        "generado_por": "optimizar_glb",
        "escala_tile_m": (escala_tile_m * 10000.0).round() / 10000.0,
        "modelos": serde_json::Value::Object(manifiesto_modelos),
        "tipos": serde_json::Value::Object(tipos),
    });
    let ruta_manifiesto = salida.join("manifest.json");
    std::fs::write(
        &ruta_manifiesto,
        serde_json::to_string_pretty(&manifiesto)? + "\n",
    )?;

    println!("--- por modelo ---");
    for l in &lineas {
        println!("{l}");
    }
    println!(
        "--- total: {:.2} MB -> {:.2} MB ({:.1}% del original) ---",
        total_orig as f64 / 1e6,
        total_dest as f64 / 1e6,
        if total_orig > 0 {
            100.0 * total_dest as f64 / total_orig as f64
        } else {
            0.0
        }
    );
    println!("manifiesto: {}", ruta_manifiesto.display());
    Ok(())
}

/// Lee los metadatos de un modelo ya optimizado (bbox etc.).
fn leer_meta(glb: &Path) -> Result<[f32; 3]> {
    let stem = glb.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ruta = glb
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("meta")
        .join(format!("{stem}.meta.json"));
    let texto = std::fs::read_to_string(&ruta)
        .with_context(|| format!("no se lee {}", ruta.display()))?;
    let v: Value = serde_json::from_str(&texto)?;
    let bbox = v
        .get("bbox")
        .and_then(|b| b.as_array())
        .map(|a| {
            [
                a.first().and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                a.get(1).and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                a.get(2).and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
            ]
        })
        .unwrap_or([1.0, 1.0, 1.0]);
    Ok(bbox)
}

fn resumen_linea(src: &Path, r: &Resumen) -> String {
    format!(
        "{:<46} {:>8.2} MB -> {:>7.2} MB  verts {:>7} -> {:>6}  tris {:>7} -> {:>6}  tex {} ({:.2} MB)",
        src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        r.origen as f64 / 1e6,
        r.destino as f64 / 1e6,
        r.verts_origen,
        r.verts_destino,
        r.tri_origen,
        r.tri_destino,
        r.texturas,
        r.bytes_texturas as f64 / 1e6,
    )
}

// ---------------------------------------------------------------------------
// Optimizacion de un GLB
// ---------------------------------------------------------------------------

fn optimizar_fichero(
    entrada: &Path,
    salida: &Path,
    grid_override: Option<f32>,
    dir_texturas: Option<PathBuf>,
) -> Result<Resumen> {
    let origen_bytes = std::fs::metadata(entrada)?.len();
    let (document, buffers, images) =
        gltf::import(entrada).with_context(|| format!("no se importa {}", entrada.display()))?;

    let mut vertices: Vec<Vrt> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut textos_visitados: Vec<usize> = Vec::new();

    // --- 1. recorrer la escena horneando transformaciones ---------------
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or_else(|| anyhow!("el GLB no tiene escena"))?;

    for node in scene.nodes() {
        recorrer_nodo(
            &node,
            glam::Mat4::IDENTITY,
            &buffers,
            &images,
            &document,
            &mut vertices,
            &mut indices,
            &mut textos_visitados,
        );
    }
    if vertices.is_empty() {
        return Err(anyhow!("{} no tiene geometria", entrada.display()));
    }

    let tri_origen = indices.len() / 3;
    let verts_origen = vertices.len();

    // --- 2. color de las texturas -> ficheros sueltos --------------------
    let mut bytes_texturas = 0u64;
    let mut texturas = 0usize;
    if let Some(dir) = dir_texturas.as_ref() {
        std::fs::create_dir_all(dir)?;
        for (i, img) in images.iter().enumerate() {
            if !textos_visitados.contains(&i) {
                continue;
            }
            let bytes = escribir_png(dir, i, img)?;
            bytes_texturas += bytes;
            texturas += 1;
        }
    }

    // --- 3. normalizar (base en y=0, centrado en x/z) y bounding box -----
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in &vertices {
        for k in 0..3 {
            min[k] = min[k].min(v.p[k]);
            max[k] = max[k].max(v.p[k]);
        }
    }
    let bbox = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let centro_x = (min[0] + max[0]) * 0.5;
    let centro_z = (min[2] + max[2]) * 0.5;
    for v in &mut vertices {
        v.p[0] -= centro_x;
        v.p[1] -= min[1];
        v.p[2] -= centro_z;
    }

    // --- 4. decimar por rejilla -----------------------------------------
    let extent_max = bbox[0].max(bbox[1]).max(bbox[2]).max(1e-6);
    let grid = grid_override.unwrap_or(extent_max / 220.0).max(1e-6);
    let (mut vertices, indices) = decimar(&vertices, &indices, grid);

    // --- 5. redondear a 3 decimales --------------------------------------
    for v in &mut vertices {
        for k in 0..3 {
            v.p[k] = (v.p[k] * 1000.0).round() / 1000.0;
            v.n[k] = (v.n[k] * 1000.0).round() / 1000.0;
        }
        for k in 0..2 {
            v.uv[k] = (v.uv[k] * 1000.0).round() / 1000.0;
        }
    }

    let mut resumen = Resumen {
        origen: origen_bytes,
        destino: 0,
        verts_origen,
        verts_destino: vertices.len(),
        tri_origen,
        tri_destino: indices.len() / 3,
        texturas,
        bytes_texturas,
        bbox,
    };

    // --- 6. escribir el GLB ----------------------------------------------
    if let Some(dir) = salida.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = escribir_glb(&vertices, &indices)?;
    std::fs::write(salida, &bytes)?;
    resumen.destino = bytes.len() as u64;

    // --- 7. metadatos del modelo (los usa el generador de manifiesto) ----
    let meta = json!({
        "archivo": salida.file_name().map(|s| s.to_string_lossy().into_owned()),
        "bbox": bbox,
        "verts": vertices.len(),
        "triangulos": indices.len() / 3,
        "bytes_origen": origen_bytes,
        "bytes_destino": resumen.destino,
        "grid": grid,
    });
    let dir_meta = salida
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("meta");
    std::fs::create_dir_all(&dir_meta)?;
    let ruta_meta = dir_meta.join(format!(
        "{}.meta.json",
        salida.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    std::fs::write(ruta_meta, serde_json::to_string_pretty(&meta)? + "\n")?;

    Ok(resumen)
}

#[allow(clippy::too_many_arguments)]
fn recorrer_nodo(
    node: &gltf::Node,
    padre: glam::Mat4,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
    document: &gltf::Document,
    vertices: &mut Vec<Vrt>,
    indices: &mut Vec<u32>,
    textos_visitados: &mut Vec<usize>,
) {
    let mundo = padre * glam::Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        let mut nrm_mat = glam::Mat3::from_mat4(mundo).inverse().transpose();
        if !nrm_mat.is_finite() {
            nrm_mat = glam::Mat3::from_mat4(mundo);
        }
        for prim in mesh.primitives() {
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &**d));
            let posiciones: Vec<[f32; 3]> = match reader.read_positions() {
                Some(p) => p.collect(),
                None => continue,
            };
            let normales: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|n| n.collect())
                .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; posiciones.len()]);
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|t| t.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; posiciones.len()]);
            let idx: Vec<u32> = reader
                .read_indices()
                .map(|i| i.into_u32().collect())
                .unwrap_or_else(|| (0..posiciones.len() as u32).collect());

            // color: factor del material, con la textura horneada si la hay
            let mat = prim.material();
            let pbr = mat.pbr_metallic_roughness();
            let factor = pbr.base_color_factor();
            let (tex_idx, offset, scale) = match pbr.base_color_texture() {
                Some(t) => {
                    let src = t.texture().source().index();
                    let (o, s) = if let Some(tt) = t.texture_transform() {
                        (tt.offset(), tt.scale())
                    } else {
                        ([0.0, 0.0], [1.0, 1.0])
                    };
                    (Some(src), o, s)
                }
                None => (None, [0.0, 0.0], [1.0, 1.0]),
            };
            if let Some(ti) = tex_idx {
                if !textos_visitados.contains(&ti) {
                    textos_visitados.push(ti);
                }
            }
            let img = tex_idx.and_then(|i| images.get(i));

            let base = vertices.len() as u32;
            for i in 0..posiciones.len() {
                let p = mundo.transform_point3(glam::Vec3::from(posiciones[i]));
                let n = (nrm_mat * glam::Vec3::from(normales.get(i).copied().unwrap_or([0.0, 1.0, 0.0])))
                    .normalize_or_zero();
                let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                let col = match img {
                    Some(im) => {
                        let u = uv[0] * scale[0] + offset[0];
                        let v = uv[1] * scale[1] + offset[1];
                        sample(im, u, v, factor)
                    }
                    None => factor,
                };
                vertices.push(Vrt {
                    p: [p.x, p.y, p.z],
                    n: [n.x, n.y, n.z],
                    uv,
                    c: col,
                });
            }
            for i in idx {
                if (i as usize) < posiciones.len() {
                    indices.push(base + i);
                }
            }
        }
    }
    for hijo in node.children() {
        recorrer_nodo(
            &hijo,
            mundo,
            buffers,
            images,
            document,
            vertices,
            indices,
            textos_visitados,
        );
    }
}

/// Muestrea la textura (formato RGB/RGBA) con el factor del material.
fn sample(img: &gltf::image::Data, u: f32, v: f32, tinte: [f32; 4]) -> [f32; 4] {
    if img.width == 0 || img.height == 0 || img.pixels.is_empty() {
        return tinte;
    }
    let un = u.rem_euclid(1.0);
    let vn = v.rem_euclid(1.0);
    let px = ((un * img.width as f32) as u32).min(img.width - 1);
    let py = ((vn * img.height as f32) as u32).min(img.height - 1);
    match img.format {
        gltf::image::Format::R8G8B8 => {
            let i = ((py * img.width + px) as usize * 3).min(img.pixels.len().saturating_sub(3));
            [
                img.pixels[i] as f32 / 255.0 * tinte[0],
                img.pixels[i + 1] as f32 / 255.0 * tinte[1],
                img.pixels[i + 2] as f32 / 255.0 * tinte[2],
                tinte[3],
            ]
        }
        gltf::image::Format::R8G8B8A8 => {
            let i = ((py * img.width + px) as usize * 4).min(img.pixels.len().saturating_sub(4));
            [
                img.pixels[i] as f32 / 255.0 * tinte[0],
                img.pixels[i + 1] as f32 / 255.0 * tinte[1],
                img.pixels[i + 2] as f32 / 255.0 * tinte[2],
                img.pixels[i + 3] as f32 / 255.0 * tinte[3],
            ]
        }
        _ => tinte,
    }
}

/// Agrupa vertices en una rejilla y promedia: menos vertices, mismo aspecto
/// a escala de maqueta.
fn decimar(vertices: &[Vrt], indices: &[u32], grid: f32) -> (Vec<Vrt>, Vec<u32>) {
    let inv = 1.0 / grid;
    let mut mapa: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut salida_v: Vec<Vrt> = Vec::with_capacity(vertices.len() / 2);
    // acumuladores por cluster
    struct Acum {
        p: [f32; 3],
        n: [f32; 3],
        uv: [f32; 2],
        c: [f32; 4],
        n_v: u32,
    }
    let mut acums: Vec<Acum> = Vec::with_capacity(vertices.len() / 2);
    // indice original -> indice del cluster (para remapear los indices)
    let mut remap: Vec<u32> = vec![0; vertices.len()];

    for (vi, v) in vertices.iter().enumerate() {
        let key = (
            (v.p[0] * inv).round() as i32,
            (v.p[1] * inv).round() as i32,
            (v.p[2] * inv).round() as i32,
        );
        let idx = match mapa.get(&key) {
            Some(i) => *i,
            None => {
                let i = salida_v.len() as u32;
                mapa.insert(key, i);
                salida_v.push(*v);
                acums.push(Acum {
                    p: v.p,
                    n: v.n,
                    uv: v.uv,
                    c: v.c,
                    n_v: 1,
                });
                i
            }
        };
        remap[vi] = idx;
        let a = &mut acums[idx as usize];
        a.p[0] += v.p[0];
        a.p[1] += v.p[1];
        a.p[2] += v.p[2];
        a.n[0] += v.n[0];
        a.n[1] += v.n[1];
        a.n[2] += v.n[2];
        a.uv[0] += v.uv[0];
        a.uv[1] += v.uv[1];
        a.c[0] += v.c[0];
        a.c[1] += v.c[1];
        a.c[2] += v.c[2];
        a.c[3] += v.c[3];
        a.n_v += 1;
    }

    for (i, v) in salida_v.iter_mut().enumerate() {
        let a = &acums[i];
        let f = 1.0 / a.n_v as f32;
        v.p = [a.p[0] * f, a.p[1] * f, a.p[2] * f];
        let n = glam::Vec3::from(a.n) * f;
        let n = n.normalize_or_zero();
        v.n = if n.length_squared() > 0.0 {
            [n.x, n.y, n.z]
        } else {
            v.n
        };
        v.uv = [a.uv[0] * f, a.uv[1] * f];
        v.c = [a.c[0] * f, a.c[1] * f, a.c[2] * f, a.c[3] * f];
    }

    // Remapea los indices a los vertices nuevos y recorta los triangulos
    // degenerados (los que colapsaron en la rejilla).
    //
    // OJO: sin este remapeo los indices siguen apuntando a los vertices
    // ORIGINALES y el GLB sale roto (indices por encima del numero de
    // vertices: el cargador se come los triangulos y la maquina sale con
    // agujeros).
    let mut salida_i = Vec::with_capacity(indices.len());
    for t in indices.chunks(3) {
        if t.len() < 3 {
            continue;
        }
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a == b || b == c || a == c {
            continue;
        }
        salida_i.extend_from_slice(&[a, b, c]);
    }

    (salida_v, salida_i)
}

/// Escribe la textura en PNG (a la mitad de resolucion como maximo).
fn escribir_png(dir: &Path, idx: usize, img: &gltf::image::Data) -> Result<u64> {
    use image::{RgbImage, RgbaImage};
    let max_lado = 512;
    let (w, h) = (img.width as u32, img.height as u32);
    let rgba: image::DynamicImage = match img.format {
        gltf::image::Format::R8G8B8A8 => {
            let mut b = RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) as usize * 4).min(img.pixels.len().saturating_sub(4));
                    b.put_pixel(x, y, image::Rgba([img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]));
                }
            }
            image::DynamicImage::ImageRgba8(b)
        }
        _ => {
            let mut b = RgbImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) as usize * 3).min(img.pixels.len().saturating_sub(3));
                    b.put_pixel(x, y, image::Rgb([img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]));
                }
            }
            image::DynamicImage::ImageRgb8(b)
        }
    };
    let escalada = if w.max(h) > max_lado {
        let factor = max_lado as f32 / w.max(h) as f32;
        let nw = (w as f32 * factor).max(1.0) as u32;
        let nh = (h as f32 * factor).max(1.0) as u32;
        rgba.resize(nw, nh, image::imageops::FilterType::Triangle)
    } else {
        rgba
    };
    let ruta = dir.join(format!("tex_{idx}.png"));
    escalada.save(&ruta)?;
    Ok(std::fs::metadata(&ruta)?.len())
}

// ---------------------------------------------------------------------------
// Escritor de GLB (JSON + BIN)
// ---------------------------------------------------------------------------

fn escribir_glb(vertices: &[Vrt], indices: &[u32]) -> Result<Vec<u8>> {
    let n = vertices.len();
    let uso_u32 = n > u16::MAX as usize;

    // --- buffer binario ---
    let mut bin: Vec<u8> = Vec::new();
    let mut buffer_views = Vec::new();

    fn aline4(bin: &mut Vec<u8>) {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
    }

    // posiciones (f32 x3)
    let off_pos = bin.len();
    for v in vertices {
        for k in 0..3 {
            bin.extend_from_slice(&v.p[k].to_le_bytes());
        }
    }
    buffer_views.push(json!({"buffer":0,"byteOffset":off_pos,"byteLength":n*12,"target":34962}));
    // normales (f32 x3)
    let off_nrm = bin.len();
    for v in vertices {
        for k in 0..3 {
            bin.extend_from_slice(&v.n[k].to_le_bytes());
        }
    }
    buffer_views.push(json!({"buffer":0,"byteOffset":off_nrm,"byteLength":n*12,"target":34962}));
    // uv (u16 normalizado x2)
    let off_uv = bin.len();
    for v in vertices {
        for k in 0..2 {
            let c = (v.uv[k].clamp(0.0, 1.0) * 65535.0).round() as u16;
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    buffer_views.push(json!({"buffer":0,"byteOffset":off_uv,"byteLength":n*4,"target":34962}));
    // color (u8 normalizado x4)
    let off_col = bin.len();
    for v in vertices {
        for k in 0..4 {
            let c = (v.c[k].clamp(0.0, 1.0) * 255.0).round() as u8;
            bin.push(c);
        }
    }
    buffer_views.push(json!({"buffer":0,"byteOffset":off_col,"byteLength":n*4,"target":34962}));
    // indices
    aline4(&mut bin);
    let off_idx = bin.len();
    for i in indices {
        if uso_u32 {
            bin.extend_from_slice(&i.to_le_bytes());
        } else {
            bin.extend_from_slice(&(*i as u16).to_le_bytes());
        }
    }
    buffer_views.push(json!({"buffer":0,"byteOffset":off_idx,"byteLength":indices.len()*if uso_u32 {4} else {2},"target":34963}));
    aline4(&mut bin);

    // --- accesores ---
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in vertices {
        for k in 0..3 {
            min[k] = min[k].min(v.p[k]);
            max[k] = max[k].max(v.p[k]);
        }
    }
    let accessors = json!([
        {"bufferView":0,"componentType":5126,"count":n,"type":"VEC3","min":[min[0],min[1],min[2]],"max":[max[0],max[1],max[2]]},
        {"bufferView":1,"componentType":5126,"count":n,"type":"VEC3"},
        {"bufferView":2,"componentType":5123,"normalized":true,"count":n,"type":"VEC2"},
        {"bufferView":3,"componentType":5121,"normalized":true,"count":n,"type":"VEC4"},
        {"bufferView":4,"componentType":if uso_u32 {5125} else {5123},"count":indices.len(),"type":"SCALAR"},
    ]);

    let gltf_json = json!({
        "asset": {"version":"2.0","generator":"xfactorvr optimizar_glb"},
        "scene": 0,
        "scenes": [{"nodes":[0]}],
        "nodes": [{"mesh":0,"name":"modelo"}],
        "meshes": [{"name":"modelo","primitives":[{
            "attributes": {"POSITION":0,"NORMAL":1,"TEXCOORD_0":2,"COLOR_0":3},
            "indices": 4,
            "material": 0
        }]}],
        "materials": [{
            "name":"modelo",
            "pbrMetallicRoughness": {
                "baseColorFactor":[1.0,1.0,1.0,1.0],
                "metallicFactor":0.0,
                "roughnessFactor":1.0
            },
            "doubleSided": true
        }],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": buffer_views,
        "accessors": accessors,
    });

    let mut json_bytes = serde_json::to_vec(&gltf_json)?;
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin;
    while bin_bytes.len() % 4 != 0 {
        bin_bytes.push(0);
    }

    let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x46546C67u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E4942u32.to_le_bytes()); // "BIN"
    out.extend_from_slice(&bin_bytes);
    Ok(out)
}
