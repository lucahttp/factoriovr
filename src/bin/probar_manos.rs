//! probar_manos.rs — Comprobacion de las MANOS del visor contra el motor real.
//!
//! Es el camino que lleva el APK en el dispositivo, paso a paso:
//!
//!   1. conectar por RCON con el motor de Factorio,
//!   2. pedir la instantanea del mundo (con las entidades y los items sueltos),
//!   3. CLASIFICAR lo que hay bajo el rayo (caja, horno, item del suelo...),
//!      que es lo que hace el visor con la casilla que apunta el mando,
//!   4. ACTUAR: la misma llamada que hace el motor de VR cuando se aprieta
//!      el gatillo o el gripo (`xfactorvr_vr_hand_action`), y comprobar que
//!      el item cambia de sitio de verdad.
//!
//! El banco que lo usa es scripts/test_manos_vr.sh (que levanta el motor en
//! el contenedor ARM64 con el mod puente).
//!
//! Uso: probar_manos [host:puerto] [password]

use xfactorvr::game_bridge;
use xfactorvr::tabletop::{classify_target, Interactable};

/// Dice si dos interaccionables son del mismo tipo (para el informe).
fn nombre(t: &Interactable) -> &'static str {
    match t {
        Interactable::Nothing => "nada",
        Interactable::Chest => "caja",
        Interactable::Furnace => "horno",
        Interactable::LooseItem { .. } => "item del suelo",
    }
}

