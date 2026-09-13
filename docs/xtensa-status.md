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

## The one open item: QEMU's strict ENTRY check

Symptom: `shared_cases` (success-path test) hangs; trace shows
`Illegal entry instruction (pc = 0x400BC0A4)` with a0 == pc, then the ROM
deadloop. 0x400BC0A4 is `Reset`; bytes there are `ENTRY a1, 64`.

Theory (fits all data): embedded-test's xtensa runner reboots the CPU after
a SUCCESSFUL test via a software jump to Reset (`movi a0, Reset; jx a0`-ish,
hence a0 == pc). QEMU's translate.c raises EXCP_ILLEGAL for ENTRY when it is
not preceded by a call (a0 != pc-3 / window precondition). Real silicon
tolerates ENTRY after a plain jump. The panic path (overflow_fails) calls
process::exit and never reboots, which is why only success-path tests die.

Fix: relax the ENTRY legality check in target/xtensa/translate.c
(translate_entry) to match silicon. Incremental rebuild ~2-5 min, then rerun
/tmp/run_xtqtest.sh (expects: 4 passed + 2 ignored).

Fallback: patch embedded-test's xtensa success path to do a real reset
(watchdog/RTC_CNTL or jump to ROM reset vector 0x40000400) - upstream PR
territory.

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
