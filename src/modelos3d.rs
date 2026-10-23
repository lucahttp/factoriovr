//! modelos3d — las máquinas del juego con su MODELO 3D REAL en el visor.
//!
//! Antes de este modulo, el visor dibujaba las entidades del juego como
//! billboards 2D con el sprite del atlas y el resto como primitivas escritas
//! a mano en C++. Aqui vive:
//!
//!   * la TABLA prototipo de Factorio -> modelo GLB (no metida a fuego: se
//!     lee del `manifest.json` que genera `src/bin/optimizar_glb.rs`, el
//!     mismo fichero que se empaqueta en el APK),
//!   * el CACHE de modelos (cargar un GLB es caro: en la Pico el visor va a
//!     60 fps y un modelo de 21 MB no se puede leer en cada frame),
//!   * la funcion que devuelve la GEOMETRIA ya transformada de una entidad
//!     (`Vec<DioramaVertex>`, el formato que ya usa el pipeline del visor).
//!
//! Regla de oro: si un prototipo NO tiene modelo 3D, la funcion devuelve
//! `None` y quien llama sigue dibujando el sprite 2D. Nunca se inventa
//! geometria.
//!
//! ## Convenciones
//!
//! * Las coordenadas del snapshot son en casillas (tiles) de Factorio; el
//!   mundo del visor usa metros: `x_m = tile_x * escala_tile_m`,
//!   `z_m = tile_y * escala_tile_m` (el norte de Factorio es -Z).
//! * La orientacion llega en radianes (`direction_to_rad` del mod puente:
//!   `dir * TAU / 16`, 0 = norte). Se convierte en un giro sobre Y con
//!   `YAW_BASE - orientacion`, suponiendo que el frente del modelo es +Z.
//! * Colocacion: el modelo se pone EN EL SUELO (su caja minima en y=0) y
//!   CENTRADO en la casilla de la entidad. Todas las partes de un modelo
//!   comparten la misma escala (la de la huella en casillas) y el mismo
//!   desplazamiento, para que guarden sus proporciones relativas.

use crate::tabletop::DioramaVertex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Escala por defecto: metros por casilla de Factorio (la mesa son 4 cm).
pub const ESCALA_TILE_M_POR_DEFECTO: f32 = 0.04;

/// Giro del frente del modelo respecto a la orientacion de la entidad.
/// 0 radianes = el modelo mira al norte (-Z).
pub const YAW_BASE: f32 = std::f32::consts::PI;

/// Como gira una parte del modelo respecto a la entidad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Giro {
    /// Sigue la direccion de la entidad (ensambladora, horno, poste...).
    Direccion,
    /// No gira (la base de la torreta).
    Fijo,
    /// Gira con la orientacion "suave" (la cabeza de la torreta).
    Orientacion,
    /// Gira solo (el plato del radar).
    Giro,
}

/// Una parte del modelo (un GLB). Las entidades con varias partes (radar,
/// torreta, insertador) dibujan todas.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParteModelo {
    /// Nombre del fichero dentro del directorio de modelos.
    pub archivo: String,
    pub giro: Giro,
    /// Vertices del modelo (informativo).
    #[serde(default)]
    pub verts: usize,
    /// Triangulos del modelo (informativo).
    #[serde(default)]
    pub triangulos: usize,
}

/// Lo que el manifiesto dice de un prototipo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Modelo3D {
    /// Huella del prototipo en casillas (ancho del edificio).
    pub huella_tiles: f32,
    /// Ancho mostrado en metros (a escala de referencia).
    pub ancho_m: f32,
    /// Alto mostrado en metros (a escala de referencia).
    pub alto_m: f32,
    pub partes: Vec<ParteModelo>,
}

/// Manifiesto de modelos: lo escribe `optimizar_glb` y lo lee este modulo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifiestoModelos {
    pub version: u32,
    #[serde(default = "escala_por_defecto")]
    pub escala_tile_m: f32,
    /// prototipo -> modelo
    #[serde(default)]
    pub modelos: HashMap<String, Modelo3D>,
    /// tipo numerico de entidad (el del render) -> prototipo
    #[serde(default)]
    pub tipos: HashMap<String, String>,
}

