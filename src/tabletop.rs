// tabletop.rs — Matemáticas del volumen de mesa, anclaje espacial y picking 6DoF
//
// Transforma el espacio físico de la habitación (OpenXR stage/local) en el
// espacio de simulación 2D de Factorio, permitiendo que la fábrica viva
// sobre tu mesa real como una maqueta física.

use glam::{Mat4, Vec2, Vec3};

#[derive(Clone, Debug)]
pub struct TabletopConfig {
    /// Posición del centro de la mesa en el espacio físico OpenXR (metros)
    pub anchor_position: Vec3,
    /// Dimensiones de la mesa física en metros (ancho X, profundidad Z)
    pub table_dimensions: Vec2,
    /// Escala física: metros por cada casilla (tile) de Factorio.
    /// Ejemplo: 0.04 = 4 cm por casilla (una fábrica de 25x20 celdas ocupa 1.0m x 0.8m)
    pub tile_scale_meters: f32,
    /// Altura máxima sobre la mesa antes de recortar visualmente (metros)
    pub max_height_meters: f32,
    /// Centro actual de la fábrica visible en coordenadas de tile de Factorio
    pub factory_center_tiles: Vec2,
}

impl Default for TabletopConfig {
    fn default() -> Self {
        Self {
            // Mesa estándar ubicada a 1 metro al frente y 30 cm por debajo de la cabeza
            anchor_position: Vec3::new(0.0, -0.30, -1.0),
            table_dimensions: Vec2::new(1.20, 0.80),
            tile_scale_meters: 0.04,
            max_height_meters: 0.50,
            factory_center_tiles: Vec2::ZERO,
        }
    }
}

pub struct TabletopManager {
    pub config: TabletopConfig,
}

impl TabletopManager {
    pub fn new(config: TabletopConfig) -> Self {
        Self { config }
    }

    /// Calcula la matriz de modelo para transformar la fábrica al plano de la mesa física
    pub fn table_model_matrix(&self) -> Mat4 {
        let translation = Mat4::from_translation(self.config.anchor_position);
        let scale = Mat4::from_scale(Vec3::new(
            self.config.tile_scale_meters,
            self.config.tile_scale_meters,
            self.config.tile_scale_meters,
        ));
        translation * scale
    }

    /// Calcula la intersección de un rayo 6DoF (desde el mando Touch / Pico) con el plano de la mesa
    /// Retorna: Some((x_tile, y_tile)) si el rayo impacta dentro de la mesa física
    pub fn pick_tile_from_ray(&self, ray_origin: Vec3, ray_dir: Vec3) -> Option<(f32, f32)> {
        // El plano de la mesa es Y = anchor_position.y
        let table_y = self.config.anchor_position.y;

        // Evitar división por cero si el rayo es horizontal
        if ray_dir.y.abs() < 1e-5 {
            return None;
        }

        let t = (table_y - ray_origin.y) / ray_dir.y;
        if t <= 0.0 {
            // El rayo apunta en dirección contraria
            return None;
        }

        let hit_point = ray_origin + ray_dir * t;

        // Verificar si el impacto está dentro de las dimensiones de la mesa física
        let half_w = self.config.table_dimensions.x * 0.5;
        let half_d = self.config.table_dimensions.y * 0.5;

        let local_x = hit_point.x - self.config.anchor_position.x;
        let local_z = hit_point.z - self.config.anchor_position.z;

        if local_x.abs() > half_w || local_z.abs() > half_d {
            return None; // Fuera de los bordes de la mesa
        }

        // Convertir de metros relativos a casillas (tiles) de Factorio
        let tile_dx = local_x / self.config.tile_scale_meters;
        let tile_dy = local_z / self.config.tile_scale_meters;

        let final_tile_x = self.config.factory_center_tiles.x + tile_dx;
        let final_tile_y = self.config.factory_center_tiles.y + tile_dy;

        Some((final_tile_x, final_tile_y))
    }

    /// Modifica la escala física de la mesa (pinch-to-zoom con las dos manos)
    pub fn adjust_scale(&mut self, factor: f32) {
        self.config.tile_scale_meters = (self.config.tile_scale_meters * factor).clamp(0.01, 0.20);
    }

