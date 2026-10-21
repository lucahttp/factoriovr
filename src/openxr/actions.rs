// actions.rs — Mapeo de mandos Pico 4 Ultra 6DoF y gestos a eventos de Factorio
use glam::{Vec2, Vec3};
use crate::tabletop::{classify_target, Interactable, TabletopManager};

#[derive(Clone, Debug, Default)]
pub struct ControllerState {
    /// Origen del rayo en metros (posición del mando en la habitación)
    pub ray_origin: Vec3,
    /// Dirección hacia donde apunta el mando (vector unitario)
    pub ray_direction: Vec3,
    /// Gatillo índice (Trigger): Seleccionar, construir, disparar, minar
    pub trigger_clicked: bool,
    pub trigger_value: f32,
    /// Gatillo lateral (Grip): Rotar objeto, deconstruir, cancelar
    pub grip_clicked: bool,
    /// Joystick analógico para desplazar (pan) el mapa o fábrica
    pub thumbstick: Vec2,
    /// Botón Menú / B / Y
    pub menu_clicked: bool,
}

/// Qué mano del mando ha hecho el gesto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrHand {
    /// Gatillo índice: la "mano" que coge.
    Trigger,
    /// Gatillo lateral: la "mano" que mete.
    Grip,
}

/// Acción que las manos de VR mandan al juego real.
///
/// En VR no hay ratón: la única forma de coger un item del suelo, sacarlo de
/// una caja o meterlo en un horno es apuntar y apretar. Estas acciones son el
/// equivalente VR de arrastrar con el ratón.
#[derive(Clone, Debug, PartialEq)]
pub enum VrAction {
    /// Coger un item del suelo.
    PickUpItem { tile_x: f32, tile_y: f32 },
    /// Coger un item de una caja. `item_name` vacío = el primero que haya.
    TakeFromContainer {
        tile_x: f32,
        tile_y: f32,
        item_name: String,
        count: u32,
    },
    /// Meter en una máquina o en una caja el item que lleva el actor.
    InsertItem {
        tile_x: f32,
        tile_y: f32,
        item_name: String,
        count: u32,
        /// Índice del inventario: 1 = contenido de la caja, 2 = ingredientes
        /// del horno.
        inv_index: u32,
    },
}

