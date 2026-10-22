# Modelos 3D del visor (optimizados)

Aqui van los GLB con los que el visor dibuja las maquinas del juego en 3D
(la tabla prototipo -> modelo esta en `manifest.json`; el codigo Rust que la
lee es `src/modelos3d.rs`).

## De donde salen

Los modelos originales son los del mod 3D de GDani31
(`factorio-3d-mod/models/ENTITIES/...`, 567 MB). Se optimizan con la
herramienta del repo:

```
cargo run --release --bin optimizar_glb -- \
    --lote android/src/main/assets/models/fuentes.json \
           factorio-3d-mod/models \
           android/src/main/assets/models
```

`fuentes.json` es la tabla de fuentes (prototipo -> GLB original + huella en
casillas + como gira cada parte). El optimizador:

* hornea las transformaciones de los nodos,
* hornea el color del material y de la textura en los vertices (`COLOR_0`),
* saca las texturas embebidas a ficheros sueltos (con `--texturas`),
* redondea a 3 decimales,
* decima la malla (rejilla de ~220 celdas por eje),
* escribe un GLB nuevo y regenera `manifest.json`.

Los `.meta.json` (en `meta/`) son el resumen de cada modelo (bbox, vertices,
bytes antes/despues): no hacen falta en tiempo de ejecucion.

## En el dispositivo

Los assets del APK se leen con AAssetManager, no con rutas de fichero: el
motor tiene que copiar `assets/models` a un directorio legible (por ejemplo
`/sdcard/XFactorVR/assets/models`, que es donde ya se suben los assets del
juego) o usar `XFACTORVR_MODELS_DIR`. `src/modelos3d.rs::dir_modelos()`
busca los sitios de siempre.
