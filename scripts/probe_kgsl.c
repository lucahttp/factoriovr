/* probe_kgsl.c — Sonda de bajo nivel del nodo de GPU de KGSL (Adreno).
 *
 * POR QUE EXISTE
 * --------------
 * Para que el cliente grafico de Factorio use la GPU de verdad en el visor
 * hace falta Turnip (el driver Vulkan de Mesa para Adreno), que habla con el
 * kernel por /dev/kgsl-3d0.  En el visor ese nodo es 666, asi que SE ABRE:
 * lo que puede estar prohibido es el ioctl que RESERVA memoria de GPU.
 *
 * Precedente medido en otro dispositivo Android con GPU Adreno y SELinux
 * enforcing (DroidDeck issue #245, Quest 3S): la CONSULTA del dispositivo
 * funciona, pero el ioctl de reserva de memoria devuelve EACCES y Turnip lo
 * traduce a VK_ERROR_OUT_OF_DEVICE_MEMORY.  El driver del fabricante no sufre
 * ese problema porque reserva por el gralloc (un HAL privilegiado).
 *
 * Esta sonda separa las dos cosas: si el nodo abre, que dice el kernel y si
 * la reserva de memoria esta permitida.  Sin esto, un fallo de Turnip en el
 * visor es indistinguible de un fallo de la cadena GL.
 *
 * QUE MIDE
 *   1. sysfs: gpu_model, gpu_chip_id (se leen sin ioctl: siempre funcionan).
 *   2. open(/dev/kgsl-3d0): ¿se puede abrir? ¿con que contexto SELinux?
 *   3. IOCTL_KGSL_DEVICE_GETPROPERTY / KGSL_PROP_DEVICE_INFO: la consulta.
 *   4. IOCTL_KGSL_GPUMEM_ALLOC_ID con 4K, 64K y 1M: la reserva de memoria.
 *   5. IOCTL_KGSL_GPUOBJ_ALLOC: la otra familia de reserva (esta si la
 *      permitia el policy del Quest 3S).
 *
 * CODIGOS DE SALIDA
 *   0  la GPU se puede usar (abre, consulta y RESERVA memoria)
 *   1  la GPU NO se puede usar desde este proceso (falta un permiso)
 *   2  no se pudo ni intentarlo (no hay nodo, no se pudo abrir)
 *
 * COMPILACION (ARM64, bionic: es un binario nativo del visor, no del runtime
 * glibc, para poder lanzar el en el dominio SELinux del shell tal cual):
 *   aarch64-linux-android34-clang -O2 -o probe_kgsl probe_kgsl.c
 *
 * USO: adb shell /data/local/tmp/probe_kgsl
 */

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

/* ---------------------------------------------------------------------------
 * Cabecera UAPI de KGSL.
 *
 * Copiada VERBATIM de include/uapi/linux/msm_kgsl.h (arbol LineageOS
 * android_kernel_qcom_sm8250, KGSL 3.14).  El NDK no la trae, asi que se
 * declara aqui.  Importante: el TAMANO de cada estructura va dentro del
 * numero de ioctl (_IOC codifica el size), asi que si una struct aqui no
 * coincide con la del kernel, el kernel responde ENOTTY y no hay ningun
 * error de compilacion que lo avise.  Por eso hay asserts de tamano abajo.
 * ------------------------------------------------------------------------- */
#define KGSL_IOC_TYPE 0x09

#define KGSL_PROP_DEVICE_INFO 0x1

struct kgsl_devinfo {
	unsigned int device_id;
	unsigned int chip_id;
	unsigned int mmu_enabled;
	unsigned long gmem_gpubaseaddr;
	unsigned int gpu_id;
	size_t gmem_sizebytes;
};

struct kgsl_device_getproperty {
	unsigned int type;
	void *value;
	size_t sizebytes;
};

struct kgsl_gpumem_alloc_id {
	unsigned int id;
	unsigned int flags;
	size_t size;
	size_t mmapsize;
	unsigned long gpuaddr;
	unsigned long __pad[2];
};