    /// Desplaza el centro de la fábrica (arrastre / pan con joystick o mano)
    pub fn pan_factory(&mut self, delta_tiles: Vec2) {
        self.config.factory_center_tiles += delta_tiles;
    }
}

// Tipos de entidad. Estos valores son el contrato con el mod puente
// (mods/xfactorvr_bridge/control.lua) y con el renderizador OpenXR: los tres
// lados deben usar exactamente la misma tabla.
//
//   1 ensambladora   2 perforadora   3 horno       4 torreta
//   5 radar          6 robopuerto     7 cofre       8 poste electrico
//   9 cinta          10 insertador    11 enemigo    12 arbol/roca
//   13 jugador
pub const ENTITY_ASSEMBLING_MACHINE: u32 = 1;
pub const ENTITY_MINING_DRILL: u32 = 2;
pub const ENTITY_FURNACE: u32 = 3;
pub const ENTITY_GUN_TURRET: u32 = 4;
pub const ENTITY_RADAR: u32 = 5;
pub const ENTITY_ROBOPORT: u32 = 6;
pub const ENTITY_CHEST: u32 = 7;
pub const ENTITY_ELECTRIC_POLE: u32 = 8;
pub const ENTITY_TRANSPORT_BELT: u32 = 9;
pub const ENTITY_INSERTER: u32 = 10;
pub const ENTITY_BITER: u32 = 11;
pub const ENTITY_TREE: u32 = 12;
pub const ENTITY_PLAYER: u32 = 13;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveEntity {
    pub entity_type: u32,
    pub tile_x: f32,
    pub tile_y: f32,
    pub orientation: f32,
    pub status: u32,
    pub animation_progress: f32,
    pub health_ratio: f32,
    /// Angulo del brazo en radianes (solo insertadores). 0 = sin dato.
    pub arm_angle: f32,
    /// Id del sprite real de Factorio en el atlas (0 = sin sprite, el render
    /// usa la primitiva). Lo resuelve el puente a partir del nombre del
    /// prototipo que envia el mod.
    pub sprite_id: u32,
    /// Frame de animacion actual (0 = sin dato).
    pub anim_frame: u32,
    /// Id del MODELO 3D de esta entidad (0 = sin modelo, se dibuja el sprite).
    ///
    /// Lo asigna el modulo de modelos (src/modelos3d.rs) a partir del
    /// prototipo que manda el mod puente.  El motor de VR lo usa para saber
    /// si esa entidad se dibuja con la malla 3D de verdad o con el sprite
    /// plano del atlas.
    pub model_id: u32,
}

// ---------------------------------------------------------------------------
// Inventario interactivo: lo que hay bajo el rayo de la mano
//
// En VR no hay raton ni ventana de inventario: la unica forma de coger un
// item es apuntar con la mano y apretar. Para saber QUE hay que hacer hace
// falta saber que hay en esa casilla:
//
//   - un item suelto en el suelo  -> se coge
//   - una caja                    -> se saca de ella (o se mete en ella)
//   - un horno                    -> se le meten ingredientes
//
// OJO: los items del suelo NO pueden ir en `LiveEntity`. Esa struct es
// `#[repr(C)]` y esta espejada tal cual en el motor de VR
// (android/src/openxr_engine.cpp), que indexa una tabla de sprites por
// `entity_type` con un tope de ENTITY_TYPE_COUNT. Añadir un tipo nuevo
// romperia el espejo. Por eso los items sueltos llegan en su propia seccion
// del snapshot ("loose") y se guardan aparte.
// ---------------------------------------------------------------------------

/// Item suelto en el suelo (la entidad "item-on-ground" de Factorio).
#[derive(Clone, Debug, PartialEq)]
pub struct LooseItem {
    /// Nombre del prototipo del item ("iron-plate", "coal"...).
    pub item_name: String,
    pub tile_x: f32,
    pub tile_y: f32,
    pub count: u32,
}

/// Radio (en casillas) en el que se considera que algo esta "bajo" el rayo.
pub const TARGET_RADIUS: f32 = 1.0;

