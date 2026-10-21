//! Detour minimo sobre ARM64 (AArch64).
//!
//! # Como se parchea
//!
//! En AArch64 todas las instrucciones son de 4 bytes y estan alineadas a 4,
//! asi que un salto nunca cae en mitad de una instruccion (a diferencia de
//! x86-64, donde un `jmp` de 5 bytes puede desalinear el flujo). Aun asi hay
//! que tener cuidado con dos cosas:
//!
//!   * Un salto relativo (`B`) tiene alcance limitado, pero un salto
//!     **absoluto** a una funcion de nuestra libreria no cabe en una sola
//!     instruccion: `MOV` (MOVZ) solo codifica 16 bits inmediatos.
//!   * La forma canonica de AArch64 para saltar a una direccion de 64 bits es
//!     cargar el destino en un registro y ramificar a traves de el:
//!
//!         ldr  x16, #<literal>      ; 0x58000050 + imm19<<5  (literal PC-relativo)
//!         br   x16                 ; 0xD61F0200
//!
//!     El "literal" (8 bytes con la direccion) tiene que vivir en algun sitio.
//!     Hay dos formas:
//!
//!       a) Literal en el sitio: el parche ocupa 16 bytes (8 de codigo + 8 de
//!          pool literal) y hay que reubicar 16 bytes del original.
//!       b) Literal en nuestra libreria: el parche ocupa 8 bytes (justo las
//!          dos instrucciones) pero el literal debe estar a menos de 1 MB
//!          (limite de `ldr` PC-relativo) y su contenido se escribe al
//!          instalar el detour.
//!
//!     El inyector elige (b) cuando la distancia lo permite y (a) si no.
//!
//! # Reubicacion de instrucciones PC-relativas
//!
//! Al copiar el principio de una funcion a otro sitio, las instrucciones que
//! calculan direcciones relativas al PC (`adr`, `adrp`, `ldr` de literal)
//! dejan de apuntar a lo mismo. El trampolin las **reescribe** para que
//! apunten al mismo destino desde su nueva direccion:
//!
//!   * `adrp`  -> alcance +-4 GB: se recalcula el imm21.
//!   * `adr`   -> alcance +-1 MB: se recalcula el imm21.
//!   * `ldr` de literal -> si el literal esta dentro del bloque reubicado se
//!     mantiene el desplazamiento; si esta fuera, se recalcula (alcance
//!     +-1 MB).
//!
//! Si algun destino queda fuera de alcance, el inyector **renuncia** en vez
//! de generar codigo roto.
//!
//! # Trampolin
//!
//! El trampolin es una pagina mapeada con RWX donde:
//!
//!   1. Se copia el bloque original de instrucciones (2 o 4, segun el parche),
//!      con las PC-relativas reescritas.
//!   2. Se anade un salto de vuelta a `destino + bytes_parcheados`.
//!
//! # Por que es seguro con PAC
//!
//! El motor de Factorio ARM64 usa pointer authentication: 2602 `paciasp` y
//! 3264 `autiasp`. La convencion es "el calibrador firma la direccion de
//! retorno al entrar y la verifica al salir" (con el SP como contexto). El
//! trampolin reproduce exactamente eso: la `paciasp` original se reubica tal
//! cual y se ejecuta con el mismo SP que tendria la funcion original, y la
//! `autiasp` final (que esta en el cuerpo original, sin reubicar) verifica
//! contra ese mismo SP. El juego recibe exactamente la misma direccion de
//! retorno que en el flujo sin parchear.

use std::ffi::c_void;
use std::io;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU64, AtomicUsize};

use crate::maps;

/// `ldr x16, #imm` (literal, 64 bits). imm19 va en los bits 5..23.
///
/// OJO: el imm19 de la constante es 0 a proposito; el desplazamiento se suma
/// en el punto de uso. Si la constante lo trajera ya puesto, al sumarlo otra
/// vez el `ldr` leeria el literal de la posicion equivocada: el parche
/// saltaria a un sitio invalido y el juego recibiria un SIGSEGV en el primer
/// frame, sin llegar a entrar nunca en el hook.
const OP_LDR_X16_LIT: u32 = 0x5800_0010;
/// `br x16`
const OP_BR_X16: u32 = 0xD61F_0200;
/// `ldr x17, #imm` (literal, 64 bits). imm19 = 0 (ver arriba).
const OP_LDR_X17_LIT: u32 = 0x5800_0011;
/// `br x17`
const OP_BR_X17: u32 = 0xD61F_0220;

/// Alcance de `ldr` PC-relativo: +-2^19 palabras = +-1 MB.
const ALCANCE_LDR_PC: i64 = 1 << 21;

/// Tamano de pagina del sistema.
const PAGINA: usize = 4096;

/// Pool de literales para los parches cortos. Cada detour toma una entrada.
/// Es memoria de nuestra .so: la direccion es estable de por vida y se puede
/// escribir en tiempo de ejecucion (esta en .bss, segmento RW).
static LITERALES: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static LITERAL_COUNT: AtomicUsize = AtomicUsize::new(0);

extern "C" {
    fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
}