fn escala_por_defecto() -> f32 {
    ESCALA_TILE_M_POR_DEFECTO
}

// ---------------------------------------------------------------------------
// Manifiesto
// ---------------------------------------------------------------------------

/// El manifiesto va EMBEBIDO en el binario: es el mismo fichero que se
/// empaqueta en `android/src/main/assets/models/`, asi el APK y el codigo
/// no se pueden desincronizar.
const MANIFIESTO_EMBEBIDO: &str = include_str!("../android/src/main/assets/models/manifest.json");

static MANIFIESTO: OnceLock<Option<ManifiestoModelos>> = OnceLock::new();

/// Manifiesto de modelos (el embebido, parseado una vez).
pub fn manifiesto() -> Option<&'static ManifiestoModelos> {
    MANIFIESTO
        .get_or_init(|| match serde_json::from_str::<ManifiestoModelos>(MANIFIESTO_EMBEBIDO) {
            Ok(m) => Some(m),
            Err(e) => {
                log::warn!("Manifiesto de modelos 3D ilegible: {e}");
                None
            }
        })
        .as_ref()
}

/// Modelo de un prototipo ("assembling-machine-1", "stone-furnace"...).
pub fn modelo_para_prototipo(proto: &str) -> Option<&'static Modelo3D> {
    manifiesto()?.modelos.get(proto)
}

/// Prototipo canonico de un tipo numerico de entidad (el contrato del render
/// con el mod puente). `None` = ese tipo no tiene modelo 3D (o no existe).
pub fn prototipo_para_tipo(tipo: u32) -> Option<&'static str> {
    let m = manifiesto()?;
    m.tipos.get(&tipo.to_string()).map(|s| s.as_str())
}

/// Directorio donde se leen los GLB en tiempo de ejecucion.
///
/// En el APK los assets van dentro del APK, asi que el motor tiene que
/// copiarlos a un directorio legible (igual que hace con el atlas del
/// juego): se buscan los mismos sitios que el atlas.
pub fn dir_modelos() -> PathBuf {
    if let Ok(d) = std::env::var("XFACTORVR_MODELS_DIR") {
        return PathBuf::from(d);
    }
    const CANDIDATOS: &[&str] = &[
        // Android 11+ tiene "almacenamiento con ambito": la app SOLO puede
        // leer su propio directorio en /sdcard/Android/data/<paquete>/files.
        // Por eso los modelos se dejan ahi (el deploy los copia) y no en
        // /sdcard/XFactorVR, que da Permission denied.
        "/sdcard/Android/data/com.xfactorvr.tabletop/files/models",
        "/storage/emulated/0/Android/data/com.xfactorvr.tabletop/files/models",
        // el motor puede extraer los assets del APK a su directorio interno
        "/data/data/com.xfactorvr.tabletop/files/models",
        "/data/user/0/com.xfactorvr.tabletop/files/models",
        // y el resto de sitios (escritorio, contenedor, /sdcard legacy)
        "/sdcard/XFactorVR/assets/models",
        "/storage/emulated/0/XFactorVR/assets/models",
        "/data/local/tmp/XFactorVR/assets/models",
        "android/src/main/assets/models",
    ];
    for c in CANDIDATOS {
        let p = Path::new(c);
        if p.join("manifest.json").exists() {
            return p.to_path_buf();
        }
    }
    // El directorio que se empaqueta en el APK, por si el motor lo copia
    // tal cual a un sitio legible.
    let apk = Path::new("android/src/main/assets/models");
    if apk.is_dir() {
        return apk.to_path_buf();
    }
    PathBuf::from(CANDIDATOS[0])
}

// ---------------------------------------------------------------------------
// Cache de modelos
// ---------------------------------------------------------------------------

/// Geometria cruda de un modelo (en las unidades del GLB original).
struct Geometria {
    vertices: Vec<DioramaVertex>,
    min: [f32; 3],
    max: [f32; 3],
}

