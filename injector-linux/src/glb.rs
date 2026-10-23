//! Lector de GLB (glTF binario) sin dependencias.
//!
//! Los modelos optimizados que usa el visor VR (`android/src/main/assets/models`)
//! son GLB con la jerarquia de nodos aplanada, el color horneado en los
//! vertices (`COLOR_0`) y **sin texturas** (las texturas se sacaron a PNG
//! sueltos).  Por eso aqui solo hace falta:
//!
//!   * el chunk JSON (la escena),
//!   * el chunk BIN (los datos),
//!   * los accessors de `POSITION`, `NORMAL` y `COLOR_0`, y los indices.
//!
//! Se soporta tambien la jerarquia de nodos (TRS) por si algun modelo la
//! trae: la transformacion se hornea en los vertices.
//!
//! # Formato de la malla
//!
//! Vertices intercalados: `posicion(3) normal(3) color(4)` = 10 `f32`.
//! Los indices son `u32` (se amplia el `u16` del fichero).

use crate::json::Json;

/// Floats por vertice (posicion + normal + color).
pub const FLOATS_POR_VERTICE: usize = 10;

/// Bytes por vertice.
pub const BYTES_POR_VERTICE: usize = FLOATS_POR_VERTICE * 4;

/// Una malla (un primitivo del GLB) ya en memoria.
#[derive(Clone, Debug, Default)]
pub struct Malla {
    /// Vertices intercalados: pos(3) nrm(3) col(4).
    pub vertices: Vec<f32>,
    /// Indices de los triangulos.
    pub indices: Vec<u32>,
}

/// Un modelo completo (el GLB entero).
#[derive(Clone, Debug, Default)]
pub struct Modelo {
    /// Mallas del modelo (una por primitivo).
    pub mallas: Vec<Malla>,
    /// Caja que envuelve al modelo, en las unidades del GLB.
    pub bbox_min: [f32; 3],
    /// Caja que envuelve al modelo, en las unidades del GLB.
    pub bbox_max: [f32; 3],
}

impl Modelo {
    /// Numero de triangulos del modelo.
    pub fn triangulos(&self) -> usize {
        self.mallas.iter().map(|m| m.indices.len() / 3).sum()
    }

    /// Numero de vertices del modelo.
    pub fn vertices(&self) -> usize {
        self.mallas.iter().map(|m| m.vertices.len() / FLOATS_POR_VERTICE).sum()
    }
}

/// Carga un GLB desde memoria.
///
/// Devuelve `None` si el fichero no es un GLB valido o si no se puede leer
/// ninguna malla.
pub fn cargar(bytes: &[u8]) -> Option<Modelo> {
    let (json, bin) = trocear(bytes)?;
    let raiz = crate::json::parse(&json)?;
    Some(construir(&raiz, &bin))
}

/// Trocea el GLB en (chunk JSON, chunk BIN).
fn trocear(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
        return None;
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != 2 {
        return None;
    }
    let mut json: Vec<u8> = Vec::new();
    let mut bin: Vec<u8> = Vec::new();
    let mut off = 12usize;
    while off + 8 <= bytes.len() {
        let largo = u32::from_le_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]) as usize;
        let tipo = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]);
        off += 8;
        if off + largo > bytes.len() {
            break;
        }
        let trozo = &bytes[off..off + largo];
        match tipo {
            0x4E4F_534A => {
                // El chunk JSON va relleno con espacios (o con ceros, si el
                // generador no lo hizo bien): se quita el relleno para que el
                // parser no lo rechace.
                let mut fin = trozo.len();
                while fin > 0 && (trozo[fin - 1] == 0 || trozo[fin - 1] == b' ') {
                    fin -= 1;
                }
                json = trozo[..fin].to_vec();
            }
            0x004E_4942 => bin = trozo.to_vec(), // BIN
            _ => {}
        }
        off += largo;
        // Los chunks van alineados a 4 bytes.
        off = (off + 3) & !3;
    }
    if json.is_empty() {
        return None;
    }
    Some((json, bin))
}

