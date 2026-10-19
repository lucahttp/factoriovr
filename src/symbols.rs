// symbols.rs — Escáner y resolvedor de símbolos ELF/DWARF para Linux y Android ARM64
//
// A diferencia de Windows (que requiere factorio.pdb), los binarios oficiales
// de Factorio en Linux vienen sin despojar (not stripped), conteniendo todos
// los símbolos C++ en su tabla .symtab.

use anyhow::{Context, Result};
use object::{Object, ObjectSymbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type SymbolMap = HashMap<String, usize>;

/// Encuentra la ruta al ejecutable de Factorio en ejecución o en disco
pub fn find_game_executable() -> Result<PathBuf> {
    // 1. Si estamos inyectados directamente en un proceso cuyo ejecutable es factorio:
    if let Ok(target) = std::fs::read_link("/proc/self/exe") {
        if target.to_string_lossy().contains("factorio") {
            return Ok(target);
        }
    }

    // 2. Rutas conocidas en dispositivo (Pico 4 Ultra) y host de desarrollo:
    let candidates = [
        PathBuf::from("/sdcard/XFactorVR/factorio"),
        PathBuf::from("factorio_arm64/bin/arm64/factorio"),
        PathBuf::from("/Volumes/X10Pro/Projects/xfactorvr/factorio_arm64/bin/arm64/factorio"),
        PathBuf::from("factorio_linux/bin/x64/factorio"),
        PathBuf::from("/Volumes/X10Pro/SteamLibrary/steamapps/common/Factorio_Linux/bin/x64/factorio"),
        PathBuf::from("factorio/bin/x64/factorio"),
    ];

    for path in &candidates {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    anyhow::bail!("No se encontró el ejecutable de Factorio para escanear símbolos");
}

/// Escanea los símbolos ELF de un archivo binario
pub fn scan_elf_symbols<P: AsRef<Path>>(path: P) -> Result<SymbolMap> {
    let data = std::fs::read(path.as_ref())
        .with_context(|| format!("Error al leer binario ELF: {}", path.as_ref().display()))?;

    let file = object::File::parse(&*data)
        .with_context(|| format!("Formato binario no reconocido en {}", path.as_ref().display()))?;

    let mut symbols = SymbolMap::new();

    // Iterar símbolos regulares (.symtab) y dinámicos (.dynsym)
    for sym in file.symbols().chain(file.dynamic_symbols()) {
        if let Ok(name) = sym.name() {
            let addr = sym.address() as usize;
            if addr > 0 && !name.is_empty() {
                symbols.insert(name.to_string(), addr);
            }
        }
    }

    log::info!("Escaneados {} símbolos ELF desde {}", symbols.len(), path.as_ref().display());
    Ok(symbols)
}

/// Resuelve las direcciones de funciones necesarias según una lista de patrones
pub fn resolve_symbols(symbols: &SymbolMap, targets: &[&str]) -> SymbolMap {
    let mut resolved = SymbolMap::new();

    for &pattern in targets {
        if let Some((mangled, &addr)) = symbols.iter().find(|(name, _)| name.contains(pattern)) {
            resolved.insert(pattern.to_string(), addr);
            log::debug!("Símbolo resuelto: '{}' -> '{}' @ 0x{:X}", pattern, mangled, addr);
        } else {
            log::warn!("No se encontró el símbolo para el patrón: '{}'", pattern);
        }
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_real_binary() {
        if let Ok(exe_path) = find_game_executable() {
            let syms = scan_elf_symbols(&exe_path).expect("Debe escanear el binario ELF");
            assert!(syms.len() > 1000, "El binario no-strippeado debe tener miles de símbolos");

            let target_names = [
                "_ZN12GameRenderer6renderEv",
                "_ZNK17AssemblingMachine4drawER9DrawQueue",
                "_ZN16RenderParameters8centerOnERK3MapRK11SurfaceView",
            ];

            let resolved = resolve_symbols(&syms, &target_names);
            assert_eq!(resolved.len(), target_names.len(), "Todos los símbolos clave deben resolverse");
        }
    }
}