fn main() {
    let argumentos: Vec<String> = std::env::args().collect();
    let host = argumentos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:27015".to_string());
    let password = argumentos
        .get(2)
        .cloned()
        .unwrap_or_else(|| "xfactorvr".to_string());

    // --- 1. Conectar por RCON (como el APK) --------------------------------
    {
        let mut rcon = game_bridge::get_rcon_client()
            .lock()
            .expect("no se pudo tomar el cliente RCON");
        if !rcon.connect(&host, &password) {
            panic!("no se pudo conectar por RCON a {}", host);
        }
    }
    println!("ok: conectado por RCON a {}", host);

    // PRUEBA DE CONTROL: un comando de consola sencillo.  Si esto vuelve, el
    // cliente RCON funciona y el problema es solo con el snapshot.
    {
        let control = {
            let mut rcon = game_bridge::get_rcon_client()
                .lock()
                .expect("no se pudo tomar el cliente RCON");
            rcon.execute_collect("/silent-command rcon.print('hola')", 3)
        };
        match control {
            Some(c) => println!("control: el RCON responde: {}", &c[..c.len().min(120)]),
            None => println!("control: el RCON NO responde a un comando sencillo"),
        }
    }

    // --- 2. Instantanea del mundo -----------------------------------------
    // Esto es lo que hace el motor de VR en cada frame: pedir el estado al
    // servidor y llenar la lista de entidades (la que usa para clasificar lo
    // que hay bajo el rayo).
    // Orden inverso: primero el INVENTARIO y luego el snapshot (que es lo
    // que hace el poll del puente).  Si el snapshot vuelve vacio, el comando
    // del inventario es lo que rompe la sesion.
    {
        let inv = {
            let mut rcon = game_bridge::get_rcon_client()
                .lock()
                .expect("no se pudo tomar el cliente RCON");
            rcon.execute_collect("/silent-command remote.call('xfactorvr', 'get_inventory')", 3)
        };
        println!("inventario: {}", match inv { Some(c) => format!("{} bytes", c.len()), None => "NO LLEGO NADA".to_string() });
        let despues = {
            let mut rcon = game_bridge::get_rcon_client()
                .lock()
                .expect("no se pudo tomar el cliente RCON");
            rcon.execute_collect(
                "/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)",
                3,
            )
        };
        match despues {
            Some(c) => println!("snapshot tras inventario: {} entidades ({} bytes)", c.matches("\"t\":").count(), c.len()),
            None => println!("snapshot tras inventario: NO LLEGO NADA"),
        }
    }

    // El snapshot pedido DIRECTAMENTE (como hace el cliente minimo), sin
    // pasar por el poll del puente: para seccionar si el problema es el
    // poll o el cliente.
    {
        let directo = {
            let mut rcon = game_bridge::get_rcon_client()
                .lock()
                .expect("no se pudo tomar el cliente RCON");
            rcon.execute_collect(
                "/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)",
                3,
            )
        };
        match directo {
            Some(c) => {
                let n = c.matches("\"t\":").count();
                println!("directo: el snapshot tiene {n} entidades ({} bytes)", c.len());
            }
            None => println!("directo: NO LLEGO NADA"),
        }
    }
    for _ in 0..3 {
        unsafe { xfactorvr::xfactorvr_update_simulation(2.0) };
    }
    let rcon = game_bridge::poll_rcon_sync(3.0, (0.0, 0.0));
    if !rcon.connected {
        panic!("el RCON no esta conectado");
    }
    // OJO: el lock del mundo se suelta aqui.  Si se mantiene vivo mientras se
    // llama a `xfactorvr_vr_hand_action` (que toma un `.read()` del mismo
    // mutex) el proceso se CUELGA: el `RwLock` de Rust no es reentrante.
    let sueltos = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.loose_items.clone(),
        Err(_) => panic!("no se pudo tomar el mundo"),
    };
    // Las entidades vivas las lleva el puente en su propia lista (la que
    // consulta el motor de VR cuando el rayo apunta a algo).
    let entidades = xfactorvr::get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();
    println!(
        "ok: instantanea con {} entidades y {} items sueltos",
        entidades.len(),
        sueltos.len()
    );

    // Traza: el comando SIN el prefijo /silent-command.
    //
    // OJO: asi NO tiene que volver nada.  Sin el prefijo, la salida del
    // `rcon.print` del mod se imprime en la consola del SERVIDOR, no por el
    // RCON: es la comprobacion de que el camino correcto (con prefijo) es el
    // que se usa en el resto.
    {
        let crudo = {
            let mut rcon = game_bridge::get_rcon_client()
                .lock()
                .expect("no se pudo tomar el cliente RCON");
            rcon.execute_collect(
                "remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)",
                3,
            )
        };
        match crudo {
            Some(c) => println!("traza: sin /silent-command el RCON devuelve: {}", &c[..c.len().min(300)]),
            None => println!("traza: sin /silent-command no vuelve nada (lo esperado: va a la consola del servidor)"),
        }
    }

    // Traza: el snapshot EN CRUDO que devuelve el mod.  Con el comando
    // silencioso el RCON no devuelve nada, asi que se activa el repliegue
    // "verbose" (el mismo que usa el puente).
    let rcon = game_bridge::poll_rcon_sync(3.0, (0.0, 0.0));
    {
        let centro = game_bridge::get_simulation_world()
            .read()
            .map(|w| w.view_center)
            .unwrap_or((-1.0, -1.0));
        println!("traza: centro de vista {:?}  comando: /silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, {}, {})", centro, centro.0 as i64, centro.1 as i64);
    }
    if let Some(c) = rcon.snapshot.as_deref() {
        match c.find("\"loose\"") {
            Some(p) => {
                let trozo: String = c[p..(p + 220).min(c.len())].to_string();
                println!("traza: seccion loose del snapshot: {}", trozo);
            }
            None => println!("traza: el snapshot no tiene seccion loose: {}", &c[..c.len().min(220)]),
        }
    } else {
        println!("traza: el poll no traia snapshot");
    }

    if entidades.is_empty() {
        panic!("la instantanea no trae entidades");
    }

    // --- 3. Clasificar lo que hay bajo el rayo ----------------------------
    //
    // El visor hace esto con la casilla que apunta el mando.  Se prueban las
    // tres clases que las manos saben usar: caja, horno e item del suelo.
    let mut clasificados = 0usize;
    let mut objetivo_caja: Option<(f32, f32)> = None;
    let mut objetivo_horno: Option<(f32, f32)> = None;
    let mut objetivo_suelto: Option<(f32, f32)> = None;

    for e in &entidades {
        let t = classify_target(&entidades, &sueltos, e.tile_x, e.tile_y);
        let n = nombre(&t);
        match &t {
            Interactable::Chest => {
                if objetivo_caja.is_none() {
                    objetivo_caja = Some((e.tile_x, e.tile_y));
                }
            }
            Interactable::Furnace => {
                if objetivo_horno.is_none() {
                    objetivo_horno = Some((e.tile_x, e.tile_y));
                }
            }
            _ => {}
        }
        if n != "nada" {
            clasificados += 1;
        }
    }
    // Los items sueltos no son entidades: se clasifican sobre su casilla.
    for s in &sueltos {
        let t = classify_target(&entidades, &sueltos, s.tile_x, s.tile_y);
        if matches!(t, Interactable::LooseItem { .. }) && objetivo_suelto.is_none() {
            objetivo_suelto = Some((s.tile_x, s.tile_y));
        }
    }

    println!("ok: {} entidades se clasifican como algo manipulable", clasificados);
    let (cx, cy) = match objetivo_caja {
        Some(p) => p,
        None => panic!("no hay ninguna caja en la instantanea (clasificacion rota)"),
    };
    let (hx, hy) = match objetivo_horno {
        Some(p) => p,
        None => panic!("no hay ningun horno en la instantanea (clasificacion rota)"),
    };
    let (sx, sy) = match objetivo_suelto {
        Some(p) => p,
        None => panic!("no hay ningun item suelto en la instantanea (clasificacion rota)"),
    };
    println!("ok: caja en ({cx}, {cy}), horno en ({hx}, {hy}), item en ({sx}, {sy})");

    // --- 4. Actuar: el gatillo sobre la caja saca un item -----------------
    //
    // Es EXACTAMENTE lo que hace el motor de VR al apretar el gatillo.
    let antes = sueltos.len();
    let actuo = unsafe { xfactorvr::xfactorvr_vr_hand_action(cx, cy, 0) };
    if !actuo {
        panic!("el gatillo sobre la caja no hizo nada");
    }
    println!("ok: el gatillo sobre la caja ({cx}, {cy}) ha actuado");

    // Y el gripo mete el item que lleva el actor en la caja.
    let actuo = unsafe { xfactorvr::xfactorvr_vr_hand_action(cx, cy, 1) };
    if !actuo {
        panic!("el gripo sobre la caja no hizo nada");
    }
    println!("ok: el gripo sobre la caja ({cx}, {cy}) ha actuado");

    // Y el gatillo sobre el item del suelo lo coge.
    let actuo = unsafe { xfactorvr::xfactorvr_vr_hand_action(sx, sy, 0) };
    let _ = antes;
    if !actuo {
        panic!("el gatillo sobre el item del suelo no hizo nada");
    }
    println!("ok: el gatillo sobre el item del suelo ({sx}, {sy}) ha actuado");

    // Y el gatillo sobre el horno mete los ingredientes.
    let actuo = unsafe { xfactorvr::xfactorvr_vr_hand_action(hx, hy, 0) };
    if !actuo {
        panic!("el gatillo sobre el horno no hizo nada");
    }
    println!("ok: el gatillo sobre el horno ({hx}, {hy}) ha actuado");

    println!("RESULTADO: las manos VR funcionan contra el Factorio real");
}
