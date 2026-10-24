/* probe_egl2.c — ¿Puede Mesa abrir un contexto GL en el visor Pico?
 *
 * POR QUE ESTA SONDA Y NO LA ANTERIOR (probe_egl.c)
 *
 * La primera sonda usaba dlsym() para buscar eglQueryDevicesEXT y
 * conclusion que "glvnd no expone EGL_EXT_device_base".  FALSO: glvnd
 * esconde los alias "EXT" (solo exporta los entry points marcados PUBLIC)
 * y hay que pedirlos con eglGetProcAddress().  La prueba: la MISMA sonda
 * da el mismo resultado en el contenedor, donde el motor SI abre contexto.
 *
 * Esta sonda usa eglGetProcAddress (como hace SDL) y ademas prueba
 * PLATAFORMAS que la sonda anterior no probaba:
 *
 *   1. EGL_PLATFORM_SURFACELESS_MESA (0x31DD): el display de Mesa que NO
 *      necesita /dev/dri.  Es la via que puede funcionar en Android, donde
 *      SELinux (/dev/dri -> graphics_device) no deja abrir los nodos DRM.
 *   2. EGL_PLATFORM_DEVICE_EXT (0x313F) sobre cada device: la via que usa
 *      el driver offscreen de SDL 2.0.20.
 *
 * En ambos casos se crea un contexto OpenGL de escritorio (no ES) y se
 * imprimen GL_VERSION y GL_RENDERER: eso dice si la GPU es la de verdad.
 *
 * CODIGOS DE SALIDA
 *   0  se abrio un contexto (da igual que sea software o GPU: se imprime)
 *   1  no se abrio ningun contexto
 *
 * USO: adb shell /data/local/tmp/probe_egl2
 */

/* El sysroot de glibc del proyecto NO trae cabeceras: se declaran a mano
 * lo justo (igual que scripts/probe_egl.c). */

typedef unsigned int EGLint;
typedef unsigned int EGLBoolean;

#ifndef NULL
#define NULL ((void *)0)
#endif

extern int printf(const char *fmt, ...);
extern int snprintf(char *str, unsigned long size, const char *fmt, ...);

#define EGL_PLATFORM_DEVICE_EXT 0x313F
#define EGL_PLATFORM_SURFACELESS_MESA 0x31DD
#define EGL_PLATFORM_DEVICE_FD_EXT 0x313E

#define EGL_DEFAULT_DISPLAY ((void *)0)
#define EGL_NO_DISPLAY ((void *)0)
#define EGL_NO_CONTEXT ((void *)0)
#define EGL_NO_SURFACE ((void *)0)
#define EGL_FALSE 0
#define EGL_TRUE 1
#define EGL_NO_ERROR 0x3000
#define EGL_OPENGL_API 0x30A2
#define EGL_CONTEXT_MAJOR_VERSION 0x3098
#define EGL_CONTEXT_MINOR_VERSION 0x30FB
#define EGL_CONTEXT_OPENGL_PROFILE_MASK 0x30FD
#define EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT 0x00000001
#define EGL_WIDTH 0x3057
#define EGL_HEIGHT 0x3056
#define EGL_RENDERABLE_TYPE 0x3040
#define EGL_OPENGL_BIT 0x0008
#define EGL_SURFACE_TYPE 0x3033
#define EGL_PBUFFER_BIT 0x0001
#define EGL_RENDER_BUFFER 0x0D41
#define EGL_BACK_BUFFER 0x0D42
#define EGL_RED_SIZE 0x3024
#define EGL_GREEN_SIZE 0x3023
#define EGL_BLUE_SIZE 0x3022
#define EGL_NONE 0x3038
#define EGL_VENDOR 0x3053
#define EGL_VERSION 0x3054
#define EGL_EXTENSIONS 0x3055

typedef void *EGLDisplay;
typedef void *EGLConfig;
typedef void *EGLContext;
typedef void *EGLSurface;
typedef void *EGLDeviceEXT;
typedef void *EGLNativeDisplayType;

typedef EGLDisplay (*PFN_eglGetPlatformDisplay)(EGLint, void *, const EGLint *);
typedef EGLDisplay (*PFN_eglGetDisplay)(void *);
typedef EGLDisplay (*PFN_eglGetPlatformDisplayEXT)(EGLint, void *, const EGLint *);
typedef EGLBoolean (*PFN_eglInitialize)(EGLDisplay, EGLint *, EGLint *);
typedef EGLBoolean (*PFN_eglTerminate)(EGLDisplay);
typedef const char *(*PFN_eglQueryString)(EGLDisplay, EGLint);
typedef EGLint (*PFN_eglGetError)(void);
typedef EGLBoolean (*PFN_eglBindAPI)(EGLint);
typedef EGLBoolean (*PFN_eglChooseConfig)(EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *);
typedef EGLContext (*PFN_eglCreateContext)(EGLDisplay, EGLConfig, EGLContext, const EGLint *);
typedef EGLBoolean (*PFN_eglMakeCurrent)(EGLDisplay, EGLSurface, EGLSurface, EGLContext);
typedef EGLBoolean (*PFN_eglQueryDevicesEXT)(EGLint, EGLDeviceEXT *, EGLint *);
typedef void *(*PFN_eglGetProcAddress)(const char *);
typedef const unsigned char *(*PFN_glGetString)(unsigned int);