static CACHE: OnceLock<Mutex<HashMap<String, Arc<Geometria>>>> = OnceLock::new();

/// Contador de cargas REALES de fichero (para los tests: el cache no puede
/// volver a leer el GLB).
static CARGAS: AtomicUsize = AtomicUsize::new(0);
/// Fallos de carga.
static FALLOS: AtomicUsize = AtomicUsize::new(0);

fn cache() -> &'static Mutex<HashMap<String, Arc<Geometria>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Numero de ficheros GLB leidos desde que arranca el proceso.
pub fn cargas_reales() -> usize {
    CARGAS.load(Ordering::Relaxed)
}

/// Numero de intentos de carga fallidos.
pub fn fallos_carga() -> usize {
    FALLOS.load(Ordering::Relaxed)
}

/// Numero de modelos en el cache.
pub fn modelos_en_cache() -> usize {
    cache().lock().map(|c| c.len()).unwrap_or(0)
}

/// Vacia el cache (tests).
pub fn limpiar_cache() {
    if let Ok(mut c) = cache().lock() {
        c.clear();
    }
}

/// Carga (o saca del cache) la geometria normalizada de un GLB.
fn geometria_de_fichero(nombre: &str) -> Option<Arc<Geometria>> {
    if let Ok(c) = cache().lock() {
        if let Some(g) = c.get(nombre) {
            return Some(g.clone());
        }
    }
    let ruta = dir_modelos().join(nombre);
    if !ruta.exists() {
        // En el APK el directorio puede ser otro: se prueba tambien junto
        // al directorio de modelos configurado.
        log::debug!("[modelos3d] no existe {ruta:?}");
        FALLOS.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    CARGAS.fetch_add(1, Ordering::Relaxed);
    match cargar_glb(&ruta) {
        Ok(g) => {
            let g = Arc::new(g);
            if let Ok(mut c) = cache().lock() {
                c.insert(nombre.to_string(), g.clone());
            }
            Some(g)
        }
        Err(e) => {
            log::warn!("[modelos3d] no se pudo cargar {ruta:?}: {e:#}");
            FALLOS.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Geometria
// ---------------------------------------------------------------------------

/// Carga un GLB y devuelve sus vertices en el formato del visor
/// (`DioramaVertex`), NORMALIZADOS: base en y=0, centrada en x/z y con la
/// extension mayor (x o z) midiendo 1.0.
///
/// Si el modelo trae COLOR_0 (modelo optimizado) se usa ese color; si trae
/// textura embebida se muestra (modelo original); si no, el factor del
/// material.
fn cargar_glb(ruta: &Path) -> anyhow::Result<Geometria> {
    use anyhow::anyhow;
    let (document, buffers, images) = gltf::import(ruta)?;

    let mut crudos: Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>)> =
        Vec::new();

    fn recorrer(
        node: &gltf::Node,
        padre: glam::Mat4,
        buffers: &[gltf::buffer::Data],
        images: &[gltf::image::Data],
        salida: &mut Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 4]>, Vec<u32>)>,
    ) {
        let mundo = padre * glam::Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            let mut nrm_mat = glam::Mat3::from_mat4(mundo).inverse().transpose();
            if !nrm_mat.is_finite() {
                nrm_mat = glam::Mat3::from_mat4(mundo);
            }
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &**d));
                let pos: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(p) => p.collect(),
                    None => continue,
                };
                let nrm: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|n| n.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; pos.len()]);
                let uv: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; pos.len()]);
                let col0: Vec<[f32; 4]> = match reader.read_colors(0) {
                    Some(c) => leer_colores(c),
                    None => vec![[1.0, 1.0, 1.0, 1.0]; pos.len()],
                };

                let mat = prim.material();
                let pbr = mat.pbr_metallic_roughness();
                let factor = pbr.base_color_factor();
                let tex = pbr.base_color_texture().map(|t| {
                    let src = t.texture().source().index();
                    let (off, esc) = if let Some(tt) = t.texture_transform() {
                        (tt.offset(), tt.scale())
                    } else {
                        ([0.0, 0.0], [1.0, 1.0])
                    };
                    (src, off, esc)
                });
                let img = tex.and_then(|(i, _, _)| images.get(i));

                let mut posiciones = Vec::with_capacity(pos.len());
                let mut normales = Vec::with_capacity(pos.len());
                let mut colores = Vec::with_capacity(pos.len());
                for i in 0..pos.len() {
                    let p = mundo.transform_point3(glam::Vec3::from(pos[i]));
                    let n = (nrm_mat * glam::Vec3::from(nrm.get(i).copied().unwrap_or([0.0, 1.0, 0.0])))
                        .normalize_or_zero();
                    posiciones.push([p.x, p.y, p.z]);
                    normales.push([n.x, n.y, n.z]);
                    let c = match img {
                        Some(im) => {
                            let uvo = uv.get(i).copied().unwrap_or([0.0, 0.0]);
                            let (_, off, esc) = tex.unwrap();
                            sample(im, uvo[0] * esc[0] + off[0], uvo[1] * esc[1] + off[1], factor)
                        }
                        None => {
                            // COLOR_0 si el modelo lo trae (optimizado),
                            // si no el factor del material
                            let c0 = col0.get(i).copied().unwrap_or([1.0, 1.0, 1.0, 1.0]);
                            if c0 != [1.0, 1.0, 1.0, 1.0] || img.is_none() {
                                [
                                    c0[0] * factor[0],
                                    c0[1] * factor[1],
                                    c0[2] * factor[2],
                                    c0[3] * factor[3],
                                ]
                            } else {
                                factor
                            }
                        }
                    };
                    colores.push(c);
                }
                let idx: Vec<u32> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_else(|| (0..pos.len() as u32).collect());
                salida.push((posiciones, normales, uv, colores, idx));
            }
        }
        for hijo in node.children() {
            recorrer(&hijo, mundo, buffers, images, salida);
        }
    }

    let escena = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or_else(|| anyhow!("el GLB no tiene escena"))?;
    for node in escena.nodes() {
        recorrer(&node, glam::Mat4::IDENTITY, &buffers, &images, &mut crudos);
    }
    if crudos.is_empty() {
        return Err(anyhow!("el GLB no tiene geometria"));
    }

    // bounding box en unidades del modelo
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for (pos, _, _, _, _) in &crudos {
        for p in pos {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }

    let mut vertices = Vec::new();
    for (pos, nrm, uv, col, idx) in crudos {
        for i in idx {
            let i = i as usize;
            if i >= pos.len() {
                continue;
            }
            vertices.push(DioramaVertex {
                x: pos[i][0],
                y: pos[i][1],
                z: pos[i][2],
                nx: nrm[i][0],
                ny: nrm[i][1],
                nz: nrm[i][2],
                u: uv.get(i).map(|u| u[0]).unwrap_or(0.0),
                v: uv.get(i).map(|u| u[1]).unwrap_or(0.0),
                r: col[i][0],
                g: col[i][1],
                b: col[i][2],
                a: col[i][3],
            });
        }
    }

    Ok(Geometria {
        vertices,
        min,
        max,
    })
}

