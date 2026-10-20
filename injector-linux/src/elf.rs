//! Lectura minima del ELF del motor: cabecera, secciones, tabla de simbolos
//! y segmentos de carga.
//!
//! Solo se lee lo justo: la cabecera ELF, la tabla de cabeceras de seccion,
//! la `.symtab` y su `.strtab`. El binario del motor pesa 260 MB, asi que
//! cargarlo entero en memoria es tonteria; aqui se busca y se leen solo los
//! rangos que interesan.
//!
//! Formato: ELF64 little-endian (ARM64). El motor de Factorio es un PIE
//! (ET_DYN) con el primer PT_LOAD en vaddr 0, asi que la direccion en
//! memoria de un simbolo es `load_bias + st_value`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

const SHT_SYMTAB: u32 = 2;
const SHT_DYNSYM: u32 = 11;

const STT_FUNC: u8 = 2;
/// Tipo (`st_info & 0xf`): 1 = STT_OBJECT (datos, p.ej. tablas virtuales).
const STT_OBJECT: u8 = 1;
const SHN_UNDEF: u16 = 0;

const PT_LOAD: u32 = 1;

/// Simbolo de funcion del ELF (tal cual lo define el formato).
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Nombre mangled (`_ZN12GameRenderer6renderEv`).
    pub name: String,
    /// Direccion virtual dentro del ELF (`st_value`).
    pub value: u64,
    /// Tamano en bytes (`st_size`).
    pub size: u64,
    /// Tipo (`st_info & 0xf`): 2 = STT_FUNC.
    pub kind: u8,
}

/// Imagen ELF del ejecutable del juego.
pub struct ElfImage {
    /// Ruta del fichero leido.
    pub path: PathBuf,
    /// Direccion virtual del primer segmento PT_LOAD (0 en el motor).
    pub first_load_vaddr: u64,
    /// Simbolos de funcion definidos (no indefinidos).
    pub symbols: Vec<Symbol>,
}