/* Entry points PUBLIC de glvnd: se enlazan directamente. */
extern void *eglGetProcAddress(const char *name);
extern const unsigned char *glGetString(unsigned int name);

#define GL_VENDOR 0x1F00
#define GL_RENDERER 0x1F01
#define GL_VERSION 0x1F02
#define GL_SHADING_LANGUAGE_VERSION 0x8B8C

static PFN_eglGetProcAddress pGetProc;
static PFN_eglInitialize pInitialize;
static PFN_eglTerminate pTerminate;
static PFN_eglQueryString pQueryString;
static PFN_eglGetError pGetError;
static PFN_eglBindAPI pBindAPI;
static PFN_eglChooseConfig pChooseConfig;
static PFN_eglCreateContext pCreateContext;
static PFN_eglMakeCurrent pMakeCurrent;
static PFN_eglQueryDevicesEXT pQueryDevices;
static PFN_eglGetPlatformDisplay pGetPlatformDisplay;
static PFN_eglGetPlatformDisplayEXT pGetPlatformDisplayEXT;

static const char *err_name(EGLint e)
{
	switch (e) {
	case 0x3000: return "EGL_SUCCESS";
	case 0x3001: return "EGL_NOT_INITIALIZED";
	case 0x3002: return "EGL_BAD_ACCESS";
	case 0x3003: return "EGL_BAD_ALLOC";
	case 0x3004: return "EGL_BAD_ATTRIBUTE";
	case 0x3005: return "EGL_BAD_CONFIG";
	case 0x3006: return "EGL_BAD_CONTEXT";
	case 0x3007: return "EGL_BAD_CURRENT_SURFACE";
	case 0x3008: return "EGL_BAD_DISPLAY";
	case 0x3009: return "EGL_BAD_SURFACE";
	case 0x300A: return "EGL_BAD_MATCH";
	case 0x300B: return "EGL_BAD_PARAMETER";
	case 0x300C: return "EGL_BAD_NATIVE_PIXMAP";
	case 0x300D: return "EGL_BAD_NATIVE_WINDOW";
	case 0x300E: return "EGL_CONTEXT_LOST";
	default: return "EGL_?";
	}
}

static void *resolver(void *lib, const char *name)
{
	(void)lib;
	return pGetProc ? pGetProc(name) : NULL;
}

/* Intenta abrir un contexto OpenGL de escritorio en el display dado.
 * Devuelve 1 si lo consigue. */
static int prueba_contexto(EGLDisplay dpy, const char *etiqueta)
{
	EGLConfig cfg = NULL;
	EGLContext ctx = NULL;
	EGLint num = 0, i;
	static const EGLint cfg_attr[] = {
		EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
		EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT,
		EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
		EGL_NONE
	};
	static const EGLint ctx_attr[] = {
		EGL_CONTEXT_MAJOR_VERSION, 3,
		EGL_CONTEXT_MINOR_VERSION, 3,
		EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
		EGL_NONE
	};

	if (dpy == EGL_NO_DISPLAY) {
		printf("  %-22s : sin display (%s)\n", etiqueta, err_name(pGetError()));
		return 0;
	}
	if (!pInitialize(dpy, NULL, NULL)) {
		printf("  %-22s : eglInitialize FALLO (%s)\n", etiqueta, err_name(pGetError()));
		return 0;
	}
	printf("  %-22s : eglInitialize OK  version=%s vendor=%s\n", etiqueta,
	       pQueryString(dpy, EGL_VERSION) ? pQueryString(dpy, EGL_VERSION) : "?",
	       pQueryString(dpy, EGL_VENDOR) ? pQueryString(dpy, EGL_VENDOR) : "?");

	pBindAPI(EGL_OPENGL_API);
	if (!pChooseConfig(dpy, cfg_attr, &cfg, 1, &num) || num < 1) {
		printf("      eglChooseConfig FALLO (%s)\n", err_name(pGetError()));
		pTerminate(dpy);
		return 0;
	}
	ctx = pCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attr);
	if (ctx == EGL_NO_CONTEXT) {
		printf("      eglCreateContext FALLO (%s)\n", err_name(pGetError()));
		pTerminate(dpy);
		return 0;
	}
	/* Sin pbuffer: se hace current con superficie nula (EGL 1.5 /
	 * EGL_KHR_surfaceless_context).  Si no esta soportado, da error, pero
	 * el contexto ya esta creado: eso es lo que queremos medir. */
	pMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, ctx);
	printf("      eglMakeCurrent: %s\n", pGetError() == EGL_NO_ERROR ? "OK" :
	       err_name(pGetError()));

	{
		PFN_glGetString glGetString = (PFN_glGetString)resolver(NULL, "glGetString");
		if (glGetString) {
			const unsigned char *v = glGetString(GL_VERSION);
			const unsigned char *r = glGetString(GL_RENDERER);
			const unsigned char *ve = glGetString(GL_VENDOR);
			printf("      GL_VERSION  : %s\n", v ? (const char *)v : "(null)");
			printf("      GL_RENDERER : %s\n", r ? (const char *)r : "(null)");
			printf("      GL_VENDOR   : %s\n", ve ? (const char *)ve : "(null)");
		} else {
			printf("      glGetString no disponible\n");
		}
	}
	pTerminate(dpy);
	return 1;
}

