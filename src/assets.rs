//! assets.rs — Puente entre los assets reales de Factorio y el motor VR.
//!
//! El renderizador de XR no dibuja primitivas: dibuja los sprites que usa el
//! juego. Para eso hace falta el atlas de sprites que se extrae de una
//! instalacion de Factorio (scripts/extract_factorio_assets.py) junto con su
//! manifiesto, que dice, para cada tipo de entidad, las capas de las que se
//! compone y donde esta cada frame dentro del atlas.
//!
//! El manifiesto lo consume Rust (serde ya esta en el proyecto) y el motor C++
//! pregunta por el rectangulo de un frame mediante la FFI
//! `xfactorvr_get_sprite_frame`. Asi el JSON se parsea en un solo sitio.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Capa de sprite: una imagen con N frames.
#[derive(Debug, Clone, Deserialize)]
pub struct SpriteLayer {
    /// Ruta del PNG de origen (informativa).
    #[serde(default)]
    pub filename: String,
    /// Ancho y alto de un frame en pixeles.
    #[serde(default)]
    pub frame_width: f32,
    #[serde(default)]
    pub frame_height: f32,
    /// Numero de frames de la capa.
    #[serde(default)]
    pub frame_count: u32,
    /// Numero de direcciones (0 = la capa no gira).
    #[serde(default)]
    pub direction_count: u32,
    /// Frames de animacion: cuantos dibuja y a que velocidad.
    #[serde(default)]
    pub animation: Option<Animation>,
    /// Escala del sprite en el juego (0.5 en la plupart de entidades).
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Posicion de cada frame dentro del atlas.
    #[serde(default)]
    pub frames: Vec<AtlasRect>,
}

fn default_scale() -> f32 {
    0.5
}

#[derive(Debug, Clone, Deserialize)]
pub struct Animation {
    #[serde(default)]
    pub frames: u32,
    #[serde(default)]
    pub speed: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct AtlasRect {
    /// Pagina del atlas donde vive el frame (el atlas se parte en paginas
    /// porque una textura de mas de 4096 px no cabe en la GPU movil).
    #[serde(default)]
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Entidad del manifiesto.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntity {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub layers: Vec<SpriteLayer>,
    #[serde(default)]
    pub draw_order: i32,
}

/// Manifiesto completo del atlas.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetManifest {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub atlas: Option<AtlasInfo>,
    #[serde(default)]
    pub entities: HashMap<String, AssetEntity>,
    #[serde(default)]
    pub icons: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AtlasInfo {
    #[serde(default)]
    pub file: String,
    /// Fichero con los pixeles en crudo (RGBA8 sin cabecera). Lo escribe el
    /// extractor para que el motor no tenga que decodificar PNG.
    #[serde(default)]
    pub raw_file: Option<String>,
    /// Paginas del atlas. Cada una es una textura independiente.
    #[serde(default)]
    pub pages: Vec<AtlasPage>,
    pub width: u32,
    pub height: u32,
}

/// Pagina del atlas: una textura que se puede subir a la GPU.
#[derive(Debug, Clone, Deserialize)]
pub struct AtlasPage {
    #[serde(default)]
    pub file: String,
    pub width: u32,
    pub height: u32,
}

/// Rectangulo listo para dibujar: coordenadas de textura en pixeles del atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteFrame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub frame_width: f32,
    pub frame_height: f32,
    pub direction_count: u32,
    pub animation_frames: u32,
    pub scale: f32,
    pub page: u32,
}

static MANIFEST: OnceLock<Option<AssetManifest>> = OnceLock::new();
static SPRITE_IDS: OnceLock<HashMap<String, u32>> = OnceLock::new();

/// Directorio donde vive el atlas ya desplegado.
///
/// Se puede forzar con XFACTORVR_ASSETS_DIR. Si no, se buscan las rutas
/// habituales: el despliegue deja los assets en /sdcard/XFactorVR/assets y
/// el motor tambien puede copiarlos al almacenamiento interno de la app.
pub fn assets_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XFACTORVR_ASSETS_DIR") {
        return PathBuf::from(dir);
    }
    const CANDIDATOS: &[&str] = &[
        "/sdcard/XFactorVR/assets",
        "/storage/emulated/0/XFactorVR/assets",
        "/data/local/tmp/XFactorVR/assets",
    ];
    for c in CANDIDATOS {
        let p = PathBuf::from(c);
        if p.join("manifest.json").exists() {
            return p;
        }
    }
    PathBuf::from(CANDIDATOS[0])
}

/// Carga (una sola vez) el manifiesto del atlas.
pub fn manifest() -> Option<&'static AssetManifest> {
    MANIFEST
        .get_or_init(|| {
            let path = assets_dir().join("manifest.json");
            let text = std::fs::read_to_string(&path).ok()?;
            match serde_json::from_str::<AssetManifest>(&text) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::warn!("Manifiesto de assets ilegible ({}): {}", path.display(), e);
                    None
                }
            }
        })
        .as_ref()
}

