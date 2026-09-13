# Installing QEMU for cargo-qtest

`cargo qtest` shells out to a `qemu-system-*` binary chosen from `--target`
(see the README's target table). Install the one matching your target:

| Target family | Package |
|---|---|
| `thumbv6m/7m/7em/8m.main-*` | `qemu-system-arm` |
| `aarch64-unknown-none*` | `qemu-system-aarch64` |
| `riscv32*-unknown-none-elf` | `qemu-system-riscv32` |
| `riscv64*-unknown-none-elf` | `qemu-system-riscv64` |

## With a package manager

```console
# Debian / Ubuntu
$ sudo apt-get install qemu-system-arm   # -aarch64/-riscv* ship in qemu-system-misc
# Fedora
$ sudo dnf install qemu-system-arm
# Arch
$ sudo pacman -S qemu-system-arm
# macOS
$ brew install qemu
```

Any QEMU >= 6.1 works (that is when `-semihosting-config ... ,arg=` was added);
>= 7.0 is recommended. Use `cargo qtest --qemu <path>` if the binary is not in
`PATH`.

## Without root: extracting .deb packages manually

On machines where you cannot use `sudo` (CI containers, shared dev boxes), you
can assemble a working QEMU from Debian packages without installing anything:

```console
$ mkdir qemu-root && cd qemu-root

# 1. Download the package and its dependency closure
$ apt-get download qemu-system-arm
$ apt-get download $(apt-cache depends --recurse --no-recommends --no-suggests       --no-conflicts --no-breaks --no-replaces --no-enhances qemu-system-arm       | grep -E '^[a-z0-9]' | sort -u)

# 2. Extract everything (dpkg-deb needs no root)
$ for d in *.deb; do dpkg-deb -x "$d" "$PWD/root"; done

# 3. Point the loader at the extracted libraries and run
$ export LD_LIBRARY_PATH="$PWD/root/usr/lib/x86_64-linux-gnu:$PWD/root/lib/x86_64-linux-gnu"
$ ./root/usr/bin/qemu-system-arm --version
```

Two gotchas worth knowing:

* **URL-encoded filenames**: `curl -O` on a pool URL keeps `%2B` etc. literally
  in the saved filename (`qemu-system-arm_7.2%2Bdfsg-...deb`), while
  `apt-get download` saves the decoded name. Quote whatever name is actually on
  disk when running `dpkg-deb -x`.
* **Stale mirrors**: if `apt-get download qemu-system-arm` 404s, your mirror's
  package index is behind its pool. List
  `http://<mirror>/debian/pool/main/q/qemu/` and fetch the current build
  (e.g. `..._7.2+dfsg-7+deb12u18+b3_amd64.deb`) directly. Download any libraries
  `ldd <binary> | grep "not found"` reports the same way (in our case
  `libfuse3-3`, which extracts under `root/lib/`, not `root/usr/lib/`).

## Verifying the installation

`cargo qtest` needs two things from the emulator; check both before debugging
firmware:

```console
$ qemu-system-arm --version                       # any QEMU >= 6.1
$ qemu-system-arm -machine help | grep mps2       # mps2-an385/an386 for Cortex-M3/M4
```

A quick smoke test that exercises the semihosting path `cargo qtest` relies on:

```console
$ qemu-system-arm -M mps2-an386 -cpu cortex-m4 -nographic     -semihosting-config enable=on,target=native,arg=hello     -kernel <any-thumbv7em-test.elf>
```

If `cargo qtest` reports `failed to spawn qemu-system-arm`, either install the
package above or pass `--qemu /path/to/qemu-system-arm`.
