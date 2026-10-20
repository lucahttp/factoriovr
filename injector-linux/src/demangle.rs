//! Des-mangling de nombres C++ (ABI Itanium) — subconjunto suficiente para
//! resolver los simbolos del motor de Factorio.
//!
//! El motor de Factorio para Linux ARM64 NO va stripeado: su `.symtab` tiene
//! 148.802 simbolos con nombre mangled (`_ZN12GameRenderer6renderEv`).
//! Para enganchar "por nombre" (igual que el upstream hace con el PDB de
//! Windows) hay que traducir ese nombre mangled a "GameRenderer::render".
//!
//! El des-mangler es *aproximado pero total*: nunca entra en panic ni se
//! queda en bucle; si encuentra algo que no entiende, devuelve `None` y el
//! llamante sigue con el nombre mangled. Lo que nos importa es el camino de
//! clases (`Owner::metodo`), que se construye siempre con nombres literales
//! del mangling, asi que es exacto.
//!
//! Referencia: Itanium C++ ABI, seccion " mangling" (nombres anidados,
//! sustituciones `S<n>_`, argumentos de plantilla `I...E`).

/// Tipos predefinidos del ABI (un caracter).
const BUILTIN: &[(&str, &str)] = &[
    ("v", "void"),
    ("w", "wchar_t"),
    ("b", "bool"),
    ("c", "char"),
    ("a", "signed char"),
    ("h", "unsigned char"),
    ("s", "short"),
    ("t", "unsigned short"),
    ("i", "int"),
    ("j", "unsigned int"),
    ("l", "long"),
    ("m", "unsigned long"),
    ("x", "long long"),
    ("y", "unsigned long long"),
    ("n", "__int128"),
    ("o", "unsigned __int128"),
    ("f", "float"),
    ("d", "double"),
    ("e", "long double"),
    ("g", "__float128"),
    ("z", "..."),
];

/// Codigos de operadores de dos letras (los mas usados).
const OPERATORS: &[(&str, &str)] = &[
    ("nw", "operator new"),
    ("na", "operator new[]"),
    ("dl", "operator delete"),
    ("da", "operator delete[]"),
    ("ps", "operator+"),
    ("ng", "operator-"),
    ("ad", "operator&"),
    ("de", "operator*"),
    ("co", "operator~"),
    ("pl", "operator+"),
    ("mi", "operator-"),
    ("ml", "operator*"),
    ("dv", "operator/"),
    ("rm", "operator%"),
    ("an", "operator&"),
    ("or", "operator|"),
    ("eo", "operator^"),
    ("aS", "operator="),
    ("pL", "operator+="),
    ("mI", "operator-="),
    ("mL", "operator*="),
    ("dV", "operator/="),
    ("rM", "operator%="),
    ("aN", "operator&="),
    ("oR", "operator|="),
    ("eO", "operator^="),
    ("ls", "operator<<"),
    ("rs", "operator>>"),
    ("lS", "operator<<="),
    ("rS", "operator>>="),
    ("eq", "operator=="),
    ("ne", "operator!="),
    ("lt", "operator<"),
    ("gt", "operator>"),
    ("le", "operator<="),
    ("ge", "operator>="),
    ("ss", "operator<=>"),
    ("nt", "operator!"),
    ("aa", "operator&&"),
    ("oo", "operator||"),
    ("pp", "operator++"),
    ("mm", "operator--"),
    ("cm", "operator,"),
    ("pm", "operator->*"),
    ("pt", "operator->"),
    ("cl", "operator()"),
    ("ix", "operator[]"),
    ("qu", "operator?"),
    ("cv", "operator"),
];

/// Nombre des-mangleado de un simbolo.
pub struct Demangled {
    /// Nombre completo con argumentos y calificadores:
    /// "Entity::getPrototype() const".
    pub full: String,
    /// Nombre cualificado sin argumentos ni calificadores:
    /// "Entity::getPrototype". Es la clave que se usa para resolver.
    pub qualified: String,
}