const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const PROT_EXEC: c_int = 0x4;
const MAP_PRIVATE: c_int = 0x02;
const MAP_ANONYMOUS: c_int = 0x20;
const MAP_FAILED: isize = -1;

/// Como se codifica el parche en la funcion objetivo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Estilo {
    /// 8 bytes: `ldr x16, #lit` + `br x16`, con el literal en nuestra libreria.
    /// Exige que el literal este a menos de 1 MB del destino.
    Corto,
    /// 16 bytes: `ldr x16, #2` + `br x16` + literal de 8 bytes en el sitio.
    Largo,
}

impl Estilo {
    /// Bytes que ocupa el parche.
    pub fn bytes(self) -> usize {
        match self {
            Estilo::Corto => 8,
            Estilo::Largo => 16,
        }
    }
}

/// Error al instalar o quitar un detour.
#[derive(Debug)]
pub enum Error {
    /// La direccion destino no esta en una region legible.
    DestinoIlegible(u64),
    /// La funcion es mas corta que el parche: no se puede reubicar sin
    /// corromperla. Es el caso de "renunciar" que pide el diseno.
    FuncionCorta { direccion: u64, necesita: usize, hay: usize },
    /// Alguna de las instrucciones a reubicar depende del PC y no se puede
    /// corregir desde el trampolin, o es un salto.
    InstruccionNoRelocalizable { direccion: u64, instruccion: u32 },
    /// El literal del parche corto quedaria a mas de 1 MB del destino.
    LiteralDemasiadoLejos,
    /// No quedan literales o trampolines.
    SinRecursos(&'static str),
    /// mprotect/mmap fallaron.
    Sistema(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DestinoIlegible(a) => {
                write!(f, "la direccion 0x{a:x} no esta mapeada como legible")
            }
            Error::FuncionCorta { direccion, necesita, hay } => write!(
                f,
                "funcion en 0x{direccion:x} demasiado corta: hacen falta {necesita} bytes y solo hay {hay}"
            ),
            Error::InstruccionNoRelocalizable { direccion, instruccion } => write!(
                f,
                "la instruccion 0x{instruccion:08x} en 0x{direccion:x} no se puede reubicar"
            ),
            Error::LiteralDemasiadoLejos => {
                write!(f, "el literal del parche corto quedaria a mas de 1 MB del destino")
            }
            Error::SinRecursos(q) => write!(f, "sin recursos: {q}"),
            Error::Sistema(m) => write!(f, "error del sistema: {m}"),
        }
    }
}

/// Un detour instalado.
pub struct Detour {
    /// Direccion de la funcion original.
    pub destino: u64,
    /// Direccion de nuestra funcion hook.
    pub hook: u64,
    /// Direccion del trampolin (llamar para ejecutar el original).
    pub trampolin: u64,
    /// Estilo de parche usado.
    pub estilo: Estilo,
    /// Bytes originales (para poder restaurar).
    pub original: Vec<u8>,
}

/// Firma de la funcion hook: recibe el `this` (x0) tal cual.
pub type Hook = unsafe extern "C" fn(*mut c_void);

