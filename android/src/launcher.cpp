// launcher.cpp — Lanzador nativo Android NativeActivity para Pico 4 Ultra
//
// Inicializa el contexto de Pico OS, carga el cargador OpenXR (libopenxr_loader.so),
// configura el entorno de ejecución glibc/proot y arranca Factorio con el mod 3D inyectado.

#include <android/native_activity.h>
#include <android/window.h>
#include <android/log.h>
#include <stdlib.h>
#include <unistd.h>
#include <dlfcn.h>
#include <dirent.h>
#include <errno.h>
#include <thread>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <fcntl.h>

#define LOG_TAG "XFactorVR_Launcher"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

// Configuración de rutas internas en Pico 4 Ultra
static const char* DATA_DIR = "/sdcard/XFactorVR";
static const char* MOD_LIB = "libxfactorvr.so";

// Motor real de Factorio dentro del visor.
// /sdcard esta montado como noexec, asi que el motor vive en /data/local/tmp.
static const char* FACTORIO_DIR = "/data/local/tmp/factorio_arm64";
static const char* FACTORIO_BIN = "/data/local/tmp/factorio_arm64/bin/arm64/factorio";
static const char* FACTORIO_MODS = "/data/local/tmp/factorio_arm64/mods";
static const char* START_SCRIPT = "/data/local/tmp/start_factorio_standalone.sh";
// Escenario de campana (el nivel de tutorial parcheado para multijugador).
// Lo deja el despliegue en el data del motor; si esta, el servidor arranca
// con el y no con una partida guardada.
static const char* CAMPAIGN_SCENARIO = "base/xfv-nivel";
static const char* CAMPAIGN_PATH =
    "/data/local/tmp/factorio_arm64/data/base/scenarios/xfv-nivel";
static const int   RCON_PORT = 27015;
static const char* RCON_PASSWORD = "xfactorvr";

static void ensure_directories_exist() {
    struct stat st = {0};
    if (stat(DATA_DIR, &st) == -1) {
        mkdir(DATA_DIR, 0777);
        LOGI("Creado directorio de datos: %s", DATA_DIR);
    }
}

static void setup_environment(ANativeActivity* activity) {
    // 1. Configurar LD_PRELOAD para inyectar nuestra capa 3D en el motor
    char preload_path[512];
    snprintf(preload_path, sizeof(preload_path), "%s/lib/%s", activity->internalDataPath, MOD_LIB);
    setenv("LD_PRELOAD", preload_path, 1);
    LOGI("LD_PRELOAD configurado: %s", preload_path);

    // 2. Configurar rutas de librerías nativas y datos
    setenv("FACTORIO_PATH", DATA_DIR, 1);
    setenv("OPENXR_RUNTIME", "PicoOpenXR", 1);
    setenv("PVR_SEE_THROUGH", "1", 1); // Forzar habilitación de Passthrough en Pico OS
}

#include "openxr_engine.h"

// ============================================================================
// ARRANQUE DEL SERVIDOR FACTIO (modo standalone)
// ============================================================================
// La app es un cliente RCON: el estado de la mesa viene del motor real. Si el
// servidor no esta en marcha, la mesa solo muestra la simulacion local de
// Rust, que es una demo y no el juego. Por eso al arrancar se comprueba el
// servidor y se intenta levantar si falta.

// ¿Esta escuchando el puerto RCON? Se parsea /proc/net/tcp porque no se
// presupone la disponibilidad de nc ni de netstat en la imagen de Android.
static bool rcon_port_listening() {
    const char* files[] = { "/proc/net/tcp", "/proc/net/tcp6" };
    char needle[8];
    snprintf(needle, sizeof(needle), ":%04X", RCON_PORT);

    for (int i = 0; i < 2; ++i) {
        FILE* fp = fopen(files[i], "r");
        if (!fp) continue;
        char line[512];
        while (fgets(line, sizeof(line), fp)) {
            // Columnas: sl local_address rem_address ...
            char local[64] = {0};
            if (sscanf(line, "%*d: %63s", local) != 1) continue;
            if (strstr(local, needle)) {
                fclose(fp);
                return true;
            }
        }
        fclose(fp);
    }
    return false;
}

