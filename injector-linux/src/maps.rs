//! Lectura de `/proc/self/maps`.
//!
//! Sirve para dos cosas:
//!   1. Comprobar que la direccion destino es legible (y por tanto leer los
//!      bytes originales sin reventar el proceso con un SIGSEGV).
//!   2. Saber los permisos reales de la pagina para restaurarlos tras parchear.

/// Una region mapeada del proceso.
#[derive(Clone, Debug)]
pub struct Region {
    pub inicio: u64,
    pub fin: u64,
    /// Permisos en formato rwx (posicion 0 = lectura, 1 = escritura, 2 = ejecucion).
    pub perms: [bool; 3],
    /// Ruta del fichero mapeado (vacía para region anonima).
    pub ruta: String,
}

impl Region {
    pub fn contiene(&self, addr: u64) -> bool {
        addr >= self.inicio && addr < self.fin
    }

    pub fn legible(&self) -> bool {
        self.perms[0]
    }

    pub fn ejecutable(&self) -> bool {
        self.perms[2]
    }
}

/// Lee el mapa de memoria del proceso actual.
pub fn regiones() -> Vec<Region> {
    let mut out = Vec::new();
    let texto = match std::fs::read_to_string("/proc/self/maps") {
        Ok(t) => t,
        Err(_) => return out,
    };
    for linea in texto.lines() {
        let mut it = linea.split_whitespace();
        let rango = match it.next() {
            Some(r) => r,
            None => continue,
        };
        let perms = match it.next() {
            Some(p) if p.len() >= 3 => p,
            _ => continue,
        };
        let mut addr = rango.split('-');
        let inicio = match addr.next().and_then(|s| u64::from_str_radix(s, 16).ok()) {
            Some(v) => v,
            None => continue,
        };
        let fin = match addr.next().and_then(|s| u64::from_str_radix(s, 16).ok()) {
            Some(v) => v,
            None => continue,
        };
        // tras los permisos viene "offset dev inode" y luego la ruta (puede
        // tener espacios, asi que se recoja el resto de la linea).
        let resto: Vec<&str> = it.collect();
        let ruta = if resto.len() > 3 { resto[3..].join(" ") } else { String::new() };
        out.push(Region {
            inicio,
            fin,
            perms: [
                perms.as_bytes()[0] == b'r',
                perms.as_bytes()[1] == b'w',
                perms.as_bytes()[2] == b'x',
            ],
            ruta,
        });
    }
    out
}

/// Region que contiene `addr`.
pub fn region_de(addr: u64) -> Option<Region> {
    regiones().into_iter().find(|r| r.contiene(addr))
}
