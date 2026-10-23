//! ui/mod.rs — El HUD de Factorio VR: tabla de controles única, ventanas
//! espaciales y estados de interacción.
//!
//! # De dónde sale la idea
//!
//! Del proyecto de referencia **Generals-Zero-Hour-XR** (Command & Conquer:
//! Zero Hour XR), concretamente:
//!
//! - `XrPanelLayout.h`: una sola tabla de controles define a la vez los
//!   píxeles, el hit-test y el estado. Aquí se copia el CONCEPTO: los píxeles
//!   y las zonas de pulsación salen de la MISMA tabla, así que no pueden
//!   desincronizarse.
//! - `XrSurface.h`: las superficies se agarran con una o dos manos, se mueven
//!   y se escalan. Aquí cada ventana del HUD es una superficie así.
//! - `XrTactics.h`: el gesto es diferido (armar -> soltar) y cada modo dice en
//!   texto qué va a pasar.
//! - `XrSceneUI.h`: la UI se organiza en páginas/sections con estado "armed"
//!   para no aceptar input por error.
//!
//! # Qué NO se copia
//!
//! El HUD de GZH-XR es un panel enorme de 18 botones con paginas. En Factorio
//! el juego ya tiene sus propios menus, asi que aqui el HUD da SOLO lo que la
//! VR aporta: elegir maquina, ver el inventario, fabricar, y saber que va a
//! pasar. Prioridad: CLARIDAD (pocas palabras grandes) sobre cantidad.
//!
//! # Cómo lo cablea el agente principal
//!
//! 1. Por frame: `hud.render_ventana(IdVentana::Construir)` te da la lista de
//!    primitivas; las pintas en un quad con la textura de la ventana.
//! 2. Cuando el rayo toca una ventana: `hud.hit_test(ventana, u, v)` te dice
//!    qué control hay debajo (misma tabla que se ha pintado).
//! 3. Cuando el jugador pulsa: `hud.pulsar(ventana, id)` -> `Option<Accion>`.
//!    Si devuelve `None`, el control estaba desactivado y no se hace nada.
//! 4. La pista: `hud.estados.pista` (o `hud.pista_legible()`) para el cartel
//!    grande del visor.
//!
//! El C++ NO decide geometría: solo pinta lo que sale de aquí.

pub mod estados;
pub mod items;
pub mod fuente;
pub mod tabla;
pub mod ventanas;

pub use estados::{EstadoGatillo, Estados, EventoGatillo, Modo};
pub use items::nombre_item;
pub use tabla::{
    ancho_texto, celdas_5x7, glifo, primitivas_de_controles, Accion, Celda, Color, Control,
    Estado, ItemResumen, Primitiva, Rect, Receta, Rol, Tabla,
};
pub use ventanas::{Agarre, Anclaje, IdVentana, Limites, Mano, Plano, Ventana, Ventanas};

use glam::Vec3;

/// El HUD completo: tabla de controles + ventanas + estados.
#[derive(Clone, Debug, Default)]
pub struct Hud {
    /// La tabla de controles única (los píxeles y el hit-test salen de aquí).
    pub tabla: Tabla,
    /// Las cuatro ventanas espaciales.
    pub ventanas: Ventanas,
    /// Los estados de interacción (modo, armado, hover, pista).
    pub estados: Estados,
}

impl Hud {
    /// Crea el HUD con la tabla y las ventanas por defecto.
    pub fn nuevo(inventario: &[ItemResumen], recetas: &[Receta]) -> Self {
        Self {
            tabla: Tabla::por_defecto(inventario, recetas),
            ventanas: Ventanas::por_defecto(),
            estados: Estados::new(),
        }
    }

    /// Construye un inventario-resumen a partir del mapa real del jugador
    /// (`game_bridge`): `(item_id, cantidad)`.
    pub fn inventario_desde_mapa(mapa: &[(u32, u32)]) -> Vec<ItemResumen> {
        mapa.iter()
            .map(|(item_id, cantidad)| ItemResumen {
                item_id: *item_id,
                nombre: nombre_item(*item_id).to_string(),
                cantidad: *cantidad,
            })
            .collect()
    }

