# Building Espressif's QEMU (xtensa-softmmu) + semihosting patch: one shot

Everything below is derived from an actual end-to-end build of the tree
(00:11 -> 00:42, ~30 min wall clock). Each "gotcha" was a real failure; the
script at the bottom incorporates every fix. Run it top to bottom and it
produces a working `qemu-system-xtensa` with esp32/esp32s3 machines.

> Note: this snapshot of esp-develop has `esp32` and `esp32s3` machines but
> **no esp32s2**. If you need esp32s2, check out a newer esp-develop first.

## Prerequisites

* Linux x86_64 with a working C toolchain (`cc`, `make`, `pkg-config`).
* For **building/running Xtensa test firmware** (as opposed to just running
  QEMU): the Espressif Rust toolchain, which ships the precompiled core
  libraries (`rust-std`) for `xtensa-esp32-none-elf`/`s2`/`s3`.
  Standard rustup lists the triples but -- being tier-3 -- provides no
  prebuilt `core`; an `xtensa-*` cross-build without espup fails at
  `error[E0463]: can't find crate for 'core'`.
  ```console
  $ cargo install espup && espup install --targets esp32
  $ . ~/export-esp.sh        # sets XTENSA_ESP32_RUSTUP_TOOLCHAIN etc.
  ```
* NOTE: `xtensa-esp32s2-none-elf` needs the espressif toolchain too; QEMU's
  machine coverage varies by snapshot (this one: esp32 + esp32s3).
* Network access to a Debian-family mirror (for rootless library extraction;
  root is not needed for anything here).
* ~3 GB disk for the source + build tree, ~10 min for the build itself.

## Step 1: sources

```console
# Option A: clone the pre-patched fork (no patching needed)
$ git clone --depth 1 -b esp-develop-semihosting --recursive https://github.com/kokroo/qemu esp-qemu
$ cd esp-qemu

# Option B: clone upstream and apply the patch
$ git clone --depth 1 -b esp-develop --recursive https://github.com/espressif/qemu esp-qemu
$ cd esp-qemu
$ patch -p1 --dry-run < /path/to/cargo-qemu-test/docs/patches/0001-xtensa-openocd-semihosting.patch
$ patch -p1 < /path/to/cargo-qemu-test/docs/patches/0001-xtensa-openocd-semihosting.patch
```

Using `git` (not the codeload tarball) fetches submodules; the tarball has
them empty. The patch adds OpenOCD-style ARM-compatible semihosting support
so stock embedded-test firmware runs under qemu-system-xtensa.

## Step 2: rootless library stack (no sudo)

espressif's configure ignores `--disable-slirp` and builds `net/slirp.c`,
and `hw/misc/esp32_flash_enc.c` includes `<gcrypt.h>` unconditionally. You
need libslirp + libgcrypt + libgpg-error headers and libs. Either install
the distro packages with sudo:

```console
$ sudo apt-get install libslirp-dev libgcrypt20-dev libgpg-error-dev
```

...or extract them into a private prefix (rootless):

```console
$ mkdir -p /tmp/espqemu-root && cd /tmp/espqemu-root   # pick any prefix; ROOT below

$ # libgcrypt + libgpg-error closure
$ mkdir d && cd d
$ for p in $(apt-cache depends --recurse --no-recommends --no-suggests       --no-conflicts --no-breaks --no-replaces --no-enhances libgcrypt20-dev       | grep -E '^[a-z0-9]' | sort -u); do apt-get download "$p" 2>/dev/null || true; done
$ apt-get download libgpg-error-dev libgpg-error0 2>/dev/null || true

$ # libslirp closure -- and the two debs apt tends to silently miss
$ for p in $(apt-cache depends --recurse --no-recommends --no-suggests       --no-conflicts --no-breaks --no-replaces --no-enhances libslirp-dev       | grep -E '^[a-z0-9]' | sort -u); do apt-get download "$p" 2>/dev/null || true; done
$ for f in $(curl -s http://deb.debian.org/debian/pool/main/libs/libslirp/       | grep -oE 'libslirp(dev)?0?_4[^"]*_amd64\.deb' | sort -u); do
      curl -sfO "http://deb.debian.org/debian/pool/main/libs/libslirp/$f" || true
  done
$ # ^ URL-encoding note: curl -O keeps %2B etc. literally in the saved name;
$ #   that name is what dpkg-deb sees. Fine either way, just be consistent.

$ mkdir -p "$OLDPWD/root"
$ for d in *.deb; do dpkg-deb -x "$d" "$OLDPWD/root"; done
$ cd "$OLDPWD/root"

$ # verification -- a single missing file here costs a full build round
$ ls usr/include/gcrypt.h usr/include/x86_64-linux-gnu/gpg-error.h \
      usr/include/slirp/libslirp.h \
      usr/lib/x86_64-linux-gnu/libslirp.so \
      usr/lib/x86_64-linux-gnu/libslirp.so.0 \
      usr/lib/x86_64-linux-gnu/libgcrypt.so.20 \
      usr/lib/x86_64-linux-gnu/libgpg-error.so.0 \
      usr/lib/x86_64-linux-gnu/pkgconfig/libgcrypt.pc \
      usr/lib/x86_64-linux-gnu/pkgconfig/slirp.pc
```

