// actions.rs — Mapeo de mandos Pico 4 Ultra 6DoF y gestos a eventos de Factorio
use glam::{Vec2, Vec3};
use crate::tabletop::TabletopManager;

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

            // Transición de gatillo Trigger (clic izquierdo)
            if dominant_hand.trigger_clicked && !self.prev_right_trigger {
                events.left_mouse_down = true;
            } else if !dominant_hand.trigger_clicked && self.prev_right_trigger {
                events.left_mouse_up = true;
            }

            // Transición de gatillo Grip (clic derecho / rotación)
            if dominant_hand.grip_clicked && !self.prev_right_grip {
                events.right_mouse_down = true;
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