/// Que hay bajo el rayo de la mano y con que se puede interactuar.
#[derive(Clone, Debug, PartialEq)]
pub enum Interactable {
    /// Un item suelto en el suelo: se puede coger.
    LooseItem {
        item_name: String,
        count: u32,
    },
    /// Una caja: se puede sacar de ella y meter en ella.
    Chest,
    /// Un horno: se le pueden meter ingredientes (y combustible).
    Furnace,
    /// No hay nada con lo que interactuar.
    Nothing,
}

/// Clasifica lo que hay en una casilla.
///
/// Se mira lo mas CERCA: si a la vez hay una caja y un item suelto al lado,
/// manda el mas cercano. Las entidades mandan sobre los items sueltos cuando
/// estan empatados (una caja ocupa su casilla entera).
pub fn classify_target(
    live: &[LiveEntity],
    loose: &[LooseItem],
    tile_x: f32,
    tile_y: f32,
) -> Interactable {
    let mut mejor: Option<(f32, Interactable)> = None;

    let mut considerar = |dist: f32, kind: Interactable| {
        if dist > TARGET_RADIUS {
            return;
        }
        let gana = match &mejor {
            Some((mejor_dist, _)) => dist < *mejor_dist - 0.01,
            None => true,
        };
        if gana {
            mejor = Some((dist, kind));
        }
    };

    for e in live {
        let kind = match e.entity_type {
            ENTITY_CHEST => Interactable::Chest,
            ENTITY_FURNACE => Interactable::Furnace,
            _ => continue,
        };
        let dx = e.tile_x - tile_x;
        let dy = e.tile_y - tile_y;
        considerar((dx * dx + dy * dy).sqrt(), kind);
    }

    for item in loose {
        let dx = item.tile_x - tile_x;
        let dy = item.tile_y - tile_y;
        considerar(
            (dx * dx + dy * dy).sqrt(),
            Interactable::LooseItem {
                item_name: item.item_name.clone(),
                count: item.count,
            },
        );
    }

    mejor.map(|(_, kind)| kind).unwrap_or(Interactable::Nothing)
}

