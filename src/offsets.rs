// offsets.rs — Símbolos C++ y offsets para Factorio en Linux y Android ARM64
//
// Mapea los nombres de símbolos de la ABI Itanium (GCC/Clang en Linux/Android)
// que equivalen a los símbolos PDB de Windows en factorio-3d-mod.

#[derive(Clone, Copy, Debug)]
pub struct SymbolDef {
    /// Nombre de referencia legible
    pub name: &'static str,
    /// Fragmento del símbolo mangled de GCC/Clang o nombre C++ desmangleado
    pub symbol_pattern: &'static str,
    /// Offset RVA de respaldo por si el símbolo fuese renombrado
    pub fallback_rva: usize,
}

pub const RECT_FP: f32 = 256.0;

// --- Render & Frame Pipeline ---
pub const GAME_RENDERER_RENDER: SymbolDef = SymbolDef {
    name: "GameRenderer::render",
    symbol_pattern: "_ZN12GameRenderer6renderEv",
    fallback_rva: 0x01596320,
};

pub const GAME_RENDERER_RENDER_INTERNAL: SymbolDef = SymbolDef {
    name: "GameRenderer::renderInternal",
    symbol_pattern: "_ZN12GameRenderer14renderInternalEb",
    fallback_rva: 0x01599b30,
};

pub const RP_CENTER_ON: SymbolDef = SymbolDef {
    name: "RenderParameters::centerOn",
    symbol_pattern: "_ZN16RenderParameters8centerOnERK3MapRK11SurfaceView",
    fallback_rva: 0x01587730,
};

pub const GL_SWAP_BUFFERS: SymbolDef = SymbolDef {
    name: "GraphicsInterfaceOpenGL::swapBuffers",
    symbol_pattern: "_ZN23GraphicsInterfaceOpenGL11swapBuffersEv",
    fallback_rva: 0x03316520,
};

// --- Input & Map Picking ---
pub const COMPUTE_WALK_DIR: SymbolDef = SymbolDef {
    name: "PlayerInputSource::computeWalkingDirectionFromPlayerInputs",
    symbol_pattern: "_ZN17PlayerInputSource39computeWalkingDirectionFromPlayerInputsEv",
    fallback_rva: 0x01c6b1f0,
};

pub const GET_MAP_POSITION: SymbolDef = SymbolDef {
    name: "GameView::getMapPosition",
    symbol_pattern: "_ZNK8GameView14getMapPositionE13PixelPosition",
    fallback_rva: 0x01580a00,
};

// --- Entity Drawing & Replacement ---
pub const ASSEMBLING_MACHINE_DRAW: SymbolDef = SymbolDef {
    name: "AssemblingMachine::draw",
    symbol_pattern: "_ZNK17AssemblingMachine4drawER9DrawQueue",
    fallback_rva: 0x02296ab0,
};

pub const MINING_DRILL_DRAW: SymbolDef = SymbolDef {
    name: "MiningDrill::draw",
    symbol_pattern: "_ZNK11MiningDrill4drawER9DrawQueue",
    fallback_rva: 0x01c2fc40,
};

pub const TURRET_DRAW: SymbolDef = SymbolDef {
    name: "Turret::draw",
    symbol_pattern: "_ZNK6Turret4drawER9DrawQueue",
    fallback_rva: 0x02d59f30,
};

pub const AMMO_TURRET_DRAW: SymbolDef = SymbolDef {
    name: "AmmoTurret::draw",
    symbol_pattern: "_ZNK10AmmoTurret4drawER9DrawQueue",
    fallback_rva: 0x02ff2c20,
};

pub const FURNACE_DRAW: SymbolDef = SymbolDef {
    name: "Furnace::draw",
    symbol_pattern: "_ZNK7Furnace4drawER9DrawQueue",
    fallback_rva: 0x02f42d50,
};

pub const ENTITY_DTOR: SymbolDef = SymbolDef {
    name: "Entity::~Entity",
    symbol_pattern: "_ZN6EntityD2Ev",
    fallback_rva: 0x01540c00,
};

pub const CRAFTING_MACHINE_DRAW: SymbolDef = SymbolDef {
    name: "CraftingMachine::draw",
    symbol_pattern: "CraftingMachine::draw",
    fallback_rva: 0x02297810,
};

/// Lista completa de funciones a interceptar al iniciar el mod
pub const HOOK_TARGETS: &[SymbolDef] = &[
    GAME_RENDERER_RENDER,
    RP_CENTER_ON,
    GL_SWAP_BUFFERS,
    COMPUTE_WALK_DIR,
    GET_MAP_POSITION,
    ASSEMBLING_MACHINE_DRAW,
    MINING_DRILL_DRAW,
    TURRET_DRAW,
    AMMO_TURRET_DRAW,
    FURNACE_DRAW,
    ENTITY_DTOR,
];
