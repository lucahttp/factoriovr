# ADR-011 — El cliente RCON del APK: diagnóstico corregido y bugs reales

> **Estado**: el problema original ("el cliente RCON no habla con el motor")
> **está RESUELTO**: era un diagnóstico falso. Pero al destaparlo han
> aparecido dos bugs reales que sí importan. Este documento cuenta los dos.

## El diagnóstico anterior era falso

La versión anterior de este ADR decía que el cliente RCON de Rust
(`src/game_bridge.rs`) no recibía respuesta del motor mientras que el sondeo
en Python (`scripts/rcon_probe.py`) sí.

**Era una binario viejo.** El arreglo de la lectura ya estaba en el repo (commit
`6f6ac17`, "la lectura para en el primer paquete con contenido"), pero el
binario ARM64 que se copiaba al contenedor **no se había recompilado**: el
banco avisa cuando el binario es más viejo que el fuente, pero no lo
recompila (compilar el target musl tarda minutos).

Con el binario **fresco**, contra el motor real:

```
[rcon] -> id=4 tipo=2 «/silent-command remote.call('xfactorvr','get_world_snapshot',32,0,0)»
[rcon] <- id=4 tipo=0 cuerpo=41992 «{"entities":[{"t":"tree-05",...
[rcon] poll: snapshot -> 41992 bytes
```

**El poll SÍ trae el snapshot.** El cliente RCON del crate habla con el motor.

La traza se activa con `XFACTORVR_RCON_TRACE=1` (imprime cada paquete enviado y
recibido por stderr). Es la forma de ver qué pasa por el socket sin meter un
proxy por medio.

## Bug real 1 — el snapshot llega, pero el diorama se queda vacío

El poll trae el snapshot (41992 bytes, 99 entidades) y aun así el diorama se
queda con **0 entidades**: el visor cae en la simulación local.

La causa está en el **acuerdo entre el mod y el cliente**:

- El mod (`mods/xfactorvr_bridge/control.lua`, `serialize_entity`) calcula el
  tipo canónico con `entity_type_of(e)`, que mira `e.type` ("tree",
  "simple-entity", "unit"...). Para un árbol del escenario, `e.type == "tree"`
  → tipo 12.
- Pero al serializar manda **`e.name`** (el nombre del prototipo) en el campo
  `"t"`: `"tree-05"`, `"tree-03"`, `"dry-tree"`, ...
- El cliente (`entity_type_from_name` en `src/game_bridge.rs`) traduce ese
  nombre al tipo del render. Su tabla solo conoce los nombres canónicos
  (`"tree"`, `"stone-furnace"`, `"wooden-chest"`, ...), así que `"tree-05"` cae
  en el `_ => 0` y la entidad **se descarta**.

O sea: el mod sabe el tipo y lo tira; el cliente adivina el tipo por un nombre
que no es el suyo.

El arreglo (implementado): el mod manda **el prototipo en `"t"`** (como
antes) **y el tipo canónico en un campo nuevo `"c"`**. El cliente usa `"c"`
para el tipo y `"t"` para elegir el sprite real (el manifiesto de sprites
tiene 43 nombres: cajas, hornos, cintas, "dry-tree"... pero no "tree-05", así
que hace falta el repliegue al canónico).

No se cambió `"t"` a propósito: el banco `scripts/test_inventario_vr.sh` (que
es de otro agente) parsea el snapshot con Python y depende de ese campo.

```
{"t":"tree-05","c":"tree","x":30.45,"y":-18.36,"o":0.000,"s":0,"p":0.000,
 "h":1.000,"f":0,"d":0}
```

El cliente también acepta un snapshot **viejo** (sin `"c"`): entonces el tipo
sale del prototipo, como siempre, y las entidades cuyo nombre no está en la
tabla se descartan (como antes).

## Bug real 2 — el poll martillea al motor

`poll_rcon_sync` **reinicia la sesión TCP en cada poll** (`rcon.connect` antes
de cada consulta) y manda **dos comandos de consola** (inventario + snapshot).
El poll se dispara cada `RCON_POLL_INTERVAL = 0,12 s`, o sea **unas 16
comandos de consola por segundo y 8 conexiones TCP nuevas por segundo**
contra el motor.

En el log del motor se ve la avalancha:

```
12.090 New RCON connection from IP ADDR:({127.0.0.1:36792})
12.295 New RCON connection from IP ADDR:({127.0.0.1:36802})
12.399 New RCON connection from IP ADDR:({127.0.0.1:36812})
12.500 New RCON connection from IP ADDR:({127.0.0.1:36816})
12.598 New RCON connection from IP ADDR:({127.0.0.1:36824})
```

En la Pico eso es un martillo: el motor va a ir delaying y el render se va a
sentir. Con el bug 1 arreglado, la sesión se puede reutilizar y el inventario
no hace falta pedirlo a 8 Hz.

## Bug latente — el reintento pide el snapshot con decimales

El bloque de reintento del poll construye el comando con `{:.1}`:

```rust
let snap_cmd = format!("... get_world_snapshot', {}, {:.1}, {:.1})", ...);
```

Pero el comentario del propio código (y el banco) dice que **el centro tiene que
ir como entero**: con decimales, `find_entities` con un área decimal no
devuelve nada y el snapshot llega vacío. El camino principal usa `{:.0}`
correcto; el reintento usa `{:.1}` y por eso, cuando se llega al reintento, el
snapshot sale vacío.

## Cómo se comprueba

```bash
# Traza del socket RCON (cada paquete enviado y recibido)
XFACTORVR_RCON_TRACE=1 ./probar_manos 127.0.0.1:27015 xfactorvr

# Snapshot real del motor, tal cual, para parsearlo fuera
python3 scripts/extraer_snapshot.py 127.0.0.1:27015 xfactorvr /tmp/snapshot.json
```

Y el banco que valida el camino completo del visor contra el motor real:
`scripts/test_visor_real.sh`.