// Localiza el linker de glibc. Android usa bionic y el motor ARM64 de
// Factorio exige glibc.
static const char* find_glibc_loader() {
    static const char* candidates[] = {
        "/data/local/tmp/glibc/lib/ld-linux-aarch64.so.1",
        "/data/local/tmp/glibc/lib64/ld-linux-aarch64.so.1",
        "/data/local/tmp/factorio_arm64/glibc/lib/ld-linux-aarch64.so.1",
        "/data/data/com.termux/files/usr/glibc/lib/ld-linux-aarch64.so.1",
        "/data/data/com.termux/files/usr/lib/ld-linux-aarch64.so.1",
    };
    for (size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
        if (access(candidates[i], X_OK) == 0) return candidates[i];
    }
    return nullptr;
}

static bool write_file(const char* path, const char* content);
static void arrancar_lector_pipe();

// El motor del sandbox escribe en su directorio privado, que no se puede leer
// desde fuera: se le pone un pipe para que su salida llegue al logcat.
static int g_pipe_salida[2] = {-1, -1};

static void preparar_pipe_salida() {
    if (g_pipe_salida[0] >= 0) return;
    if (pipe(g_pipe_salida) != 0) { g_pipe_salida[0] = -1; return; }
}

static void leer_pipe_salida() {
    if (g_pipe_salida[0] < 0) return;
    char buffer[1024];
    ssize_t n;
    while ((n = read(g_pipe_salida[0], buffer, sizeof(buffer) - 1)) > 0) {
        buffer[n] = 0;
        for (char* linea = strtok(buffer, "\n"); linea; linea = strtok(NULL, "\n")) {
            LOGI("motor: %s", linea);
        }
    }
}


// --- Copia del juego al sandbox de la app -----------------------------------
//
// SELinux no deja que un proceso del shell (contexto shell_data_file) cargue
// codigo con dlopen: el cliente grafico de Factorio hace dlopen de
// libGL.so.1 y falla con "Could not initialize OpenGL / GLES library".
// El unico sitio donde una app puede cargar codigo es su propio directorio
// privado (app_data_file), asi que el motor, el runtime de glibc y el payload
// de Mesa se copian ahi.
//
// El binario del motor es el mismo para el servidor (--start-server) y para el
// cliente (--mp-connect): la build "full" también corre sin ventana.

static bool copiar_fichero(const char* origen, const char* destino, bool ejecutable) {
    FILE* in = fopen(origen, "rb");
    if (!in) return false;
    FILE* out = fopen(destino, "wb");
    if (!out) { fclose(in); return false; }
    char buffer[64 * 1024];
    size_t n;
    while ((n = fread(buffer, 1, sizeof(buffer), in)) > 0) {
        if (fwrite(buffer, 1, n, out) != n) { fclose(in); fclose(out); return false; }
    }
    fclose(in);
    fclose(out);
    chmod(destino, ejecutable ? 0755 : 0644);
    return true;
}

// Copia un arbol de ficheros. Los enlaces simbolicos se reconstruyen como
// enlaces (el motor y Mesa los usan para versionar las librerias).
static bool copiar_arbol(const char* origen, const char* destino, int profundidad = 0) {
    if (profundidad > 12) return true;
    DIR* dir = opendir(origen);
    if (!dir) return false;
    struct dirent* entrada;
    bool ok = true;
    while ((entrada = readdir(dir)) != nullptr) {
        if (strcmp(entrada->d_name, ".") == 0 || strcmp(entrada->d_name, "..") == 0) continue;
        char src[1024], dst[1024];
        snprintf(src, sizeof(src), "%s/%s", origen, entrada->d_name);
        snprintf(dst, sizeof(dst), "%s/%s", destino, entrada->d_name);
        struct stat st;
        if (lstat(src, &st) != 0) continue;
        if (S_ISDIR(st.st_mode)) {
            mkdir(dst, 0755);
            if (!copiar_arbol(src, dst, profundidad + 1)) ok = false;
        } else if (S_ISLNK(st.st_mode)) {
            char target[1024];
            ssize_t len = readlink(src, target, sizeof(target) - 1);
            if (len > 0) {
                target[len] = 0;
                unlink(dst);
                if (symlink(target, dst) != 0) ok = false;
            }
        } else if (S_ISREG(st.st_mode)) {
            if (!copiar_fichero(src, dst, (st.st_mode & 0111) != 0)) ok = false;
        }
    }
    closedir(dir);
    return ok;
}

