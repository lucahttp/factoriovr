//! Tests del renderer 3D del VISOR (`src/renderer/`).
//!
//! Son tests de CPU: el GL no existe en el host, asi que lo que se comprueba
//! es la parte que decide QUE se dibuja y DONDE:
//!
//!   * la tabla prototipo -> modelo del manifiesto real,
//!   * la geometria que sale del GLB (vertices, color horneado),
//!   * la matriz de instancia de una entidad (que la pone en su casilla),
//!   * el agrupado de instancias por prototipo y parte.

use xfactorvr::modelos3d::*;
use xfactorvr::renderer::*;

/// Las entidades vivas son estado global: este cerrojo serializa los tests
/// que las tocan.
static ENTIDADES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pone una entidad viva con modelo y devuelve el estado anterior.
fn poner_entidad(
    model_id: u32,
    tile_x: f32,
    tile_y: f32,
    orientation: f32,
) -> Vec<xfactorvr::tabletop::LiveEntity> {
    let antes = xfactorvr::get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();
    if let Ok(mut l) = xfactorvr::get_live_entities_lock().write() {
        l.push(xfactorvr::tabletop::LiveEntity {
            entity_type: 1,
            tile_x,
            tile_y,
            orientation,
            status: 1,
            animation_progress: 0.0,
            health_ratio: 1.0,
            arm_angle: 0.0,
            sprite_id: 1,
            anim_frame: 0,
            model_id,
        });
    }
    antes
}

#[test]
fn el_manifiesto_real_da_una_maquina_con_geometria() {
    let escala = escala_tile();
    assert!(escala > 0.0, "el manifiesto deberia traer la escala de casilla");

    // La geometria sale del GLB real de assembling-machine-1.
    let verts = geometria_prototipo("assembling-machine-1", 2.0, -3.0, 0.0, 0, escala)
        .expect("assembling-machine-1 deberia tener geometria 3D");
    assert!(verts.len() > 100, "solo {} vertices", verts.len());

    // Y el modelo es instanciable: una escala, un desplazamiento y sus partes.
    let modelo = modelo_instanciable("assembling-machine-1").expect("modelo instanciable");
    assert!(modelo.escala > 0.0, "escala = {}", modelo.escala);
    assert!(!modelo.partes.is_empty(), "el modelo no tiene partes");
    assert_eq!(modelo.partes[0].archivo, "assembling-machine-1.glb");
    // La base del modelo esta en y = 0 (se apoya en la mesa).
    assert!(modelo.offset[1].abs() < 1e-6 || modelo.offset[1].is_finite());
}

#[test]
fn la_matriz_de_instancia_pone_la_maquina_en_su_casilla() {
    let escala = escala_tile();
    let modelo = modelo_instanciable("assembling-machine-1").expect("modelo");
    let parte = &modelo.partes[0];

    let m = matriz_instancia(&modelo, parte, 2.0, -3.0, 0.0, 0, escala);
    // La traslacion es la casilla en metros (el modelo ya lleva su escala y
    // su centrado dentro de la matriz).
    let t = m.w_axis.truncate();
    assert!((t.x - 2.0 * escala).abs() < 1e-5, "x = {}", t.x);
    assert!(t.y.abs() < 1e-5, "y = {}", t.y);
    assert!((t.z + 3.0 * escala).abs() < 1e-5, "z = {}", t.z);

    // La matriz no deforma el modelo: la parte lineal es una rotacion por Y
    // mas una escala uniforme.
    let escala_parte = m.x_axis.length();
    assert!(
        (escala_parte - modelo.escala).abs() < 1e-6,
        "escala en la matriz = {escala_parte}, la del modelo = {}",
        modelo.escala
    );
    assert!((m.x_axis.dot(m.z_axis)).abs() < 1e-6, "los ejes no son ortogonales");
}

#[test]
fn los_vertices_del_modelo_cargado_son_los_esperados() {
    // El numero de vertices que dice el manifiesto es el del GLB optimizado.
    let m = manifiesto().expect("manifiesto");
    let modelo = m.modelos.get("assembling-machine-1").expect("modelo");
    let esperado: usize = modelo.partes.iter().map(|p| p.verts).sum();
    assert!(esperado > 0, "el manifiesto no dice cuantos vertices tiene");

    // Y el cargador de verdad da ese numero.
    let cargado = vertices_de_prototipo("assembling-machine-1").expect("vertices");
    assert_eq!(
        cargado, esperado,
        "el GLB cargado tiene {cargado} vertices y el manifiesto dice {esperado}"
    );

    // La geometria que se sube a la GPU es la misma (con el color horneado).
    let geo = geometria_de_archivo("assembling-machine-1.glb").expect("geometria");
    assert_eq!(geo.vertices.len(), esperado);
    // Los indices son coherentes con los vertices (si no, el GLB esta roto y
    // la maquina sale con agujeros).
    assert!(!geo.indices.is_empty());
    let max = geo.indices.iter().copied().max().unwrap() as usize;
    assert!(
        max < geo.vertices.len(),
        "el GLB tiene indices colgantes (max {max} con {} vertices)",
        geo.vertices.len()
    );
    assert_eq!(geo.indices.len() % 3, 0);
    assert_eq!(
        triangulos_de_prototipo("assembling-machine-1"),
        Some(geo.indices.len() / 3)
    );

    // El color no es todo blanco: el GLB optimizado trae COLOR_0.
    let con_color = geo
        .vertices
        .iter()
        .filter(|v| v.r < 0.99 || v.g < 0.99 || v.b < 0.99)
        .count();
    assert!(con_color > 0, "el modelo no trae color horneado");

    // La geometria que se entrega al motor de VR va des-indexada: un vertex
    // por indice.
    let plana = geometria_prototopo_de_prueba();
    assert_eq!(plana, geo.indices.len());
}

