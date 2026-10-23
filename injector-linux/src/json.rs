//! Parser de JSON minimo, sin dependencias.
//!
//! El inyector no puede arrastrar ni una libreria externa (se carga con
//! `LD_PRELOAD` en el proceso del juego y solo debe usar `std`). Pero hay
//! que leer JSON: el manifiesto de modelos (`manifest.json`) y el chunk JSON
//! de los GLB.
//!
//! Este parser cubre lo que necesitan esos dos ficheros:
//!
//!   * objetos, arrays, cadenas, numeros, `true`, `false`, `null`,
//!   * escapes de cadena: `\"` `\\` `\/` `\b` `\f` `\n` `\r` `\t` y
//!     `\uXXXX` (con pares surrogados),
//!   * numeros con exponente y con signo.
//!
//! No acepta comentarios ni JSON5 (no hacen falta).

/// Valor de JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Objeto: pares (clave, valor) en el orden del fichero.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Valor de una clave (solo en objetos).
    pub fn get(&self, clave: &str) -> Option<&Json> {
        match self {
            Json::Obj(pares) => pares.iter().find(|(k, _)| k == clave).map(|(_, v)| v),
            _ => None,
        }
    }

    /// El valor como numero (los enteros tambien valen).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// El valor como entero (trunca si hay parte decimal).
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|n| n as i64)
    }

    /// El valor como cadena.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// El valor como array.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// El valor como objeto (pares sueltos).
    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(p) => Some(p.as_slice()),
            _ => None,
        }
    }

    /// El valor como booleano.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Parsea un JSON. Devuelve `None` si esta mal formado.
pub fn parse(texto: &[u8]) -> Option<Json> {
    let mut p = Parser { b: texto, i: 0 };
    p.saltar_espacios();
    let v = p.valor()?;
    p.saltar_espacios();
    if p.i != p.b.len() {
        return None;
    }
    Some(v)
}