// Prepara el motor en el sandbox: motor (con datos), runtime de glibc y Mesa.
// Solo copia si no esta ya (asi el coste es una vez).
static bool preparar_juego_en_sandbox(const char* internal_data_path) {
    LOGI("Launcher: preparando el motor en el sandbox (%s)...", internal_data_path);
    char raiz[512], motor[512], marca[512];
    snprintf(raiz, sizeof(raiz), "%s/juego", internal_data_path);
    snprintf(motor, sizeof(motor), "%s/factorio", raiz);
    snprintf(marca, sizeof(marca), "%s/.copiado_v5", motor);

    if (access(marca, F_OK) == 0) {
        LOGI("Launcher: el motor ya estaba copiado.");
        return true;  // ya copiado
    }

    LOGI("Launcher: copiando el motor al sandbox (solo la primera vez)...");
    mkdir(raiz, 0755);
    mkdir(motor, 0755);

    // El motor (binario + datos) va de /data/local/tmp/factorio.
    char destino[512];
    snprintf(destino, sizeof(destino), "%s/bin", motor);
    mkdir(destino, 0755);
    snprintf(destino, sizeof(destino), "%s/bin/arm64", motor);
    mkdir(destino, 0755);
    char origen[512], bin[512];
    snprintf(origen, sizeof(origen), "/data/local/tmp/factorio/bin/arm64/factorio");
    snprintf(bin, sizeof(bin), "%s/bin/arm64/factorio", motor);
    if (!copiar_fichero(origen, bin, true)) {
        LOGE("Launcher: no se pudo copiar el motor grafico (%s)", origen);
        return false;
    }
    LOGI("Launcher: motor copiado (%s)", bin);
    chmod(bin, 0755);
    // Los datos: los del motor headless (traen la campana de la base).
    snprintf(origen, sizeof(origen), "/data/local/tmp/factorio_arm64/data");
    snprintf(destino, sizeof(destino), "%s/data", motor);
    mkdir(destino, 0755);
    copiar_arbol(origen, destino);

    // El runtime de glibc.
    snprintf(origen, sizeof(origen), "/data/local/tmp/glibc");
    snprintf(destino, sizeof(destino), "%s/glibc", raiz);
    mkdir(destino, 0755);
    copiar_arbol(origen, destino);

    // El payload de Mesa (Zink + lavapipe).
    snprintf(origen, sizeof(origen), "/data/local/tmp/mesa");
    snprintf(destino, sizeof(destino), "%s/mesa", raiz);
    mkdir(destino, 0755);
    copiar_arbol(origen, destino);

    // Los mods del puente (el motor no puede leer /data/local/tmp desde el
    // sandbox de la app: el contexto SELinux no lo permite).
    snprintf(origen, sizeof(origen), "/data/local/tmp/factorio_arm64/mods");
    snprintf(destino, sizeof(destino), "%s/mods", raiz);
    mkdir(destino, 0755);
    copiar_arbol(origen, destino);

    // El resolutor de nombres y el inyector (son .so de glibc: tambien
    // tienen que vivir en el sandbox para poder cargarse).
    snprintf(origen, sizeof(origen), "/data/local/tmp/libxfactorvr_resolver.so");
    snprintf(destino, sizeof(destino), "%s/resolver.so", raiz);
    copiar_fichero(origen, destino, true);
    snprintf(origen, sizeof(origen), "/data/local/tmp/libxfactorvr_injector.so");
    snprintf(destino, sizeof(destino), "%s/injector.so", raiz);
    copiar_fichero(origen, destino, true);

    write_file(marca, "ok\n");
    LOGI("Launcher: motor copiado al sandbox.");
    return true;
}

// Escribe un fichero con permisos 0644 creando los directorios intermedios.
static bool write_file(const char* path, const char* content) {    char dir[512];
    snprintf(dir, sizeof(dir), "%s", path);
    char* slash = strrchr(dir, '/');
    if (slash) {
        *slash = '\0';
        // Crear cada nivel (mkdir -p).
        for (char* p = dir + 1; *p; ++p) {
            if (*p == '/') { *p = '\0'; mkdir(dir, 0755); *p = '/'; }
        }
        mkdir(dir, 0755);
    }
    FILE* fp = fopen(path, "w");
    if (!fp) return false;
    fputs(content, fp);
    fclose(fp);
    chmod(path, 0644);
    return true;
}