/// Convierte el COLOR_0 del accessor a color por vertice.
fn leer_colores(c: gltf::mesh::util::ReadColors<'_>) -> Vec<[f32; 4]> {
    use gltf::mesh::util::ReadColors;
    match c {
        ReadColors::RgbU8(it) => it.map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]).collect(),
        ReadColors::RgbU16(it) => it.map(|c| [c[0] as f32 / 65535.0, c[1] as f32 / 65535.0, c[2] as f32 / 65535.0, 1.0]).collect(),
        ReadColors::RgbF32(it) => it.map(|c| [c[0], c[1], c[2], 1.0]).collect(),
        ReadColors::RgbaU8(it) => it.map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect(),
        ReadColors::RgbaU16(it) => it.map(|c| [c[0] as f32 / 65535.0, c[1] as f32 / 65535.0, c[2] as f32 / 65535.0, c[3] as f32 / 65535.0]).collect(),
        ReadColors::RgbaF32(it) => it.map(|c| [c[0], c[1], c[2], c[3]]).collect(),
    }
}

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

// ---------------------------------------------------------------------------
// API de geometria
// ---------------------------------------------------------------------------

/// Giro (radianes) de una parte segun la orientacion de la entidad.
fn yaw_de_parte(giro: Giro, orientacion: f32, frame: u32) -> f32 {
    match giro {
        Giro::Fijo => 0.0,
        Giro::Direccion | Giro::Orientacion => YAW_BASE - orientacion,
        // el plato del radar: gira con el frame (16 direcciones del juego)
        Giro::Giro => (frame as f32 / 16.0) * std::f32::consts::TAU,
    }
}

