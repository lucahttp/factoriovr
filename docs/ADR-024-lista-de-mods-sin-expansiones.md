# ADR-024 — La misma lista de mods en servidor y cliente (y sin expansiones)

> **Estado**: implementado y **verificado en parte**: el servidor headless con
> la lista canónica arranca y hospeda la partida en el visor Pico 4 Ultra
> (medido: `changing state from(CreatingGame) to(InGame)`); la conexión del
> cliente gráfico se estaba verificando cuando el cable USB se soltó otra vez.

---

## 1. El problema

El cliente gráfico del visor **nunca se conectaba** al servidor headless. El
log del cliente se paraba en `Factorio initialised` y no entraba en la fase
de conexión multijugador. Con el servidor sano (RCON abierto, `InGame`) no
había ni una línea de error en el cliente: se quedaba ahí.

## 2. Lo que se encontró (medido, no supuesto)

### 2.1 El cliente y el servidor tenían listas de mods DISTINTAS

El servidor llevaba seis mods:

```json
{ "mods": [ base, elevated-rails, quality, recycler, space-age, xfactorvr_bridge ] }
```

y el cliente solo uno:

```json
{ "mods": [ base ] }
```

El servidor y el cliente deben llevar **exactamente** la misma lista, o la
conexión se rechaza.

### 2.2 El cliente GRÁFICO no puede cargar las expansiones

Al darle al cliente la lista del servidor, el motor dijo:

```
Failed to load mods: Mod elevated-rails requires Space Age expansion.

Mods to be disabled:
• elevated-rails (2.1.22)
```

El mensaje del motor es, en el binario real:

```
"%s (%s) requires the %s expansion be enabled."
```

Es un **chequeo de licencia de expansión**, no de dependencias entre mods: el
motor tiene `FeatureFlags::Expansion`, `FeatureFlags::Quality`,
`FeatureFlags::RailBridges`… y los activa a partir de la respuesta del
servidor de autenticación. Sin cuenta de Factorio (`service-token` vacío en
`player-data.json`) esas banderas no se activan.

**El servidor headless NO hace ese chequeo**: carga los seis mods sin
problema. Por eso el(server, client) nunca encajaba — el servidor cargaba 6
mods, el cliente rechazaba 2, y la lista no coincidía.

### 2.3 Las expansiones se auto-activan si están en el `read-data`

Los mods de expansión no están en `mods/`: viven en el `read-data`, junto a
`base` y `core`:

```
factorio_arm64/data/  →  base/ core/ elevated-rails/ quality/ recycler/ space-age/
```

Si están ahí, el motor los carga aunque `mod-list.json` no los mencione. Por
eso el cliente las tenía "de paso" (copiadas a su `read-data`) y se negaba a
arrancar.

## 3. La decisión

**Los dos lados llevan una lista canónica, la misma, sin expansiones:**

```json
{ "mods": [ { "name": "base", "enabled": true },
            { "name": "xfactorvr_bridge", "enabled": true } ] }
```

Está en `scripts/mod-list.json` y la escriben los dos scripts:

- `start_factorio_standalone.sh` la copia a `$MOD_DIR/mod-list.json`.
- `start_factorio_client.sh` la copia a `$CLIENT_DIR/mods/mod-list.json`,
  **copia el mod puente desde el árbol del servidor** (para que no haya ni un
  byte de diferencia entre los dos lados) y **aparta las expansiones** de su
  `read-data` a `<read-data>/_sin_licencia/`.

Se pierde el contenido de la expansión (Space Age). Es el precio: el juego es
Factorio **vanilla**, que es lo que se puede jugar sin cuenta de Factorio. La
partida guardada carga igual sin las expansiones (medido: `Loading map
...xfactorvr_world.zip` sin error).

## 4. Lo que NO se decide aquí

El mundo no usa nada de Space Age. Si en el futuro hace falta, la vía no es
tirar de la expansión: es un `service-token` de una cuenta con la licencia,
que el motor acepta tal cual en `player-data.json`.

## 5. Bancos

`scripts/test_mods_cliente.sh` fija las invariantes (misma lista, sin
expansiones, FIFO en el write-data del cliente, detección de proceso vivo por
`cmdline`). Sale con 0.

## 6. De paso: otros fallos reales encontrados en el mismo Devices

- **El servidor nunca detectaba que ya estaba en marcha.** En el visor el
  proceso del motor se llama `ld-linux-aarch64.so.1` (lo arranca el linker de
  glibc), no `factorio`: el chequeo por `/proc/<pid>/comm` nunca daba, el
  script lanzaba un segundo motor y este moría con
  `Host address is already in use`. Ahora se mira el `cmdline`.
- **La FIFO de la consola estaba en un directorio de root**
  (`/data/local/tmp/factorio/`), así que no se podía crear y el canal de
  órdenes sin RCON quedaba sin usar. Ahora cuelga del `write-data` del
  cliente.
- **El cliente anterior bloqueaba al nuevo** con
  `Couldn't acquire exclusive lock for .../factorio_cliente/.lock`. Ahora el
  script mata solo la instancia anterior del cliente (la del servidor se
  distingue por su `--config`).
