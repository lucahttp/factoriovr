//! stream.rs — Mandar la imagen REAL del juego al visor VR.
//!
//! El visor no dibuja una reconstruccion con sprites: para que se vea el
//! juego de verdad (y el cubo 3D que pinta el inyector) hace falta que la
//! imagen que compone el motor llegue al visor. El inyector ya sabe leer el
//! framebuffer del mundo en el momento del flush (`modelo3d.rs`), asi que
//! desde ahi se codifica el frame y se manda por TCP al visor, que lo pinta
//! como un panel en MR.
//!
//! # Protocolo "XFV1"
//!
//! TCP, todo en little-endian. El emisor (el inyector) es el CLIENTE y el
//! visor es el SERVIDOR: asi el visor puede arrancar y parar sin que el juego
//! se entere, y el juego puede reconectar solo si el visor se reinicia.
//!
//! Cada frame es:
//!
//! ```text
//!   cabecera (16 bytes)
//!     0..4    magic   u32 = 0x31564658  ("XFV1")
//!     4..8    numero de frame  u32
//!     8..10   ancho   u16
//!     10..12  alto    u16
//!     12      flags   u8   (bit 0: el ojo al que pertenece el frame: 0 = izquierdo, 1 = derecho)
//!     13      formato u8   (0 = RGBA8888)
//!     14..16  reservado u16
//!   carga: ancho * alto * 4 bytes RGBA
//! ```
//!
//! Las filas van en orden de GL (la fila 0 es la de ABAJO de la imagen): al
//! subirlo como textura seSUBE tal cual y el shader invierte la V.
//!
//! # Variables de entorno
//!
//! * `XFACTORVR_STREAM` — "host:puerto" del visor. Si no esta, no se
//!   transmite nada (el inyector hace lo de siempre).
//! * `XFACTORVR_STREAM_ANCHO` — ancho objetivo del frame (por defecto 640).
//! * `XFACTORVR_STREAM_ALTO` — alto objetivo (por defecto 360).
//! * `XFACTORVR_STREAM_FPS` — fps objetivo (por defecto 15).

use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::log;

/// "XFV1" en little-endian.
pub const MAGIC: u32 = 0x3156_4658;
/// Bytes de la cabecera.
pub const TAM_CABECERA: usize = 16;
/// RGBA de 8 bits por canal.
pub const FORMATO_RGBA8: u8 = 0;
/// La imagen va invertida verticalmente respecto a lo normal.
pub const FLAG_INVERTIDA: u8 = 1;

/// Configuracion de la transmision (leida del entorno).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// "host:puerto" del visor.
    pub destino: String,
    /// Ancho objetivo del frame.
    pub ancho: u32,
    /// Alto objetivo del frame.
    pub alto: u32,
    /// Frames por segundo objetivo.
    pub fps: u32,
}

impl Config {
    /// Lee la configuracion del entorno. `None` si no hay que transmitir.
    pub fn desde_entorno() -> Option<Config> {
        let destino = match std::env::var("XFACTORVR_STREAM") {
            Ok(v) if !v.is_empty() => v,
            _ => return None,
        };
        Some(Config {
            destino,
            ancho: env_u32("XFACTORVR_STREAM_ANCHO", 640),
            alto: env_u32("XFACTORVR_STREAM_ALTO", 360),
            fps: env_u32("XFACTORVR_STREAM_FPS", 15),
        })
    }
}

/// Lee un entero del entorno con valor por defecto.
fn env_u32(nombre: &str, por_defecto: u32) -> u32 {
    match std::env::var(nombre) {
        Ok(v) => match v.trim().parse::<u32>() {
            Ok(n) if n > 0 => n,
            _ => por_defecto,
        },
        Err(_) => por_defecto,
    }
}

/// Estado global de la transmision.
struct Estado {
    config: Mutex<Option<Config>>,
    /// Canal al hilo emisor. `None` = no hay nadie escuchando todavia.
    canal: Mutex<Option<SyncSender<Vec<u8>>>>,
    /// Momento del ultimo frame enviado (para el limite de fps).
    ultimo: Mutex<Option<Instant>>,
    /// Numero de frame (contador).
    contador: Mutex<u32>,
    /// Frames que se han enviado de verdad.
    enviados: Mutex<u64>,
    /// Veces que no se pudo enviar porque el canal estaba lleno.
    perdidos: Mutex<u64>,
    /// Frames descartados por ir vacios (pantalla en negro).
    vacias: AtomicU64,
}

/// Estado global (se inicializa la primera vez).
static ESTADO: OnceLock<Estado> = OnceLock::new();