/// Construye el modelo a partir del JSON y del chunk binario.
fn construir(raiz: &Json, bin: &[u8]) -> Modelo {
    let mut modelo = Modelo::default();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];

    // Matrices de los nodos (world matrix de cada nodo).
    let matrices = matrices_de_nodos(raiz);

    let mallas = match raiz.get("meshes").and_then(|m| m.as_arr()) {
        Some(m) => m,
        None => return modelo,
    };

    for (indice_malla, malla) in mallas.iter().enumerate() {
        let primitivas = match malla.get("primitives").and_then(|p| p.as_arr()) {
            Some(p) => p,
            None => continue,
        };
        for prim in primitivas {
            let attrs = match prim.get("attributes") {
                Some(a) => a,
                None => continue,
            };
            let pos_acc = match attrs.get("POSITION").and_then(|a| a.as_i64()) {
                Some(a) => a,
                None => continue,
            };
            let pos = leer_vectores(raiz, bin, pos_acc, 3);
            if pos.is_empty() {
                continue;
            }
            let n = pos.len() / 3;
            let nrm = match attrs.get("NORMAL").and_then(|a| a.as_i64()) {
                Some(a) => leer_vectores(raiz, bin, a, 3),
                None => vec![0.0; n * 3],
            };
            let col = match attrs.get("COLOR_0").and_then(|a| a.as_i64()) {
                Some(a) => leer_vectores(raiz, bin, a, 4),
                None => vec![1.0; n * 4],
            };

            // Transformacion del nodo que usa esta malla.
            let matriz = matrices
                .iter()
                .find(|(malla_idx, _)| *malla_idx == indice_malla)
                .map(|(_, m)| *m)
                .unwrap_or(IDENTIDAD);

            let mut vertices = vec![0f32; n * FLOATS_POR_VERTICE];
            for v in 0..n {
                let mut p = [
                    pos[v * 3],
                    pos[v * 3 + 1],
                    pos[v * 3 + 2],
                ];
                let mut nr = [
                    nrm.get(v * 3).copied().unwrap_or(0.0),
                    nrm.get(v * 3 + 1).copied().unwrap_or(0.0),
                    nrm.get(v * 3 + 2).copied().unwrap_or(0.0),
                ];
                // La rotacion/escala se aplica tambien a la normal (sin la
                // traslacion); como el rotulo es rigido, el mismo producto
                // sirve (la escala de las normales es la del modelo).
                let p2 = transformar(&matriz, p);
                let n2 = transformar_sin_traslacion(&matriz, nr);
                p = p2;
                nr = n2;
                let base = v * FLOATS_POR_VERTICE;
                vertices[base] = p[0];
                vertices[base + 1] = p[1];
                vertices[base + 2] = p[2];
                vertices[base + 3] = nr[0];
                vertices[base + 4] = nr[1];
                vertices[base + 5] = nr[2];
                vertices[base + 6] = col.get(v * 4).copied().unwrap_or(1.0);
                vertices[base + 7] = col.get(v * 4 + 1).copied().unwrap_or(1.0);
                vertices[base + 8] = col.get(v * 4 + 2).copied().unwrap_or(1.0);
                vertices[base + 9] = col.get(v * 4 + 3).copied().unwrap_or(1.0);
                for k in 0..3 {
                    if p[k] < min[k] {
                        min[k] = p[k];
                    }
                    if p[k] > max[k] {
                        max[k] = p[k];
                    }
                }
            }

            // Indices.
            let mut indices = Vec::new();
            if let Some(acc) = prim.get("indices").and_then(|i| i.as_i64()) {
                let crudos = leer_indices(raiz, bin, acc);
                indices.reserve(crudos.len());
                for i in crudos {
                    indices.push(i);
                }
            } else {
                indices.extend(0..n as u32);
            }

            modelo.mallas.push(Malla { vertices, indices });
        }
    }

    if !min[0].is_finite() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    modelo.bbox_min = min;
    modelo.bbox_max = max;
    modelo
}

/// Matriz identidad (column-major, como GL).
const IDENTIDAD: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// Matrices de los nodos de la escena (world matrix de cada nodo).
///
/// Se recorre la escena y se acumulan las transformaciones.  Si el GLB es
/// plano (un solo nodo sin transformacion) sale la identidad, que es el caso
/// de los modelos optimizados.
fn matrices_de_nodos(raiz: &Json) -> Vec<(usize, [f32; 16])> {
    let nodos = match raiz.get("nodes").and_then(|n| n.as_arr()) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let escena = raiz
        .get("scenes")
        .and_then(|s| s.as_arr())
        .and_then(|s| s.get(raiz.get("scene").and_then(|i| i.as_i64()).unwrap_or(0) as usize))
        .and_then(|s| s.get("nodes"))
        .and_then(|n| n.as_arr());
    let raices: Vec<usize> = match escena {
        Some(n) => n.iter().filter_map(|v| v.as_i64()).map(|v| v as usize).collect(),
        None => (0..nodos.len()).collect(),
    };

    let mut salida: Vec<(usize, [f32; 16])> = Vec::new();
    for r in raices {
        recorrer(nodos, r, IDENTIDAD, &mut salida);
    }
    salida
}