/// Instala un detour: la funcion en `destino` pasa a llamar a `hook`.
///
/// `hook` recibe el puntero `this` (x0) tal cual, para poder pasarlo al
/// original a traves del trampolin.
pub fn instalar(
    destino: u64,
    hook: u64,
    forzar: Option<Estilo>,
    tamano_funcion: u64,
) -> Result<Detour, Error> {
    // 1. El destino tiene que estar en una region legible (si no, leerlo
    //    seria un SIGSEGV dentro del juego).
    let region = match maps::region_de(destino) {
        Some(r) => r,
        None => return Err(Error::DestinoIlegible(destino)),
    };

    // 2. Estilo del parche. Preferimos el corto de 8 bytes si el literal
    //    (que vive en nuestra libreria) queda a menos de 1 MB.
    let literal = reservar_literal(hook);
    let estilo = match forzar {
        Some(e) => e,
        None => {
            if literal.map(|l| distancia_ok(destino, l)).unwrap_or(false) {
                Estilo::Corto
            } else {
                Estilo::Largo
            }
        }
    };
    if estilo == Estilo::Corto {
        match literal {
            Some(l) if distancia_ok(destino, l) => {}
            _ => return Err(Error::LiteralDemasiadoLejos),
        }
    }

    let mut bytes = estilo.bytes();

    // 3. Leer los bytes originales (con cuidado de no leer fuera de la
    //    region mapeada).
    let mut original = leer_memoria(destino, bytes)?.to_vec();

    // 3b. Con el parche largo, el bloque reubicado tiene que ser mas grande
    //     que el parche: si dentro hay un salto condicional (B.cond,
    //     CBZ/CBNZ, TBZ/TBNZ) que apunte fuera del bloque, su destino
    //     relativo quedaria corrupto al copiar el bloque (el salto se
    //     saltaria a un sitio cualquiera y el juego moriria con SIGILL).
    //     Se crece el bloque hasta que todos esos saltos apunten dentro.
    if estilo == Estilo::Largo {
        loop {
            let t = tamanho_del_bloque(destino, &original, MAXIMO_BLOQUE);
            if t <= bytes {
                break;
            }
            // El bloque no puede pasar de la funcion: si se reubicaran bytes
            // de la funcion siguiente, al escribir el parche se corromperia.
            if (tamano_funcion != 0) && (t as u64) > tamano_funcion {
                return Err(Error::FuncionCorta {
                    direccion: destino,
                    necesita: t,
                    hay: tamano_funcion as usize,
                });
            }
            if t > MAXIMO_BLOQUE {
                return Err(Error::InstruccionNoRelocalizable {
                    direccion: destino,
                    instruccion: 0,
                });
            }
            bytes = t;
            original = leer_memoria(destino, bytes)?.to_vec();
        }
    }

    // 4. Montar el trampolin (reubica el bloque y reescribe las
    //    instrucciones PC-relativas; si no se puede, renuncia).
    let trampolin = montar_trampolin(destino, &original, bytes)?;

    // 5. Construir el parche.
    let parche = match estilo {
        Estilo::Corto => {
            let lit = literal.ok_or(Error::LiteralDemasiadoLejos)?;
            let off = (lit as i64 - destino as i64) / 4;
            let mut p = Vec::with_capacity(8);
            p.extend_from_slice(&OP_LDR_X16_LIT.wrapping_add(((off as u32) & 0xF_FFFF) << 5).to_le_bytes());
            p.extend_from_slice(&OP_BR_X16.to_le_bytes());
            p
        }
        Estilo::Largo => {
            let mut p = Vec::with_capacity(bytes);
            p.extend_from_slice(&OP_LDR_X16_LIT.wrapping_add(2u32 << 5).to_le_bytes());
            p.extend_from_slice(&OP_BR_X16.to_le_bytes());
            p.extend_from_slice(&hook.to_le_bytes());
            // Si el bloque reubicado es mas grande que el parche minimo, los
            // bytes sobrantes se copian del original: son codigo muerto (el
            // salto al hook no vuelve aqui), pero hay que sobrescribirlos
            // para que no se ejecuten dos veces.
            if bytes > 16 {
                p.extend_from_slice(&original[16..bytes]);
            }
            p
        }
    };

    // 6. Escribir el parche en la pagina del destino (RWX temporal).
    // XFACTORVR_HOOK_NOPATCH=1 deja el destino intacto (diagnostico).
    if std::env::var("XFACTORVR_HOOK_NOPATCH").as_deref() == Ok("1") {
        return Ok(Detour { destino, hook, trampolin, estilo, original });
    }
    // Diagnostico: volcar los bytes reales del parche y las direcciones, para
    // poder comprobar a mano el codigo que se va a ejecutar.
    if std::env::var("XFACTORVR_HOOK_TRACE").as_deref() == Ok("1") {
        let mut hex = String::with_capacity(parche.len() * 2);
        for b in &parche {
            hex.push_str(&format!("{:02x}", b));
        }
        crate::log::info(&format!(
            "parche en 0x{:x}: {} (hook en 0x{:x}, bytes reubicados {})",
            destino, hex, hook, bytes
        ));
    }
    escribir_memoria(destino, &parche, region.perms)?;

    Ok(Detour { destino, hook, trampolin, estilo, original })
}

/// Restaura los bytes originales (deshace el parche).
pub fn desinstalar(d: &Detour) -> Result<(), Error> {
    let region = match maps::region_de(d.destino) {
        Some(r) => r,
        None => return Err(Error::DestinoIlegible(d.destino)),
    };
    escribir_memoria(d.destino, &d.original, region.perms)?;
    Ok(())
}

/// Reserva un literal de 8 bytes en nuestra .so y le escribe la direccion del hook.
fn reservar_literal(hook: u64) -> Option<u64> {
    let i = LITERAL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if i >= LITERALES.len() {
        return None;
    }
    // La direccion de un elemento de un array estatico es estable durante
    // toda la vida del proceso: ese es el "literal" que lee el parche.
    let slot = &LITERALES[i] as *const AtomicU64 as u64;
    LITERALES[i].store(hook, std::sync::atomic::Ordering::Relaxed);
    Some(slot)
}

/// True si la distancia destino->literal cabe en el alcance de `ldr`.
fn distancia_ok(destino: u64, literal: u64) -> bool {
    let d = literal as i64 - destino as i64;
    d >= -ALCANCE_LDR_PC && d < ALCANCE_LDR_PC && d % 4 == 0
}

// --- pagina de trampolines -------------------------------------------------
//
// Todos los trampolines viven en una sola pagina mapeada con RWX, con un
// contador. Asi el .so solo necesita una reserva de memoria ejecutable.

static TRAMPOLIN_BASE: AtomicUsize = AtomicUsize::new(0);
static TRAMPOLIN_USADO: AtomicUsize = AtomicUsize::new(0);