// Prepara el directorio de escritura del motor dentro del almacenamiento
// privado de la app: el motor no puede escribir en /data/local/tmp porque
// ese directorio pertenece al usuario "shell".
static bool prepare_app_factorio_home(const char* internal_data_path) {
    char home[512], cfg_dir[512], cfg_path[512], ini_path[512], content[1400];
    snprintf(home, sizeof(home), "%s/factorio", internal_data_path);
    snprintf(cfg_dir, sizeof(cfg_dir), "%s/config", home);
    snprintf(cfg_path, sizeof(cfg_path), "%s/config-path.cfg", home);
    snprintf(ini_path, sizeof(ini_path), "%s/config/config.ini", home);

    mkdir(home, 0755);
    mkdir(cfg_dir, 0755);
    mkdir("/data/local/tmp/factorio_arm64/saves", 0755);

    snprintf(content, sizeof(content),
             "config-path=%s/config\n"
             "use-system-read-write-data-directories=false\n", home);
    if (!write_file(cfg_path, content)) return false;

    snprintf(content, sizeof(content),
             "[path]\n"
             "read-data=%s/data\n"
             "write-data=%s\n", FACTORIO_DIR, home);
    if (!write_file(ini_path, content)) return false;

    // Ajustes del servidor.  El motor los lee del directorio de escritura
    // (el de la app), asi que hay que ponerlos aqui y no solo en el motor.
    //
    // auto_pause=true congela el mapa mientras no haya nadie conectado.  Con
    // la campana es lo que queremos (el nivel espera al jugador); sin ella,
    // el mundo se quedaria parado en el visor.
    bool campana = (access(CAMPAIGN_PATH, F_OK) == 0);
    snprintf(content, sizeof(content),
             "{\n"
             "  \"name\": \"XFactorVR\",\n"
             "  \"description\": \"Factorio para Realidad Mixta\",\n"
             "  \"visibility\": { \"public\": false, \"lan\": true },\n"
             "  \"require_user_verification\": false,\n"
             "  \"auto_pause\": %s,\n"
             "  \"auto_pause_when_players_connect\": false,\n"
             "  \"autosave_interval\": 10,\n"
             "  \"autosave_slots\": 3\n"
             "}\n", campana ? "true" : "false");
    snprintf(cfg_dir, sizeof(cfg_dir), "%s/config/server-settings.json", home);
    write_file(cfg_dir, content);
    return true;
}