int main(void)
{
	EGLDeviceEXT devices[16];
	EGLint num_devices = 0;
	int ok = 0;
	int i;

	/* glvnd exporta estos dos como entry points PUBLIC: se enlazan
	 * directamente.  Todo lo demas (eglQueryDevicesEXT,
	 * eglGetPlatformDisplayEXT) se pide con eglGetProcAddress, porque glvnd
	 * esconde los alias "EXT" en la tabla de simbolos. */
	pGetProc = eglGetProcAddress;

	pInitialize = (PFN_eglInitialize)resolver(NULL, "eglInitialize");
	pTerminate = (PFN_eglTerminate)resolver(NULL, "eglTerminate");
	pQueryString = (PFN_eglQueryString)resolver(NULL, "eglQueryString");
	pGetError = (PFN_eglGetError)resolver(NULL, "eglGetError");
	pBindAPI = (PFN_eglBindAPI)resolver(NULL, "eglBindAPI");
	pChooseConfig = (PFN_eglChooseConfig)resolver(NULL, "eglChooseConfig");
	pCreateContext = (PFN_eglCreateContext)resolver(NULL, "eglCreateContext");
	pMakeCurrent = (PFN_eglMakeCurrent)resolver(NULL, "eglMakeCurrent");
	pQueryDevices = (PFN_eglQueryDevicesEXT)resolver(NULL, "eglQueryDevicesEXT");
	pGetPlatformDisplay = (PFN_eglGetPlatformDisplay)resolver(NULL, "eglGetPlatformDisplay");
	pGetPlatformDisplayEXT = (PFN_eglGetPlatformDisplayEXT)resolver(NULL, "eglGetPlatformDisplayEXT");

	printf("=== funciones resueltas por eglGetProcAddress ===\n");
	printf("  eglQueryDevicesEXT      : %s\n", pQueryDevices ? "si" : "NO");
	printf("  eglGetPlatformDisplay    : %s\n", pGetPlatformDisplay ? "si" : "NO");
	printf("  eglGetPlatformDisplayEXT : %s\n", pGetPlatformDisplayEXT ? "si" : "NO");

	if (pQueryString) {
		const char *ext = pQueryString(EGL_NO_DISPLAY, EGL_EXTENSIONS);
		printf("\n=== extensiones de cliente (EGL_NO_DISPLAY) ===\n");
		printf("  %s\n", ext ? ext : "(null)");
	}

	printf("\n=== devices ===\n");
	if (pQueryDevices) {
		if (pQueryDevices(16, devices, &num_devices) == EGL_TRUE)
			printf(" eglQueryDevicesEXT -> %d device(s)\n", num_devices);
		else
			printf("  eglQueryDevicesEXT FALLO (%s)\n", err_name(pGetError()));
	} else {
		printf("  (no hay eglQueryDevicesEXT)\n");
	}

	printf("\n=== via DEVICE (la que usa el driver offscreen de SDL) ===\n");
	for (i = 0; i < num_devices; i++) {
		char et[64];
		EGLDisplay dpy = EGL_NO_DISPLAY;
		if (pGetPlatformDisplayEXT)
			dpy = pGetPlatformDisplayEXT(EGL_PLATFORM_DEVICE_EXT, devices[i], NULL);
		else if (pGetPlatformDisplay)
			dpy = pGetPlatformDisplay(EGL_PLATFORM_DEVICE_EXT, devices[i], NULL);
		snprintf(et, sizeof(et), "device %d", i);
		if (prueba_contexto(dpy, et))
			ok = 1;
	}

	printf("\n=== via SURFACELESS (no necesita /dev/dri) ===\n");
	{
		EGLDisplay dpy = EGL_NO_DISPLAY;
		if (pGetPlatformDisplay)
			dpy = pGetPlatformDisplay(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
		else if (pGetPlatformDisplayEXT)
			dpy = pGetPlatformDisplayEXT(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
		if (prueba_contexto(dpy, "surfaceless"))
			ok = 1;
	}

	printf("\n=== via DEFAULT (eglGetDisplay) ===\n");
	{
		PFN_eglGetDisplay pGetDisplay =
			(PFN_eglGetDisplay)resolver(NULL, "eglGetDisplay");
		if (pGetDisplay) {
			EGLDisplay dpy = pGetDisplay(EGL_DEFAULT_DISPLAY);
			if (prueba_contexto(dpy, "default"))
				ok = 1;
		} else {
			printf("  (no hay eglGetDisplay)\n");
		}
	}

	printf("\n=== VEREDICTO: %s ===\n", ok ? "se abrio un contexto" : "no se abrio ninguno");
	return ok ? 0 : 1;
}