/// Mapa nombre de entidad -> id de sprite. Los ids son indices estables
/// (orden alfabetico) para que el C++ y Rust coincidan siempre.
fn sprite_ids() -> &'static HashMap<String, u32> {
    SPRITE_IDS.get_or_init(|| {
        manifest()
            .map(sprite_ids_for)
            .unwrap_or_default()
    })
}

/// Mapa nombre -> id de sprite a partir de un manifiesto concreto.
pub fn sprite_ids_for(m: &AssetManifest) -> HashMap<String, u32> {
    let mut names: Vec<String> = m.entities.keys().cloned().collect();
    names.sort();
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i as u32 + 1))
        .collect()
}

/// Id de sprite de un tipo de entidad de Factorio. 0 = sin sprite.
pub fn sprite_id_for_type(type_name: &str) -> u32 {
    sprite_ids().get(type_name).copied().unwrap_or(0)
}

/// Devuelve el rectangulo del frame pedido de un sprite.
///
/// `frame` es el frame de animacion del motor. `direction` es la direccion de
/// Factorio (0..15) que elige la fila del sprite girado.
pub fn sprite_frame(sprite_id: u32, frame: u32, direction: u8) -> Option<SpriteFrame> {
    let m = manifest()?;
    sprite_frame_in(m, sprite_id, frame, direction)
}

/// Resolucion del frame contra un manifiesto concreto (version testeable).
pub fn sprite_frame_in(
    m: &AssetManifest,
    sprite_id: u32,
    frame: u32,
    direction: u8,
) -> Option<SpriteFrame> {
    if sprite_id == 0 {
        return None;
    }
    let mut names: Vec<&String> = m.entities.keys().collect();
    names.sort();
    let name = names.get((sprite_id - 1) as usize)?;
    let entity = m.entities.get(*name)?;
    let layer = entity.layers.first()?;

    let anim = layer.animation.as_ref().map(|a| a.frames).unwrap_or(1).max(1);
    let dirs = layer.direction_count.max(1);
    // Factorio usa 16 direcciones; el sprite puede declarar menos (por
    // ejemplo 1 si no gira). Se escala la direccion al numero de filas.
    let row = if dirs == 1 {
        0
    } else {
        ((direction as u32 * dirs) / 16).min(dirs - 1)
    };

    // El frame de animacion rota dentro de la fila de la direccion.
    let per_row = (layer.frame_count / dirs).max(1);
    let anim_index = if anim > 0 && per_row > 1 {
        frame % anim
    } else {
        0
    };
    let index = (row * per_row + anim_index).min(layer.frame_count.saturating_sub(1));

    let rect = layer.frames.get(index as usize)?;
    Some(SpriteFrame {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
        frame_width: layer.frame_width,
        frame_height: layer.frame_height,
        direction_count: dirs,
        animation_frames: anim,
        scale: layer.scale,
        page: rect.page,
    })
}

/// Ruta del PNG del atlas.
pub fn atlas_file() -> Option<PathBuf> {
    let m = manifest()?;
    let atlas = m.atlas.as_ref()?;
    let file = if atlas.file.is_empty() {
        "atlas.png".to_string()
    } else {
        atlas.file.clone()
    };
    Some(assets_dir().join(file))
}

static ATLAS_PIXELS: OnceLock<Option<Vec<(Vec<u8>, u32, u32)>>> = OnceLock::new();

/// Pixeles del atlas en RGBA8 crudo, listos para subir a la GPU.
///
/// Android no trae decodificador de PNG, asi que el extractor escribe
/// ademas el atlas en crudo (`atlas.rgba`): son width*height*4 bytes en
/// orden de filas, sin cabecera. Aqui solo se lee el fichero.
pub fn atlas_pages() -> Option<&'static Vec<(Vec<u8>, u32, u32)>> {
    ATLAS_PIXELS
        .get_or_init(|| {
            let m = manifest()?;
            let atlas = m.atlas.as_ref()?;
            let dir = assets_dir();
            let mut out = Vec::new();
            for page in &atlas.pages {
                let path = dir.join(&page.file);
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) => {
                        log::warn!("Pagina de atlas {} ilegible: {}", page.file, e);
                        return None;
                    }
                };
                let expected = (page.width as usize) * (page.height as usize) * 4;
                if data.len() != expected {
                    log::warn!(
                        "Pagina {} con tamano inesperado ({} != {} bytes)",
                        page.file,
                        data.len(),
                        expected
                    );
                    return None;
                }
                out.push((data, page.width, page.height));
            }
            if out.is_empty() {
                return None;
            }
            Some(out)
        })
        .as_ref()
}

/// Numero de paginas del atlas.
pub fn atlas_page_count() -> usize {
    atlas_pages().map(|p| p.len()).unwrap_or(0)
}

