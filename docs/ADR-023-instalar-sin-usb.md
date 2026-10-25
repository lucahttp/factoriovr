# ADR-023 — Instalar y arrancar en el visor SIN USB

> **Estado**: implementado (scripts) y **verificado en parte**: la conexión
> por red no se ha podido probar porque la Pico no ha estado en la misma red
> que el Mac con la depuración activada.

---

## 1. El problema

El cable USB de la Pico 4 Ultra es errático: se ha soltado **cuatro veces** en
una tarde y hay veces que no enumera aunque esté enchufada. Todo el flujo
actual (`deploy_pico.sh` → arranque → capturas) depende de adb por USB.

## 2. La restricción que manda de verdad

De los propios comentarios del launcher (`android/src/launcher.cpp`):

> *"El motor de Factorio **NO lo arranca la app**. El proceso del motor
> hereda el contexto SELinux de la app (untrusted_app) y **no puede leer
> /data/local/tmp** (shell_data_file); además la Pico **no deja ejecutar un
> ELF desde el sandbox de una app** (execv → Permission denied). El motor lo
> arranca el shell y la app se limita a conectarse por RCON."*

Traducción: **el motor solo puede arrancarlo un shell con el uid de
`shell`**. La app no puede (W^X + SELinux), y sin un shell no hay motor.

Un shell sin adb es **Termux**, que es justo lo que no queremos.

## 3. La solución: depuración INALÁMBRICA

La Pico 4 Ultra lleva **Android 14**, que tiene **depuraciónINALámbrica**
(`adb` por Wi-Fi, sin USB y sin Termux). Eso da un shell remoto con el uid
correcto ⇒ el motor puede arrancar igual que siempre.

```bash
# 1. En la Pico: Ajustes > Opciones de desarrollador > Depuracióninalámbrica
#    (sale una IP:puerto en pantalla)
# 2. En el Mac:
bash scripts/conectar_sin_usb.sh              # busca la Pico en la subred
bash scripts/conectar_sin_usb.sh 192.168.1.50:5555   # con la IP que sale
bash scripts/conectar_sin_usb.sh --desplegar  # y despliega
```

El script **busca solo**: recorre la subred del Mac probando los puertos de
depuración (5555, 5037 y el rango 37000-44000 que usa Android).

## 4. Además: servir los ficheros gordos por HTTP

1,5 GB de datos del juego por adb van bien por USB pero mal por Wi-Fi. Para
eso, un servidor HTTP en el Mac:

```bash
bash scripts/servir_para_el_visor.sh
# en el navegador de la Pico: http://<IP-DEL-MAC>:8099/
```

Allí está la APK (para instalar sin USB: "permitir orígenes desconocidos"),
el motor, el payload de Mesa, el runtime de glibc, el inyector y el mod.

## 5. Lo que NO se puede hacer (y por qué)

- **Que la app arranque el motor**: imposible sin root (W^X + SELinux) ✗.
- **Que el motor viva en el sandbox de la app**: por lo mismo ✗.
- **Un instalador "todo en uno" dentro de la APK**: el payload de Mesa son
  594 MB de `.so` que **tienen que ser ejecutables** ⇒ no valen como assets
  (se extraen sin permiso de ejecución) ✗.

Por eso la instalación sin USB es **"instalar la APK desde el navegador" +
"el motor lo arranca el shell por Wi-Fi"**, no un instalador autocontenido.

## 6. Lo que esto deja

| Sin USB | Cómo |
|---|---|
| Instalar la APK | navegador de la Pico → `http://<mac>:8099/` → instalar (permitir orígenes desconocidos) |
| Desplegar el motor, el Mesa, el glibc, el inyector, el mod | `bash scripts/conectar_sin_usb.sh --desplegar` (adb por Wi-Fi) |
| Los 1,5 GB de datos del juego | `bash scripts/servir_para_el_visor.sh` + descarga (o el propio despliegue por Wi-Fi) |
| Arrancar el motor | el shell remoto (adb por Wi-Fi) |

## 7. El script de bootstrap del visor

`scripts/bootstrap_visor.sh` es la parte que se ejecuta **dentro** del
visor: baja los ficheros con el `wget` de toybox y los desempaqueta con su
`tar`, en el directorio de datos de la app. Está pensado para que la app lo
lance (`sh` sí se puede lanzar desde una app; lo que no se puede es
ejecutar el motor).

## 8. Bancos

- `scripts/probe_xfv1.py` — un visor **falso** que habla XFV1: comprueba que
  llegan los píxeles y los mensajes de geometría. Sirve en el contenedor
  (donde no hay visor OpenXR) y en el visor.
