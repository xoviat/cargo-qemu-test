# Xtensa (ESP32) support — status, design, and debugging notes

This document captures the complete state of the Xtensa port: what is proven,
what is in the tree, and the one open item (the QEMU ENTRY-check relaxation).

## The pipeline (all verified working)

```
suite.rs (#[qemu_test::tests])
  -> qemu-test-macros expands: no_std twin + host twin + libtest-mimic main
  -> no_std twin: #[embedded_test::tests] (feature xtensa-semihosting ->
     semihosting crate's openocd-semihosting backend)
  -> cargo qtest --target xtensa-esp32-none-elf
       -> cargo-qtest: xtensa target table (qemu.rs) -> qemu-system-xtensa -M esp32
       -> linker fixup (compiler.rs): -Tlink.x + generated memory.x
          (vectors_seg/ROTEXT/RWDATA/... + _stack_start_cpu0; addresses from
          espressif/qemu hw/xtensa/esp32.c; skipped when esp-hal is present)
       -> QEMU (espressif fork + docs/patches/0001): `break 1,14` trap
          (word 0x00401E, op in a2, param in a3) routed into the shared
          ARM-compatible semihosting core: SYS_GET_CMDLINE serves the
          per-test run_addr, SYS_EXIT carries the verdict.
```

### Verified evidence
- Firmware compiles for `xtensa-esp32-none-elf` (RC=0) with the Espressif
  toolchain (`esp` rustup link at a manual install), rust-src, and
  `CARGO_UNSTABLE_BUILD_STD=core` + `CARGO_UNSTABLE_BUILD_STD_FEATURES=compiler-builtins-mem`
  (env vars inherited by cargo-qtest's inner cargo call; keep them AWAY from
  `cargo install` of host tools - E0152). Linking uses the `xtensa-esp-elf`
  GCC (crosstool-NG unified tarball).
- ELF layout correct: readelf shows Entry 0x400BC0A4 (Reset, .rwtext,
  inside QEMU IRAM), test fns at 0x40080xxx, data at 0x3FFAE000+.
- `overflow_fails` (should_panic, both binaries) PASSES under QEMU: real
  boot, cmdline read, panic, SYS_EXIT verdict - the semihosting bridge
  (trap word, a2/a3 mapping, shared-core routing) is correct.
- Host side (`cargo test` on the same suite) passes via the generated
  libtest-mimic harness, as on ARM.

## Resolved: QEMU semihosting trap return & windowed register reset

Root cause and fix:
1. Semihosting trap return: In `target/xtensa/xtensa-semi.c`, OpenOCD semihosting traps
   now restore `PS` from `env->sregs[EPS2 + level - 2]`, clearing `PS.EXCM = 0` and
   restoring the interrupted interrupt level.
2. Windowed register reset: On CPU reset (`target/xtensa/cpu.c`), `WINDOW_BASE = 0`,
   `WINDOW_START = 1`, and `PS = PS_WOE | PS_UM` are initialized for windowed-ABI guests.
3. Secondary CPU parking: In `hw/xtensa/esp32.c` and `esp32s3.c`, a boot blob installs
   a loop parking the APP core (`waiti 15; j 0`) under SMP so it does not starve CPU 0.

With these fixes applied (available via `docs/patches/0001-xtensa-openocd-semihosting.patch` or
the pre-patched fork at https://github.com/kokroo/qemu branch `esp-develop-semihosting`),
all tests pass cleanly:
- `xtensa-esp32-demo`: 4 passed, 0 failed, 2 ignored.

## Reproducing the full loop from scratch

1. Toolchain: fork rustc tarball + rust-src from one esp-rs/rust-build
   release (e.g. v1.97.0.0; install.sh --prefix=<dir>, rustup toolchain link
   esp <dir>). Plus xtensa-esp-elf GCC (espressif/crosstool-NG releases).
2. QEMU: see building-esp-qemu.md (gotchas, rootless libs, known-good
   configure) + docs/patches/0001-xtensa-openocd-semihosting.patch (+ the
   ENTRY hunk, patch 0002, once added).
3. Env: RUSTUP_TOOLCHAIN=esp, CARGO_UNSTABLE_BUILD_STD=core,
   CARGO_UNSTABLE_BUILD_STD_FEATURES=compiler-builtins-mem,
   PATH including the gcc bin dir.
4. Run: cd examples/xtensa-esp32-demo && cargo qtest
   --target xtensa-esp32-none-elf --qemu /path/to/qemu-system-xtensa -v

## Known quirks

- esp32s2: no machine in this qemu snapshot (esp32 + esp32s3 exist).
- The zip is build-state only; /tmp artifacts (toolchain, qemu tree with
  patches applied, gcc) are reproducible from the docs above.