/// Reserva la pagina donde viven los trampolines (idempotente).
///
/// La pagina se pide **cerca del destino** (con una pista para `mmap`): las
/// instrucciones `adrp` del bloque reubicado solo se pueden corregir si el
/// trampolin esta a menos de 4 GB (su alcance), y lo normal es que este en
/// ese rango. Si el motor esta lejos (por ejemplo con ASLR Jabalina), se
/// prueban varias pistas; si ninguna sirve, se cae a una pagina cualquiera y
/// seran los hooks con `adrp` los que no se puedan instalar.
fn asegurar_trampolines(destino: u64) -> Result<usize, Error> {
    let p = TRAMPOLIN_BASE.load(std::sync::atomic::Ordering::SeqCst);
    if p != 0 {
        return Ok(p);
    }

    // Distancias candidatas. No pueden ser muy pequenas: el motor ocupa
    // ~768 MB de direccion virtual contiguos, asi que una pista a 2 MB cae
    // dentro de su mapa. Con una pista mayor, el kernel coloca la pagina en
    // el hueco libre MAS CERCANO (asi sigue quedandose a pocos cientos de
    // MB, dentro del alcance de +-4 GB del `adrp`).
    const DELTAS: [i64; 8] = [
        0x0100_0000,
        -0x0100_0000,
        0x0400_0000,
        -0x0400_0000,
        0x1000_0000,
        -0x1000_0000,
        0x4000_0000,
        -0x4000_0000,
    ];

    for delta in DELTAS {
        let pista = (destino as i64).wrapping_add(delta) as *mut c_void;
        let p = unsafe {
            mmap(
                pista,
                PAGINA,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if p as isize == MAP_FAILED || p.is_null() {
            continue;
        }
        let distancia = (p as i64).wrapping_sub(destino as i64).abs();
        // +-2 GB de sobra para corregir cualquier `adrp` (±4 GB).
        if distancia < (1i64 << 31) {
            TRAMPOLIN_BASE.store(p as usize, std::sync::atomic::Ordering::SeqCst);
            return Ok(p as usize);
        }
        // El kernel no honoring la pista: se devuelve la pagina al sistema y
        // se prueba otra distancia.
        unsafe {
            munmap(p, PAGINA);
        }
    }

    // Ultimo recurso: cualquier pagina (los hooks sin `adrp` funcionaran).
    let p = unsafe {
        mmap(
            std::ptr::null_mut(),
            PAGINA,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if p as isize == MAP_FAILED || p.is_null() {
        return Err(Error::Sistema("mmap del trampolin fallo".into()));
    }
    TRAMPOLIN_BASE.store(p as usize, std::sync::atomic::Ordering::SeqCst);
    Ok(p as usize)
}

/// Escribe el trampolin y devuelve su direccion.
///
/// `original` es el bloque de bytes a reubicar y `bytes` su longitud.
fn montar_trampolin(destino: u64, original: &[u8], bytes: usize) -> Result<u64, Error> {
    let base = asegurar_trampolines(destino)?;
    let usado = TRAMPOLIN_USADO.load(std::sync::atomic::Ordering::SeqCst);
    // El trampolin ocupa: bytes reubicados + 8 (ldr+br) + 8 (literal).
    let tam = bytes + 16;
    if usado + tam > PAGINA {
        return Err(Error::SinRecursos("la pagina de trampolines esta llena"));
    }
    let addr = (base + usado) as u64;

    // --- copiar el bloque original, reescribiendo lo PC-relativo ---
    let mut bloque: Vec<u8> = Vec::with_capacity(tam);
    for (i, chunk) in original.chunks_exact(4).enumerate() {
        let insn = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let viejo_pc = destino + (i as u64) * 4;
        let nuevo_pc = addr + (i as u64) * 4;
        let nuevo_insn = match reescribir_pc_relativa(
            insn,
            viejo_pc,
            nuevo_pc,
            destino,
            bytes as u64,
        ) {
            Reescritura::Corregida(n) => n,
            Reescritura::Igual => insn,
            Reescritura::Imposible => {
                if !es_relocalizable(insn) {
                    return Err(Error::InstruccionNoRelocalizable {
                        direccion: viejo_pc,
                        instruccion: insn,
                    });
                }
                insn
            }
        };
        bloque.extend_from_slice(&nuevo_insn.to_le_bytes());
    }

    // --- salto de vuelta al original: ldr x17, #2 ; br x17 ; literal ---
    bloque.extend_from_slice(&OP_LDR_X17_LIT.wrapping_add(2u32 << 5).to_le_bytes());
    bloque.extend_from_slice(&OP_BR_X17.to_le_bytes());
    bloque.extend_from_slice(&(destino + bytes as u64).to_le_bytes());

    // Diagnostico: el bloque original que se va a copiar y donde va a parar.
    {
        let mut hex = String::new();
        for b in original.iter().take(16) {
            hex.push_str(&format!("{:02x}", b));
        }
        crate::log::info(&format!(
            "bloque original ({} bytes): {} -> reubicado en 0x{:x} (bloque de {} bytes)",
            original.len(), hex, addr, bloque.len()
        ));
    }

    unsafe {
        std::ptr::copy_nonoverlapping(bloque.as_ptr(), addr as *mut u8, bloque.len());
        // La pagina se deja en RWX: el codigo tiene que ser ejecutable.
        //
        // `mprotect` exige que la direccion este alineada a pagina, y aqui
        // `addr` es `base + usado`: el segundo trampolin de la pagina empieza
        // en `base + 32`, que no esta alineado, y `mprotect` fallaria con
        // EINVAL. Se protege la BASE de la pagina, no el trampolin.
        let base_pagina = TRAMPOLIN_BASE.load(std::sync::atomic::Ordering::SeqCst) as *mut c_void;
        if mprotect(base_pagina, PAGINA, PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
            return Err(Error::Sistema(format!(
                "mprotect de la pagina de trampolines (0x{:x}) fallo: {}",
                base_pagina as usize,
                io::Error::last_os_error()
            )));
        }
        limpiar_cache(addr as *mut c_void, (addr + bloque.len() as u64) as *mut c_void);
    }

    let nuevo = (usado + bloque.len() + 3) & !3;
    TRAMPOLIN_USADO.store(nuevo, std::sync::atomic::Ordering::SeqCst);
    Ok(addr)
}

/// Lee `len` bytes de memoria comprobando antes que la region sea legible.
fn leer_memoria(addr: u64, len: usize) -> Result<&'static [u8], Error> {
    let region = match maps::region_de(addr) {
        Some(r) => r,
        None => return Err(Error::DestinoIlegible(addr)),
    };
    if !region.legible() {
        return Err(Error::DestinoIlegible(addr));
    }
    if addr + len as u64 > region.fin {
        return Err(Error::DestinoIlegible(addr));
    }
    // SAFETY: la region es legible y el rango cae dentro de ella.
    Ok(unsafe { std::slice::from_raw_parts(addr as *const u8, len) })
}

/// Escribe `datos` en `addr` abriendo los permisos de la pagina temporalmente.
fn escribir_memoria(addr: u64, datos: &[u8], permisos: [bool; 3]) -> Result<(), Error> {
    let inicio_pagina = addr & !((PAGINA as u64) - 1);
    let fin_pagina = (addr + datos.len() as u64 + PAGINA as u64 - 1) & !((PAGINA as u64) - 1);

    let mut prot = PROT_READ;
    if permisos[1] {
        prot |= PROT_WRITE;
    }
    if permisos[2] {
        prot |= PROT_EXEC;
    }
    // Para poder escribir hace falta PROT_WRITE; si la pagina era r-x se abre
    // a rwx y luego se restaura.
    let r = unsafe {
        mprotect(
            inicio_pagina as *mut c_void,
            (fin_pagina - inicio_pagina) as usize,
            prot | PROT_WRITE,
        )
    };
    if r != 0 {
        return Err(Error::Sistema(format!(
            "mprotect(0x{inicio_pagina:x}) fallo: {}",
            io::Error::last_os_error()
        )));
    }

    unsafe {
        std::ptr::copy_nonoverlapping(datos.as_ptr(), addr as *mut u8, datos.len());
        limpiar_cache(addr as *mut c_void, (addr + datos.len() as u64) as *mut c_void);
        // Restaura los permisos originales.
        mprotect(inicio_pagina as *mut c_void, (fin_pagina - inicio_pagina) as usize, prot);
    }
    Ok(())
}

/// Resultado de intentar reescribir una instruccion PC-relativa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reescritura {
    /// La instruccion no depende del PC: se copia tal cual.
    Igual,
    /// Se ha corregido para que apunte al mismo destino.
    Corregida(u32),
    /// Depende del PC y no se puede corregir desde el trampolin.
    Imposible,
}

/// Reescribe una instruccion PC-relativa para que apunte al mismo destino
/// desde su nueva direccion.
///
/// Parametros:
///   * `insn`: la instruccion original.
///   * `viejo_pc`: direccion de la instruccion en el codigo original.
///   * `nuevo_pc`: direccion que tendra en el trampolin.
///   * `base_bloque` / `largo_bloque`: el bloque original reubicado, para
///     saber si un literal esta dentro (entonces el desplazamiento se
///     mantiene) o fuera (hay que corregirlo).
pub fn reescribir_pc_relativa(
    insn: u32,
    viejo_pc: u64,
    nuevo_pc: u64,
    base_bloque: u64,
    largo_bloque: u64,
) -> Reescritura {
    // --- ADR / ADRP ---
    if insn & 0x9F00_0000 == 0x1000_0000 || insn & 0x9F00_0000 == 0x9000_0000 {
        let es_adrp = (insn & 0x8000_0000) != 0;
        let immlo = (insn >> 29) & 0x3;
        let immhi = (insn >> 5) & 0x7_FFFF;
        // 21 bits con signo. OJO: el signo hay que extenderlo DENTRO de los
        // 21 bits ((x << 11) as i32 >> 11).  Si se hace con una mascara de
        // 32 bits (| 0xFFE0_0000) y luego se desplaza 12 para calcular la
        // pagina, los bits altos se salen de rango y el ADRP reubicado
        // apunta a una pagina inventada: el motor lee y escribe en una
        // direccion cualquiera (y el cubo 3D no sale).
        let imm21 = ((((immhi << 2) | immlo) << 11) as i32) >> 11;
        if es_adrp {
            // ADRP: alcance +-4 GB.
            let destino = (viejo_pc & !0xFFF).wrapping_add(((imm21 as i64) << 12) as u64);
            let nuevo_pagina = nuevo_pc & !0xFFF;
            let delta = (destino as i64).wrapping_sub(nuevo_pagina as i64);
            if delta < -(1i64 << 31) || delta > (1i64 << 31) - 0x1000 {
                return Reescritura::Imposible;
            }
            // El inmediato de ADRP son 21 bits con signo (immhi:immlo). La
            // mascara debe ser de 21 bits: con 19 se perdian los dos bits
            // altos y el ADRP relocated apuntaba a la pagina equivocada
            // (el juego se moria al ejecutar el salto).
            let nuevo = ((delta >> 12) as u32) & 0x1F_FFFF;
            let nuevo_immlo = nuevo & 0x3;
            let nuevo_immhi = (nuevo >> 2) & 0x7_FFFF;
            return Reescritura::Corregida(
                (insn & 0x9F00_001F) | (nuevo_immlo << 29) | (nuevo_immhi << 5),
            );
        }
        // ADR: alcance +-1 MB.
        let destino = viejo_pc.wrapping_add(imm21 as i64 as u64);
        let delta = (destino as i64).wrapping_sub(nuevo_pc as i64);
        if delta < -(1i64 << 20) || delta > (1i64 << 20) {
            return Reescritura::Imposible;
        }
        // ADR: alcance +-1 MB. El inmediato tambien es de 21 bits con signo.
        let nuevo = (delta as u32) & 0x1F_FFFF;
        let nuevo_immlo = nuevo & 0x3;
        let nuevo_immhi = (nuevo >> 2) & 0x7_FFFF;
        return Reescritura::Corregida(
            (insn & 0x9F00_001F) | (nuevo_immlo << 29) | (nuevo_immhi << 5),
        );
    }

    // --- B / BL con inmediato de 26 bits ---
    // Los saltos relatives de 26 bits tienen un alcance de +-128 MB. Como el
    // trampolin se reserva CERCA del destino (a few MB), se pueden reubicar
    // corrigiendo el desplazamiento. Es imprescindible: los prologos de
    // Factorio en ARM64 contienen BL (llamadas a funciones auxiliares) y sin
    // esto no se puede instalar ningun detour.
    if insn & 0xFC00_0000 == 0x1400_0000 || insn & 0xFC00_0000 == 0x9400_0000 {
        let imm26 = (insn & 0x03FF_FFFF) as i32;
        // Extension de signo de los 26 bits.
        let imm26 = if (imm26 & 0x0200_0000) != 0 {
            (imm26 | !0x03FF_FFFF) as i64
        } else {
            imm26 as i64
        };
        let destino = viejo_pc.wrapping_add((imm26 * 4) as u64);
        // Si el destino cae dentro del bloque reubicado, la distancia
        // relativa se conserva (el bloque se copia entero y en orden).
        if destino >= base_bloque && destino < base_bloque + largo_bloque {
            return Reescritura::Igual;
        }
        // Si esta fuera, hay que corregir el desplazamiento: alcance +-128 MB.
        let delta = (destino as i64).wrapping_sub(nuevo_pc as i64);
        if delta % 4 != 0 || delta < -(1i64 << 26) || delta > (1i64 << 26) {
            return Reescritura::Imposible;
        }
        let nuevo = ((delta / 4) as u32) & 0x7_FFFF;
        return Reescritura::Corregida((insn & 0xFC00_0000) | nuevo);
    }

    // --- LDR / PRFM de literal (32 y 64 bits) ---
    if insn & 0xFF00_0000 == 0x5800_0000
        || insn & 0xFF00_0000 == 0x1800_0000
        || insn & 0xFF00_0000 == 0xD800_0000
    {
        let imm19 = (insn >> 5) & 0x7_FFFF;
        let imm19 = if (imm19 & 0x4_0000) != 0 {
            imm19 | 0xFFF8_0000
        } else {
            imm19
        };
        let literal = viejo_pc.wrapping_add(((imm19 as i64) * 4) as u64);
        // Si el literal esta dentro del bloque reubicado, el desplazamiento
        // relativo se mantiene (el bloque se copia entero y en orden).
        if literal >= base_bloque && literal < base_bloque + largo_bloque {
            return Reescritura::Igual;
        }
        // Si esta fuera, hay que corregir el desplazamiento: alcance +-1 MB.
        let delta = (literal as i64).wrapping_sub(nuevo_pc as i64);
        if delta % 4 != 0 || delta < -(1i64 << 20) || delta > (1i64 << 20) {
            return Reescritura::Imposible;
        }
        let nuevo = ((delta / 4) as u32) & 0x7_FFFF;
        return Reescritura::Corregida((insn & 0xFF00_001F) | (nuevo << 5));
    }

    // --- Saltos condicionales de 19 bits (B.cond, CBZ/CBNZ, TBZ/TBNZ) ---
    //
    // Estos saltos tienen un alcance pequeño (+-1 MB, o +-32 KB los TBZ) y
    // el trampolin puede estar a cientos de MB del destino, asi que NO se
    // pueden rebasar a la direccion original. La solucion es que el bloque
    // reubicado incluya tambien su destino (ver `tamanho_del_bloque`): al
    // copiarse el bloque entero y en orden, la distancia relativa se
    // conserva y el salto sigue apuntando al codigo correcto.
    if let Some((bits, escala, inmediato)) = inmediato_salto_condicional(insn) {
        let inm = extender_signo(inmediato, bits);
        let destino = viejo_pc.wrapping_add(((inm as i64) * escala) as u64);
        // Destino dentro del bloque reubicado: se conserva tal cual.
        if destino >= base_bloque && destino < base_bloque + largo_bloque {
            return Reescritura::Igual;
        }
        // Fuera del bloque: solo se puede corregir si el trampolin esta
        // dentro del alcance del salto.
        let delta = (destino as i64).wrapping_sub(nuevo_pc as i64);
        let alcance = 1i64 << (bits + 1);
        if delta % escala != 0 || delta < -alcance || delta > alcance {
            return Reescritura::Imposible;
        }
        let nuevo = ((delta / escala) as u32) & ((1u32 << bits) - 1);
        let conservar = if bits >= 19 {
            0xFF00_001F
        } else {
            0xFFF8_001F
        };
        return Reescritura::Corregida((insn & conservar) | (nuevo << 5));
    }

    Reescritura::Igual
}

/// Tamano maximo del bloque reubicado (bytes).
pub const MAXIMO_BLOQUE: usize = 256;

/// Inmediato y escala de un salto condicional de 19 bits.
///
/// Devuelve `None` si la instruccion no es de ese tipo. Los tres tipos
/// comparten el immediato en los bits 23:5, pero `TBZ/TBNZ` usa 14 bits y
/// escala 2 (el destino tiene que estar alineado a dos bytes).
fn inmediato_salto_condicional(insn: u32) -> Option<(u32, i64, u32)> {
    // B.cond: 01010100 imm19 0 cond
    if insn & 0xFF00_0010 == 0x5400_0000 {
        return Some((19, 4, (insn >> 5) & 0x7_FFFF));
    }
    // CBZ / CBNZ: sf 1 0 1 0 1 op 0 imm19 Rt
    if insn & 0x7E00_0000 == 0x3400_0000 {
        return Some((19, 4, (insn >> 5) & 0x7_FFFF));
    }
    // TBZ / TBNZ: b5 1 1 0 1 1 0 0 op b40 imm14 Rn
    if insn & 0x7E00_0000 == 0x3600_0000 {
        return Some((14, 2, (insn >> 5) & 0x3_FFF));
    }
    None
}

/// Extiende el signo de un inmediato de `bits` bits.
fn extender_signo(valor: u32, bits: u32) -> i64 {
    let signo = 1u32 << (bits - 1);
    if valor & signo != 0 {
        (valor as i64) - (1i64 << bits)
    } else {
        valor as i64
    }
}

/// Calcula cuantos bytes hay que reubicar para que todos los saltos
/// condicionales que haya dentro del bloque apunten a codigo tambien
/// reubicado.
///
/// Un salto condicional de 19 bits no se puede rebasar a su destino
/// original (el trampolin esta lejos), asi que el destino tiene que formar
/// parte del bloque reubicado: al copiar el bloque entero y en orden, las
/// distancias relativas se conservan y los saltos siguen funcionando.
///
/// Por ejemplo, el prologo de `SpriteDrawOrder::setup` empieza con
///
///     ldrb w8, [x1, #0x40]
///     ldr  d1, [x3]
///     tbz  w8, #7, +0x18      <- salta a destino+0x18
///
/// asi que el bloque tiene que crecer hasta incluir `destino + 0x18 + 4`.
fn tamanho_del_bloque(destino: u64, bytes: &[u8], maximo: usize) -> usize {
    let mut tam = bytes.len().max(16);
    let limite = bytes.len() / 4 * 4;
    let mut i = 0usize;
    while i < limite {
        let insn = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        if let Some((bits, escala, inmediato)) = inmediato_salto_condicional(insn) {
            let inm = extender_signo(inmediato, bits);
            let pc = destino as i64 + i as i64;
            let objetivo = pc + inm * escala;
            if objetivo < destino as i64 || objetivo >= destino as i64 + tam as i64 {
                let nuevo_tam = (objetivo - destino as i64 + 4) as usize;
                if nuevo_tam > tam {
                    tam = nuevo_tam;
                }
            }
        }
        i += 4;
    }
    tam.min(maximo)
}

/// Indica si una instruccion se puede reubicar sin mas.
///
/// No se pueden reubicar tal cual las que dependen del PC (adr, adrp, ldr de
/// literal) ni los saltos (b, bl, b.cond, cbz, tbz, br, blr, ret) porque su
/// destino relativo dejaria de ser correcto.
///
/// Las PC-relativas SI se pueden reubicar **reescribiendolas**: ver
/// `reescribir_pc_relativa`.
pub fn es_relocalizable(insn: u32) -> bool {
    // ADR / ADRP (se reescriben, no se copian tal cual)
    if insn & 0x9F00_0000 == 0x1000_0000 || insn & 0x9F00_0000 == 0x9000_0000 {
        return false;
    }
    // LDR literal de 32/64 bits y PRFM literal
    if insn & 0xFF00_0000 == 0x5800_0000
        || insn & 0xFF00_0000 == 0x1800_0000
        || insn & 0xFF00_0000 == 0xD800_0000
    {
        return false;
    }
    // B / BL
    if insn & 0xFC00_0000 == 0x1400_0000 || insn & 0xFC00_0000 == 0x9400_0000 {
        return false;
    }
    // B.cond
    if insn & 0xFF00_0010 == 0x5400_0000 {
        return false;
    }
    // CBZ / CBNZ y TBZ / TBNZ
    if insn & 0x7E00_0000 == 0x3400_0000 || insn & 0x7E00_0000 == 0x3600_0000 {
        return false;
    }
    // BR / BLR / RET
    if insn & 0xFFFF_FC1F == 0xD61F_0000
        || insn & 0xFFFF_FC1F == 0xD63F_0000
        || insn & 0xFFFF_FC1F == 0xD65F_0000
    {
        return false;
    }
    // Generacion de excepciones (svc, brk, hlt...)
    if insn & 0xFFE0_0000 == 0xD400_0000 {
        return false;
    }
    true
}

/// Invalida la cache de instrucciones para el rango indicado.
///
/// En ARM64 la cache de instrucciones y la de datos NO son coherentes entre
/// si: despues de escribir codigo hay que limpiar la D-cache (para que el
/// dato llegue a la PoU), vaciar la I-cache y sincronizar. glibc no exporta
/// `__clear_cache` en aarch64 (es un simbolo de compiler-rt), asi que se
/// hace a mano con la secuencia estandar, que es la misma que usan los
/// motores JIT.
///
/// Linux deja el bit SCTLR_EL1.UCI activo, asi que `dc cvau` esta permitido
/// en modo usuario.
#[cfg(target_arch = "aarch64")]
fn limpiar_cache(inicio: *mut c_void, fin: *mut c_void) {
    const LINEA: usize = 64; // granula de la cache de datos
    let desde = inicio as usize;
    let hasta = fin as usize;

    unsafe {
        // 1. Limpiar la cache de datos hasta el punto de unicidad.
        let mut p = desde & !(LINEA - 1);
        while p < hasta {
            core::arch::asm!("dc cvau, {p}", p = in(reg) p, options(nostack));
            p += LINEA;
        }
        // 2. Que la limpieza se complete antes de invalidar la I-cache.
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        // 3. Invalidar la cache de instrucciones.
        let mut q = desde & !(LINEA - 1);
        while q < hasta {
            core::arch::asm!("ic ivau, {q}", q = in(reg) q, options(nostack));
            q += LINEA;
        }
        // 4. Sincronizar y vaciar la tuberia de instrucciones.
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));
    }
}