fn estado() -> &'static Estado {
    ESTADO.get_or_init(|| Estado {
        config: Mutex::new(None),
        canal: Mutex::new(None),
        ultimo: Mutex::new(None),
        contador: Mutex::new(0),
        enviados: Mutex::new(0),
        perdidos: Mutex::new(0),
        vacias: AtomicU64::new(0),
    })
}

/// Prepara la transmision: lee el entorno y arranca el hilo emisor.
///
/// Se puede llamar mas de una vez; la segunda no hace nada (la primera deja
/// el estado ya montado).
pub fn configurar() {
    let cfg = match Config::desde_entorno() {
        Some(c) => c,
        None => return,
    };
    let e = estado();
    {
        let mut guardado = e.config.lock().unwrap_or_else(|p| p.into_inner());
        if guardado.is_some() {
            return;
        }
        *guardado = Some(cfg.clone());
    }

    // El canal lleva el frame YA empaquetado (cabecera + carga). Capacidad
    // 2: si el visor va lento se descartan los frames viejos en vez de
    // acumular retardos (la imagen tiene que ser la ULTIMA, no la mas vieja).
    let (tx, rx) = sync_channel::<Vec<u8>>(2);
    {
        let mut canal = e.canal.lock().unwrap_or_else(|p| p.into_inner());
        *canal = Some(tx);
    }

    let destino = cfg.destino.clone();
    std::thread::Builder::new()
        .name("xfv-stream".to_string())
        .spawn(move || hilo_emisor(&destino, rx))
        .ok();
    log::info(&format!(
        "transmision activada hacia {} ({}x{} a {} fps)",
        cfg.destino, cfg.ancho, cfg.alto, cfg.fps
    ));
}

/// Hilo emisor: conecta (con reintentos) y escribe los frames.
///
/// Va por su cuenta para que el hilo de render del juego NUNCA se bloquee
/// escribiendo en el socket: si el visor va lento, se tira el frame.
fn hilo_emisor(destino: &str, rx: Receiver<Vec<u8>>) {
    let mut conexion: Option<TcpStream> = None;
    loop {
        if conexion.is_none() {
            match TcpStream::connect(destino) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    log::info(&format!("visor conectado en {destino}"));
                    conexion = Some(s);
                }
                Err(e) => {
                    log::aviso(&format!("no se pudo conectar con {destino}: {e}"));
                    std::thread::sleep(Duration::from_millis(1000));
                    continue;
                }
            }
        }

        // Se espera un frame. El timeout permite revisar la conexion.
        let paquete = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(p) => p,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let s = match conexion.as_mut() {
            Some(s) => s,
            None => continue,
        };
        if s.write_all(&paquete).is_err() {
            log::aviso("se perdio la conexion con el visor");
            conexion = None;
        }
    }
}

/// Hay transmision activada?
pub fn activo() -> bool {
    config().is_some()
}