pub fn init_default_factory_entities() -> Vec<LiveEntity> {
    vec![
        // Ensambladora en (-5.5, -5.5) tiles
        LiveEntity { entity_type: ENTITY_ASSEMBLING_MACHINE, tile_x: -5.5, tile_y: -5.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Minera eléctrica en (6.25, -5.5) tiles
        LiveEntity { entity_type: ENTITY_MINING_DRILL, tile_x: 6.25, tile_y: -5.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Horno de acero en (11.0, -1.5) tiles
        LiveEntity { entity_type: ENTITY_FURNACE, tile_x: 11.0, tile_y: -1.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Radar en (-11.0, 5.5) tiles
        LiveEntity { entity_type: ENTITY_RADAR, tile_x: -11.0, tile_y: 5.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Robopuerto en (6.25, 5.5) tiles
        LiveEntity { entity_type: ENTITY_ROBOPORT, tile_x: 6.25, tile_y: 5.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Torreta en (-11.0, -5.5) tiles orientada a 45°
        LiveEntity { entity_type: ENTITY_GUN_TURRET, tile_x: -11.0, tile_y: -5.5, orientation: std::f32::consts::FRAC_PI_4, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Cofre de acero en (-10.5, -1.5) tiles
        LiveEntity { entity_type: ENTITY_CHEST, tile_x: -10.5, tile_y: -1.5, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Postes eléctricos en (0.0, -7.0) y (0.0, 7.0)
        LiveEntity { entity_type: ENTITY_ELECTRIC_POLE, tile_x: 0.0, tile_y: -7.0, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        LiveEntity { entity_type: ENTITY_ELECTRIC_POLE, tile_x: 0.0, tile_y: 7.0, orientation: 0.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Biter hostil en el borde (12.5, 6.25)
        LiveEntity { entity_type: ENTITY_BITER, tile_x: 12.5, tile_y: 6.25, orientation: -2.0, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
        // Árbol en (-12.5, 8.0)
        LiveEntity { entity_type: ENTITY_TREE, tile_x: -12.5, tile_y: 8.0, orientation: 0.5, status: 1, animation_progress: 0.0, health_ratio: 1.0, arm_angle: 0.0, sprite_id: 0, anim_frame: 0, model_id: 0, },
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DioramaVertex {
    pub x: f32, pub y: f32, pub z: f32,
    pub nx: f32, pub ny: f32, pub nz: f32,
    pub u: f32, pub v: f32,
    pub r: f32, pub g: f32, pub b: f32, pub a: f32,
}

fn sample_image(img: &gltf::image::Data, u: f32, v: f32, tint: [f32; 4]) -> [f32; 4] {
    if img.width == 0 || img.height == 0 || img.pixels.is_empty() {
        return tint;
    }
    let u_norm = u.rem_euclid(1.0);
    let v_norm = v.rem_euclid(1.0);
    let px = ((u_norm * img.width as f32) as u32).min(img.width.saturating_sub(1));
    let py = ((v_norm * img.height as f32) as u32).min(img.height.saturating_sub(1));

    match img.format {
        gltf::image::Format::R8G8B8 => {
            let idx = ((py * img.width + px) as usize * 3).min(img.pixels.len().saturating_sub(3));
            let r = (img.pixels[idx] as f32 / 255.0) * tint[0];
            let g = (img.pixels[idx + 1] as f32 / 255.0) * tint[1];
            let b = (img.pixels[idx + 2] as f32 / 255.0) * tint[2];
            [r, g, b, tint[3]]
        }
        gltf::image::Format::R8G8B8A8 => {
            let idx = ((py * img.width + px) as usize * 4).min(img.pixels.len().saturating_sub(4));
            let r = (img.pixels[idx] as f32 / 255.0) * tint[0];
            let g = (img.pixels[idx + 1] as f32 / 255.0) * tint[1];
            let b = (img.pixels[idx + 2] as f32 / 255.0) * tint[2];
            let a = (img.pixels[idx + 3] as f32 / 255.0) * tint[3];
            [r, g, b, a]
        }
        _ => tint,
    }
}

pub fn load_glb_as_vertices<P: AsRef<std::path::Path>>(path: P, target_width: f32) -> anyhow::Result<Vec<DioramaVertex>> {
    let (document, buffers, images) = gltf::import(path.as_ref())?;

    struct RawPrim {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        colors: Vec<[f32; 4]>,
        indices: Vec<u32>,
    }

    fn walk_node(
        node: &gltf::Node,
        parent: glam::Mat4,
        buffers: &[gltf::buffer::Data],
        images: &[gltf::image::Data],
        out: &mut Vec<RawPrim>,
    ) {
        let world = parent * glam::Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            let mut nrm_mat = glam::Mat3::from_mat4(world).inverse().transpose();
            if !nrm_mat.is_finite() {
                nrm_mat = glam::Mat3::from_mat4(world);
            }
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let raw_pos: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(p) => p.collect(),
                    None => continue,
                };
                let raw_norm: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|n| n.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; raw_pos.len()]);
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; raw_pos.len()]);

                let mut positions = Vec::with_capacity(raw_pos.len());
                let mut normals = Vec::with_capacity(raw_pos.len());
                for i in 0..raw_pos.len() {
                    let p = world.transform_point3(glam::Vec3::from(raw_pos[i]));
                    let n = (nrm_mat * glam::Vec3::from(raw_norm[i])).normalize_or_zero();
                    positions.push(p.into());
                    normals.push(n.into());
                }

                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_else(|| (0..positions.len() as u32).collect());

                let mat = prim.material();
                let pbr = mat.pbr_metallic_roughness();
                let base_color = pbr.base_color_factor();
                let tex_source = pbr.base_color_texture().map(|t| {
                    let source_idx = t.texture().source().index();
                    let (offset, scale) = if let Some(tt) = t.texture_transform() {
                        (tt.offset(), tt.scale())
                    } else {
                        ([0.0, 0.0], [1.0, 1.0])
                    };
                    (source_idx, offset, scale)
                });

                let mut colors = Vec::with_capacity(raw_pos.len());
                for i in 0..raw_pos.len() {
                    let raw_uv = if i < uvs.len() { uvs[i] } else { [0.0, 0.0] };
                    let c = match tex_source {
                        Some((src_idx, offset, scale)) if src_idx < images.len() => {
                            let u = raw_uv[0] * scale[0] + offset[0];
                            let v = raw_uv[1] * scale[1] + offset[1];
                            sample_image(&images[src_idx], u, v, base_color)
                        }
                        _ => base_color,
                    };
                    colors.push(c);
                }

                out.push(RawPrim {
                    positions,
                    normals,
                    uvs,
                    colors,
                    indices,
                });
            }
        }
        for child in node.children() {
            walk_node(&child, world, buffers, images, out);
        }
    }

    let mut raw_prims = Vec::new();
    let scene = document.default_scene().or_else(|| document.scenes().next()).ok_or_else(|| anyhow::anyhow!("No scene in GLTF"))?;
    for node in scene.nodes() {
        walk_node(&node, glam::Mat4::IDENTITY, &buffers, &images, &mut raw_prims);
    }

    if raw_prims.is_empty() {
        return Ok(Vec::new());
    }

    let mut min_pt = glam::Vec3::splat(f32::MAX);
    let mut max_pt = glam::Vec3::splat(f32::MIN);
    for prim in &raw_prims {
        for p in &prim.positions {
            let v = glam::Vec3::from(*p);
            min_pt = min_pt.min(v);
            max_pt = max_pt.max(v);
        }
    }

    let extent_x = max_pt.x - min_pt.x;
    let extent_y = max_pt.y - min_pt.y;
    let extent_z = max_pt.z - min_pt.z;
    if path.as_ref().to_string_lossy().contains("PLAYER") {
        println!("PLAYER GLB raw extents: x={:.3}, y={:.3}, z={:.3}", extent_x, extent_y, extent_z);
    }
    let max_extent = if extent_y > extent_x.max(extent_z) * 1.5 {
        // Objeto vertical alto (jugador, poste, árbol) -> target_width escala la altura
        extent_y.max(0.001)
    } else {
        extent_x.max(extent_z).max(0.001)
    };
    let scale = target_width / max_extent;

    let center_x = (min_pt.x + max_pt.x) * 0.5;
    let center_z = (min_pt.z + max_pt.z) * 0.5;
    let base_y = min_pt.y;

    let mut result = Vec::new();
    for prim in raw_prims {
        for idx in prim.indices {
            let i = idx as usize;
            if i >= prim.positions.len() {
                continue;
            }
            let pos = prim.positions[i];
            let norm = prim.normals[i];
            let uv = if i < prim.uvs.len() { prim.uvs[i] } else { [0.0, 0.0] };
            let col = if i < prim.colors.len() { prim.colors[i] } else { [1.0, 1.0, 1.0, 1.0] };

            let x = (pos[0] - center_x) * scale;
            let y = (pos[1] - base_y) * scale;
            let z = (pos[2] - center_z) * scale;

            result.push(DioramaVertex {
                x, y, z,
                nx: norm[0], ny: norm[1], nz: norm[2],
                u: uv[0], v: uv[1],
                r: col[0], g: col[1], b: col[2], a: col[3],
            });
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_all_models_dimensions() {
        let models = [
            ("assembling-machine", "factorio-3d-mod/models/ENTITIES/STRUCTURES/assembling-machine/static.glb", 0.18),
            ("mining-drill", "factorio-3d-mod/models/ENTITIES/STRUCTURES/electric-mining-drill/static.glb", 0.16),
            ("steel-furnace", "factorio-3d-mod/models/ENTITIES/STRUCTURES/steel-furnace/static.glb", 0.13),
            ("radar", "factorio-3d-mod/models/ENTITIES/STRUCTURES/radar/static1.glb", 0.14),
            ("roboport", "factorio-3d-mod/models/ENTITIES/STRUCTURES/roboport/static.glb", 0.18),
            ("turret1", "factorio-3d-mod/models/ENTITIES/STRUCTURES/gun-turret/static1.glb", 0.10),
            ("turret2", "factorio-3d-mod/models/ENTITIES/STRUCTURES/gun-turret/static2.glb", 0.09),
            ("pole", "factorio-3d-mod/models/ENTITIES/STRUCTURES/medium-electric-pole/static.glb", 0.06),
            ("player", "factorio-3d-mod/models/ENTITIES/PLAYER/idle.glb", 0.075),
            ("biter", "factorio-3d-mod/models/ENTITIES/ENEMIES/biters/biter_tier1.glb", 0.09),
            ("tree", "factorio-3d-mod/models/ENTITIES/FOLIAGE/tree1/static.glb", 0.13),
        ];

        for (name, path, width) in models {
            if std::path::Path::new(path).exists() {
                if name == "player" {
                    let (doc, buffers, _) = gltf::import(path).unwrap();
                    let scene = doc.default_scene().or_else(|| doc.scenes().next()).unwrap();
                    let mut min_pt = glam::Vec3::splat(f32::MAX);
                    let mut max_pt = glam::Vec3::splat(f32::MIN);
                    for node in scene.nodes() {
                        if let Some(mesh) = node.mesh() {
                            for prim in mesh.primitives() {
                                let r = prim.reader(|b| Some(&buffers[b.index()]));
                                if let Some(pos) = r.read_positions() {
                                    for p in pos {
                                        min_pt = min_pt.min(glam::Vec3::from(p));
                                        max_pt = max_pt.max(glam::Vec3::from(p));
                                    }
                                }
                            }
                        }
                    }
                    println!("Player raw bounds: min={:?}, max={:?}, extent={:?}", min_pt, max_pt, max_pt - min_pt);
                }
                let verts = load_glb_as_vertices(path, width).expect("failed to load glb");
                let mut min_y = f32::MAX;
                let mut max_y = f32::MIN;
                for v in &verts {
                    min_y = min_y.min(v.y);
                    max_y = max_y.max(v.y);
                }
                println!("Model {:18}: {} verts, height = {:.3}m, base = {:.3}m", name, verts.len(), max_y - min_y, min_y);

    #[test]
    fn test_clasifica_lo_que_hay_bajo_el_rayo() {
        let live = vec![
            LiveEntity {
                entity_type: ENTITY_CHEST,
                tile_x: 5.0,
                tile_y: 5.0,
                orientation: 0.0,
                status: 1,
                animation_progress: 0.0,
                health_ratio: 1.0,
                arm_angle: 0.0,
                sprite_id: 0,
                anim_frame: 0, model_id: 0,
            },
            LiveEntity {
                entity_type: ENTITY_FURNACE,
                tile_x: -5.0,
                tile_y: -5.0,
                orientation: 0.0,
                status: 1,
                animation_progress: 0.0,
                health_ratio: 1.0,
                arm_angle: 0.0,
                sprite_id: 0,
                anim_frame: 0, model_id: 0,
            },
        ];
        let loose = vec![LooseItem {
            item_name: "iron-plate".to_string(),
            tile_x: 0.5,
            tile_y: 0.5,
            count: 3,
        }];

        assert_eq!(
            classify_target(&live, &loose, 5.0, 5.0),
            Interactable::Chest
        );
        assert_eq!(
            classify_target(&live, &loose, -5.0, -5.0),
            Interactable::Furnace
        );
        assert_eq!(
            classify_target(&live, &loose, 0.5, 0.5),
            Interactable::LooseItem {
                item_name: "iron-plate".to_string(),
                count: 3,
            }
        );
        // Lejos de todo: no hay nada con lo que interactuar.
        assert_eq!(
            classify_target(&live, &loose, 100.0, 100.0),
            Interactable::Nothing
        );
    }

    #[test]
    fn test_lo_que_mas_cerca_manda() {
        // Un item suelto pegado a una caja: manda el item (esta mas cerca).
        let live = vec![LiveEntity {
            entity_type: ENTITY_CHEST,
            tile_x: 5.0,
            tile_y: 5.0,
            orientation: 0.0,
            status: 1,
            animation_progress: 0.0,
            health_ratio: 1.0,
            arm_angle: 0.0,
            sprite_id: 0,
            anim_frame: 0, model_id: 0 }];
        let loose = vec![LooseItem {
            item_name: "coal".to_string(),
            tile_x: 4.2,
            tile_y: 5.0,
            count: 1,
        }];
        assert_eq!(
            classify_target(&live, &loose, 4.2, 5.0),
            Interactable::LooseItem {
                item_name: "coal".to_string(),
                count: 1,
            }
        );
        // Y si se apunta a la caja, la caja.
        assert_eq!(
            classify_target(&live, &loose, 5.0, 5.0),
            Interactable::Chest
        );
    }
            }
        }
    }
}