/// Recorre un nodo y sus hijos, acumulando la transformacion.
fn recorrer(nodos: &[Json], indice: usize, padre: [f32; 16], salida: &mut Vec<(usize, [f32; 16])>) {
    let nodo = match nodos.get(indice) {
        Some(n) => n,
        None => return,
    };
    let local = matriz_local(nodo);
    let mundo = multiplicar(&padre, &local);
    if let Some(malla) = nodo.get("mesh").and_then(|m| m.as_i64()) {
        salida.push((malla as usize, mundo));
    }
    if let Some(hijos) = nodo.get("children").and_then(|c| c.as_arr()) {
        for h in hijos {
            if let Some(h) = h.as_i64() {
                recorrer(nodos, h as usize, mundo, salida);
            }
        }
    }
}

/// Matriz local de un nodo (TRS).
fn matriz_local(nodo: &Json) -> [f32; 16] {
    let t = nodo
        .get("translation")
        .and_then(|v| v.as_arr())
        .map(|a| {
            [
                a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                a.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            ]
        })
        .unwrap_or([0.0; 3]);
    let r = nodo
        .get("rotation")
        .and_then(|v| v.as_arr())
        .map(|a| {
            [
                a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                a.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                a.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            ]
        })
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = nodo
        .get("scale")
        .and_then(|v| v.as_arr())
        .map(|a| {
            [
                a.first().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                a.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                a.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            ]
        })
        .unwrap_or([1.0; 3]);

    // R (cuaternion) -> matriz 3x3.
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let rot = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
    ];
    let mut m = IDENTIDAD;
    for col in 0..3 {
        for fila in 0..3 {
            m[col * 4 + fila] = rot[col * 3 + fila] * s[col];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

/// Multiplica dos matrices 4x4 (column-major): `a * b`.
fn multiplicar(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for fila in 0..4 {
            let mut s = 0f32;
            for k in 0..4 {
                s += a[k * 4 + fila] * b[col * 4 + k];
            }
            out[col * 4 + fila] = s;
        }
    }
    out
}

/// Aplica la matriz a un punto (con traslacion).
fn transformar(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Aplica la rotacion y la escala de la matriz a una normal (sin traslacion).
fn transformar_sin_traslacion(m: &[f32; 16], n: [f32; 3]) -> [f32; 3] {
    let mut r = [
        m[0] * n[0] + m[4] * n[1] + m[8] * n[2],
        m[1] * n[0] + m[5] * n[1] + m[9] * n[2],
        m[2] * n[0] + m[6] * n[1] + m[10] * n[2],
    ];
    let largo = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    if largo > 1e-6 {
        r[0] /= largo;
        r[1] /= largo;
        r[2] /= largo;
    }
    r
}

/// Lee un accessor de vectores de `componentes` floats.
fn leer_vectores(raiz: &Json, bin: &[u8], accessor: i64, componentes: usize) -> Vec<f32> {
    let accs = match raiz.get("accessors").and_then(|a| a.as_arr()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let acc = match accs.get(accessor as usize) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let count = acc.get("count").and_then(|c| c.as_i64()).unwrap_or(0) as usize;
    let tipo = acc.get("componentType").and_then(|c| c.as_i64()).unwrap_or(0);
    let normalizado = acc.get("normalized").and_then(|n| n.as_bool()).unwrap_or(false);
    let vista = match acc.get("bufferView").and_then(|v| v.as_i64()) {
        Some(v) => v as usize,
        None => return Vec::new(),
    };
    let vistas = match raiz.get("bufferViews").and_then(|v| v.as_arr()) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let vista = match vistas.get(vista) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let offset_vista = vista.get("byteOffset").and_then(|o| o.as_i64()).unwrap_or(0) as usize;
    let inicio = offset_vista + acc.get("byteOffset").and_then(|o| o.as_i64()).unwrap_or(0) as usize;
    let tam = (tipo_tam(tipo) * componentes) as usize;

    let mut out = Vec::with_capacity(count * componentes);
    for i in 0..count {
        let off = inicio + i * tam;
        if off + tam > bin.len() {
            break;
        }
        for c in 0..componentes {
            let v = leer_escalar(bin, off + c * tam_tipo(tipo), tipo, normalizado);
            out.push(v);
        }
    }
    out
}

/// Lee un accessor de indices.
fn leer_indices(raiz: &Json, bin: &[u8], accessor: i64) -> Vec<u32> {
    let accs = match raiz.get("accessors").and_then(|a| a.as_arr()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let acc = match accs.get(accessor as usize) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let count = acc.get("count").and_then(|c| c.as_i64()).unwrap_or(0) as usize;
    let tipo = acc.get("componentType").and_then(|c| c.as_i64()).unwrap_or(0);
    let vista = match acc.get("bufferView").and_then(|v| v.as_i64()) {
        Some(v) => v as usize,
        None => return Vec::new(),
    };
    let vistas = match raiz.get("bufferViews").and_then(|v| v.as_arr()) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let vista = match vistas.get(vista) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let offset_vista = vista.get("byteOffset").and_then(|o| o.as_i64()).unwrap_or(0) as usize;
    let inicio = offset_vista + acc.get("byteOffset").and_then(|o| o.as_i64()).unwrap_or(0) as usize;
    let paso = tam_tipo(tipo);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = inicio + i * paso;
        if off + paso > bin.len() {
            break;
        }
        out.push(leer_escalar(bin, off, tipo, false) as u32);
    }
    out
}

/// Tamano en bytes de un tipo de componente.
fn tam_tipo(tipo: i64) -> usize {
    match tipo {
        5120 | 5121 => 1, // BYTE, UNSIGNED_BYTE
        5122 | 5123 => 2, // SHORT, UNSIGNED_SHORT
        5125 | 5126 => 4, // UNSIGNED_INT, FLOAT
        _ => 4,
    }
}

/// Tamano en bytes de un accessor con `componentes` componentes.
fn tipo_tam(tipo: i64) -> usize {
    tam_tipo(tipo)
}

/// Lee un escalar del buffer.
fn leer_escalar(bin: &[u8], off: usize, tipo: i64, normalizado: bool) -> f32 {
    match tipo {
        5120 => {
            let v = bin[off] as i8 as f32;
            if normalizado {
                v / 127.0
            } else {
                v
            }
        }
        5121 => {
            let v = bin[off] as f32;
            if normalizado {
                v / 255.0
            } else {
                v
            }
        }
        5122 => {
            let v = i16::from_le_bytes([bin[off], bin[off + 1]]) as f32;
            if normalizado {
                v / 32767.0
            } else {
                v
            }
        }
        5123 => {
            let v = u16::from_le_bytes([bin[off], bin[off + 1]]) as f32;
            if normalizado {
                v / 65535.0
            } else {
                v
            }
        }
        5125 => {
            let v = u32::from_le_bytes([
                bin[off],
                bin[off + 1],
                bin[off + 2],
                bin[off + 3],
            ]);
            v as f32
        }
        5126 => f32::from_le_bytes([
            bin[off],
            bin[off + 1],
            bin[off + 2],
            bin[off + 3],
        ]),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construye un GLB minimo en memoria (un triangulo con color).
    pub(crate) fn glb_de_prueba() -> Vec<u8> {
        let json = br#"{"asset":{"version":"2.0"},
            "accessors":[
              {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
              {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
              {"bufferView":2,"componentType":5123,"count":3,"normalized":true,"type":"VEC4"},
              {"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}],
            "bufferViews":[
              {"buffer":0,"byteOffset":0,"byteLength":36},
              {"buffer":0,"byteOffset":36,"byteLength":36},
              {"buffer":0,"byteOffset":72,"byteLength":24},
              {"buffer":0,"byteOffset":96,"byteLength":6}],
            "buffers":[{"byteLength":102}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"COLOR_0":2},"indices":3}]}],
            "nodes":[{"mesh":0}],
            "scenes":[{"nodes":[0]}],"scene":0}"#;

        let mut bin: Vec<u8> = Vec::new();
        // posiciones
        for v in [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ] {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        // normales
        for _ in 0..3 {
            bin.extend_from_slice(&0.0f32.to_le_bytes());
            bin.extend_from_slice(&0.0f32.to_le_bytes());
            bin.extend_from_slice(&1.0f32.to_le_bytes());
        }
        // colores (ushort normalizados): rojo, verde, azul (3 vertices)
        for _ in 0..3 {
            for c in [65535u16, 0, 0, 65535] {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        // indices
        for i in [0u16, 1, 2] {
            bin.extend_from_slice(&i.to_le_bytes());
        }

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // longitud, se ajusta luego
        let json_pad = (4 - (json.len() % 4)) % 4;
        let bin_pad = (4 - (bin.len() % 4)) % 4;
        out.extend_from_slice(&(json.len() as u32 + json_pad as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        out.extend_from_slice(json);
        out.extend(std::iter::repeat(0u8).take(json_pad));
        out.extend_from_slice(&(bin.len() as u32 + bin_pad as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(&bin);
        out.extend(std::iter::repeat(0u8).take(bin_pad));
        let total = out.len() as u32;
        out[8..12].copy_from_slice(&total.to_le_bytes());
        out
    }

    #[test]
    fn lee_una_malla() {
        let m = cargar(&glb_de_prueba()).expect("no cargo el GLB");
        assert_eq!(m.mallas.len(), 1);
        assert_eq!(m.vertices(), 3);
        assert_eq!(m.triangulos(), 1);
        let v = &m.mallas[0].vertices;
        assert_eq!(v.len(), 3 * FLOATS_POR_VERTICE);
        // posicion del primer vertice
        assert!((v[0] - 0.0).abs() < 1e-6);
        assert!((v[1] - 0.0).abs() < 1e-6);
        // normal del primer vertice (0,0,1)
        assert!((v[5] - 1.0).abs() < 1e-6);
        // color del primer vertice: rojo opaco
        assert!((v[6] - 1.0).abs() < 1e-3);
        assert!((v[7] - 0.0).abs() < 1e-3);
        assert!((v[9] - 1.0).abs() < 1e-3);
        // indices
        assert_eq!(m.mallas[0].indices, vec![0, 1, 2]);
        // bbox
        assert!((m.bbox_max[0] - 1.0).abs() < 1e-6);
        assert!((m.bbox_max[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lo_que_no_es_glb_no_entra() {
        assert!(cargar(b"no soy un glb").is_none());
        assert!(cargar(&[]).is_none());
    }

    #[test]
    fn carga_todos_los_modelos_del_repositorio() {
        // Todos los GLB que se empaquetan en el APK tienen que cargar (si el
        // repositorio no esta a mano, el test no dice nada).
        let dir = format!(
            "{}/../android/src/main/assets/models",
            env!("CARGO_MANIFEST_DIR")
        );
        let entradas = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut n = 0usize;
        for e in entradas.flatten() {
            let ruta = e.path();
            if ruta.extension().map(|x| x != "glb").unwrap_or(true) {
                continue;
            }
            let bytes = match std::fs::read(&ruta) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let m = match cargar(&bytes) {
                Some(m) => m,
                None => panic!("no se cargo {}", ruta.display()),
            };
            assert!(
                m.vertices() > 0 && m.triangulos() > 0,
                "{} salio vacio",
                ruta.display()
            );
            // La caja tiene que ser razonable (los modelos estan en metros).
            assert!(
                m.bbox_max[0] - m.bbox_min[0] > 0.0,
                "{} sin anchura",
                ruta.display()
            );
            n += 1;
        }
        assert!(n >= 20, "solo se probaron {n} modelos");
    }

    #[test]
    fn carga_el_modelo_real_del_repositorio() {
        // Solo si el repositorio esta a mano (el test no debe fallar en un
        // entorno sin los assets).
        let ruta = format!(
            "{}/../android/src/main/assets/models/iron-chest.glb",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = match std::fs::read(&ruta) {
            Ok(b) => b,
            Err(_) => return,
        };
        let m = cargar(&bytes).expect("no cargo el GLB real");
        assert!(m.vertices() > 100, "el modelo real tiene pocos vertices");
        assert!(m.triangulos() > 50);
        assert!(m.bbox_max[0] > m.bbox_min[0]);
    }
}