/// Qué hace la mano según lo que hay bajo el rayo.
///
/// - Gatillo sobre un item del suelo -> cogerlo.
/// - Gatillo sobre una caja -> sacar el primer item que haya dentro.
/// - Gatillo sobre un horno -> meter el item que lleva el actor en el
///   inventario de ingredientes (el 2). Si el horno está vacío, el mod le
///   además mete combustible.
/// - Gripo sobre una caja -> meter dentro el item que lleva el actor (el 1).
///
/// Si no hay nada interactuable NO se hace nada: el gatillo sigue siendo el
/// clic de ratón de siempre (y el gripo el de rotar).
pub fn decide_vr_action(
    target: &Interactable,
    tile_x: f32,
    tile_y: f32,
    hand: VrHand,
) -> Option<VrAction> {
    match (hand, target) {
        (VrHand::Trigger, Interactable::LooseItem { .. }) => {
            Some(VrAction::PickUpItem { tile_x, tile_y })
        }
        (VrHand::Trigger, Interactable::Chest) => Some(VrAction::TakeFromContainer {
            tile_x,
            tile_y,
            item_name: String::new(),
            count: 1,
        }),
        (VrHand::Trigger, Interactable::Furnace) => Some(VrAction::InsertItem {
            tile_x,
            tile_y,
            item_name: String::new(),
            count: 1,
            inv_index: 2,
        }),
        (VrHand::Grip, Interactable::Chest) => Some(VrAction::InsertItem {
            tile_x,
            tile_y,
            item_name: String::new(),
            count: 1,
            inv_index: 1,
        }),
        // CASO RARO: en un horno el índice 1 es el COMBUSTIBLE, no el
        // contenido. Meter ahí una plancha de hierro no vale de nada, así que
        // el gripo también va al índice 2 (los ingredientes).
        (VrHand::Grip, Interactable::Furnace) => Some(VrAction::InsertItem {
            tile_x,
            tile_y,
            item_name: String::new(),
            count: 1,
            inv_index: 2,
        }),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
pub struct FactorioInputEvents {
    /// Posición del cursor en casillas de Factorio (si apunta a la mesa)
    pub hovered_tile: Option<(f32, f32)>,
    /// Clic izquierdo emulado
    pub left_mouse_down: bool,
    pub left_mouse_up: bool,
    /// Clic derecho emulado
    pub right_mouse_down: bool,
    pub right_mouse_up: bool,
    /// Vector de movimiento del mapa
    pub pan_delta_tiles: Vec2,
    /// Acciones de inventario que han hecho las manos (coger / meter items).
    pub vr_actions: Vec<VrAction>,
}

pub struct InputManager {
    #[allow(dead_code)]
    prev_left_trigger: bool,
    prev_right_trigger: bool,
    #[allow(dead_code)]
    prev_left_grip: bool,
    prev_right_grip: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            prev_left_trigger: false,
            prev_right_trigger: false,
            prev_left_grip: false,
            prev_right_grip: false,
        }
    }

    /// Qué hay bajo el rayo, mirando el estado real del mundo (las entidades
    /// vivas del diorama y los items sueltos que ha mandado el mod).
    fn target_bajo(&self, tile_x: f32, tile_y: f32) -> Interactable {
        let live = crate::get_live_entities_lock()
            .read()
            .map(|v| v.clone())
            .unwrap_or_default();
        let loose = crate::game_bridge::get_simulation_world()
            .read()
            .map(|w| w.loose_items.clone())
            .unwrap_or_default();
        classify_target(&live, &loose, tile_x, tile_y)
    }

    /// Procesa el estado de los mandos de la Pico 4 Ultra y genera eventos de Factorio
    pub fn process_controllers(
        &mut self,
        dominant_hand: &ControllerState,
        secondary_hand: &ControllerState,
        tabletop: &mut TabletopManager,
        delta_time: f32,
    ) -> FactorioInputEvents {
        let mut events = FactorioInputEvents::default();

        // 1. Raycast desde el mando dominante hacia la mesa física
        if let Some((tile_x, tile_y)) = tabletop.pick_tile_from_ray(
            dominant_hand.ray_origin,
            dominant_hand.ray_direction,
        ) {
            events.hovered_tile = Some((tile_x, tile_y));

            // --- Manos de VR: coger y meter items -----------------------------
            //
            // Solo hay gesto si el rayo apunta a algo INTERACTUABLE (un item
            // del suelo, una caja o un horno). Si no hay nada, el gatillo
            // sigue siendo el clic de raton de siempre: no se rompe nada.
            let trigger_press = dominant_hand.trigger_clicked && !self.prev_right_trigger;
            let grip_press = dominant_hand.grip_clicked && !self.prev_right_grip;

            // La pinza a dos manos (pinch-to-scale) manda sobre el gripo: si
            // los dos mandos estan agarrados, el gripo NO mete items.
            let pinza = dominant_hand.grip_clicked && secondary_hand.grip_clicked;

            let mut accion: Option<VrAction> = None;
            if trigger_press || (grip_press && !pinza) {
                let target = self.target_bajo(tile_x, tile_y);
                let mano = if trigger_press {
                    VrHand::Trigger
                } else {
                    VrHand::Grip
                };
                accion = decide_vr_action(&target, tile_x, tile_y, mano);
            }
            if let Some(a) = accion.clone() {
                events.vr_actions.push(a);
            }

            // Transición de gatillo Trigger (clic izquierdo)
            if trigger_press {
                // Si la mano ha cogido algo, no se emula el raton: seria
                // duplicar la accion.
                if accion.is_none() {
                    events.left_mouse_down = true;
                }
            } else if !dominant_hand.trigger_clicked && self.prev_right_trigger {
                events.left_mouse_up = true;
            }

            // Transición de gatillo Grip (clic derecho / rotación)
            if grip_press {
                if accion.is_none() {
                    events.right_mouse_down = true;
                }
            } else if !dominant_hand.grip_clicked && self.prev_right_grip {
                events.right_mouse_up = true;
            }
        }

        self.prev_right_trigger = dominant_hand.trigger_clicked;
        self.prev_right_grip = dominant_hand.grip_clicked;

        // 2. Desplazamiento del mapa con el joystick del mando secundario o dominante
        let pan_stick = if dominant_hand.thumbstick.length_squared() > 0.04 {
            dominant_hand.thumbstick
        } else if secondary_hand.thumbstick.length_squared() > 0.04 {
            secondary_hand.thumbstick
        } else {
            Vec2::ZERO
        };

        if pan_stick != Vec2::ZERO {
            // Velocidad: 20 casillas por segundo
            let pan_speed = 20.0 * delta_time;
            let pan_delta = Vec2::new(pan_stick.x * pan_speed, -pan_stick.y * pan_speed);
            tabletop.pan_factory(pan_delta);
            events.pan_delta_tiles = pan_delta;
        }

        // 3. Zoom / Escala a dos manos (Pinch-to-Scale si ambos grips están presionados)
        if dominant_hand.grip_clicked && secondary_hand.grip_clicked {
            let dist = (dominant_hand.ray_origin - secondary_hand.ray_origin).length();
            // Ajuste suave de escala
            if dist > 0.15 {
                let scale_delta = (dist - 0.30) * delta_time * 0.5;
                tabletop.adjust_scale(1.0 + scale_delta);
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabletop::TabletopConfig;

    #[test]
    fn test_gatillo_sobre_item_suelto_lo_coge() {
        let target = Interactable::LooseItem {
            item_name: "iron-plate".to_string(),
            count: 3,
        };
        let accion = decide_vr_action(&target, 4.0, 5.0, VrHand::Trigger);
        assert_eq!(accion, Some(VrAction::PickUpItem { tile_x: 4.0, tile_y: 5.0 }));
    }

    #[test]
    fn test_gatillo_sobre_caja_saca_el_primer_item() {
        let accion = decide_vr_action(&Interactable::Chest, 4.0, 5.0, VrHand::Trigger);
        assert_eq!(
            accion,
            Some(VrAction::TakeFromContainer {
                tile_x: 4.0,
                tile_y: 5.0,
                item_name: String::new(),
                count: 1,
            })
        );
    }

    #[test]
    fn test_gatillo_sobre_horno_mete_ingredientes() {
        // El indice 2 del horno son los ingredientes (el 1 es el combustible).
        let accion = decide_vr_action(&Interactable::Furnace, 4.0, 5.0, VrHand::Trigger);
        assert_eq!(
            accion,
            Some(VrAction::InsertItem {
                tile_x: 4.0,
                tile_y: 5.0,
                item_name: String::new(),
                count: 1,
                inv_index: 2,
            })
        );
    }

    #[test]
    fn test_gripo_sobre_caja_mete_dentro() {
        // El indice 1 de una caja es su contenido.
        let accion = decide_vr_action(&Interactable::Chest, 4.0, 5.0, VrHand::Grip);
        assert_eq!(
            accion,
            Some(VrAction::InsertItem {
                tile_x: 4.0,
                tile_y: 5.0,
                item_name: String::new(),
                count: 1,
                inv_index: 1,
            })
        );
    }

    #[test]
    fn test_gripo_sobre_horno_no_mete_en_el_combustible() {
        // Caso raro: en un horno el 1 es el combustible, asi que el gripo
        // tambien va al 2 (los ingredientes).
        let accion = decide_vr_action(&Interactable::Furnace, 4.0, 5.0, VrHand::Grip);
        match accion {
            Some(VrAction::InsertItem { inv_index, .. }) => assert_eq!(inv_index, 2),
            otra => panic!("se esperaba un InsertItem, se obtuvo {:?}", otra),
        }
    }

    #[test]
    fn test_sin_nada_interactuable_no_se_hace_nada() {
        assert_eq!(
            decide_vr_action(&Interactable::Nothing, 4.0, 5.0, VrHand::Trigger),
            None
        );
        assert_eq!(
            decide_vr_action(&Interactable::Nothing, 4.0, 5.0, VrHand::Grip),
            None
        );
        // El gripo no coge items del suelo: el gripo es "meter".
        assert_eq!(
            decide_vr_action(
                &Interactable::LooseItem {
                    item_name: "coal".to_string(),
                    count: 1,
                },
                4.0,
                5.0,
                VrHand::Grip
            ),
            None
        );
    }

    /// El mapeo de mandos a eventos de Factorio: el gatillo es el raton
    /// izquierdo y el gripo el derecho.  Este test vivia en openxr/mod.rs
    /// (que se borro: la sesion OpenXR de Rust no la usaba nadie).
    #[test]
    fn test_controller_input_to_factorio_event() {
        let mut input = InputManager::new();
        let mut tabletop = TabletopManager::new(TabletopConfig::default());

        let mut right_controller = ControllerState {
            ray_origin: Vec3::new(0.0, 0.0, -1.0),
            ray_direction: Vec3::new(0.0, -1.0, 0.0),
            trigger_clicked: true,
            ..Default::default()
        };
        let left_controller = ControllerState::default();

        let events = input.process_controllers(
            &right_controller,
            &left_controller,
            &mut tabletop,
            0.016,
        );

        assert!(events.hovered_tile.is_some());
        assert!(events.left_mouse_down, "Debe registrarse el clic izquierdo al presionar el gatillo");

        // Soltar gatillo
        right_controller.trigger_clicked = false;
        let events2 = input.process_controllers(
            &right_controller,
            &left_controller,
            &mut tabletop,
            0.016,
        );
        assert!(events2.left_mouse_up, "Debe registrarse el levantamiento de boton");
    }
}