/// En el resto de arquitecturas no hay nada que hacer: la coherencia de
/// cache la garantiza el propio cargador al marcar la pagina ejecutable.
#[cfg(not(target_arch = "aarch64"))]
fn limpiar_cache(_inicio: *mut c_void, _fin: *mut c_void) {}

#[cfg(test)]
mod tests_adrp_real {
    use super::*;

    /// Caso REAL del motor 2.1.22 ARM64:
    ///   GameRenderer::render empieza con
    ///     0x17837e0  stp  x29, x30, [sp, #-0x20]!
    ///     0x17837e4  stp  x20, x19, [sp, #0x10]
    ///     0x17837e8  mov  x29, sp
    ///     0x17837ec  adrp x8, 0x3eba000
    /// El ADRP debe seguir apuntando a la MISMA pagina cuando se reubica en el
    /// trampolin, que esta a otra pagina.
    const ADRP: u32 = 0xf001_39a8;
    const VIEJO_PC: u64 = 0xAAAA_BBBF_37EC;
    const NUEVO_PC: u64 = 0xAAAA_BFBF_300C;
    /// Pagina a la que apunta el ADRP original: (pc & !0xFFF) + imm21<<12.
    const PAGINA_ESPERADA: u64 = 0xAAAA_BE32_A000;