struct kgsl_gpumem_free_id {
	unsigned int id;
	unsigned long __pad;
};

struct kgsl_gpuobj_alloc {
	uint64_t size;
	uint64_t flags;
	uint64_t va_len;
	uint64_t mmapsize;
	unsigned int id;
	unsigned int metadata_len;
	uint64_t metadata;
};

#define IOCTL_KGSL_DEVICE_GETPROPERTY \
	_IOWR(KGSL_IOC_TYPE, 0x2, struct kgsl_device_getproperty)
#define IOCTL_KGSL_GPUMEM_ALLOC_ID \
	_IOWR(KGSL_IOC_TYPE, 0x34, struct kgsl_gpumem_alloc_id)
#define IOCTL_KGSL_GPUMEM_FREE_ID \
	_IOWR(KGSL_IOC_TYPE, 0x35, struct kgsl_gpumem_free_id)
#define IOCTL_KGSL_GPUOBJ_ALLOC \
	_IOWR(KGSL_IOC_TYPE, 0x45, struct kgsl_gpuobj_alloc)

/* Los tamanos que espera el kernel.  Si alguno cambia, el ioctl deja de
 * coincidir y el kernel responde ENOTTY: mejor que se note al compilar. */
_Static_assert(sizeof(struct kgsl_device_getproperty) == 24, "getproperty");
_Static_assert(sizeof(struct kgsl_gpumem_alloc_id) == 48, "alloc_id");
_Static_assert(sizeof(struct kgsl_gpumem_free_id) == 16, "free_id");
_Static_assert(sizeof(struct kgsl_gpuobj_alloc) == 48, "gpuobj");
_Static_assert(sizeof(struct kgsl_devinfo) == 40, "devinfo");

#define NODO "/dev/kgsl-3d0"
#define SYSFS "/sys/class/kgsl/kgsl-3d0"

static void lee_sysfs(const char *nombre) {
	char ruta[256];
	char buf[256];
	FILE *f;
	size_t n;

	snprintf(ruta, sizeof(ruta), "%s/%s", SYSFS, nombre);
	f = fopen(ruta, "r");
	if (!f) {
		printf("  %-14s : (no se puede leer: %s)\n", nombre, strerror(errno));
		return;
	}
	n = fread(buf, 1, sizeof(buf) - 1, f);
	fclose(f);
	buf[n] = 0;
	while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r'))
		buf[--n] = 0;
	printf("  %-14s : %s\n", nombre, n ? buf : "(vacio)");
}

static void muestra_contexto_selinux(void) {
	char buf[256];
	FILE *f = fopen("/proc/self/attr/current", "r");
	size_t n;
	if (!f) {
		printf("  contexto SELinux: (no se puede leer /proc/self/attr/current)\n");
		return;
	}
	n = fread(buf, 1, sizeof(buf) - 1, f);
	fclose(f);
	buf[n] = 0;
	while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r'))
		buf[--n] = 0;
	printf("  contexto SELinux: %s\n", buf);
}

/* Reserva de memoria "clásica" (la que usa Turnip para cada buffer). */
static int prueba_alloc_id(int fd, size_t tam) {
	struct kgsl_gpumem_alloc_id a;
	int r;

	memset(&a, 0, sizeof(a));
	a.size = tam;
	r = ioctl(fd, IOCTL_KGSL_GPUMEM_ALLOC_ID, &a);
	if (r == 0) {
		printf("    %8zu bytes : OK (id=%u gpuaddr=0x%lx)\n", tam, a.id, a.gpuaddr);
		/* Se libera: deja el dispositivo como estaba. */
		{
			struct kgsl_gpumem_free_id f;
			memset(&f, 0, sizeof(f));
			f.id = a.id;
			ioctl(fd, IOCTL_KGSL_GPUMEM_FREE_ID, &f);
		}
		return 0;
	}
	printf("    %8zu bytes : FALLO (%s)\n", tam, strerror(errno));
	return -1;
}

/* La otra familia de reserva (gpuobj).  En el Quest 3S esta si estaba
 * permitida aunque la de arriba no lo estuviera. */