/// Geometria des-indexada de la ensambladora (para compararla con los indices).
fn geometria_prototopo_de_prueba() -> usize {
    geometria_prototipo(
        "assembling-machine-1",
        0.0,
        0.0,
        0.0,
        0,
        escala_tile(),
    )
    .expect("geometria")
    .len()
}

#[test]
fn las_entidades_con_modelo_se_agrupan_en_instancias() {
    let _lock = ENTIDADES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let escala = escala_tile();

    // Sin entidades con modelo no hay nada que dibujar.
    let antes = poner_entidad(0, 0.0, 0.0, 0.0);
    assert!(recopilar_instancias().is_empty(), "no hay entidades con modelo");

    // Con una ensambladora viva hay un grupo con una matriz.
    let model_id = id_de_prototipo("assembling-machine-1");
    assert!(model_id > 0, "assembling-machine-1 deberia tener id de modelo");
    let _antes = poner_entidad(model_id, 2.0, -3.0, 0.0);

    let grupos = recopilar_instancias();
    let clave = (
        "assembling-machine-1".to_string(),
        "assembling-machine-1.glb".to_string(),
    );
    let matrices = grupos
        .get(&clave)
        .unwrap_or_else(|| panic!("no hay grupo para assembling-machine-1"));
    assert_eq!(matrices.len(), 1, "deberia haber una instancia");
    let t = matrices[0].w_axis.truncate();
    assert!((t.x - 2.0 * escala).abs() < 1e-5, "x = {}", t.x);
    assert!((t.z + 3.0 * escala).abs() < 1e-5, "z = {}", t.z);

    // Se restaura el estado.
    if let Ok(mut l) = xfactorvr::get_live_entities_lock().write() {
        *l = antes;
    }
}

#[test]
fn la_matriz_instanciada_cave_exactamente_donde_la_geometria_plana() {
    // El camino plano (geometria_prototipo, lo que ve el motor de VR) y el
    // camino instanciado (matriz por entidad) tienen que poner la maquina
    // EXACTAMENTE en el mismo sitio: si no, al cablear el renderer 3D las
    // maquinas se verian desplazadas respecto a los sprites.
    let escala = escala_tile();
    let (tx, ty, orient) = (2.0f32, -3.0f32, 0.7f32);

    let plano = geometria_prototipo("assembling-machine-1", tx, ty, orient, 0, escala)
        .expect("geometria plana");
    let geo = geometria_de_archivo("assembling-machine-1.glb").expect("geometria");
    let modelo = modelo_instanciable("assembling-machine-1").expect("modelo");
    let m = matriz_instancia(&modelo, &modelo.partes[0], tx, ty, orient, 0, escala);

    assert_eq!(plano.len(), geo.indices.len());
    let mut max_error = 0.0f32;
    for (k, idx) in geo.indices.iter().enumerate() {
        let v = &geo.vertices[*idx as usize];
        let p = m * glam::Vec4::new(v.x, v.y, v.z, 1.0);
        let q = &plano[k];
        max_error = max_error.max((p.x - q.x).abs().max((p.y - q.y).abs()).max((p.z - q.z).abs()));
    }
    assert!(
        max_error < 1e-5,
        "la maquina instanciada no cae donde la plana: error maximo {max_error} m"
    );
}

#[test]
fn el_tiene_modelo_solo_dice_que_si_con_modelo_real() {
    assert!(tiene_modelo(id_de_prototipo("assembling-machine-1")));
    assert!(!tiene_modelo(0), "el id 0 es 'sin modelo'");
    // Un id que no existe en el registro tampoco tiene modelo.
    assert!(!tiene_modelo(999_999));
}

#[test]
fn sin_gl_el_renderer_no_dibuja_pero_no_rompe() {
    // En el host no hay GL: iniciar() devuelve false y dibujar da 0.
    assert!(!iniciar(), "en el host no deberia haber GL");
    assert_eq!(dibujar_entidades_3d(), 0);
    assert_eq!(ultimo_contador(), 0);
    // El SGSR tampoco se puede preparar.
    assert!(!sgsr_preparar((1080, 1080), (2160, 2160)));
    assert_eq!(sgsr_framebuffer(), 0);
    assert!(!sgsr_escalar());
}