// Lanza el motor en segundo plano. Devuelve el PID o -1.
static pid_t spawn_factorio_server(const char* internal_data_path) {
    // El motor, el runtime de glibc y Mesa se copian al sandbox de la app:
    // SELinux no deja que un proceso del shell cargue codigo con dlopen, y
    // el cliente grafico necesita cargar Mesa para crear el contexto GL.
    if (!preparar_juego_en_sandbox(internal_data_path)) {
        LOGE("No se pudo preparar el motor en el sandbox.");
        return -1;
    }

    char raiz[512], motor[512], glibc_dir[512], mesa_lib[512];
    snprintf(raiz, sizeof(raiz), "%s/juego", internal_data_path);
    snprintf(motor, sizeof(motor), "%s/factorio", raiz);
    snprintf(glibc_dir, sizeof(glibc_dir), "%s/glibc/lib", raiz);
    snprintf(mesa_lib, sizeof(mesa_lib), "%s/mesa/usr/lib/aarch64-linux-gnu", raiz);
    char sandbox_loader[512];
    sandbox_loader[0] = 0;
    const char* subrutas[] = {"%s/ld-linux-aarch64.so.1", "%s/../link/ld-linux-aarch64.so.1",
                              "%s/../lib64/ld-linux-aarch64.so.1"};
    for (size_t i = 0; i < sizeof(subrutas) / sizeof(subrutas[0]); ++i) {
        char candid[512];
        snprintf(candid, sizeof(candid), subrutas[i], glibc_dir);
        if (access(candid, X_OK) == 0) {
            snprintf(sandbox_loader, sizeof(sandbox_loader), "%s", candid);
            break;
        }
    }
    if (sandbox_loader[0] == 0) {
        LOGE("No esta el linker de glibc en el sandbox (%s)", glibc_dir);
        return -1;
    }
    LOGI("Launcher: linker del sandbox: %s", sandbox_loader);

    // Cada proceso (servidor y cliente) tiene su write-data para no pelear
    // por el .lock, pero los dos leen los mismos datos del juego.
    char server_dir[512], client_dir[512], server_cfg[512], client_cfg[512];
    char save[600], mod_dir[512], saves_dir[600];
    snprintf(server_dir, sizeof(server_dir), "%s/servidor", raiz);
    snprintf(client_dir, sizeof(client_dir), "%s/cliente", raiz);
    snprintf(server_cfg, sizeof(server_cfg), "%s/config/config.ini", server_dir);
    snprintf(client_cfg, sizeof(client_cfg), "%s/config/config.ini", client_dir);
    snprintf(saves_dir, sizeof(saves_dir), "%s/saves", server_dir);
    snprintf(save, sizeof(save), "%s/saves/xfactorvr_world.zip", server_dir);
    snprintf(mod_dir, sizeof(mod_dir), "%s/mods", raiz);
    mkdir(server_dir, 0755);
    mkdir(client_dir, 0755);
    mkdir(saves_dir, 0755);

    char cfg[600];
    snprintf(cfg, sizeof(cfg),
             "[path]\nread-data=%s/data\nwrite-data=%s\n\n[other]\ncheck-updates=false\n",
             motor, server_dir);
    write_file(server_cfg, cfg);
    snprintf(cfg, sizeof(cfg),
             "[path]\nread-data=%s/data\nwrite-data=%s\n\n[other]\ncheck-updates=false\n",
             motor, client_dir);
    write_file(client_cfg, cfg);

    // Ajustes del servidor (auto_pause en campana).
    bool campana = (access(CAMPAIGN_PATH, F_OK) == 0);
    char settings_dir[512], settings[600];
    snprintf(settings_dir, sizeof(settings_dir), "%s/config", server_dir);
    mkdir(settings_dir, 0755);
    snprintf(settings, sizeof(settings),
             "{\n  \"name\": \"XFactorVR\",\n  \"visibility\": { \"public\": false, \"lan\": true },\n"
             "  \"require_user_verification\": false,\n  \"auto_pause\": %s,\n"
             "  \"auto_pause_when_players_connect\": false,\n  \"autosave_interval\": 10,\n"
             "  \"autosave_slots\": 3\n}\n", campana ? "true" : "false");
    snprintf(settings, sizeof(settings), "%s/server-settings.json", settings_dir);
    write_file(settings, settings);

    // ICD de Vulkan: el del fabricante es bionic y el motor es glibc, asi que
    // se usa lavapipe (Vulkan por software) con la ruta de aqui.
    char lvp_json[512], lvp_so[512];
    snprintf(lvp_json, sizeof(lvp_json), "%s/mesa/lvp_icd.json", raiz);
    snprintf(lvp_so, sizeof(lvp_so), "%s/libvulkan_lvp.so", mesa_lib);
    {
        char icd[700];
        snprintf(icd, sizeof(icd),
                 "{\n    \"file_format_version\": \"1.0.0\",\n    \"ICD\": {\n"
                 "        \"library_path\": \"%s\",\n        \"api_version\": \"1.3.239\"\n    }\n}\n",
                 lvp_so);
        write_file(lvp_json, icd);
    }

    char bin[600];
    snprintf(bin, sizeof(bin), "%s/bin/arm64/factorio", motor);

    // La partida se crea la primera vez; si ya existe se reutiliza.
    if (!campana && access(save, F_OK) != 0) {
        LOGI("Creando la partida inicial del motor...");
        pid_t cpid = fork();
        if (cpid == 0) {
            setenv("LD_LIBRARY_PATH", mesa_lib, 1);
            char* args[] = {
                (char*)sandbox_loader, (char*)"--library-path", glibc_dir, (char*)bin,
                (char*)"--create", (char*)save,
                (char*)"--mod-directory", mod_dir,
                (char*)"--config", server_cfg,
                nullptr
            };
            execv(sandbox_loader, args);
            _exit(127);
        }
        if (cpid > 0) {
            int status = 0;
            waitpid(cpid, &status, 0);
        }
    }

    // El resolutor de nombres + proxy CONNECT: sin el, el servidor
    // multijugador no puede pedir el padlock a auth.factorio.com.
    char resolver[512];
    snprintf(resolver, sizeof(resolver), "%s/resolver.so", raiz);

    LOGI("Arrancando el motor Factorio headless (RCON en 127.0.0.1:%d)...", RCON_PORT);
    preparar_pipe_salida();
    arrancar_lector_pipe();
    pid_t pid = fork();
    if (pid == 0) {
        // El motor hereda este proceso: hay que desprenderlo para que no
        // muera cuando la app se cierre.
        setsid();
        if (g_pipe_salida[1] >= 0) {
            dup2(g_pipe_salida[1], STDOUT_FILENO);
            dup2(g_pipe_salida[1], STDERR_FILENO);
            close(g_pipe_salida[0]);
            close(g_pipe_salida[1]);
        }
        setenv("LD_LIBRARY_PATH", mesa_lib, 1);
        setenv("HOME", server_dir, 1);
        setenv("VK_ICD_FILENAMES", lvp_json, 1);
        if (access(resolver, R_OK) == 0) {
            setenv("LD_PRELOAD", resolver, 1);
            setenv("https_proxy", "http://127.0.0.1:8899", 1);
            setenv("http_proxy", "http://127.0.0.1:8899", 1);
        }
        char* args_campana[] = {
            (char*)sandbox_loader, (char*)"--library-path", glibc_dir, (char*)bin,
            (char*)"--start-server-load-scenario", (char*)CAMPAIGN_SCENARIO,
            (char*)"--rcon-port", (char*)"27015",
            (char*)"--rcon-password", (char*)RCON_PASSWORD,
            (char*)"--mod-directory", mod_dir,
            (char*)"--config", server_cfg,
            nullptr
        };
        char* args_partida[] = {
            (char*)sandbox_loader, (char*)"--library-path", glibc_dir, (char*)bin,
            (char*)"--start-server", save,
            (char*)"--rcon-port", (char*)"27015",
            (char*)"--rcon-password", (char*)RCON_PASSWORD,
            (char*)"--mod-directory", mod_dir,
            (char*)"--config", server_cfg,
            nullptr
        };
        execv(sandbox_loader, campana ? args_campana : args_partida);
        LOGE("motor: execv fallo (%s): %s", sandbox_loader, strerror(errno));
        _exit(127);
    }
    return pid;
}

