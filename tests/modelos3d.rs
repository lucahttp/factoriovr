//! Tests de `src/modelos3d.rs`.
//!
//! El modulo ya lo declara `src/lib.rs`, asi que aqui se usa el de la
//! libreria (y no una copia con `#[path]`, que duplicaria el codigo y
//! dejaria el cache de modelos en dos sitios distintos).

use xfactorvr::modelos3d::*;

/// Los tests de cache tocan el cache global: se serializan.
static CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn el_manifiesto_tiene_las_maquinas_del_juego() {
    let m = manifiesto().expect("el manifiesto de modelos deberia estar embebido");
    assert!(m.version >= 1);
    assert!(m.escala_tile_m > 0.0);
    for proto in [
        "assembling-machine-1",
        "assembling-machine-2",
        "assembling-machine-3",
        "electric-mining-drill",
        "stone-furnace",
        "steel-furnace",
        "electric-furnace",
        "wooden-chest",
        "iron-chest",
        "steel-chest",
        "transport-belt",
        "inserter",
        "small-electric-pole",
        "medium-electric-pole",
        "gun-turret",
        "radar",
        "roboport",
        "lab",
        "boiler",
        "storage-tank",
    ] {
        let modelo = modelo_para_prototipo(proto)
            .unwrap_or_else(|| panic!("{proto} deberia tener modelo 3D"));
        assert!(
            !modelo.partes.is_empty(),
            "{proto} tiene modelo pero sin partes"
        );
        assert!(modelo.huella_tiles > 0.0, "{proto} sin huella");
    }
}

#[test]
fn el_logistic_container_usa_modelo_de_cofre() {
    // el snapshot tambien trae "logistic-container": no es un cofre de
    // madera, pero tiene que caer en ALGUN modelo (el gris de las cajas
    // logisticas), no en None.
    let modelo = modelo_para_prototipo("logistic-container").expect("logistic-container");
    assert_eq!(modelo.huella_tiles, 1.0);
}

#[test]
fn la_tabla_de_tipos_cuadra_con_el_render() {
    use xfactorvr::tabletop::*;
    assert_eq!(
        prototipo_para_tipo(ENTITY_ASSEMBLING_MACHINE),
        Some("assembling-machine-1")
    );
    assert_eq!(
        prototipo_para_tipo(ENTITY_MINING_DRILL),
        Some("electric-mining-drill")
    );
    assert_eq!(prototipo_para_tipo(ENTITY_FURNACE), Some("stone-furnace"));
    assert_eq!(prototipo_para_tipo(ENTITY_GUN_TURRET), Some("gun-turret"));
    assert_eq!(prototipo_para_tipo(ENTITY_RADAR), Some("radar"));
    assert_eq!(prototipo_para_tipo(ENTITY_ROBOPORT), Some("roboport"));
    assert_eq!(prototipo_para_tipo(ENTITY_CHEST), Some("wooden-chest"));
    assert_eq!(
        prototipo_para_tipo(ENTITY_ELECTRIC_POLE),
        Some("small-electric-pole")
    );
    assert_eq!(
        prototipo_para_tipo(ENTITY_TRANSPORT_BELT),
        Some("transport-belt")
    );
    assert_eq!(prototipo_para_tipo(ENTITY_INSERTER), Some("inserter"));
}

#[test]
fn un_tipo_desconocido_no_devuelve_geomtria() {
    // tipo que no existe en la tabla
    assert_eq!(prototipo_para_tipo(9999), None);
    assert!(geometria_tipo(9999, 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO).is_none());
    // prototipo que no existe (los items sueltos, "item-on-ground", etc.)
    assert!(modelo_para_prototipo("item-on-ground").is_none());
    assert!(geometria_prototipo("item-on-ground", 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO).is_none());
    // tampoco las entidades sin modelo 3D (biters, arboles, jugador)
    assert!(geometria_prototipo("small-biter", 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO).is_none());
}

#[test]
fn la_geometria_sale_en_su_sitio_y_en_el_suelo() {
    let escala = ESCALA_TILE_M_POR_DEFECTO;
    let verts = geometria_prototipo("assembling-machine-1", 2.0, -3.0, 0.0, 0, escala)
        .expect("la ensambladora deberia tener geometria");
    assert!(verts.len() > 100, "solo {} vertices", verts.len());
    // apoyada en el suelo
    let min_y = verts.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!(min_y.abs() < 1e-3, "min_y = {min_y}");
    // el ancho en x cabe en su huella (3 casillas * 4 cm)
    let ancho = verts.iter().map(|v| v.x).fold(f32::MIN, f32::max)
        - verts.iter().map(|v| v.x).fold(f32::MAX, f32::min);
    assert!(ancho <= 3.0 * escala + 1e-3, "ancho = {ancho}");
    // y centrada en su casilla (2, -3) -> x = 8 cm, z = -12 cm
    // (se mira el CENTRO DE LA CAJA, no la media de los vertices: la malla
    // no tiene los vertices repartidos por igual)
    let cx = (verts.iter().map(|v| v.x).fold(f32::MAX, f32::min)
        + verts.iter().map(|v| v.x).fold(f32::MIN, f32::max))
        * 0.5;
    let cz = (verts.iter().map(|v| v.z).fold(f32::MAX, f32::min)
        + verts.iter().map(|v| v.z).fold(f32::MIN, f32::max))
        * 0.5;
    assert!((cx - 2.0 * escala).abs() < 1e-3, "centro x = {cx}");
    assert!((cz + 3.0 * escala).abs() < 1e-3, "centro z = {cz}");
}

#[test]
fn el_cache_no_vuelve_a_leer_el_fichero() {
    let _lock = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    limpiar_cache();
    let antes = cargas_reales();
    // primera llamada: lee el GLB
    let a = geometria_prototipo("wooden-chest", 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO)
        .expect("cofre");
    let tras_primera = cargas_reales();
    assert!(a.len() > 10);
    assert!(
        tras_primera > antes,
        "la primera llamada deberia leer el GLB ({antes} -> {tras_primera})"
    );
    // segunda llamada: NADA de I/O
    let b = geometria_prototipo("wooden-chest", 1.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO)
        .expect("cofre");
    assert_eq!(cargas_reales(), tras_primera, "el cache no debe releer el fichero");
    assert_eq!(a.len(), b.len());
    // y el resultado es el mismo shifted
    let dx = b[0].x - a[0].x;
    assert!((dx - ESCALA_TILE_M_POR_DEFECTO).abs() < 1e-4, "dx = {dx}");
    // limpia el cache: la siguiente llamada vuelve a leer
    limpiar_cache();
    assert_eq!(modelos_en_cache(), 0);
    let _ = geometria_prototipo("wooden-chest", 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO);
    assert_eq!(cargas_reales(), tras_primera + 1);
}

#[test]
fn el_item_suelto_se_queda_sin_modelo() {
    // los items del suelo no tienen modelo 3D: se dibujan con el sprite
    assert!(geometria_tipo(xfactorvr::tabletop::ENTITY_BITER, 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO).is_none());
    assert!(geometria_tipo(xfactorvr::tabletop::ENTITY_TREE, 0.0, 0.0, 0.0, 0, ESCALA_TILE_M_POR_DEFECTO).is_none());
}