/// Estado del parser.
struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    /// Se salta espacios, tabuladores, saltos de linea y retornos.
    fn saltar_espacios(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                _ => break,
            }
        }
    }

    /// ¿Le queda algo por leer?
    fn queda(&self) -> bool {
        self.i < self.b.len()
    }

    /// El siguiente byte sin consumir.
    fn mira(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Parsea un valor completo.
    fn valor(&mut self) -> Option<Json> {
        self.saltar_espacios();
        match self.mira()? {
            b'{' => self.objeto(),
            b'[' => self.array(),
            b'"' => self.cadena().map(Json::Str),
            b't' => self.literal(b"true", Json::Bool(true)),
            b'f' => self.literal(b"false", Json::Bool(false)),
            b'n' => self.literal(b"null", Json::Null),
            _ => self.numero(),
        }
    }

    /// Consume una palabra clave (`true`, `false`, `null`).
    fn literal(&mut self, palabra: &[u8], valor: Json) -> Option<Json> {
        if self.b.len() >= self.i + palabra.len() && &self.b[self.i..self.i + palabra.len()] == palabra
        {
            self.i += palabra.len();
            Some(valor)
        } else {
            None
        }
    }

    /// Parsea un objeto.
    fn objeto(&mut self) -> Option<Json> {
        self.i += 1; // '{'
        let mut pares: Vec<(String, Json)> = Vec::new();
        self.saltar_espacios();
        if self.mira()? == b'}' {
            self.i += 1;
            return Some(Json::Obj(pares));
        }
        loop {
            self.saltar_espacios();
            if self.mira()? != b'"' {
                return None;
            }
            let clave = self.cadena()?;
            self.saltar_espacios();
            if self.mira()? != b':' {
                return None;
            }
            self.i += 1;
            let valor = self.valor()?;
            pares.push((clave, valor));
            self.saltar_espacios();
            match self.mira()? {
                b',' => {
                    self.i += 1;
                }
                b'}' => {
                    self.i += 1;
                    return Some(Json::Obj(pares));
                }
                _ => return None,
            }
        }
    }

    /// Parsea un array.
    fn array(&mut self) -> Option<Json> {
        self.i += 1; // '['
        let mut items: Vec<Json> = Vec::new();
        self.saltar_espacios();
        if self.mira()? == b']' {
            self.i += 1;
            return Some(Json::Arr(items));
        }
        loop {
            let v = self.valor()?;
            items.push(v);
            self.saltar_espacios();
            match self.mira()? {
                b',' => {
                    self.i += 1;
                }
                b']' => {
                    self.i += 1;
                    return Some(Json::Arr(items));
                }
                _ => return None,
            }
        }
    }

    /// Parsea una cadena (con las comillas).
    fn cadena(&mut self) -> Option<String> {
        if self.mira()? != b'"' {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = self.mira()?;
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let e = self.mira()?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            // Par surrogate: se combina con el siguiente.
                            let ch = if (0xD800..0xDC00).contains(&cp) {
                                if self.mira() == Some(b'\\') && self.b.get(self.i + 1) == Some(&b'u')
                                {
                                    self.i += 2;
                                    let bajo = self.hex4()?;
                                    if (0xDC00..0xE000).contains(&bajo) {
                                        let combined =
                                            0x10000 + ((cp - 0xD800) << 10) + (bajo - 0xDC00);
                                        char::from_u32(combined)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                char::from_u32(cp)
                            };
                            // Si el caracter no es valido, se pone el
                            // sustituto: mejor un texto raro que perderlo.
                            out.push(ch.unwrap_or('\u{FFFD}'));
                        }
                        _ => return None,
                    }
                }
                _ => {
                    // Se copia el byte tal cual (UTF-8 pasa intacto).
                    let inicio = self.i - 1;
                    let mut fin = self.i;
                    // Si el byte es multibyte, se copia entero.
                    if c >= 0x80 {
                        while fin < self.b.len() && (self.b[fin] & 0xC0) == 0x80 {
                            fin += 1;
                        }
                        self.i = fin;
                    }
                    out.push_str(&String::from_utf8_lossy(&self.b[inicio..fin]));
                }
            }
        }
    }

    /// Lee cuatro hexadecimales (para `\uXXXX`).
    fn hex4(&mut self) -> Option<u32> {
        if self.i + 4 > self.b.len() {
            return None;
        }
        let s = std::str::from_utf8(&self.b[self.i..self.i + 4]).ok()?;
        let v = u32::from_str_radix(s, 16).ok()?;
        self.i += 4;
        Some(v)
    }

    /// Parsea un numero.
    fn numero(&mut self) -> Option<Json> {
        let inicio = self.i;
        if self.mira() == Some(b'-') || self.mira() == Some(b'+') {
            self.i += 1;
        }
        let mut decimales = false;
        while self.queda() {
            match self.b[self.i] {
                b'0'..=b'9' => self.i += 1,
                b'.' | b'e' | b'E' => {
                    decimales = true;
                    self.i += 1;
                }
                b'-' | b'+' => self.i += 1,
                _ => break,
            }
        }
        if inicio == self.i {
            return None;
        }
        let texto = std::str::from_utf8(&self.b[inicio..self.i]).ok()?;
        if decimales {
            texto.parse::<f64>().ok().map(Json::Num)
        } else {
            match texto.parse::<i64>() {
                Ok(n) => Some(Json::Num(n as f64)),
                Err(_) => texto.parse::<f64>().ok().map(Json::Num),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objeto_simple() {
        let j = parse(br#"{"a": 1, "b": "dos", "c": [1, 2, 3]}"#).expect("no parseo");
        assert_eq!(j.get("a").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(j.get("b").and_then(|v| v.as_str()), Some("dos"));
        assert_eq!(j.get("c").and_then(|v| v.as_arr()).map(|a| a.len()), Some(3));
        assert!(j.get("z").is_none());
    }

    #[test]
    fn numeros_y_exponentes() {
        let j = parse(b"[1, -2.5, 1e3, 2.5e-2]").expect("no parseo");
        let a = j.as_arr().unwrap();
        assert_eq!(a[0].as_f64(), Some(1.0));
        assert_eq!(a[1].as_f64(), Some(-2.5));
        assert_eq!(a[2].as_f64(), Some(1000.0));
        assert!((a[3].as_f64().unwrap() - 0.025).abs() < 1e-9);
    }

    #[test]
    fn escapes_en_cadenas() {
        let j = parse(br#""a\"b\\c\ndA""#).expect("no parseo");
        assert_eq!(j.as_str(), Some("a\"b\\c\ndA"));
    }

    #[test]
    fn anidamiento() {
        let j = parse(br#"{"m": {"modelos": {"x": {"huella_tiles": 3.0}}}}"#).expect("no parseo");
        let v = j
            .get("m")
            .and_then(|m| m.get("modelos"))
            .and_then(|m| m.get("x"))
            .and_then(|x| x.get("huella_tiles"))
            .and_then(|h| h.as_f64());
        assert_eq!(v, Some(3.0));
    }

    #[test]
    fn json_malo_no_entra() {
        assert!(parse(b"{").is_none());
        assert!(parse(b"{\"a\":}").is_none());
        assert!(parse(b"[1, 2").is_none());
        assert!(parse(b"nulo").is_none());
    }
}