static int prueba_gpuobj(int fd, size_t tam) {
	struct kgsl_gpuobj_alloc a;
	int r;

	memset(&a, 0, sizeof(a));
	a.size = tam;
	a.va_len = tam;
	r = ioctl(fd, IOCTL_KGSL_GPUOBJ_ALLOC, &a);
	if (r == 0) {
		printf("    %8zu bytes : OK (id=%u)\n", tam, a.id);
		return 0;
	}
	printf("    %8zu bytes : FALLO (%s)\n", tam, strerror(errno));
	return -1;
}

int main(void) {
	int fd;
	int consulta_ok = 0;
	int reserva_ok = 0;

	printf("=== 1. Informacion del dispositivo (sysfs, sin ioctl) ===\n");
	lee_sysfs("gpu_model");
	lee_sysfs("gpu_chip_id");
	lee_sysfs("gpubusy");
	muestra_contexto_selinux();

	printf("\n=== 2. Abrir el nodo de GPU ===\n");
	fd = open(NODO, O_RDWR);
	if (fd < 0) {
		printf("  open(%s) FALLO: %s\n", NODO, strerror(errno));
		printf("\nVEREDICTO: no se puede ni abrir el nodo de GPU.\n");
		printf("          Turnip NO puede funcionar aqui.\n");
		return 2;
	}
	printf("  open(%s) OK (fd=%d)\n", NODO, fd);

	printf("\n=== 3. Consulta del dispositivo (ioctl) ===\n");
	printf("  ioctl DEVICE_GETPROPERTY = 0x%lx\n", (unsigned long)IOCTL_KGSL_DEVICE_GETPROPERTY);
	{
		struct kgsl_device_getproperty prop;
		struct kgsl_devinfo info;
		memset(&prop, 0, sizeof(prop));
		memset(&info, 0, sizeof(info));
		prop.type = KGSL_PROP_DEVICE_INFO;
		prop.value = &info;
		prop.sizebytes = sizeof(info);
		if (ioctl(fd, IOCTL_KGSL_DEVICE_GETPROPERTY, &prop) == 0) {
			printf("    chip_id=0x%08x gpu_id=%u mmu=%u gmem=%zu bytes\n",
			       info.chip_id, info.gpu_id, info.mmu_enabled,
			       (size_t)info.gmem_sizebytes);
			consulta_ok = 1;
		} else {
			printf("    FALLO: %s\n", strerror(errno));
			if (errno == ENOTTY)
				printf("    (ENOTTY: el numero de ioctl no coincide con el del kernel)\n");
		}
	}

	printf("\n=== 4. Reserva de memoria de GPU (lo que hace Turnip) ===\n");
	printf("  ioctl GPUMEM_ALLOC_ID = 0x%lx\n", (unsigned long)IOCTL_KGSL_GPUMEM_ALLOC_ID);
	if (prueba_alloc_id(fd, 4096) == 0) reserva_ok = 1;
	prueba_alloc_id(fd, 65536);
	prueba_alloc_id(fd, 1048576);

	printf("\n=== 5. La otra familia de reserva (gpuobj) ===\n");
	printf("  ioctl GPUOBJ_ALLOC = 0x%lx\n", (unsigned long)IOCTL_KGSL_GPUOBJ_ALLOC);
	prueba_gpuobj(fd, 4096);

	close(fd);

	printf("\n=== VEREDICTO ===\n");
	if (reserva_ok) {
		printf("  GPU UTILIZABLE: este proceso puede reservar memoria de GPU.\n");
		printf("  Turnip deberia poder crear un device Vulkan sobre el Adreno.\n");
		return 0;
	}
	if (consulta_ok) {
		printf("  GPU PARCIAL: se puede consultar el dispositivo pero NO reservar memoria.\n");
		printf("  Turnip fallara al crear el device (VK_ERROR_OUT_OF_DEVICE_MEMORY).\n");
		printf("  Es el policy de SELinux del fabricante (precedente: Quest 3S).\n");
		return 1;
	}
	printf("  GPU NO UTILIZABLE desde este proceso.\n");
	return 1;
}
