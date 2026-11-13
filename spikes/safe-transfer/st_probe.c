// Safe Transfer measurement probe (proposal only; not product code).
// Build: clang -O2 -o st_probe st_probe.c
//
//   st_probe volid <path>            volume identity: statfs f_fsid, st_dev, ATTR_VOL_UUID
//   st_probe fullfsync <dir>         does F_FULLFSYNC succeed here? (errno if not)
//   st_probe write <dir> <n> <mb> <chunk_mb> <nocache 0|1> <sync none|fsync|full|chunk>
//                                    write n files of <mb> MiB from RAM in chunks, timing
//                                    write and sync separately; prints residency (mincore)
//   st_probe readback <dir> <n>      re-read the n files from fresh fds with F_NOCACHE,
//                                    printing residency first and MB/s
//   st_probe resident <file>         % of the file's pages in the unified buffer cache
//   st_probe evict <file>            try to drop the file's cached pages (msync MS_INVALIDATE),
//                                    printing residency before and after
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/attr.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static double now(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec + t.tv_nsec / 1e9;
}

static double resident_pct(const char *p) {
  int fd = open(p, O_RDONLY);
  if (fd < 0) return -1;
  struct stat st;
  fstat(fd, &st);
  if (st.st_size == 0) { close(fd); return 0; }
  void *m = mmap(NULL, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
  if (m == MAP_FAILED) { close(fd); return -1; }
  size_t pg = getpagesize(), n = (st.st_size + pg - 1) / pg, in = 0;
  char *v = malloc(n);
  if (mincore(m, st.st_size, v) == 0)
    for (size_t i = 0; i < n; i++) in += (v[i] & 1);
  free(v);
  munmap(m, st.st_size);
  close(fd);
  return 100.0 * in / n;
}

struct volattr { uint32_t len; uuid_t uuid; } __attribute__((packed));

static int cmd_volid(const char *p) {
  struct statfs s;
  if (statfs(p, &s)) { perror("statfs"); return 1; }
  struct stat st;
  stat(p, &st);
  printf("fstype=%s mnt=%s from=%s fsid=%08x:%08x st_dev=%d\n", s.f_fstypename, s.f_mntonname,
         s.f_mntfromname, s.f_fsid.val[0], s.f_fsid.val[1], st.st_dev);
  struct attrlist al = {.bitmapcount = ATTR_BIT_MAP_COUNT, .volattr = ATTR_VOL_INFO | ATTR_VOL_UUID};
  struct volattr va;
  if (getattrlist(s.f_mntonname, &al, &va, sizeof va, 0)) {
    printf("ATTR_VOL_UUID: error %s\n", strerror(errno));
  } else {
    printf("ATTR_VOL_UUID=");
    for (int i = 0; i < 16; i++) printf("%02X", va.uuid[i]);
    printf("\n");
  }
  return 0;
}

static int cmd_fullfsync(const char *dir) {
  char p[1024];
  snprintf(p, sizeof p, "%s/.st_probe_fullfsync", dir);
  int fd = open(p, O_CREAT | O_WRONLY | O_TRUNC, 0600);
  if (fd < 0) { perror("open"); return 1; }
  char buf[65536];
  memset(buf, 7, sizeof buf);
  write(fd, buf, sizeof buf);
  int r = fcntl(fd, F_FULLFSYNC);
  printf("F_FULLFSYNC rc=%d errno=%s\n", r, r ? strerror(errno) : "-");
  int b = fcntl(fd, F_BARRIERFSYNC);
  printf("F_BARRIERFSYNC rc=%d errno=%s\n", b, b ? strerror(errno) : "-");
  close(fd);
  int dfd = open(dir, O_RDONLY);
  r = fcntl(dfd, F_FULLFSYNC);
  printf("dir F_FULLFSYNC rc=%d errno=%s\n", r, r ? strerror(errno) : "-");
  close(dfd);
  unlink(p);
  return 0;
}

static int cmd_write(const char *dir, int n, long mb, long chunk_mb, int nocache, const char *sync) {
  size_t chunk = (size_t)chunk_mb << 20, size = (size_t)mb << 20;
  char *buf = malloc(chunk);
  for (size_t i = 0; i < chunk; i++) buf[i] = (char)(rand() >> 7);
  double tw = 0, ts = 0;
  char p[1024];
  for (int f = 0; f < n; f++) {
    snprintf(p, sizeof p, "%s/f%04d.bin", dir, f);
    int fd = open(p, O_CREAT | O_WRONLY | O_TRUNC, 0600);
    if (fd < 0) { perror("open"); return 1; }
    if (nocache) fcntl(fd, F_NOCACHE, 1);
    for (size_t off = 0; off < size; off += chunk) {
      size_t len = size - off < chunk ? size - off : chunk;
      buf[0] = (char)f; buf[1] = (char)(off >> 20);  // distinct content per chunk
      double t0 = now();
      if (pwrite(fd, buf, len, off) != (ssize_t)len) { perror("pwrite"); return 1; }
      double t1 = now();
      tw += t1 - t0;
      if (!strcmp(sync, "chunk")) { fcntl(fd, F_FULLFSYNC); ts += now() - t1; }
    }
    double t1 = now();
    if (!strcmp(sync, "fsync")) fsync(fd);
    else if (!strcmp(sync, "full") || !strcmp(sync, "chunk")) fcntl(fd, F_FULLFSYNC);
    ts += now() - t1;
    close(fd);
  }
  double total = (double)n * size / 1e6;
  snprintf(p, sizeof p, "%s/f%04d.bin", dir, n - 1);
  printf("write n=%d %ldMiB chunk=%ldMiB nocache=%d sync=%s: write %.3fs, sync %.3fs (%.1f ms/file), "
         "%.0f MB/s durable; last file resident %.0f%%\n",
         n, mb, chunk_mb, nocache, sync, tw, ts, 1000 * ts / n, total / (tw + ts), resident_pct(p));
  return 0;
}

static int cmd_readback(const char *dir, int n, int nocache) {
  char p[1024];
  size_t chunk = 64 << 20;
  char *buf = malloc(chunk);
  double t = 0, bytes = 0, res = 0;
  for (int f = 0; f < n; f++) {
    snprintf(p, sizeof p, "%s/f%04d.bin", dir, f);
    res += resident_pct(p);
    int fd = open(p, O_RDONLY);
    if (nocache) fcntl(fd, F_NOCACHE, 1);
    double t0 = now();
    ssize_t r;
    off_t off = 0;
    while ((r = pread(fd, buf, chunk, off)) > 0) { off += r; bytes += r; }
    t += now() - t0;
    close(fd);
  }
  printf("readback n=%d nocache=%d: %.3fs, %.0f MB/s; avg resident before read %.0f%%\n", n, nocache, t,
         bytes / 1e6 / t, res / n);
  return 0;
}

static int cmd_evict(const char *p) {
  double before = resident_pct(p);
  int fd = open(p, O_RDONLY);
  struct stat st;
  fstat(fd, &st);
  void *m = mmap(NULL, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
  int r = msync(m, st.st_size, MS_INVALIDATE);
  int e = errno;
  munmap(m, st.st_size);
  close(fd);
  printf("evict: msync rc=%d (%s); resident %.0f%% -> %.0f%%\n", r, r ? strerror(e) : "-", before,
         resident_pct(p));
  return 0;
}

int main(int argc, char **argv) {
  if (argc >= 3 && !strcmp(argv[1], "evict")) return cmd_evict(argv[2]);
  if (argc >= 3 && !strcmp(argv[1], "volid")) return cmd_volid(argv[2]);
  if (argc >= 3 && !strcmp(argv[1], "fullfsync")) return cmd_fullfsync(argv[2]);
  if (argc >= 8 && !strcmp(argv[1], "write"))
    return cmd_write(argv[2], atoi(argv[3]), atol(argv[4]), atol(argv[5]), atoi(argv[6]), argv[7]);
  if (argc >= 4 && !strcmp(argv[1], "readback"))
    return cmd_readback(argv[2], atoi(argv[3]), argc >= 5 ? atoi(argv[4]) : 1);
  if (argc >= 3 && !strcmp(argv[1], "resident")) { printf("%.0f%%\n", resident_pct(argv[2])); return 0; }
  fprintf(stderr, "see the comment at the top of st_probe.c\n");
  return 2;
}