    /// Pagina a la que apunta una instruccion ADRP ya reescrita.
    fn pagina_de(instruccion: u32, pc: u64) -> u64 {
        let immlo = (instruccion >> 29) & 0x3;
        let immhi = (instruccion >> 5) & 0x7_FFFF;
        // 21 bits con signo (igual que en reescribir_pc_relativa).
        let imm21 = ((((immhi << 2) | immlo) << 11) as i32) >> 11;
        (pc & !0xFFF).wrapping_add(((imm21 as i64) << 12) as u64)
    }

    #[test]
    fn adrp_reubicado_apunta_a_la_misma_pagina() {
        // El ADRP original apunta a esta pagina:
        assert_eq!(
            pagina_de(ADRP, VIEJO_PC),
            PAGINA_ESPERADA,
            "el ADRP original no apunta donde se espera"
        );

        match reescribir_pc_relativa(ADRP, VIEJO_PC, NUEVO_PC, VIEJO_PC, 16) {
            Reescritura::Corregida(nueva) => {
                assert_eq!(
                    pagina_de(nueva, NUEVO_PC),
                    PAGINA_ESPERADA,
                    "el ADRP reubicado (0x{nueva:08x}) apunta a 0x{:x} en vez de 0x{:x}",
                    pagina_de(nueva, NUEVO_PC),
                    PAGINA_ESPERADA
                );
            }
            Reescritura::Igual => panic!("el ADRP no se reescribio"),
            Reescritura::Imposible => panic!("el ADRP se dio por imposible"),
        }
    }
}