/// Geometria de una entidad por su PROTOTIPO ("assembling-machine-1").
///
/// Devuelve `None` si el prototipo no tiene modelo 3D (o no se pudo
/// cargar): el que llama sigue con el sprite 2D.
#[allow(clippy::too_many_arguments)]
pub fn geometria_prototipo(
    proto: &str,
    tile_x: f32,
    tile_y: f32,
    orientacion: f32,
    frame: u32,
    escala_tile_m: f32,
) -> Option<Vec<DioramaVertex>> {
    let modelo = modelo_para_prototipo(proto)?;
    let escala = if escala_tile_m > 0.0 {
        escala_tile_m
    } else {
        ESCALA_TILE_M_POR_DEFECTO
    };
    let huella_m = modelo.huella_tiles * escala;

    // 1. cargar las partes (del cache) y calcular la caja del MODELO ENTERO
    let mut partes_geo = Vec::with_capacity(modelo.partes.len());
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for parte in &modelo.partes {
        let geo = geometria_de_fichero(&parte.archivo)?;
        for k in 0..3 {
            min[k] = min[k].min(geo.min[k]);
            max[k] = max[k].max(geo.max[k]);
        }
        partes_geo.push((parte, geo));
    }
    if partes_geo.is_empty() {
        return None;
    }
    let ext_x = max[0] - min[0];
    let ext_z = max[2] - min[2];
    // 2. escala comun: el modelo entra en su huella en casillas
    let s = huella_m / ext_x.max(ext_z).max(1e-6);
    // 3. desplazamiento comun: centrado en la casilla y apoyado en el suelo
    let off_x = (min[0] + max[0]) * 0.5;
    let off_z = (min[2] + max[2]) * 0.5;
    let off_y = min[1];
    let px = tile_x * escala;
    let pz = tile_y * escala;

    let mut salida: Vec<DioramaVertex> = Vec::new();
    for (parte, geo) in partes_geo {
        let yaw = yaw_de_parte(parte.giro, orientacion, frame);
        let (sy, cy) = yaw.sin_cos();
        for v in &geo.vertices {
            let x = (v.x - off_x) * s;
            let y = (v.y - off_y) * s;
            let z = (v.z - off_z) * s;
            salida.push(DioramaVertex {
                x: x * cy + z * sy + px,
                y,
                z: -x * sy + z * cy + pz,
                nx: v.nx * cy + v.nz * sy,
                ny: v.ny,
                nz: -v.nx * sy + v.nz * cy,
                u: v.u,
                v: v.v,
                r: v.r,
                g: v.g,
                b: v.b,
                a: v.a,
            });
        }
    }
    Some(salida)
}

/// Geometria de una entidad por su TIPO numerico (el que va en
/// `LiveEntity::entity_type`).
///
/// Ojo: el tipo numerico es grueso (todos los cofres son el tipo 7), asi que
/// para el modelo exacto hay que llamar a `geometria_prototipo` con el
/// nombre del prototipo que manda el mod puente.
pub fn geometria_tipo(
    tipo: u32,
    tile_x: f32,
    tile_y: f32,
    orientacion: f32,
    frame: u32,
    escala_tile_m: f32,
) -> Option<Vec<DioramaVertex>> {
    let proto = prototipo_para_tipo(tipo)?;
    geometria_prototipo(proto, tile_x, tile_y, orientacion, frame, escala_tile_m)
}

