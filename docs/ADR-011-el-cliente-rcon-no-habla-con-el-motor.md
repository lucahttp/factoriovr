# ADR-011 — El cliente RCON del APK no habla con el motor (problema abierto)

## Qué se ha encontrado

Al Trying de validar **el camino completo del visor** (el mismo código que
lleva el APK en el dispositivo) contra un Factorio real en el contenedor, se
vio que **el cliente RCON de Rust (`src/game_bridge.rs`) no recibe respuesta
del servidor**, mientras que el sondeo en Python del banco
(`scripts/rcon_probe.py`) sí la recibe.

La prueba de control (un `rcon.print('hola')` a pelo, con el mismo servidor y
el mismo binario) da:

```
ok: conectado por RCON a 127.0.0.1:27021     <- la autenticacion SI funciona
control: el RCON NO responde a un comando sencillo
traza: el RCON no devolvio nada
traza: el poll no traia snapshot
ok: instantanea con 11 entidades y 0 items sueltos   <- las 11 son de la SIMULACION LOCAL
```

Las 11 entidades son las de la simulación local de reserva
(`tabletop::init_default_factory_entities`), no las del juego.

## Por qué importa

**El visor nunca ha visto el juego real.** El diorama que se ve en MR siempre
fue la simulación local de ejemplo, no la fábrica del servidor. Por eso el
banco del visor (FASE 34, `scripts/test_visor_juega.sh`) nunca se pudo
ejecutar: necesita el dispositivo, y contra el contenedor el cliente RCON no
trae el mundo.

Lo que **sí** está validado (con el sondeo en Python, que sí funciona):
- los remotos del mod (coger, meter, sacar) ✓
- la campaña entera en un servidor headless ✓
- el snapshot del mod con su sección "loose" (items del suelo) ✓

## Lo que se sabe del servidor

Del log del propio motor (el que hay que leer cuando esto se atasca):

```
[WARNING] Player <server> tried using the command "remote.call(...)".
[WARNING] Using Lua console commands will disable achievements. Please repeat the command to proceed.
```

**Factorio rechaza el PRIMER comando de consola de cada sesión.** El sondeo
del banco lo sortea con un "calentamiento" (`rcon.print('listo')`) antes de
cada consulta, y abre una conexión nueva para cada una.

## Correcciones aplicadas (aún sin validar en conjunto)

En `src/game_bridge.rs`:

1. **El timeout de lectura pasa de 400 ms a 3 s.** El sondeo en Python espera
   5 s; el snapshot con el terreno (miles de tiles) tarda bastante más que el
   inventario, y con 400 ms el cliente se cansaba antes de que llegara.
2. **Ya no se quita el prefijo `/silent-command`** cuando el snapshot no trae
   JSON. En modo silencioso, la salida del `rcon.print` del mod vuelve por el
   RCON; **sin** el prefijo se imprime en la consola del *servidor* y el
   cliente se queda sin nada. Ahora se repite el comando (con prefijo).
3. **Se reconoce el aviso de rechazo** ("Please repeat the command to
   proceed") y se repite el comando en vez de dar la respuesta por buena.
4. **Calentamiento al conectar**, para gastar el rechazo del primer comando.
5. **Reconexión** si el snapshot no llega.

Y en `scripts/test_inventario_vr.sh`: el banco **compila siempre** el binario
ARM64 de las manos. Antes solo lo compilaba si no existía, así que validaba un
binario viejo (esto hizo que varias pruebas pareciesen fallar sin estar probando
el código nuevo).

## Por qué sigue sin resolverse

Con todo lo anterior a la vez, el control sigue dando "no responde". La
diferencia que queda entre el cliente que funciona (Python) y el que no
(Rust) está en la **lectura**: el Python lee `length - 8` bytes del cuerpo y el
Rust `length - 10`.

**Siguiente paso**: probar el cliente Rust con la lectura exactamente igual
que la del sondeo (mismo `length`, mismos bucles de `recv`, mismos timeouts) y
**una conexión nueva por consulta**, como hace el Python. Si eso funciona, la
diferencia está en el reuse de la sesión y el arreglo es reiniciar la sesión
(o el sondeo del banco, que es lo que hoy valida de verdad).

## Cómo se comprueba

```bash
bash scripts/test_inventario_vr.sh        # 12 fases de los remotos + el aviso
```

La fase "El camino completo del APK" deja aviso (no fallo) mientras esto siga
abierto, y avisa de que el visor cae en la simulación local.
