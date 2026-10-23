/* probe_egl.c — Sonda EGL minima para el visor (Pico 4 Ultra).
 *
 * Sin includes a proposito: el sysroot de glibc del proyecto trae las
 * librerias pero no las cabeceras, y las de la NDK son de bionic.  Se
 * declaran a mano las funciones que se usan.
 *
 * Compilacion (ARM64, glibc):
 *   aarch64-linux-android34-clang --target=aarch64-linux-gnu \
 *     --sysroot=build/glibc_sysroot -O2 -o probe_egl probe_egl.c \
 *     -Lbuild/glibc_sysroot/link -ldl
 *
 * Uso en el visor:
 *   LD_LIBRARY_PATH=... ./glibc/lib/ld-linux-aarch64.so.1 --library-path ... ./probe_egl
 */

extern int printf(const char *fmt, ...);
extern void *dlopen(const char *file, int mode);
extern void *dlsym(void *handle, const char *name);
extern const char *dlerror(void);

#define RTLD_NOW 2
#define RTLD_LOCAL 0

#define EGL_NO_DISPLAY ((void *)0)
#define EGL_EXTENSIONS 0x3055
#define EGL_VERSION 0x3054
#define EGL_PLATFORM_SURFACELESS_MESA 0x31DD
#define EGL_PLATFORM_DEVICE_EXT 0x3138

typedef const char *(*fn_query_string)(void *display, int name);
typedef unsigned int (*fn_query_devices)(int max, void **devices, int *count);
typedef void *(*fn_get_platform_display)(unsigned int platform, void *device,
                                         const void *attribs);
typedef unsigned int (*fn_initialize)(void *display, int *major, int *minor);
typedef unsigned int (*fn_get_error)(void);

static int contiene(const char *s, const char *needle) {
    if (!s || !needle) return 0;
    const char *p = s;
    while (*p) {
        const char *q = needle;
        while (*q && *p == *q) { p++; q++; }
        if (!*q) return 1;
        while (*p && *p != ' ') p++;
        while (*p == ' ') p++;
    }
    return 0;
}

static void inspecciona_vendor(const char *nombre, void *lib) {
    fn_query_string q = (fn_query_string)dlsym(lib, "eglQueryString");
    fn_query_devices qd = (fn_query_devices)dlsym(lib, "eglQueryDevicesEXT");
    fn_get_platform_display gp = (fn_get_platform_display)dlsym(lib, "eglGetPlatformDisplay");
    fn_initialize ini = (fn_initialize)dlsym(lib, "eglInitialize");
    fn_get_error ge = (fn_get_error)dlsym(lib, "eglGetError");

    printf("--- %s ---\n", nombre);
    if (!q) { printf("  sin eglQueryString\n"); return; }
    const char *ext = q(EGL_NO_DISPLAY, EGL_EXTENSIONS);
    printf("  extensions de cliente: [%s]\n", ext ? ext : "(null)");
    printf("  EGL_EXT_device_base: %s\n", contiene(ext, "EGL_EXT_device_base") ? "SI" : "NO");
    printf("  EGL_MESA_platform_surfaceless: %s\n",
           contiene(ext, "EGL_MESA_platform_surfaceless") ? "SI" : "NO");
    printf("  EGL_MESA_device_software: %s\n",
           contiene(ext, "EGL_MESA_device_software") ? "SI" : "NO");

    if (qd) {
        void *devs[16];
        int n = 0;
        if (qd(16, devs, &n)) {
            printf("  eglQueryDevicesEXT -> %d device(s)\n", n);
            for (int i = 0; i < n && i < 8; ++i) {
                const char *de = q(devs[i], EGL_EXTENSIONS);
                printf("    device %d: %s\n", i, de ? de : "(sin extensions)");
            }
            /* Se intenta inicializar el primero, como hace SDL. */
            if (gp && ini && n > 0) {
                void *dpy = gp(EGL_PLATFORM_DEVICE_EXT, devs[0], 0);
                if (dpy) {
                    int a = 0, b = 0;
                    if (ini(dpy, &a, &b)) {
                        printf("    device 0: EGL %d.%d INICIALIZADO\n", a, b);
                    } else {
                        printf("    device 0: eglInitialize fallo 0x%x\n", ge ? ge() : 0);
                    }
                } else {
                    printf("    device 0: eglGetPlatformDisplay(DEVICE_EXT) = NULL (0x%x)\n",
                           ge ? ge() : 0);
                }
            }
        } else {
            printf("  eglQueryDevicesEXT -> fallo\n");
        }
    } else {
        printf("  NO exporta eglQueryDevicesEXT\n");
    }

    if (gp && ini) {
        void *dpy = gp(EGL_PLATFORM_SURFACELESS_MESA, 0, 0);
        if (dpy) {
            int a = 0, b = 0;
            if (ini(dpy, &a, &b)) {
                printf("  SURFACELESS: EGL %d.%d OK\n", a, b);
            } else {
                printf("  SURFACELESS: eglInitialize fallo 0x%x\n", ge ? ge() : 0);
            }
        } else {
            printf("  SURFACELESS: NULL (0x%x)\n", ge ? ge() : 0);
        }
    }
}

int main(void) {
    void *egl = dlopen("libEGL.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!egl) {
        printf("dlopen libEGL.so.1 fallo: %s\n", dlerror());
        return 1;
    }
    inspecciona_vendor("libEGL.so.1 (despachador glvnd)", egl);

    void *vend = dlopen("libEGL_mesa.so.0", RTLD_NOW | RTLD_LOCAL);
    if (!vend) {
        printf("dlopen libEGL_mesa.so.0 fallo: %s\n", dlerror());
    } else {
        inspecciona_vendor("libEGL_mesa.so.0 (vendor de Mesa)", vend);
    }
    return 0;
}