/// Numero de vertices de un modelo (sin transformarlo). Util para medir.
pub fn vertices_de_prototipo(proto: &str) -> Option<usize> {
    let modelo = modelo_para_prototipo(proto)?;
    let mut n = 0;
    for parte in &modelo.partes {
        n += geometria_de_fichero(&parte.archivo)?.vertices.len();
    }
    Some(n)
}

// ---------------------------------------------------------------------------
// FFI para el motor de VR (el C++ lo llama por dlsym, como el resto)
// ---------------------------------------------------------------------------

/// Geometria 3D de una entidad por su TIPO numerico (`LiveEntity::entity_type`).
///
/// Devuelve `false` si ese tipo no tiene modelo 3D (o no se pudo cargar):
/// el motor dibuja entonces el sprite 2D del atlas, como siempre.
///
/// La memoria se libera con `xfactorvr_free_glb` (mismo `DioramaVertex`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_modelo3d_geometria(
    tipo: u32,
    tile_x: f32,
    tile_y: f32,
    orientacion: f32,
    frame: u32,
    escala_tile_m: f32,
    out_verts: *mut *mut DioramaVertex,
    out_count: *mut i32,
) -> bool {
    if out_verts.is_null() || out_count.is_null() {
        return false;
    }
    *out_verts = std::ptr::null_mut();
    *out_count = 0;
    match geometria_tipo(tipo, tile_x, tile_y, orientacion, frame, escala_tile_m) {
        Some(mut verts) if !verts.is_empty() => {
            verts.shrink_to_fit();
            let count = verts.len() as i32;
            let ptr = verts.as_mut_ptr();
            std::mem::forget(verts);
            *out_verts = ptr;
            *out_count = count;
            true
        }
        _ => false,
    }
}

/// Geometria 3D de una entidad por su PROTOTIPO ("assembling-machine-1").
///
/// El prototipo llega del mod puente y es mas fino que el tipo numerico
/// (distingue "wooden-chest" de "steel-chest"). Mismo contrato que
/// `xfactorvr_modelo3d_geometria`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_modelo3d_geometria_proto(
    prototipo: *const std::os::raw::c_char,
    tile_x: f32,
    tile_y: f32,
    orientacion: f32,
    frame: u32,
    escala_tile_m: f32,
    out_verts: *mut *mut DioramaVertex,
    out_count: *mut i32,
) -> bool {
    if prototipo.is_null() || out_verts.is_null() || out_count.is_null() {
        return false;
    }
    *out_verts = std::ptr::null_mut();
    *out_count = 0;
    let c_str = match std::ffi::CStr::from_ptr(prototipo).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match geometria_prototipo(c_str, tile_x, tile_y, orientacion, frame, escala_tile_m) {
        Some(mut verts) if !verts.is_empty() => {
            verts.shrink_to_fit();
            let count = verts.len() as i32;
            let ptr = verts.as_mut_ptr();
            std::mem::forget(verts);
            *out_verts = ptr;
            *out_count = count;
            true
        }
        _ => false,
    }
}

/// Dice si un tipo de entidad tiene modelo 3D (para no dibujar el sprite).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_modelo3d_tiene(tipo: u32) -> bool {
    prototipo_para_tipo(tipo).is_some()
}

// ---------------------------------------------------------------------------
// REGISTRO DE PROTOTIPOS Y GEOMETRIA VIVA
// ---------------------------------------------------------------------------

/// Registro de prototipos -> id estable.
///
/// El id va en `LiveEntity::model_id`, asi el motor de VR sabe sin mirar
/// cadenas que esa entidad se dibuja con una malla 3D y no con el sprite
/// plano.  Los ids son estables entre frames (el mismo prototipo, el mismo
/// id), que es lo que permite cachear.
static REGISTRO: OnceLock<std::sync::Mutex<HashMap<String, u32>>> = OnceLock::new();
static SIGUIENTE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn registro() -> &'static std::sync::Mutex<HashMap<String, u32>> {
    REGISTRO.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Id estable de un prototipo (0 = sin modelo 3D).
