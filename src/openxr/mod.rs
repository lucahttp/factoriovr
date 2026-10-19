// mod.rs — Capa de integración con PICO OpenXR SDK y estereoscopía 6DoF
pub mod actions;
pub mod passthrough;

use anyhow::Result;
use glam::{Mat4, Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Left = 0,
    Right = 1,
}

#[derive(Clone, Debug)]
pub struct EyePose {
    pub position: Vec3,
    pub orientation: Quat,
    /// Ángulos del campo de visión en radianes (left, right, up, down)
    pub fov: (f32, f32, f32, f32),
}

impl EyePose {
    /// Genera la matriz de proyección simétrica o asimétrica a partir del FOV de OpenXR
    pub fn projection_matrix(&self, near_z: f32, far_z: f32) -> Mat4 {
        let (angle_left, angle_right, angle_up, angle_down) = self.fov;
        let tan_left = angle_left.tan();
        let tan_right = angle_right.tan();
        let tan_down = angle_down.tan();
        let tan_up = angle_up.tan();

        let width = tan_right - tan_left;
        let height = tan_up - tan_down;

        let a = 2.0 / width;
        let b = 2.0 / height;
        let c = -(tan_right + tan_left) / width;
        let d = -(tan_up + tan_down) / height;
        let q = far_z / (near_z - far_z);
        let qn = (far_z * near_z) / (near_z - far_z);

        Mat4::from_cols_array(&[
            a,   0.0, 0.0, 0.0,
            0.0, b,   0.0, 0.0,
            c,   d,   q,  -1.0,
            0.0, 0.0, qn,  0.0,
        ])
    }

    /// Genera la matriz de vista (View Matrix) a partir de la pose 6DoF de la cabeza/ojo
    pub fn view_matrix(&self) -> Mat4 {
        let rot = Mat4::from_quat(self.orientation);
        let trans = Mat4::from_translation(self.position);
        (trans * rot).inverse()
    }

    /// Matriz combinada View-Projection
    pub fn view_projection_matrix(&self, near_z: f32, far_z: f32) -> Mat4 {
        self.projection_matrix(near_z, far_z) * self.view_matrix()
    }
}

pub struct OpenXrSession {
    pub passthrough: passthrough::PassthroughController,
    pub input: actions::InputManager,
    pub left_eye: EyePose,
    pub right_eye: EyePose,
}

impl OpenXrSession {
    pub fn new() -> Result<Self> {
        let passthrough = passthrough::PassthroughController::new(passthrough::PassthroughMode::MixedReality);
        let input = actions::InputManager::new();

        // Parámetros oculares iniciales típicos para Pico 4 Ultra (FOV ~105°, IPD 64mm)
        let half_ipd = 0.032; // 32 mm
        let fov_rad = 52.5f32.to_radians();

        let left_eye = EyePose {
            position: Vec3::new(-half_ipd, 0.0, 0.0),
            orientation: Quat::IDENTITY,
            fov: (-fov_rad, fov_rad, fov_rad, -fov_rad),
        };

        let right_eye = EyePose {
            position: Vec3::new(half_ipd, 0.0, 0.0),
            orientation: Quat::IDENTITY,
            fov: (-fov_rad, fov_rad, fov_rad, -fov_rad),
        };

        Ok(Self {
            passthrough,
            input,
            left_eye,
            right_eye,
        })
    }

    /// Actualiza la pose de los ojos desde el tracking 6DoF de Pico OS
    pub fn update_eye_poses(&mut self, left_pos: Vec3, left_rot: Quat, right_pos: Vec3, right_rot: Quat) {
        self.left_eye.position = left_pos;
        self.left_eye.orientation = left_rot;
        self.right_eye.position = right_pos;
        self.right_eye.orientation = right_rot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabletop::{TabletopConfig, TabletopManager};

    #[test]
    fn test_eye_projection_matrix() {
        let fov_rad = 50.0f32.to_radians();
        let eye = EyePose {
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
            fov: (-fov_rad, fov_rad, fov_rad, -fov_rad),
        };

        let proj = eye.projection_matrix(0.05, 100.0);
        assert!(proj.col(0).x > 0.0);
        assert!(proj.col(1).y > 0.0);
        assert_eq!(proj.col(2).w, -1.0);
    }

    #[test]
    fn test_controller_input_to_factorio_event() {
        let mut session = OpenXrSession::new().unwrap();
        let mut tabletop = TabletopManager::new(TabletopConfig::default());

        let mut right_controller = actions::ControllerState {
            ray_origin: Vec3::new(0.0, 0.0, -1.0),
            ray_direction: Vec3::new(0.0, -1.0, 0.0),
            trigger_clicked: true,
            ..Default::default()
        };
        let left_controller = actions::ControllerState::default();

        let events = session.input.process_controllers(
            &right_controller,
            &left_controller,
            &mut tabletop,
            0.016,
        );

        assert!(events.hovered_tile.is_some());
        assert!(events.left_mouse_down, "Debe registrarse el clic izquierdo al presionar el gatillo");

        // Soltar gatillo
        right_controller.trigger_clicked = false;
        let events2 = session.input.process_controllers(
            &right_controller,
            &left_controller,
            &mut tabletop,
            0.016,
        );
        assert!(events2.left_mouse_up, "Debe registrarse el levantamiento de botón");
    }
}