// Arranca el CLIENTE GRAFICO: es el que aporta el jugador que la campana
// exige (el nivel usa game.players[1] en su init) y el proceso donde se
// engancha el inyector del mod 3D.
//
// Va desde el sandbox de la app: alli SELinux si permite cargar Mesa, cosa
// que en /data/local/tmp no pasa ("Could not initialize OpenGL").
static int spawn_factorio_cliente(const char* internal_data_path) {
    if (!preparar_juego_en_sandbox(internal_data_path)) {
        LOGE("No se pudo preparar el motor en el sandbox para el cliente.");
        return -1;
    }
    char raiz[512], motor[512], glibc_dir[512], mesa_lib[512];
    snprintf(raiz, sizeof(raiz), "%s/juego", internal_data_path);
    snprintf(motor, sizeof(motor), "%s/factorio", raiz);
    snprintf(glibc_dir, sizeof(glibc_dir), "%s/glibc/lib", raiz);
    snprintf(mesa_lib, sizeof(mesa_lib), "%s/mesa/usr/lib/aarch64-linux-gnu", raiz);

    char sandbox_loader[512], bin[600], client_dir[512], client_cfg[512];
    sandbox_loader[0] = 0;
    const char* subrutas[] = {"%s/ld-linux-aarch64.so.1", "%s/../link/ld-linux-aarch64.so.1",
                              "%s/../lib64/ld-linux-aarch64.so.1"};
    for (size_t i = 0; i < sizeof(subrutas) / sizeof(subrutas[0]); ++i) {
        char candid[512];
        snprintf(candid, sizeof(candid), subrutas[i], glibc_dir);
        if (access(candid, X_OK) == 0) {
            snprintf(sandbox_loader, sizeof(sandbox_loader), "%s", candid);
            break;
        }
    }
    if (sandbox_loader[0] == 0) {
        LOGE("No esta el linker de glibc en el sandbox (%s)", glibc_dir);
        return -1;
    }
    snprintf(bin, sizeof(bin), "%s/bin/arm64/factorio", motor);
    snprintf(client_dir, sizeof(client_dir), "%s/cliente", raiz);
    snprintf(client_cfg, sizeof(client_cfg), "%s/config/config.ini", client_dir);
    mkdir(client_dir, 0755);
    {
        char cfg[600];
        snprintf(cfg, sizeof(cfg),
                 "[path]\nread-data=%s/data\nwrite-data=%s\n\n[other]\ncheck-updates=false\n",
                 motor, client_dir);
        write_file(client_cfg, cfg);
    }

    char lvp_json[512];
    snprintf(lvp_json, sizeof(lvp_json), "%s/mesa/lvp_icd.json", raiz);

    LOGI("Arrancando el cliente grafico (Zink -> lavapipe)...");
    preparar_pipe_salida();
    arrancar_lector_pipe();
    pid_t pid = fork();
    if (pid == 0) {
        setsid();
        if (g_pipe_salida[1] >= 0) {
            dup2(g_pipe_salida[1], STDOUT_FILENO);
            dup2(g_pipe_salida[1], STDERR_FILENO);
            close(g_pipe_salida[0]);
            close(g_pipe_salida[1]);
        }
        setenv("LD_LIBRARY_PATH", mesa_lib, 1);
        setenv("HOME", client_dir, 1);
        setenv("VK_ICD_FILENAMES", lvp_json, 1);
        setenv("SDL_VIDEODRIVER", "offscreen", 1);
        setenv("LIBGL_DRIVERS_PATH", mesa_lib, 1);
        setenv("MESA_LOADER_DRIVER_OVERRIDE", "zink", 1);
        setenv("GALLIUM_DRIVER", "zink", 1);
        setenv("ZINK_DESCRIPTORS", "auto", 1);
        char* args[] = {
            (char*)sandbox_loader, (char*)"--library-path", glibc_dir, (char*)bin,
            (char*)"--config", client_cfg,
            (char*)"--window-size", (char*)"1280x720",
            (char*)"--mp-connect", (char*)"127.0.0.1:34197",
            nullptr
        };
        execv(sandbox_loader, args);
        _exit(127);
    }
    return pid;
}

