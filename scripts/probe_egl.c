/* probe_egl.c — Sonda EGL minima para el visor (Pico 4 Ultra).
 *
 * Responde: ¿que ve Mesa en este dispositivo?  Con esto se diagnostica el
 * "Could not find a valid EGL device to initialize" que SDL2 tira al crear
 * la ventana: el cliente grafico de Factorio lo dice sin mas detalle.
 *
 * NO lleva includes a proposito: el sysroot de glibc del proyecto trae las
 * librerias pero no las cabeceras, y las de la NDK son de bionic.  Se
 * declaran a mano las cuatro funciones que se usan.
 *
 * Se compila contra el sysroot de glibc ARM64:
 *   aarch64-linux-android34-clang --target=aarch64-linux-gnu \
 *       --sysroot=build/glibc_sysroot -O2 -o probe_egl probe_egl.c \
 *       -Lbuild/glibc_sysroot/link -ldl
 * y se ejecuta en el visor con el loader de glibc del payload.
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

    /* --- El vendor de Mesa por su cuenta: sin glvnd por medio --- */
    {
        void *vend = dlopen("libEGL_mesa.so.0", RTLD_NOW | RTLD_LOCAL);
        if (!vend) {
            printf("dlopen libEGL_mesa.so.0 fallo: %s\n", dlerror());
        } else {
            printf("libEGL_mesa.so.0: cargada\n");
            fn_query_string vq = (fn_query_string)dlsym(vend, "eglQueryString");
            fn_get_platform_display vgp = (fn_get_platform_display)dlsym(vend, "eglGetPlatformDisplay");
            fn_initialize vi = (fn_initialize)dlsym(vend, "eglInitialize");
            fn_get_error vge = (fn_get_error)dlsym(vend, "eglGetError");
            if (vq) {
                const char *ve = vq(EGL_NO_DISPLAY, EGL_EXTENSIONS);
                printf("  extensions del vendor: %s\n", ve ? ve : "(null)");
                printf("  EGL_EXT_device_base: %s\n", contiene(ve, "EGL_EXT_device_base") ? "SI" : "NO");
                printf("  EGL_MESA_platform_surfaceless: %s\n",
                       contiene(ve, "EGL_MESA_platform_surfaceless") ? "SI" : "NO");
            }
            if (vgp && vi) {
                void *vdpy = vgp(EGL_PLATFORM_SURFACELESS_MESA, 0, 0);
                if (vdpy) {
                    int a = 0, b = 0;
                    if (vi(vdpy, &a, &b)) {
                        printf("  vendor SURFACELESS: EGL %d.%d OK\n", a, b);
                        const char *v = vq(vdpy, EGL_VERSION);
                        printf("    version: %s\n", v ? v : "(?)");
                        const char *de = vq(vdpy, EGL_EXTENSIONS);
                        printf("    extensions del display: %s\n", de ? de : "(null)");
                    } else {
                        printf("  vendor SURFACELESS: eglInitialize fallo 0x%x\n", vge ? vge() : 0);
                    }
                } else {
                    printf("  vendor SURFACELESS: NULL (eglGetError=0x%x)\n", vge ? vge() : 0);
                }
            }
        }
    }
    return 0;
}

int main(void) {
    void *egl = dlopen("libEGL.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!egl) {
        printf("dlopen libEGL.so.1 fallo: %s\n", dlerror());
        return 1;
    }
    printf("libEGL.so.1: cargada\n");

    fn_query_string query_string = (fn_query_string)dlsym(egl, "eglQueryString");
    fn_query_devices query_devices = (fn_query_devices)dlsym(egl, "eglQueryDevicesEXT");
    fn_get_platform_display get_platform_display =
        (fn_get_platform_display)dlsym(egl, "eglGetPlatformDisplay");
    fn_initialize initialize = (fn_initialize)dlsym(egl, "eglInitialize");
    fn_get_error get_error = (fn_get_error)dlsym(egl, "eglGetError");

    if (query_string) {
        const char *exts = query_string(EGL_NO_DISPLAY, EGL_EXTENSIONS);
        printf("extensiones del cliente EGL:\n  %s\n", exts ? exts : "(ninguna)");
        printf("  EGL_EXT_device_base: %s\n", contiene(exts, "EGL_EXT_device_base") ? "SI" : "NO");
        printf("  EGL_MESA_platform_surfaceless: %s\n",
               contiene(exts, "EGL_MESA_platform_surfaceless") ? "SI" : "NO");
        printf("  EGL_MESA_device_software: %s\n",
               contiene(exts, "EGL_MESA_device_software") ? "SI" : "NO");
    } else {
        printf("no hay eglQueryString\n");
    }

    if (query_devices) {
        void *devs[16];
        int n = 0;
        if (query_devices(16, devs, &n)) {
            printf("eglQueryDevicesEXT -> %d device(s)\n", n);
            for (int i = 0; i < n && i < 8; ++i) {
                const char *exts = query_string(devs[i], EGL_EXTENSIONS);
                printf("  device %d: %s\n", i, exts ? exts : "(sin extensions)");
            }
        } else {
            printf("eglQueryDevicesEXT -> fallo\n");
        }
    } else {
        printf("no hay eglQueryDevicesEXT (EGL_EXT_device_base no presente)\n");
    }

    if (get_platform_display && initialize) {
        void *dpy = get_platform_display(EGL_PLATFORM_SURFACELESS_MESA, 0, 0);
        if (dpy) {
            int maj = 0, min = 0;
            if (initialize(dpy, &maj, &min)) {
                printf("plataforma SURFACELESS: EGL %d.%d inicializada\n", maj, min);
                const char *v = query_string(dpy, EGL_VERSION);
                printf("  version: %s\n", v ? v : "(?)");
            } else {
                printf("plataforma SURFACELESS: eglInitialize fallo, eglGetError=0x%x\n",
                       get_error ? get_error() : 0);
            }
        } else {
            printf("plataforma SURFACELESS: eglGetPlatformDisplay devolvio NULL\n");
        }
    }
    return 0;
}