    /// Hit-test: ¿qué control hay bajo el punto (u, v) de la ventana?
    ///
    /// `u` y `v` van de 0 a 1 (coordenadas normalizadas del quad de la
    /// ventana). Devuelve el id del control, o `None` si no hay nada.
    pub fn hit_test(&self, ventana: IdVentana, u: f32, v: f32) -> Option<u32> {
        let (w, h) = self.tamano_px(ventana);
        if w == 0 || h == 0 {
            return None;
        }
        self.tabla
            .hit_test(ventana, u * w as f32, v * h as f32)
            .map(|c| c.id)
    }

    /// Hit-test en píxeles de la ventana.
    pub fn hit_test_px(&self, ventana: IdVentana, x: f32, y: f32) -> Option<u32> {
        self.tabla.hit_test(ventana, x, y).map(|c| c.id)
    }

    /// Tamaño en píxeles de la ventana.
    pub fn tamano_px(&self, ventana: IdVentana) -> (u32, u32) {
        match self.ventanas.ventana(ventana) {
            Some(v) => (v.ancho_px, v.alto_px),
            None => (0, 0),
        }
    }

    /// Actualiza el hover: mueve el estado Hover en la tabla y en los estados.
    ///
    /// Devuelve el control sobre el que está el rayo, si lo hay.
    pub fn hover(&mut self, ventana: IdVentana, u: f32, v: f32) -> Option<u32> {
        let id = self.hit_test(ventana, u, v);
        self.tabla.limpiar_hover(ventana, id);
        if let Some(id) = id {
            self.tabla.set_estado(ventana, id, Estado::Hover);
        }
        self.estados.set_hover(id.map(|id| (ventana, id)));
        id
    }

    /// El jugador pulsa un control. Devuelve la acción a ejecutar, o `None`
    /// si el control no existe o está desactivado.
    pub fn pulsar(&mut self, ventana: IdVentana, id: u32) -> Option<Accion> {
        if !self.tabla.puede_pulsar(ventana, id) {
            return None;
        }
        let accion = self.tabla.control(ventana, id)?.accion;
        Some(accion)
    }

    /// Selecciona una máquina (o una receta) a partir de la acción del control.
    ///
    /// Devuelve el item_id seleccionado.
    pub fn aplicar_accion(&mut self, ventana: IdVentana, id: u32) -> Option<u32> {
        let accion = self.pulsar(ventana, id)?;
        match accion {
            Accion::Construir(item_id) => {
                self.estados.elegir_maquina(item_id, (ventana, id));
                self.sincronizar_estados();
                Some(item_id)
            }
            Accion::Fabricar(item_id) => {
                self.estados.elegir_receta(item_id, (ventana, id));
                self.sincronizar_estados();
                Some(item_id)
            }
            _ => None,
        }
    }

    /// Vuelca el estado abstracto (modo, selección, hover) en la tabla, para
    /// que el dibujo y el hit-test sean coherentes.
    pub fn sincronizar_estados(&mut self) {
        let hover = self.estados.hover_actual();
        let sel = self.estados.control_seleccionado;
        let ids: Vec<(IdVentana, u32)> = self
            .tabla
            .controles
            .iter()
            .map(|c| (c.ventana, c.id))
            .collect();
        for (v, id) in ids {
            let es_sel = sel == Some((v, id));
            let es_hover = hover == Some((v, id));
            if es_sel {
                self.tabla.set_estado(v, id, Estado::Seleccionado);
            } else if es_hover {
                self.tabla.set_estado(v, id, Estado::Hover);
            } else if !self
                .tabla
                .control(v, id)
                .map(|c| c.estado == Estado::Desactivado)
                .unwrap_or(false)
            {
                self.tabla.set_estado(v, id, Estado::Reposo);
            }
        }
        // La pista se escribe en el cartel de la ventana VISTA.
        self.tabla.set_etiqueta(
            IdVentana::Vista,
            50,
            self.estados.pista_legible().to_uppercase(),
        );
    }