The `libslirp.so.0` check matters twice over: the `-dev` package's
`libslirp.so` is a **symlink** whose target lives in the *runtime* package,
and ld reports a dangling symlink as `cannot find -lslirp` at link time.

## Step 3: configure

```console
$ export ROOT=/tmp/espqemu-root/root
$ export PKG_CONFIG_PATH=$ROOT/usr/lib/x86_64-linux-gnu/pkgconfig
$ cd esp-qemu
$ ./configure --target-list=xtensa-softmmu --enable-gcrypt --enable-slirp \
    --extra-cflags="-I$ROOT/usr/include -I$ROOT/usr/include/x86_64-linux-gnu \
      -I$ROOT/usr/include/slirp -I$ROOT/usr/include/glib-2.0 \
      -I$ROOT/lib/x86_64-linux-gnu/glib-2.0/include" \
    --extra-ldflags="-L$ROOT/usr/lib/x86_64-linux-gnu \
      -Wl,-rpath,$ROOT/usr/lib/x86_64-linux-gnu" \
    --disable-werror --disable-docs --disable-sdl --disable-gtk --disable-vnc \
    --disable-guest-agent --disable-user --disable-gnutls --disable-nettle \
    --disable-capstone --disable-fdt --disable-alsa --disable-pa --disable-rdma \
    --disable-libiscsi --disable-libnfs --disable-libssh
```

Flag notes (each rejected/ignored flag was hit during the real build):

* `--disable-crypto` / `--disable-bluez`: unknown to espressif's configure
  snapshot -- do not use them.
* `--disable-slirp` is silently ignored, hence `--enable-slirp` + the
  include path for `include/slirp/`.
* gcrypt cannot be disabled (esp32_flash_enc.c uses it), hence
  `--enable-gcrypt`.

**After configure, sanity-check the log actually ran** (`ls -la` it; an
empty log after a "successful" run means a `set -e` probe died first and you
are about to build against a stale build.ninja). Then build:

```console
$ cd build && ninja qemu-system-xtensa        # ~10 min, all cores
$ ninja qemu-system-xtensa                     # confirms "no work to do"
```

## Step 4: verify

```console
$ ./qemu-system-xtensa -machine help | grep esp32   # expect: esp32, esp32s3 (no esp32s2 here)
$ ./qemu-system-xtensa -M esp32 -nographic \
    -semihosting-config enable=on,target=native,arg=run_addr,arg=0x40080000 \
    -kernel /bin/true
```

The second command is the semihosting canary: you want an error about
loading the ELF (`could not load ELF file`), **not** `unknown option arg`
or an option-parse error. That proves the `-semihosting-config ...,arg=`
channel cargo-qtest uses to pass test addresses is compiled in for Xtensa.

## The failure-mode cheat sheet (all observed)

| symptom | cause | fix |
|---|---|---|
| `net/slirp.c: fatal error: libslirp.h: No such file...` | submodule-less tarball or missing `-I.../include/slirp` | Step 2 + `--extra-cflags` above |
| `hw/misc/esp32_flash_enc.c: gcrypt.h: No such file` | gcrypt assumed present; can't be disabled | Step 2 gcrypt + `--enable-gcrypt` |
| `unrecognized option: --disable-crypto` | espressif configure predates the flag | drop the flag |
| same error after "reconfiguring" | a `set -e` probe (e.g. checking for the wrong `.pc` name -- it is `libgcrypt.pc`, not `gcrypt.pc`) died before configure ran | check the configure log is non-empty, then rerun |
| link: `cannot find -lslirp` | `libslirp.so` symlink exists but `libslirp.so.0` target missing | verify the `.so.0` in Step 2 |
| link: undefined `gpg_err_*` | transitive dep not linked | add `-lgpg-error` to `--extra-ldflags` and reconfigure |
| build died mid-run when my shell died | process not detached | `setsid nohup ninja ... &` |
| silent hang at first semihosting call in firmware | trap encoding wrong (word should be `0x00401E` = `break 1,14`) | objdump the firmware trap site |
| test always reports failed | register offset wrong (op must be in `a2`, param in `a3`) | check `common-semi-target.h` mapping |
