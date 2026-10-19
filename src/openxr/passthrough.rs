// passthrough.rs — Control del modo Passthrough estereoscópico a color en Pico 4 Ultra
use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassthroughMode {
    /// Realidad Mixta: Las cámaras duales de 32 MP muestran tu habitación real y la fábrica se posa sobre tu mesa
    MixedReality,
    /// Realidad Virtual: Fondo virtual opaco (modo inmersivo tradicional)
    VirtualReality,
}

pub struct PassthroughController {
    pub current_mode: PassthroughMode,
    pub opacity: f32,
}

impl PassthroughController {
    pub fn new(initial_mode: PassthroughMode) -> Self {
        Self {
            current_mode: initial_mode,
            opacity: 1.0,
        }
    }

    /// Activa el modo Realidad Mixta en la Pico 4 Ultra
    pub fn enable_mixed_reality(&mut self) -> Result<()> {
        self.current_mode = PassthroughMode::MixedReality;
        log::info!("Passthrough MR activado para Pico 4 Ultra (modo Alpha Blend)");
        Ok(())
    }

    /// Conmuta al modo VR opaco
    pub fn enable_virtual_reality(&mut self) -> Result<()> {
        self.current_mode = PassthroughMode::VirtualReality;
        log::info!("Passthrough desactivado (modo VR inmersivo)");
        Ok(())
    }

    /// Retorna el blend mode adecuado para la capa de composición de OpenXR
    /// En Pico 4 Ultra:
    /// - XR_ENVIRONMENT_BLEND_MODE_ALPHA_BLEND (3) muestra las cámaras cuando el pixel tiene alfa 0.
    /// - XR_ENVIRONMENT_BLEND_MODE_OPAQUE (1) muestra fondo negro/opaco.
    pub fn openxr_blend_mode(&self) -> u32 {
        match self.current_mode {
            PassthroughMode::MixedReality => 3, // XR_ENVIRONMENT_BLEND_MODE_ALPHA_BLEND
            PassthroughMode::VirtualReality => 1, // XR_ENVIRONMENT_BLEND_MODE_OPAQUE
        }
    }

    /// Indica si los shaders de fondo deben escribir alfa transparente
    pub fn is_passthrough_active(&self) -> bool {
        self.current_mode == PassthroughMode::MixedReality
    }
}