// Intenta arrancar el servidor con root (Magisk), que si puede ejecutar
// binarios alojados en /data/local/tmp.
static bool spawn_via_su() {
    if (access("/system/bin/su", X_OK) != 0 && access("/system/xbin/su", X_OK) != 0) {
        return false;
    }
    pid_t pid = fork();
    if (pid == 0) {
        setsid();
        execl("/system/bin/su", "su", "-c",
              "sh /data/local/tmp/start_factorio_standalone.sh >/dev/null 2>&1",
              (char*)nullptr);
        _exit(127);
    }
    return true;
}

// Comprueba el servidor y, si falta, intenta levantarlo. Cualquier fallo
// aqui no es fatal: la app arranca igualmente y el puente RCON se
// reconectara solo cuando el servidor aparezca.
static void arrancar_lector_pipe() {
    static bool ya = false;
    if (ya) return;
    ya = true;
    std::thread([]() {
        for (;;) {
            char buffer[1024];
            ssize_t n = read(g_pipe_salida[0], buffer, sizeof(buffer) - 1);
            if (n <= 0) break;
            buffer[n] = 0;
            for (char* linea = strtok(buffer, "\n"); linea; linea = strtok(NULL, "\n")) {
                LOGI("motor: %s", linea);
            }
        }
    }).detach();
}

static void ensure_factorio_server(ANativeActivity* activity) {
    // El motor se copia al sandbox siempre: el cliente grafico lo necesita
    // para cargar Mesa (SELinux no lo permite desde /data/local/tmp).
    if (!preparar_juego_en_sandbox(activity->internalDataPath)) {
        LOGE("No se pudo preparar el motor en el sandbox.");
    }

    if (rcon_port_listening()) {
        LOGI("Servidor Factorio detectado en 127.0.0.1:%d", RCON_PORT);
        return;
    }

    LOGI("No hay servidor Factorio en marcha; se intenta arrancar en modo standalone...");

    if (spawn_via_su()) {
        LOGI("Lanzamiento del servidor solicitado via su (root).");
    } else {
        pid_t pid = spawn_factorio_server(activity->internalDataPath);
        if (pid > 0) {
            LOGI("Motor lanzado con PID %d", (int)pid);
        }
    }

    // Dar tiempo a que el motor abra el puerto antes de que la app intente
    // conectarse por RCON.
    for (int i = 0; i < 20; ++i) {
        usleep(500000);
        if (rcon_port_listening()) {
            LOGI("Servidor Factorio listo en 127.0.0.1:%d", RCON_PORT);
            return;
        }
    }
    LOGE("El servidor Factorio no respondio. Se ejecutara la simulacion local.");
    LOGE("Para arrancarlo a mano: sh %s", START_SCRIPT);
}

// El cliente grafico aporta el jugador que la campana exige y el proceso
// donde se engancha el inyector del mod 3D.  Se arranca despues de que el
// servidor este escuchando.
static void ensure_factorio_client(ANativeActivity* activity) {
    static bool ya_lanzado = false;
    if (ya_lanzado) return;
    ya_lanzado = true;

    pid_t pid = spawn_factorio_cliente(activity->internalDataPath);
    if (pid > 0) {
        LOGI("Cliente grafico lanzado con PID %d", (int)pid);
    } else {
        LOGE("No se pudo lanzar el cliente grafico (el motor esta en el sandbox).");
    }
}

static void onStart(ANativeActivity* activity) {
    LOGI("ANativeActivity onStart");
}

static void onResume(ANativeActivity* activity) {
    LOGI("ANativeActivity onResume");
}