/// Configuracion activa (si la hay).
pub fn config() -> Option<Config> {
    estado()
        .config
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Dice si ya toca enviar otro frame (limite de fps).
///
/// Se consulta ANTES de leer el framebuffer del juego, que es lo caro: si
/// todavia no toca, no se lee nada.
pub fn toca_enviar() -> bool {
    let e = estado();
    let fps = match config() {
        Some(c) => c.fps,
        None => return false,
    };
    if fps == 0 {
        return false;
    }
    let periodo = Duration::from_millis((1000u64 / fps.max(1) as u64).max(1));
    let mut ultimo = e.ultimo.lock().unwrap_or_else(|p| p.into_inner());
    match *ultimo {
        Some(t) if t.elapsed() < periodo => false,
        _ => {
            *ultimo = Some(Instant::now());
            true
        }
    }
}

/// Envia el frame (pixeles RGBA del framebuffer del mundo).
///
/// Reduce la imagen al tamano objetivo y la manda. Devuelve `true` si se
/// puso en manos del hilo emisor.
pub fn enviar(pix: &[u8], w: u32, h: u32, ojo: u8) -> bool {
    let cfg = match config() {
        Some(c) => c,
        None => return false,
    };
    let (w2, h2, datos) = match reducir(pix, w, h, cfg.ancho, cfg.alto) {
        Some(r) => r,
        None => return false,
    };

    let numero = {
        let mut c = estado().contador.lock().unwrap_or_else(|p| p.into_inner());
        *c = c.wrapping_add(1);
        *c
    };

    // Si la imagen esta practicamente vacia no se manda: el motor dibuja en
    // negro en certains momentos (carga, menus, transiciones) y en el visor
    // eso son parpadeos. Es mejor que se quede la ultima imagen buena.
    let con_contenido = fraccion_con_contenido(&datos);
    if con_contenido < UMBRAL_CONTENIDO {
        estado().vacias.fetch_add(1, Ordering::Relaxed);
        return false;
    }

    // El paquete es cabecera + carga: va entero al canal, el hilo emisor lo
    // escribe tal cual en el socket.
    //
    // flags bit 0: el ojo al que pertenece el frame (0 = izquierdo,
    // 1 = derecho).  Con la camara desplazada por ojo, el visor muestra cada
    // vista en el ojo que le toca (estereo real).
    let paquete = empaquetar(numero, w2, h2, ojo & 1, &datos);

    let canal = {
        let c = estado().canal.lock().unwrap_or_else(|p| p.into_inner());
        c.clone()
    };
    match canal {
        Some(tx) => {
            let r = tx.try_send(paquete);
            match r {
                Ok(()) => {
                    let mut e = estado().enviados.lock().unwrap_or_else(|p| p.into_inner());
                    *e += 1;
                    true
                }
                Err(TrySendError::Full(_)) => {
                    let mut p = estado().perdidos.lock().unwrap_or_else(|p| p.into_inner());
                    *p += 1;
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        }
        None => false,
    }
}

/// Escribe la cabecera del protocolo en un buffer.
pub fn escribir_cabecera(buf: &mut Vec<u8>, numero: u32, w: u32, h: u32, flags: u8) {
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&numero.to_le_bytes());
    buf.extend_from_slice(&(w as u16).to_le_bytes());
    buf.extend_from_slice(&(h as u16).to_le_bytes());
    buf.push(flags);
    buf.push(FORMATO_RGBA8);
    buf.extend_from_slice(&0u16.to_le_bytes());
}

/// Empaqueta cabecera + carga en un solo paquete (lo que viaja por el socket).
pub fn empaquetar(numero: u32, w: u32, h: u32, flags: u8, datos: &[u8]) -> Vec<u8> {
    let mut paquete = Vec::with_capacity(TAM_CABECERA + datos.len());
    escribir_cabecera(&mut paquete, numero, w, h, flags);
    paquete.extend_from_slice(datos);
    paquete
}

/// Reduce la imagen al tamano objetivo con un filtro de caja.
///
/// Devuelve `(ancho, alto, pixeles)`. Si la imagen ya es menor que el
/// objetivo, se devuelve tal cual (no se amplifica).
pub fn reducir(pix: &[u8], w: u32, h: u32, ancho_objetivo: u32, alto_objetivo: u32) -> Option<(u32, u32, Vec<u8>)> {
    if w == 0 || h == 0 {
        return None;
    }
    if pix.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    if w <= ancho_objetivo && h <= alto_objetivo {
        return Some((w, h, pix[..(w as usize) * (h as usize) * 4].to_vec()));
    }
    let fx = (w / ancho_objetivo.max(1)).max(1);
    let fy = (h / alto_objetivo.max(1)).max(1);
    let ow = w / fx;
    let oh = h / fy;
    if ow == 0 || oh == 0 {
        return None;
    }
    let mut salida = vec![0u8; (ow as usize) * (oh as usize) * 4];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in 0..fy {
                let y = oy * fy + sy;
                if y >= h {
                    break;
                }
                for sx in 0..fx {
                    let x = ox * fx + sx;
                    if x >= w {
                        break;
                    }
                    let i = ((y * w + x) * 4) as usize;
                    acc[0] += pix[i] as u32;
                    acc[1] += pix[i + 1] as u32;
                    acc[2] += pix[i + 2] as u32;
                    acc[3] += pix[i + 3] as u32;
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let o = ((oy * ow + ox) * 4) as usize;
            for c in 0..4 {
                salida[o + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    Some((ow, oh, salida))
}

/// Fraccion de pixeles con contenido (no totalmente negros) de una imagen.
///
/// Se usa para no mandar frames en negro: en el visor eso son parpadeos.
pub fn fraccion_con_contenido(pix: &[u8]) -> f32 {
    if pix.is_empty() {
        return 0.0;
    }
    let mut con = 0u64;
    let mut total = 0u64;
    for p in pix.chunks_exact(4) {
        total += 1;
        if p[0] != 0 || p[1] != 0 || p[2] != 0 {
            con += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        con as f32 / total as f32
    }
}

/// Por debajo de esta fraccion de contenido el frame se considera vacio.
pub const UMBRAL_CONTENIDO: f32 = 0.01;

/// Numero de frames enviados de verdad.
pub fn enviados() -> u64 {
    *estado().enviados.lock().unwrap_or_else(|p| p.into_inner())
}

/// Frames que se descartaron por ir vacios.
pub fn vacias() -> u64 {
    estado().vacias.load(Ordering::Relaxed)
}

/// Frames que se descartaron porque el visor iba lento.
pub fn perdidos() -> u64 {
    *estado().perdidos.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    /// Monta la transmision contra un puerto concreto (solo para tests).
    #[cfg(test)]
    fn configurar_para_test(destino: String, ancho: u32, alto: u32, fps: u32) {
        let e = estado();
        {
            let mut guardado = e.config.lock().unwrap_or_else(|p| p.into_inner());
            if guardado.is_some() {
                return;
            }
            *guardado = Some(Config {
                destino,
                ancho,
                alto,
                fps,
            });
        }
        let (tx, rx) = sync_channel::<Vec<u8>>(2);
        {
            let mut canal = e.canal.lock().unwrap_or_else(|p| p.into_inner());
            *canal = Some(tx);
        }
        let destino = config().map(|c| c.destino).unwrap_or_default();
        std::thread::Builder::new()
            .name("xfv-stream-test".to_string())
            .spawn(move || hilo_emisor(&destino, rx))
            .ok();
    }

    /// El camino COMPLETO (emisor -> canal -> hilo -> socket) tiene que
    /// mandar un frame con la cabecera del protocolo y la carga detras.
    #[test]
    fn el_emisor_manda_un_frame_completo() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("no se pudo escuchar");
        let puerto = listener.local_addr().expect("sin direccion local").port();
        let esperado = 16 + 4 * 4;
        let lector = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("no acepto");
            let mut buf = vec![0u8; esperado];
            s.read_exact(&mut buf).expect("lectura corta");
            buf
        });

        configurar_para_test(format!("127.0.0.1:{puerto}"), 2, 2, 30);

        let pix = vec![9u8; 2 * 2 * 4];
        assert!(enviar(&pix, 2, 2, 0), "el frame deberia haberse enviado");

        let buf = lector.join().expect("hilo muerto");
        assert_eq!(&buf[0..4], &MAGIC.to_le_bytes());
        assert_eq!(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]), 1);
        assert_eq!(u16::from_le_bytes([buf[8], buf[9]]), 2);
        assert_eq!(u16::from_le_bytes([buf[10], buf[11]]), 2);
        assert_eq!(buf[13], FORMATO_RGBA8);
        assert_eq!(&buf[16..], &pix[..]);
    }

    /// El filtro de caja tiene que promediar bien: un bloque de 2x2 con los
    /// cuatro pixeles a 10 tiene que dar 10 (y no 40).
    #[test]
    fn reducir_promedia_el_bloque() {
        let w = 4u32;
        let h = 4u32;
        let mut pix = vec![0u8; (w * h * 4) as usize];
        for i in 0..16 {
            let base = (i * 4) as usize;
            pix[base] = 10;
            pix[base + 1] = 20;
            pix[base + 2] = 30;
            pix[base + 3] = 40;
        }
        let (ow, oh, salida) = reducir(&pix, w, h, 2, 2).expect("deberia reducir");
        assert_eq!((ow, oh), (2, 2));
        assert_eq!(salida.len(), 2 * 2 * 4);
        for p in salida.chunks_exact(4) {
            assert_eq!(p[0], 10);
            assert_eq!(p[1], 20);
            assert_eq!(p[2], 30);
            assert_eq!(p[3], 40);
        }
    }

    /// Una imagen con un pixel claro en una esquina tiene que aparecer (mas
    /// atenuado) en la salida: si no, el visor recibiria imagenes negras.
    #[test]
    fn reducir_conserva_el_contenido() {
        let w = 8u32;
        let h = 8u32;
        let mut pix = vec![0u8; (w * h * 4) as usize];
        pix[0] = 255; // primer pixel (x=0, y=0)
        let (ow, oh, salida) = reducir(&pix, w, h, 4, 4).expect("deberia reducir");
        assert_eq!((ow, oh), (4, 4));
        assert!(salida[0] > 0, "el pixel claro deberia quedar en la salida");
    }

    /// La cabecera tiene que ser exactamente la que espera el visor.
    #[test]
    fn la_cabecera_es_la_del_protocolo() {
        let mut buf = Vec::new();
        escribir_cabecera(&mut buf, 7, 640, 360, 0);
        assert_eq!(buf.len(), TAM_CABECERA);
        assert_eq!(&buf[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&buf[4..8], &7u32.to_le_bytes());
        assert_eq!(&buf[8..10], &640u16.to_le_bytes());
        assert_eq!(&buf[10..12], &360u16.to_le_bytes());
        assert_eq!(buf[12], 0); // flags
        assert_eq!(buf[13], FORMATO_RGBA8);
    }

    /// El paquete es cabecera + carga, y la carga va DESPUES de la cabecera.
    #[test]
    fn el_paquete_lleva_la_carga_detras() {
        let carga = vec![1u8, 2, 3, 4];
        let p = empaquetar(1, 1, 1, 0, &carga);
        assert_eq!(p.len(), TAM_CABECERA + 4);
        assert_eq!(&p[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&p[TAM_CABECERA..], &carga[..]);
    }
}
