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
//   st_probe batch <dir> <n> <group> <mode> [verify 0|1]
//                                    the small-file pipeline for n files of 1-64 KiB
//                                    (deterministic sizes and content): write partials with
//                                    F_NOCACHE, then per <mode>, then evict + read back +
//                                    compare + renamex_np(RENAME_EXCL) + one dir flush per group.
//                                    modes: perfile_full (F_FULLFSYNC each file),
//                                    fsync_group (fsync each, one F_FULLFSYNC per group),
//                                    nocache_group (no per-file call, one F_FULLFSYNC per group),
//                                    none (no flush at all; for comparison only)
//   st_probe batchwrite <dir> <n> <mode>
//                                    writes final-named files only (no verify/rename), for
//                                    yank checks: modes cached_none | nocache_none | fsync_each
//                                    | cached_group_full | nocache_group_full
//   st_probe batchcheck <dir> <n>    counts files that are missing, short or wrong
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

static size_t small_size(int i) { return 1024 + (size_t)((i * 2654435761u) % (63 * 1024)); }
static void small_fill(char *b, size_t n, int i) {
  uint32_t x = 2166136261u ^ (uint32_t)i;
  for (size_t k = 0; k < n; k++) { x = x * 16777619u + 7; b[k] = (char)(x >> 13); }
}

static int cmd_batch(const char *dir, int n, int group, const char *mode, int verify) {
  char *buf = malloc(65536), *back = malloc(65536);
  int perfile = !strcmp(mode, "perfile_full"), fs = !strcmp(mode, "fsync_group"),
      nc = !strcmp(mode, "nocache_group"), none = !strcmp(mode, "none");
  if (!(perfile || fs || nc || none)) { fprintf(stderr, "bad mode\n"); return 2; }
  double t0 = now(), tsync = 0, tverify = 0;
  int barriers = 0;
  char p[1024], q[1024];
  for (int g = 0; g < n; g += group) {
    int end = g + group < n ? g + group : n;
    int lastfd = -1;
    for (int i = g; i < end; i++) {
      size_t sz = small_size(i);
      small_fill(buf, sz, i);
      snprintf(p, sizeof p, "%s/.f%05d.st-partial", dir, i);
      int fd = open(p, O_CREAT | O_EXCL | O_WRONLY, 0600);
      if (fd < 0) { perror("open"); return 1; }
      fcntl(fd, F_NOCACHE, 1);
      if (write(fd, buf, sz) != (ssize_t)sz) { perror("write"); return 1; }
      double ts = now();
      if (perfile) { fcntl(fd, F_FULLFSYNC); barriers++; }
      if (fs) fsync(fd);
      tsync += now() - ts;
      if (lastfd >= 0) close(lastfd);
      lastfd = fd;
    }
    double ts = now();
    if ((fs || nc) && lastfd >= 0) { fcntl(lastfd, F_FULLFSYNC); barriers++; }
    tsync += now() - ts;
    if (lastfd >= 0) close(lastfd);
    for (int i = g; i < end; i++) {
      snprintf(p, sizeof p, "%s/.f%05d.st-partial", dir, i);
      snprintf(q, sizeof q, "%s/f%05d.bin", dir, i);
      if (verify) {
        double tv = now();
        size_t sz = small_size(i);
        int fd = open(p, O_RDONLY);
        void *m = mmap(NULL, sz, PROT_READ, MAP_SHARED, fd, 0);
        msync(m, sz, MS_SYNC | MS_INVALIDATE);
        munmap(m, sz);
        fcntl(fd, F_NOCACHE, 1);
        ssize_t r = pread(fd, back, sz, 0);
        close(fd);
        small_fill(buf, sz, i);
        if (r != (ssize_t)sz || memcmp(buf, back, sz)) { fprintf(stderr, "verify failed %d\n", i); return 1; }
        tverify += now() - tv;
      }
      if (renamex_np(p, q, RENAME_EXCL)) {
        if (errno != ENOTSUP && errno != EINVAL) { perror("renamex_np"); return 1; }
        rename(p, q);
      }
    }
    int dfd = open(dir, O_RDONLY);
    ts = now();
    if (!none) fcntl(dfd, F_FULLFSYNC);
    tsync += now() - ts;
    close(dfd);
  }
  double t = now() - t0;
  printf("batch n=%d group=%d mode=%-13s verify=%d: %.2f s total (sync %.2f s, verify %.2f s), "
         "%d data barriers + %d dir flushes, %.2f ms/file\n",
         n, group, mode, verify, t, tsync, tverify, barriers, none ? 0 : (n + group - 1) / group,
         1000 * t / n);
  return 0;
}

static int cmd_batchwrite(const char *dir, int n, const char *mode) {
  char *buf = malloc(65536);
  int nocache = strstr(mode, "nocache") != NULL, each = !strcmp(mode, "fsync_each"),
      group = strstr(mode, "group_full") != NULL;
  char p[1024];
  int lastfd = -1;
  for (int i = 0; i < n; i++) {
    size_t sz = small_size(i);
    small_fill(buf, sz, i);
    snprintf(p, sizeof p, "%s/f%05d.bin", dir, i);
    int fd = open(p, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    if (fd < 0) { perror("open"); return 1; }
    if (nocache) fcntl(fd, F_NOCACHE, 1);
    write(fd, buf, sz);
    if (each) fsync(fd);
    if (lastfd >= 0) close(lastfd);
    lastfd = fd;
  }
  if (group && lastfd >= 0) fcntl(lastfd, F_FULLFSYNC);
  if (lastfd >= 0) close(lastfd);
  if (group || each) { int d = open(dir, O_RDONLY); fcntl(d, F_FULLFSYNC); close(d); }
  printf("wrote %d files, mode %s\n", n, mode);
  fflush(stdout);
  return 0;
}

static int cmd_batchcheck(const char *dir, int n) {
  char *buf = malloc(65536), *back = malloc(65536);
  int missing = 0, shortf = 0, wrong = 0;
  char p[1024];
  for (int i = 0; i < n; i++) {
    size_t sz = small_size(i);
    snprintf(p, sizeof p, "%s/f%05d.bin", dir, i);
    int fd = open(p, O_RDONLY);
    if (fd < 0) { missing++; continue; }
    ssize_t r = pread(fd, back, sz, 0);
    close(fd);
    small_fill(buf, sz, i);
    if (r != (ssize_t)sz) shortf++;
    else if (memcmp(buf, back, sz)) wrong++;
  }
  printf("check %d files: %d missing, %d short, %d wrong content, %d intact\n", n, missing, shortf,
         wrong, n - missing - shortf - wrong);
  return 0;
}

int main(int argc, char **argv) {
  if (argc >= 6 && !strcmp(argv[1], "batch"))
    return cmd_batch(argv[2], atoi(argv[3]), atoi(argv[4]), argv[5], argc >= 7 ? atoi(argv[6]) : 1);
  if (argc >= 5 && !strcmp(argv[1], "batchwrite")) return cmd_batchwrite(argv[2], atoi(argv[3]), argv[4]);
  if (argc >= 4 && !strcmp(argv[1], "batchcheck")) return cmd_batchcheck(argv[2], atoi(argv[3]));
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