static void onPause(ANativeActivity* activity) {
    LOGI("ANativeActivity onPause");
}

static void onStop(ANativeActivity* activity) {
    LOGI("ANativeActivity onStop");
}

static void onWindowFocusChanged(ANativeActivity* activity, int hasFocus) {
    LOGI("ANativeActivity onWindowFocusChanged: %d", hasFocus);
}

static void onConfigurationChanged(ANativeActivity* activity) {
    LOGI("ANativeActivity onConfigurationChanged");
}

static void onLowMemory(ANativeActivity* activity) {
    LOGI("ANativeActivity onLowMemory");
}

static void onNativeWindowCreated(ANativeActivity* activity, ANativeWindow* window) {
    LOGI("ANativeWindow creada: %p", window);
    ANativeActivity_setWindowFlags(activity,
        AWINDOW_FLAG_FULLSCREEN | AWINDOW_FLAG_KEEP_SCREEN_ON,
        0);
}

#include <android/input.h>

static AInputQueue* s_input_queue = nullptr;

static void onInputQueueCreated(ANativeActivity* activity, AInputQueue* queue) {
    LOGI("ANativeActivity onInputQueueCreated: %p", queue);
    s_input_queue = queue;
}

static void onInputQueueDestroyed(ANativeActivity* activity, AInputQueue* queue) {
    LOGI("ANativeActivity onInputQueueDestroyed: %p", queue);
    s_input_queue = nullptr;
}

void pump_android_input_queue() {
    if (!s_input_queue) return;
    AInputEvent* event = nullptr;
    while (AInputQueue_getEvent(s_input_queue, &event) >= 0) {
        if (AInputQueue_preDispatchEvent(s_input_queue, event)) {
            continue;
        }
        AInputQueue_finishEvent(s_input_queue, event, 0);
    }
}

static void onNativeWindowDestroyed(ANativeActivity* activity, ANativeWindow* window) {
    LOGI("ANativeWindow destruida: %p", window);
}

static void onDestroy(ANativeActivity* activity) {
    LOGI("ANativeActivity onDestroy");
    stop_openxr_thread();
}

// Callback de inicio de NativeActivity
extern "C" void ANativeActivity_onCreate(ANativeActivity* activity, void* savedState, size_t savedStateSize) {
    LOGI("=== Iniciando XFactorVR Tabletop en Pico 4 Ultra ===");

    // Registrar callbacks para ciclo de vida Android
    activity->callbacks->onStart = onStart;
    activity->callbacks->onResume = onResume;
    activity->callbacks->onPause = onPause;
    activity->callbacks->onStop = onStop;
    activity->callbacks->onDestroy = onDestroy;
    activity->callbacks->onWindowFocusChanged = onWindowFocusChanged;
    activity->callbacks->onNativeWindowCreated = onNativeWindowCreated;
    activity->callbacks->onNativeWindowDestroyed = onNativeWindowDestroyed;
    activity->callbacks->onInputQueueCreated = onInputQueueCreated;
    activity->callbacks->onInputQueueDestroyed = onInputQueueDestroyed;
    activity->callbacks->onConfigurationChanged = onConfigurationChanged;
    activity->callbacks->onLowMemory = onLowMemory;

    ensure_directories_exist();
    setup_environment(activity);

    // Cargar la librería del mod de Realidad Mixta
    void* mod_handle = dlopen(MOD_LIB, RTLD_NOW | RTLD_GLOBAL);
    if (!mod_handle) {
        LOGE("No se pudo cargar %s: %s", MOD_LIB, dlerror());
    } else {
        LOGI("Módulo XFactorVR OpenXR cargado exitosamente");
        typedef void (*init_fn_t)();
        init_fn_t xfactorvr_init = (init_fn_t)dlsym(mod_handle, "xfactorvr_init");
        if (xfactorvr_init) {
            LOGI("Llamando a xfactorvr_init()...");
            xfactorvr_init();
            LOGI("xfactorvr_init() ejecutado correctamente");
        } else {
            LOGE("Símbolo xfactorvr_init no encontrado en %s: %s", MOD_LIB, dlerror());
        }
    }

    // Arrancar el motor real de Factorio dentro del visor. Sin esto la mesa
    // solo refleja la simulacion local de Rust (modo demo).
    ensure_factorio_server(activity);
    ensure_factorio_client(activity);

    // Arrancar el motor y sesión estéreo OpenXR para Pico OS
    start_openxr_thread(activity);

    LOGI("Entorno listo. Factorio headless y el Tabletop OpenXR están activos.");
}