impl ElfImage {
    /// Parsea el ELF de `path`. Solo se guarda lo imprescindible.
    pub fn parse(path: &Path) -> io::Result<ElfImage> {
        let mut f = File::open(path)?;

        // --- cabecera ELF completa (64 bytes en ELF64) ---
        let mut hdr = [0u8; 64];
        f.read_exact(&mut hdr)?;
        let ident = &hdr[..16];
        if ident[..4] != ELF_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "no es un ELF"));
        }
        if ident[4] != ELFCLASS64 || ident[5] != ELFDATA2LSB {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "solo ELF64 little-endian (el motor ARM64 lo es)",
            ));
        }

        // Offsets dentro de Elf64_Ehdr:
        //   e_shoff 0x28, e_shentsize 0x3a, e_shnum 0x3c, e_shstrndx 0x3e
        let e_shoff = u64::from_le_bytes(hdr[0x28..0x30].try_into().unwrap_or([0; 8]));
        let e_shentsize = u16::from_le_bytes(hdr[0x3a..0x3c].try_into().unwrap_or([0; 2])) as usize;
        let e_shnum = u16::from_le_bytes(hdr[0x3c..0x3e].try_into().unwrap_or([0; 2])) as usize;
        let e_shstrndx = u16::from_le_bytes(hdr[0x3e..0x40].try_into().unwrap_or([0; 2])) as usize;

        if e_shoff == 0 || e_shnum == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ELF sin tabla de secciones",
            ));
        }

        // --- tabla de cabeceras de seccion ---
        let mut shdrs = vec![0u8; e_shnum * e_shentsize];
        f.seek(SeekFrom::Start(e_shoff))?;
        f.read_exact(&mut shdrs)?;

        struct Sh {
            kind: u32,
            offset: u64,
            size: u64,
            link: u32,
            entsize: u64,
        }
        let mut secs: Vec<Sh> = Vec::with_capacity(e_shnum);
        for i in 0..e_shnum {
            let s = &shdrs[i * e_shentsize..(i + 1) * e_shentsize];
            if s.len() < 64 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "shdr corto"));
            }
            secs.push(Sh {
                kind: u32::from_le_bytes(s[4..8].try_into().unwrap_or([0; 4])),
                offset: u64::from_le_bytes(s[0x18..0x20].try_into().unwrap_or([0; 8])),
                size: u64::from_le_bytes(s[0x20..0x28].try_into().unwrap_or([0; 8])),
                link: u32::from_le_bytes(s[0x28..0x2c].try_into().unwrap_or([0; 4])),
                entsize: u64::from_le_bytes(s[0x38..0x40].try_into().unwrap_or([0; 8])),
            });
        }

        // La tabla de nombres de seccion no hace falta para resolver
        // simbolos: se comprueba que el indice sea valido y se sigue.
        if e_shstrndx >= e_shnum {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "e_shstrndx fuera de rango"));
        }

        // --- symtab (con .strtab enlazada) ---
        let symtab = secs
            .iter()
            .find(|s| s.kind == SHT_SYMTAB)
            .or_else(|| secs.iter().find(|s| s.kind == SHT_DYNSYM))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "ELF sin .symtab ni .dynsym")
            })?;
        let strtab = secs.get(symtab.link as usize).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, ".symtab sin .strtab")
        })?;
        let strdata = leer(&mut f, strtab.offset, strtab.size)?;
        let symdata = leer(&mut f, symtab.offset, symtab.size)?;

        let entsize = if symtab.entsize == 0 { 24 } else { symtab.entsize as usize };
        let mut symbols = Vec::new();
        let mut off = 0usize;
        while off + entsize <= symdata.len() {
            let s = &symdata[off..off + entsize];
            let st_name = u32::from_le_bytes(s[0..4].try_into().unwrap_or([0; 4])) as usize;
            let st_info = s[4];
            let st_shndx = u16::from_le_bytes(s[6..8].try_into().unwrap_or([0; 2]));
            let st_value = u64::from_le_bytes(s[8..16].try_into().unwrap_or([0; 8]));
            let st_size = u64::from_le_bytes(s[16..24].try_into().unwrap_or([0; 8]));
            off += entsize;

            if st_shndx == SHN_UNDEF {
                continue;
            }
            // Se guardan las funciones (STT_FUNC) y tambien los objetos
            // (STT_OBJECT): hacen falta las tablas virtuales (`_ZTV6InGame`,
            // etc.) para poder empujar un estado `InGame` con la vtable
            // correcta.
            let tipo = st_info & 0xf;
            if tipo != STT_FUNC && tipo != STT_OBJECT {
                continue;
            }
            if st_name == 0 || st_name >= strdata.len() {
                continue;
            }
            let fin = strdata[st_name..]
                .iter()
                .position(|c| *c == 0)
                .map(|p| st_name + p)
                .unwrap_or(strdata.len());
            let nombre = String::from_utf8_lossy(&strdata[st_name..fin]).into_owned();
            if nombre.is_empty() {
                continue;
            }
            symbols.push(Symbol { name: nombre, value: st_value, size: st_size, kind: st_info & 0xf });
        }

        // --- primer PT_LOAD (para el sesgo de carga) ---
        let mut phdrs = vec![0u8; 56 * 64];
        f.seek(SeekFrom::Start(0x40))?;
        let leidos = f.read(&mut phdrs)?;
        let mut first_load_vaddr = 0u64;
        let mut visto = false;
        for i in 0..(leidos / 56) {
            let p = &phdrs[i * 56..(i + 1) * 56];
            let p_type = u32::from_le_bytes(p[0..4].try_into().unwrap_or([0; 4]));
            if p_type == PT_LOAD {
                let p_vaddr = u64::from_le_bytes(p[0x10..0x18].try_into().unwrap_or([0; 8]));
                if !visto || p_vaddr < first_load_vaddr {
                    first_load_vaddr = p_vaddr;
                    visto = true;
                }
            }
        }

        Ok(ElfImage { path: path.to_path_buf(), first_load_vaddr, symbols })
    }

    /// Direccion virtual del primer PT_LOAD (0 en el motor).
    pub fn first_load_vaddr(&self) -> u64 {
        self.first_load_vaddr
    }

}

/// Lee `len` bytes en `offset`.
fn leer(f: &mut File, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if len > 512 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "seccion demasiado grande"));
    }
    let mut buf = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}