/// Traduce un nombre mangled a su forma legible.
///
/// Devuelve `None` si el nombre no sigue el ABI de Itanium o si el parser se
/// queda sin gas (cosa que solo pasa con construcciones exoticas que no
/// necesitamos).
pub fn demangle(mangled: &str) -> Option<Demangled> {
    let body = mangled.strip_prefix("_Z")?;
    let mut p = Parser::new(body.as_bytes());
    let (components, is_const) = p.parse_name()?;

    // Nombre cualificado: el ultimo componente es la funcion, el resto el ambito.
    let qualified = if components.is_empty() {
        return None;
    } else {
        components.join("::")
    };

    // Parametros (best effort). El ABI de Itanium no codifica el tipo de
    // retorno cuando es void, asi que la lista que queda son los parametros
    // (igual que muestra `nm -C`).
    let mut tipos = Vec::new();
    while p.i < p.b.len() {
        match p.parse_type() {
            Some(t) => tipos.push(t),
            None => break,
        }
    }
    if tipos.len() == 1 && tipos[0] == "void" {
        tipos.clear();
    }
    let params = tipos;

    let mut full = format!("{qualified}({})", params.join(", "));
    if is_const {
        full.push_str(" const");
    }

    Some(Demangled { full, qualified })
}

/// Parser recursivo descendente del mangling.
struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    /// Tabla de sustituciones del ABI: los componentes ya parseados.
    subs: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(b: &'a [u8]) -> Self {
        Parser { b, i: 0, subs: Vec::new() }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Option<()> {
        if self.eat(c) {
            Some(())
        } else {
            None
        }
    }

    /// Numero decimal en base 10.
    fn number(&mut self) -> Option<usize> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return None;
        }
        std::str::from_utf8(&self.b[start..self.i]).ok()?.parse::<usize>().ok()
    }

    /// `<source-name>`: numero de longitud seguido del identificador.
    fn source_name(&mut self) -> Option<String> {
        let len = self.number()?;
        if self.i + len > self.b.len() {
            return None;
        }
        let s = std::str::from_utf8(&self.b[self.i..self.i + len]).ok()?.to_string();
        self.i += len;
        Some(s)
    }

    fn push_sub(&mut self, s: String) {
        self.subs.push(s);
    }

    /// Sustitucion `S_` (indice 0) o `S<seq-id>_` (indice seq-id + 1).
    fn parse_substitution(&mut self) -> Option<String> {
        self.expect(b'S')?;
        let idx = if self.eat(b'_') {
            0
        } else {
            let n = self.number()?;
            self.expect(b'_')?;
            n + 1
        };
        self.subs.get(idx).cloned()
    }

    /// Nombre: anidado (`N...E`), no cualificado o sustitucion.
    /// Devuelve (componentes, es_const).
    fn parse_name(&mut self) -> Option<(Vec<String>, bool)> {
        match self.peek()? {
            b'N' => {
                self.i += 1;
                self.parse_nested()
            }
            b'S' => {
                let s = self.parse_substitution()?;
                Some((vec![s], false))
            }
            b'Z' => None, // nombre local: no lo soportamos
            c if c.is_ascii_digit() => {
                let n = self.source_name()?;
                Some((vec![n], false))
            }
            _ => {
                let op = self.parse_operator()?;
                Some((vec![op], false))
            }
        }
    }

    /// Nombre anidado `N [cv] <prefijo>... E`.
    fn parse_nested(&mut self) -> Option<(Vec<String>, bool)> {
        let mut is_const = false;
        loop {
            match self.peek()? {
                b'K' => {
                    self.i += 1;
                    is_const = true;
                }
                b'V' | b'r' | b'R' => {
                    self.i += 1;
                }
                _ => break,
            }
        }

        let mut componentes = Vec::new();
        loop {
            match self.peek()? {
                b'E' => {
                    self.i += 1;
                    break;
                }
                _ => {
                    let c = self.parse_prefix()?;
                    componentes.push(c);
                }
            }
        }
        Some((componentes, is_const))
    }

    /// Un componente del prefijo: nombre fuente, plantilla o sustitucion.
    fn parse_prefix(&mut self) -> Option<String> {
        match self.peek()? {
            b'S' => self.parse_substitution(),
            b'I' => {
                let (args, _lista) = self.parse_template_args()?;
                Some(args)
            }
            b'L' | b'M' | b'T' | b'Z' => None,
            c if c.is_ascii_digit() => {
                let nombre = self.source_name()?;
                if self.peek() == Some(b'I') {
                    let (args, lista) = self.parse_template_args()?;
                    let completo = format!("{nombre}{args}");
                    // Orden del ABI: primero el prefijo de plantilla, luego sus args.
                    self.push_sub(completo.clone());
                    for a in lista {
                        self.push_sub(a);
                    }
                    Some(completo)
                } else {
                    self.push_sub(nombre.clone());
                    Some(nombre)
                }
            }
            _ => {
                let op = self.parse_operator()?;
                self.push_sub(op.clone());
                Some(op)
            }
        }
    }

    /// Argumentos de plantilla `I <arg>+ E`.
    /// Devuelve (texto, lista de args para la tabla de sustituciones).
    fn parse_template_args(&mut self) -> Option<(String, Vec<String>)> {
        self.expect(b'I')?;
        let mut args = Vec::new();
        loop {
            match self.peek()? {
                b'E' => {
                    self.i += 1;
                    break;
                }
                _ => {
                    let a = self.parse_type()?;
                    args.push(a);
                }
            }
        }
        Some((format!("<{}>", args.join(", ")), args))
    }

    /// Nombre de operador.
    fn parse_operator(&mut self) -> Option<String> {
        let a = self.peek()?;
        let b = self.b.get(self.i + 1).copied()?;
        let code = &self.b[self.i..self.i + 2];
        self.i += 2;
        if let Some((_, txt)) = OPERATORS.iter().find(|(k, _)| k.as_bytes() == code) {
            return Some((*txt).to_string());
        }
        // Operadores "vendor extended" (v<digit><num>) y desconocida.
        if a == b'v' && b.is_ascii_digit() {
            let n = self.number()?;
            return Some(format!("operator#{n}"));
        }
        None
    }

    /// Tipo del ABI (simplificado: suficiente para pintar argumentos).
    fn parse_type(&mut self) -> Option<String> {
        match self.peek()? {
            c if c.is_ascii_digit() => {
                let nombre = self.source_name()?;
                if self.peek() == Some(b'I') {
                    let (args, lista) = self.parse_template_args()?;
                    let completo = format!("{nombre}{args}");
                    self.push_sub(completo.clone());
                    for a in lista {
                        self.push_sub(a);
                    }
                    Some(completo)
                } else {
                    self.push_sub(nombre.clone());
                    Some(nombre)
                }
            }
            b'P' => {
                self.i += 1;
                let t = self.parse_type()?;
                Some(format!("{t}*"))
            }
            b'R' => {
                self.i += 1;
                let t = self.parse_type()?;
                Some(format!("{t}&"))
            }
            b'O' => {
                self.i += 1;
                let t = self.parse_type()?;
                Some(format!("{t}&&"))
            }
            b'K' => {
                self.i += 1;
                let t = self.parse_type()?;
                Some(format!("const {t}"))
            }
            b'V' => {
                self.i += 1;
                let t = self.parse_type()?;
                Some(format!("volatile {t}"))
            }
            b'A' => {
                self.i += 1;
                let _ = self.number();
                self.expect(b'_')?;
                let t = self.parse_type()?;
                Some(format!("{t}[]"))
            }
            b'S' => self.parse_substitution(),
            b'I' => {
                let (args, _) = self.parse_template_args()?;
                Some(args)
            }
            b'u' => {
                self.i += 1;
                self.source_name()
            }
            b'T' => {
                // parametro de plantilla: T_<n>_
                self.i += 1;
                let n = self.number().unwrap_or(0);
                self.expect(b'_')?;
                Some(format!("T{n}"))
            }
            b'M' => {
                // puntero a miembro: M <clase> <miembro>
                self.i += 1;
                let c = self.parse_type()?;
                let m = self.parse_type()?;
                Some(format!("{c}::* {m}"))
            }
            b'F' => {
                self.i += 1;
                if self.eat(b'Y') {
                    // tipo de retorno explicito
                    let _ = self.parse_type();
                }
                let mut params = Vec::new();
                while let Some(c) = self.peek() {
                    if c == b'E' {
                        self.i += 1;
                        break;
                    }
                    match self.parse_type() {
                        Some(t) => params.push(t),
                        None => break,
                    }
                }
                Some(format!("{}()", params.join(", ")))
            }
            b'C' => {
                // constructor/destructor: C1/C2/C3 + tipo
                self.i += 1;
                let _ = self.number();
                let t = self.parse_type()?;
                Some(t)
            }
            b'D' => {
                // Dp/Dt/Da + tipo
                self.i += 1;
                let _ = self.eat(b'p');
                let _ = self.eat(b't');
                let _ = self.eat(b'a');
                let t = self.parse_type()?;
                Some(t)
            }
            c => {
                let codigo = [c];
                let txt = BUILTIN.iter().find(|(k, _)| k.as_bytes() == codigo)?;
                self.i += 1;
                Some((*txt.1).to_string())
            }
        }
    }
}