    /// La pista de texto que se muestra al usuario en este momento.
    pub fn pista(&self) -> &str {
        &self.estados.pista
    }

    /// Lista de primitivas para pintar una ventana (marco + controles).
    pub fn render_ventana(&self, ventana: IdVentana) -> Vec<Primitiva> {
        let Some(v) = self.ventanas.ventana(ventana) else {
            return Vec::new();
        };
        let mut out = ventanas::primitivas_marco(v);
        let controles: Vec<&Control> = self.tabla.controles_de(ventana).collect();
        out.extend(tabla::primitivas_de_controles(&controles));
        out.extend(ventanas::primitivas_agarre(v, self.ventanas.agarre(ventana)));
        out
    }

    /// ¿Se puede agarrar esta ventana con las manos?
    pub fn puede_agarrar(&self, ventana: IdVentana) -> bool {
        self.ventanas
            .ventana(ventana)
            .map(|v| v.agarrable)
            .unwrap_or(false)
    }

    /// Agarra una ventana con una mano.
    pub fn agarrar_una_mano(&mut self, ventana: IdVentana, mano: Mano, posicion: Vec3) {
        self.ventanas.agarrar_una_mano(ventana, mano, posicion);
    }

    /// Agarra una ventana con dos manos (captura la pose base sin saltos).
    pub fn agarrar_dos_manos(&mut self, ventana: IdVentana, distancia: f32, centro: Vec3) {
        self.ventanas.agarrar_dos_manos(ventana, distancia, centro);
    }

    /// Actualiza el agarre con dos manos (escala con límites).
    pub fn actualizar_dos_manos(
        &mut self,
        ventana: IdVentana,
        distancia: f32,
        centro: Vec3,
    ) -> f32 {
        self.ventanas.actualizar_dos_manos(ventana, distancia, centro)
    }