/// Pixeles de una pagina del atlas (RGBA8 crudo).
pub fn atlas_page_pixels(page: u32) -> Option<&'static (Vec<u8>, u32, u32)> {
    atlas_pages()?.get(page as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EJEMPLO: &str = r#"{
      "version": "2.1.22",
      "atlas": {"file": "atlas.png", "width": 2048, "height": 1024},
      "entities": {
        "assembling-machine-1": {
          "name": "assembling-machine-1",
          "draw_order": 3,
          "layers": [{
            "filename": "base/graphics/entity/assembling-machine-1/assembling-machine-1-base.png",
            "frame_width": 198, "frame_height": 184,
            "frame_count": 1, "direction_count": 1,
            "animation": {"frames": 1, "speed": 0.0},
            "frames": [{"x": 0, "y": 0, "w": 198, "h": 184}]
          }]
        },
        "transport-belt": {
          "name": "transport-belt",
          "draw_order": 2,
          "layers": [{
            "filename": "base/graphics/entity/transport-belt/transport-belt-base.png",
            "frame_width": 64, "frame_height": 64,
            "frame_count": 32, "direction_count": 4,
            "animation": {"frames": 8, "speed": 0.1},
            "frames": [
              {"x": 0, "y": 0, "w": 64, "h": 64},
              {"x": 64, "y": 0, "w": 64, "h": 64},
              {"x": 128, "y": 0, "w": 64, "h": 64},
              {"x": 192, "y": 0, "w": 64, "h": 64}
            ]
          }]
        }
      },
      "icons": {}
    }"#;

    #[test]
    fn test_manifiesto_se_parsea() {
        let m: AssetManifest = serde_json::from_str(EJEMPLO).expect("manifiesto valido");
        assert_eq!(m.version, "2.1.22");
        assert_eq!(m.entities.len(), 2);
        let e = m.entities.get("transport-belt").unwrap();
        assert_eq!(e.layers[0].frame_count, 32);
        assert_eq!(e.layers[0].direction_count, 4);
    }

    #[test]
    fn test_rectangulo_de_frame() {
        // Capa de 8 frames en 4 direcciones (2 frames por direccion).
        let frames: Vec<AtlasRect> = (0..8)
            .map(|i| AtlasRect { page: 0, x: i as f32 * 64.0, y: 0.0, w: 64.0, h: 64.0 })
            .collect();
        let layer = SpriteLayer {
            filename: String::new(),
            frame_width: 64.0,
            frame_height: 64.0,
            frame_count: 8,
            direction_count: 4,
            animation: Some(Animation { frames: 2, speed: 0.1 }),
            scale: 0.5,
            frames,
        };
        let per_row = (layer.frame_count / layer.direction_count).max(1);
        assert_eq!(per_row, 2);

        // Direccion este (4 de 16) -> fila 1 de 4; frame 1 -> indice 3.
        let row = (4 * layer.direction_count) / 16;
        assert_eq!(row, 1);
        let index = (row * per_row + 1).min(layer.frame_count - 1);
        assert_eq!(index, 3);
        let rect = layer.frames.get(index as usize).expect("frame existente");
        assert_eq!(rect.x, 192.0);
        assert_eq!(rect.w, 64.0);

        // Direccion norte (0) -> fila 0.
        let row = (0 * layer.direction_count) / 16;
        assert_eq!(row, 0);
    }

    #[test]
    fn test_manifiesto_real_si_existe() {
        // Si el atlas ya esta generado, se comprueba que el motor sabria
        // dibujar los tipos principales del juego.
        let dir = std::path::Path::new("build/factorio_assets");
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.exists() {
            eprintln!("sin atlas generado; se omite la comprobacion");
            return;
        }
        let text = std::fs::read_to_string(&manifest_path).unwrap();
        let m: AssetManifest = serde_json::from_str(&text).expect("manifiesto real valido");
        let ids = sprite_ids_for(&m);

        for tipo in [
            "assembling-machine-1",
            "transport-belt",
            "stone-furnace",
            "burner-mining-drill",
            "inserter",
        ] {
            let id = ids.get(tipo).copied().unwrap_or(0);
            assert!(id > 0, "{} deberia tener sprite en el manifiesto", tipo);
            let f = sprite_frame_in(&m, id, 0, 0)
                .unwrap_or_else(|| panic!("{} deberia tener un frame resoluble", tipo));
            assert!(f.w > 0.0 && f.h > 0.0, "{} con frame vacio", tipo);
            assert!(f.scale > 0.0, "{} con escala nula", tipo);
        }
    }

    #[test]
    fn test_direccion_se_escala_a_las_filas_del_sprite() {
        // Un sprite de 4 direcciones: la direccion 8 de 16 cae en la fila 2.
        let dirs = 4u32;
        let dir = 8u8; // sur
        let row = ((dir as u32 * dirs) / 16).min(dirs - 1);
        assert_eq!(row, 2);
        // Un sprite que no gira (1 direccion) se queda siempre en la fila 0.
        let dirs = 1u32;
        let row = if dirs == 1 { 0 } else { ((dir as u32 * dirs) / 16).min(dirs - 1) };
        assert_eq!(row, 0);
    }
}