pub fn id_de_prototipo(proto: &str) -> u32 {
    if proto.is_empty() || modelo_para_prototipo(proto).is_none() {
        return 0;
    }
    let mut reg = registro().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(id) = reg.get(proto) {
        return *id;
    }
    let id = SIGUIENTE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    reg.insert(proto.to_string(), id);
    id
}

/// Prototipo de un id (el inverso de `id_de_prototipo`).
pub fn prototipo_de_id(id: u32) -> Option<String> {
    if id == 0 {
        return None;
    }
    let reg = registro().lock().unwrap_or_else(|e| e.into_inner());
    reg.iter()
        .find(|(_, v)| **v == id)
        .map(|(k, _)| k.clone())
}

/// Escala de una casilla en metros (la del manifiesto).
pub fn escala_tile() -> f32 {
    manifiesto()
        .map(|m| m.escala_tile_m)
        .unwrap_or(ESCALA_TILE_M_POR_DEFECTO)
}

/// Geometria 3D de TODAS las entidades vivas que tienen modelo.
///
/// Se cachea por version: el cache se invalida cuando llega una
/// instantanea nueva del motor, no en cada frame (si no, el visor
/// recalcularia la geometria de toda la fabrica 60 veces por segundo).
static CACHE_VIVA: OnceLock<std::sync::Mutex<Option<(u64, Vec<DioramaVertex>)>>> = OnceLock::new();
static VERSION_VIVA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn cache_viva() -> &'static std::sync::Mutex<Option<(u64, Vec<DioramaVertex>)>> {
    CACHE_VIVA.get_or_init(|| std::sync::Mutex::new(None))
}

/// Invalida la geometria cacheada (al llegar una instantanea nueva).
pub fn invalidar_geometria() {
    VERSION_VIVA.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Geometria viva (con la cache por version).
pub fn geometria_viva() -> Vec<DioramaVertex> {
    let version = VERSION_VIVA.load(std::sync::atomic::Ordering::Relaxed);
    {
        let cache = cache_viva().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((v, verts)) = cache.as_ref() {
            if *v == version {
                return verts.clone();
            }
        }
    }
    let verts = calcular_geometria_viva();
    let mut cache = cache_viva().lock().unwrap_or_else(|e| e.into_inner());
    *cache = Some((version, verts.clone()));
    verts
}

/// Calcula la geometria de todas las entidades vivas con modelo.
fn calcular_geometria_viva() -> Vec<DioramaVertex> {
    let escala = escala_tile();
    // El desplazamiento de la vista se aplica aqui, igual que lo hace el
    // motor de VR con el diorama: las coordenadas del modelo son relativas al
    // centro de la vista, no al origen del mapa.
    let (pan_x, pan_y) = crate::game_bridge::get_simulation_world()
        .read()
        .map(|w| w.view_center)
        .unwrap_or((0.0, 0.0));
    let entidades = crate::get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();
    let mut salida: Vec<DioramaVertex> = Vec::new();
    for e in entidades {
        if e.model_id == 0 {
            continue;
        }
        let proto = match prototipo_de_id(e.model_id) {
            Some(p) => p,
            None => continue,
        };
        if let Some(geo) = geometria_prototipo(
            &proto,
            e.tile_x - pan_x,
            e.tile_y - pan_y,
            e.orientation,
            e.anim_frame,
            escala,
        ) {
            salida.extend_from_slice(&geo);
        }
    }
    salida
}

/// Geometria 3D de las entidades vivas, lista para el motor de VR.
///
/// Devuelve cuantos vertices escribio en `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_modelo3d_geometria_viva(
    out: *mut DioramaVertex,
    max: i32,
) -> i32 {
    if out.is_null() || max <= 0 {
        return 0;
    }
    let verts = geometria_viva();
    let n = verts.len().min(max as usize);
    std::ptr::copy_nonoverlapping(verts.as_ptr(), out, n);
    n as i32
}