    /// Suelta una ventana.
    pub fn soltar(&mut self, ventana: IdVentana) {
        self.ventanas.soltar(ventana);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn hud() -> Hud {
        let inv = Hud::inventario_desde_mapa(&[
            (items::ITEM_IRON_PLATE, 12),
            (items::ITEM_COPPER_PLATE, 4),
        ]);
        let recetas = vec![Receta {
            item_id: items::ITEM_IRON_GEAR,
            nombre: "ENGRANAJE".to_string(),
            coste: vec![(items::ITEM_IRON_PLATE, 2)],
            cantidad: 1,
            disponible: true,
        }];
        Hud::nuevo(&inv, &recetas)
    }

    #[test]
    fn test_hit_test_acierta_el_control_y_no_el_vecino() {
        let h = hud();
        // Botón "ENSAMBLADORA" (id 10) en (16,80,152,56): su centro.
        let c = h
            .tabla
            .control(IdVentana::Construir, 10)
            .expect("debe existir el botón");
        let x = c.rect.x as f32 + c.rect.w as f32 / 2.0;
        let y = c.rect.y as f32 + c.rect.h as f32 / 2.0;
        assert_eq!(h.hit_test_px(IdVentana::Construir, x, y), Some(10));

        // Un vecino NO puede ser el golpeado: el botón de al lado está a 8 px.
        let ancho = h.tamano_px(IdVentana::Construir).0 as f32;
        let alto = h.tamano_px(IdVentana::Construir).1 as f32;
        assert_eq!(
            h.hit_test(IdVentana::Construir, (x + 1.0) / ancho, y / alto),
            Some(10),
            "un píxel dentro del borde derecho sigue siendo el mismo control"
        );
        // Justo fuera del borde derecho: el vecino (id 11) o nada.
        let fuera = h.hit_test_px(IdVentana::Construir, c.rect.x as f32 + c.rect.w as f32, y);
        assert_ne!(fuera, Some(10), "fuera del control no puede ser el mismo control");
    }

    #[test]
    fn test_hit_test_no_acepta_un_control_inalcanzable_por_rayo() {
        let h = hud();
        // La cabecera de sección no es pulsable: el rayo pasa de largo.
        let seccion = h
            .tabla
            .control(IdVentana::Construir, 1)
            .expect("debe existir la sección");
        assert!(!seccion.rol.es_pulsable());
        let x = seccion.rect.x as f32 + 4.0;
        let y = seccion.rect.y as f32 + 4.0;
        assert_eq!(h.hit_test_px(IdVentana::Construir, x, y), None);
    }

    #[test]
    fn test_control_desactivado_no_recibe_pulsacion() {
        let mut h = hud();
        let receta = h
            .tabla
            .control(IdVentana::Fabricar, 10)
            .expect("debe existir la receta");
        assert_eq!(receta.estado, Estado::Reposo);

        // La marcamos como desactivada (como si faltaran materiales).
        h.tabla.set_estado(IdVentana::Fabricar, 10, Estado::Desactivado);
        assert!(
            h.pulsar(IdVentana::Fabricar, 10).is_none(),
            "un control desactivado no debe devolver acción"
        );
        // Y tampoco aparece bajo el rayo.
        let r = h.tabla.control(IdVentana::Fabricar, 10).unwrap().rect;
        let x = r.x as f32 + r.w as f32 / 2.0;
        let y = r.y as f32 + r.h as f32 / 2.0;
        assert_eq!(h.hit_test_px(IdVentana::Fabricar, x, y), None);

        // Con materiales sí.
        h.tabla.set_estado(IdVentana::Fabricar, 10, Estado::Reposo);
        assert!(h.pulsar(IdVentana::Fabricar, 10).is_some());
    }

    #[test]
    fn test_pista_del_modo_actual() {
        let mut h = hud();
        assert_eq!(h.pista(), Estados::pista_de(Modo::Normal));
        h.estados.set_modo(Modo::Construir);
        assert_eq!(h.pista(), "Modo construir: elige una maquina y colocala con el gatillo");
        h.estados.set_modo(Modo::Fabricar);
        assert_eq!(h.pista(), "Modo fabricar: elige una receta y pulsa para fabricar");
        h.estados.set_modo(Modo::Mover);
        assert!(h.pista().contains("agarra la ventana"));
        h.estados.set_modo(Modo::Rotar);
        assert!(h.pista().contains("gira la mano"));
    }

    #[test]
    fn test_elegir_maquina_entra_en_modo_construir_y_escribe_la_pista() {
        let mut h = hud();
        let item = items::ITEM_ASSEMBLING_MACHINE;
        let control = h
            .tabla
            .controles_de(IdVentana::Construir)
            .find(|c| c.accion == Accion::Construir(item))
            .map(|c| c.id)
            .expect("debe haber un botón de ensambladora");
        let elegido = h.aplicar_accion(IdVentana::Construir, control);
        assert_eq!(elegido, Some(item));
        assert_eq!(h.estados.modo, Modo::Construir);
        assert!(h.pista().contains("colocala con el gatillo"));
        // El botón queda marcado como seleccionado en la tabla.
        assert_eq!(
            h.tabla.control(IdVentana::Construir, control).unwrap().estado,
            Estado::Seleccionado
        );
    }

    #[test]
    fn test_gatillo_diferido_no_actua_sin_armed() {
        let mut h = hud();
        // Sin armar, el gatillo se rechaza.
        assert_eq!(h.estados.gatillo_presionado(true), EventoGatillo::Rechazado);
        // Armado: pulsar deja la acción pendiente; soltar la emite.
        h.estados.armar();
        assert_eq!(h.estados.gatillo_presionado(true), EventoGatillo::Armando);
        assert_eq!(h.estados.gatillo_presionado(true), EventoGatillo::Idle);
        assert_eq!(h.estados.gatillo_presionado(false), EventoGatillo::Soltado);
    }

    #[test]
    fn test_ventana_respeta_los_limites_al_agarrarla_con_dos_manos() {
        let mut h = hud();
        let v = IdVentana::Construir;
        let ancho_inicial = h.ventanas.ventana(v).unwrap().ancho_m;

        // Una mano: la ventana se mueve, no se escala.
        h.agarrar_una_mano(v, Mano::Derecha, Vec3::new(0.5, 1.2, -0.9));
        assert_eq!(h.ventanas.ventana(v).unwrap().ancho_m, ancho_inicial);

        // Pasa a dos manos: NO debe saltar el ancho en el instante de la captura.
        h.agarrar_dos_manos(v, 0.40, Vec3::new(0.0, 1.2, -0.9));
        assert_eq!(
            h.ventanas.ventana(v).unwrap().ancho_m,
            ancho_inicial,
            "al pasar a dos manos no puede haber salto de tamaño"
        );

        // Estirar mucho: se para en el máximo (1,5 m).
        let ancho = h.actualizar_dos_manos(v, 4.0, Vec3::new(0.0, 1.2, -0.9));
        assert!((ancho - 1.50).abs() < 1e-5, "ancho máximo 1,5 m, fue {}", ancho);
        assert_eq!(h.ventanas.ventana(v).unwrap().ancho_m, 1.50);

        // Juntarlas mucho: se para en el mínimo (0,25 m).
        let ancho = h.actualizar_dos_manos(v, 0.01, Vec3::new(0.0, 1.2, -0.9));
        assert!((ancho - 0.25).abs() < 1e-5, "ancho mínimo 0,25 m, fue {}", ancho);
        assert_eq!(h.ventanas.ventana(v).unwrap().ancho_m, 0.25);
    }

    #[test]
    fn test_las_ventanas_no_estan_apiladas_en_el_mismo_sitio() {
        let h = hud();
        let posiciones: Vec<Vec3> = IdVentana::TODAS
            .iter()
            .map(|v| h.ventanas.ventana(*v).unwrap().plano.posicion)
            .collect();
        for i in 0..posiciones.len() {
            for j in (i + 1)..posiciones.len() {
                assert!(
                    posiciones[i].distance(posiciones[j]) > 0.05,
                    "las ventanas {:?} y {:?} están en el mismo sitio",
                    i,
                    j
                );
            }
        }
    }

    #[test]
    fn test_el_titulo_de_cada_ventana_se_dibuja() {
        let h = hud();
        for v in IdVentana::TODAS {
            let prims = h.render_ventana(v);
            assert!(!prims.is_empty(), "la ventana {:?} debe tener primitivas", v);
            // Debe haber al menos un texto (el título).
            assert!(
                prims.iter().any(|p| matches!(p, Primitiva::Texto { .. })),
                "la ventana {:?} debe tener texto",
                v
            );
        }
    }

    #[test]
    fn test_fuente_5x7_cubre_las_letras_mayusculas_y_los_numeros() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            assert!(glifo(c).is_some(), "falta el glifo de {}", c);
        }
        // Las minúsculas con acento caen en la letra base.
        assert_eq!(glifo('á'), glifo('A'));
        assert_eq!(glifo('ó'), glifo('O'));
        // La Ñ tiene glifo propio.
        assert_ne!(glifo('ñ'), glifo('N'));
    }

    #[test]
    fn test_render_devuelve_primitivas_no_geometria_oculta() {
        let h = hud();
        let prims = h.render_ventana(IdVentana::Construir);
        // Cada rectángulo dibujado debe venir de un control de la tabla.
        let rects: Vec<(i32, i32, i32, i32)> = prims
            .iter()
            .filter_map(|p| match p {
                Primitiva::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        let de_tabla: Vec<(i32, i32, i32, i32)> = h
            .tabla
            .controles_de(IdVentana::Construir)
            .map(|c| (c.rect.x, c.rect.y, c.rect.w, c.rect.h))
            .collect();
        // El marco (fondo + barra de título) es lo único que no viene de la
        // tabla de controles; todo lo demás tiene que salir de ella.
        let marco: Vec<(i32, i32, i32, i32)> = ventanas::primitivas_marco(
            h.ventanas.ventana(IdVentana::Construir).unwrap(),
        )
        .iter()
        .filter_map(|p| match p {
            Primitiva::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        })
        .collect();
        for r in &rects {
            assert!(
                de_tabla.contains(r) || marco.contains(r),
                "rectángulo {:?} que no viene de la tabla ni del marco",
                r
            );
        }
    }
}
