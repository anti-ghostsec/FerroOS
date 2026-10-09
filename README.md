# FerroOS

A tiny Linux-based OS with a 100% Rust userspace, a Windows 95 desktop,
`C:\` paths, privacy by default, and an idle RAM budget of **50 MB** (it
idles at about 31 MB).

![The FerroOS desktop with the Command Prompt and Start menu](docs/screenshots/terminal.png)

Linux is used purely as the driver layer (GPU, input, storage, network).
Everything above the kernel is one static Rust binary, `C:\bin\ferro`, which
`/init` and every program name in `C:\bin` link to.

> **Status: experimental.** FerroOS is a hobby project that hasn't been
> security-audited. Don't rely on it where your safety depends on it; for
> that, use Tails or Whonix.

## Highlights

- **Encrypted vault** for your settings and files (Argon2id +
  XChaCha20-Poly1305); everything else lives in RAM and vanishes at
  shutdown. Swap, hibernation and core dumps are compiled out.
- **Sandboxed apps** (Landlock, seccomp, cgroups) that ask before going
  online, with typed RAM budgets (`5G`, `50 MB`...).
- **Kill switches** in the tray for the network, microphone and camera.
- **Encrypted DNS** (DNS over HTTPS) and **Encrypted Client Hello**.
- **WireGuard VPN** and **Tor** (both off by default), leak-proof by kernel
  routing, and Tor through the VPN.
- A random MAC address every boot, quiet DHCP, verified clock.
- **Remove Metadata** from photos with a right-click.

| | | |
|---|---|---|
| ![Task Manager](docs/screenshots/processes.png) | ![RAM budgets](docs/screenshots/budgets.png) | ![VPN and Tor](docs/screenshots/vpn.png) |
| ![Remove Metadata](docs/screenshots/privacy.png) | ![Performance](docs/screenshots/performance.png) | ![FerroOS Setup](docs/screenshots/setup.png) |

## Try it or install it

Download `ferroos.iso` (about 40 MB) from the
[Releases page](https://github.com/anti-ghostsec/FerroOS/releases), or build it
yourself with `cargo xtask iso` (see below). Write it to a USB stick with [Rufus](https://rufus.ie) or
[balenaEtcher](https://etcher.balena.io), or boot it in a virtual machine. The
boot menu offers:

- **Try FerroOS (nothing is saved):** a live session in RAM.
- **Install FerroOS:** opens **FerroOS Setup**, which erases the disk you pick
  and installs FerroOS on it. A live session also has an **Install FerroOS**
  icon on the desktop.

The ISO starts on both UEFI and older BIOS PCs. An installed FerroOS needs UEFI
with Secure Boot turned off (its boot loader isn't signed). At its first start,
FerroOS asks for the password that encrypts your settings and files.

An installed disk holds three partitions: the EFI system partition (FerroOS's
own UEFI boot loader, `ferro-boot`, plus the kernel and initramfs), the
programs partition (the Tor service), and the encrypted vault.

## Layout

| Crate | Role |
|---|---|
| `crates/ferro` | The multicall binary: runs init, the desktop, the services or a tool, depending on the name it was started as |
| `crates/ferro-path` | `C:\...` ⇄ POSIX translation through a drive table (`no_std`, fully unit-tested) |
| `crates/ferro-sys` | `/proc` readers: memory, processes, CPU, clock; the modalias glob matcher |
| `crates/ferro-gfx` | Software renderer: Win95 bevels, palette, 8x8 font, icons, clipped (damage-limited) drawing |
| `crates/ferro-shell` | The desktop: logon, taskbar with kill switches, Start menu, Explorer, context menus, Task Manager, App Permissions, Command Prompt (VT100). Runs as the unprivileged user on DRM/KMS + evdev |
| `crates/ferro-system` | The one root helper: encrypted vault, kill switches, RAM budgets, the Tor service, FerroOS Setup (GPT + FAT32 written from scratch), behind a narrow socket |
| `crates/ferro-vault` | Encrypted storage: Argon2id + XChaCha20-Poly1305, crash-safe two-slot layout, folder archive |
| `crates/ferro-sandbox` | `ferro-run`: Landlock, seccomp, network prompts, cgroup RAM budgets; the remembered-choices store |
| `crates/ferro-net` | Network service: random MAC, quiet DHCP, DNS over HTTPS (HTTP/2), ECH-capable TLS, verified clock sync, WireGuard VPN and Tor routing (netlink); the `nslookup` and `fetch` tools |
| `crates/ferro-meta` | Removes personal metadata from JPEG, PNG and WebP without re-encoding |
| `crates/ferro-cmd` | The DOS-style command interpreter, shared by the rescue console and the Command Prompt |
| `crates/ferro-init` | PID 1: mounts, hardware-matched driver loading and hotplug, user session setup, service supervision, rescue console |
| `crates/ferro-preview` | Runs the desktop in a window on Windows/macOS/Linux, and renders the screenshots |
| `crates/ferro-boot` | FerroOS's UEFI boot loader for installed disks (built separately for `x86_64-unknown-uefi`) |
| `xtask` | Builds the kernel, the Tor service and the ISO (in VMs), cross-builds static musl binaries, packs the initramfs and the programs disk, runs QEMU; a WireGuard test server |
| `tools/` | Kernel config fragments, the build VMs' init scripts (kernel, Tor, ISO, WireGuard test server), a two-header C shim for the TLS crypto, `build-kernel.sh` for Linux hosts |

## Remembering things, encrypted

Settings, remembered choices, app data (`C:\ProgramData`) and your files
(`C:\home`) survive reboots in an **encrypted vault** on its own small disk or
partition:

- Your password becomes the key through **Argon2id**. It's memory-hard: each
  guess costs 16 MiB and real time, which slows brute force on a stolen disk.
  The key itself is never stored, and it's wiped from RAM when the vault
  closes.
- The contents are sealed with **XChaCha20-Poly1305**. A tampered disk fails
  to decrypt instead of feeding the system modified files.
- Writes alternate between two slots, so a power cut mid-save leaves the
  previous save intact.
- While running, those folders live in RAM. Plaintext never touches the disk.
  Changes are saved within about 2 seconds, and once more at shutdown.
- Temp folders are never saved.
- **Don't Save** or **Skip** at logon gives an amnesic session.

## Sandbox and RAM budgets

The desktop and everything you start run as an ordinary user, never root.
Every program goes through `ferro-run`, which adds:

| Layer | Default |
|---|---|
| RAM budget (cgroup v2, no swap) | 256 MB, plus a 256-process limit |
| Files (Landlock) | read/execute system programs and the app's own folder; read `C:\etc`; write the current folder (never `C:\` itself) and `C:\ProgramData\<app>` |
| Network (seccomp on `socket()`: TCP, UDP, ICMP) | asked at first use; raw packet sockets always denied |
| Syscalls (seccomp) | ptrace, module loading, mount, namespaces, bpf, io_uring, keyrings, clock/reboot changes return EPERM |
| Privileges | all capabilities dropped, `no_new_privs` |

**Network prompt:** the first time an app opens an internet socket, it pauses
and asks: **Y** allow once, **A** always, **N** deny once, **V** never.
`RUN /NET` and `RUN /NONET` decide one run up front, and `RUN /UNSAFE` skips
the sandbox and says so.

**RAM budgets are typed:** right-click a process in Task Manager, choose **Set
RAM Budget...**, and enter `5G`, `512M`, `50 MB` or `none`. "Always use this
budget" applies it every time that app starts. An app over its budget is
stopped alone, so a runaway browser can't freeze the PC. `MEMTEST 24 /MEM:16M`
demonstrates it. Budgets apply to your own programs; system services are out
of reach.

**Remembered choices** live in `C:\ProgramData\ferro\choices.conf`, inside the
vault. Start > Settings > **App Permissions** has **Forget** and **Forget
All**, and `PERMS /FORGET ALL` does the same from the prompt.

## Privacy tools

- **Encrypted DNS:** every lookup goes over HTTPS (HTTP/2) to Quad9, falling
  back to Mullvad, then Cloudflare. FerroOS reaches them by IP address, pads
  queries to 128 bytes, and caches answers in RAM only.
- **Encrypted Client Hello (ECH):** sites' ECH keys pass through encrypted DNS
  to ECH-capable apps. FerroOS's own TLS (`fetch`) uses ECH, and Cloudflare's
  trace page reports `sni=encrypted`.
- **Tray kill switches:** network, microphone and camera, enforced by the
  kernel (network down, devices unbound from their drivers). Microphone and
  camera start off. On most PCs "microphone off" also mutes speakers, because
  one sound chip handles both.
- **Anonymous on the network:** a random MAC address every boot, and DHCP
  requests that send no hostname or vendor ID.
- **Verified clock:** a build-date floor, then the `Date` header from a
  TLS-verified HTTPS server. Plain NTP is unauthenticated and spoofable, so
  it isn't used.
- **No traces:** swap, hibernation and core dumps are compiled out. Logs live
  in RAM only. Freed memory is zeroed, and shutdown frees the caches so they
  are zeroed too.
- **Remove Metadata:** right-click a photo to strip EXIF, GPS, XMP, IPTC and
  comments; pixels are untouched.
- **Secure delete, honestly:** overwriting doesn't erase flash storage. The
  vault's encryption is the real protection: without the password, every
  leftover block is noise.

## VPN and Tor (both off by default)

Start > Settings > **VPN and Tor...**, or `VPN` and `TOR` at the Command Prompt.

**VPN (WireGuard).** Get a WireGuard file from your VPN provider, then run
`VPN IMPORT <file>` and `VPN ON`. The profile (it holds your private key) is
kept in the encrypted vault. FerroOS never runs commands from it (`PostUp`...),
and keeps its own encrypted DNS, which then travels inside the tunnel.

- **Leak-proof by construction.** While the VPN is on, the only route to the
  internet goes into the tunnel; only the VPN server itself is reached
  directly. If the tunnel can't come up, or the server stops answering,
  there's simply no route: nothing goes out unprotected.
- IPv6 on the network card is switched off while the VPN is on, because
  router advertisements would otherwise hand it a route around the tunnel.
  Profiles with IPv6 addresses carry IPv6 through the tunnel.
- The server must be given by IP address. Looking up a name would happen
  outside the tunnel and reveal which VPN you're connecting to.
- WireGuard's driver loads only when you switch the VPN on.

**Tor.** `TOR ON` starts the Tor service (Arti, the Tor Project's Rust Tor).
From then on **only Tor can reach the internet**: a kernel routing rule gives
the Tor service's own user, and nobody else, a route out. DNS goes through
Tor too. Apps reach the web through Tor's SOCKS port `127.0.0.1:9150`; apps
that don't use it stay offline instead of leaking. `fetch` uses it
automatically, and `fetch --direct` is a deliberate leak test that must fail.

- **Tor through the VPN:** with both on, Tor connects through the VPN tunnel,
  so your network sees only VPN traffic, and the VPN sees only Tor traffic.
- Tor costs no RAM while it's off. It lives on the read-only programs disk,
  is checked against a SHA-256 compiled into FerroOS (a modified copy is
  refused), and is copied into its own RAM disk only while on. Switching it
  off deletes the program, its keys and the network directory it downloaded.
- Before you log on, nothing but DHCP leaves the machine: the VPN and Tor
  settings are inside the vault and aren't known yet.

## The kernel: balanced, not stripped

Essentials are built in. Drivers are zstd-compressed modules, and init loads
**only those matching the hardware it finds** (from `modalias` and
`modules.alias`). It then frees the files nobody needs, keeping USB, HID and
input drivers for hotplug, which a uevent listener handles. Hardening is on:
Landlock, Yama, lockdown (integrity mode, compiled in rather than a boot
option), seccomp, KASLR, stack protector, hardened usercopy, slab hardening,
PTI, enforced module signatures, and `init_on_free`. There is no kernel
symbol table, which saves about 1 MB and hides function addresses.

There's no framebuffer text console either: it kept its own full-screen copy
of the display (4 MB at 1280x800, 8 MB at 1080p) that the desktop never
shows. Boot messages and the rescue console use the serial port.

## Measured memory (QEMU, 64 MB, 1280x800)

| | Before | Now |
|---|---|---|
| Kernel reserved (image + boot, excluding page tracking) | 15,103 KB | 11,236 KB |
| Root file system in RAM (programs, drivers kept for hotplug) | 4,732 KB | 2,904 KB |
| Screen buffers | 8,000 KB | 4,000 KB |
| Kernel objects (slab) | 6,276 KB | 5,844 KB |
| Private process memory (heaps, stacks) | 436 KB | 688 KB |
| **Footprint at logon** | 43,883 KB | **29,176 KB** |

"Now" includes the VPN, Tor and Setup code. `MEM /DETAIL` prints this
breakdown on a running system.

**The footprint is the same on any amount of RAM** (29.2 MB at 64 MB,
28.2 MB at 512 MB, 28.5 MB at 2 GB), because it counts only what FerroOS uses.
Two things grow with installed RAM on every OS, and `MEM` shows them separately
instead of counting them:

- **Page tracking:** 64 bytes of kernel bookkeeping per 4 KB page, 1.6% of
  RAM (256 MB on a 16 GB PC).
- **Free memory kept aside:** the kernel's emergency reserve and its per-CPU
  caches of free pages.

The kernel would also size its lookup tables (file names, files, network
connections) from installed RAM, about 30 MB on a 16 GB PC. FerroOS fixes them
at desktop sizes on the kernel command line (`dhash_entries` and friends).

What saved the most:

- **One binary instead of six.** Each static binary carried its own copy of
  the standard library and shared crates, and the root file system lives in
  RAM: 4.6 MB of programs became 2.2 MB.
- **No fbdev console:** a whole second screen buffer gone.
- **Smaller kernel:** no symbol table, no slab debugging, small-system table
  sizes, and no 2 MB alignment padding after its zeroed data (only AMD memory
  encryption, which FerroOS doesn't enable, needs it).

The desktop draws straight into a DRM buffer. Moving the mouse redraws and
flushes only the cursor's area. On GPUs that scan out 16-bit RGB565 (Intel,
AMD), the desktop uses 16-bit High Color like Windows 95, which halves the
buffer. QEMU's virtual GPU only offers 32-bit, so there it stays at 32.

## Roadmap

1. Microphone-only mute (ALSA capture switch), so speakers keep working
2. A verified (dm-verity) read-only system partition, and a signed boot loader for Secure Boot
3. Double-buffered page flips for GPUs that scan out directly (i915, amdgpu); hardware cursor plane
4. Steam/Proton runtime in a container, with GPU handoff from the compositor

## Design notes

- **Paths:** `C:` maps to `/` and `D:` to `/mnt/d`. `..` never climbs above a
  drive root.
- **LD_PRELOAD shim:** static binaries never load preloaded libraries.
  FerroOS programs call `ferro-path` directly, and Wine already maps drive
  letters.
- **Gaming vs. the 50 MB budget:** the budget covers idle only. A running game
  will need gigabytes, which is expected and fine.

## Building from source

You need Rust (via [rustup](https://rustup.rs)) and
[QEMU](https://www.qemu.org). Nothing else: no WSL, Linux host or cross
compilers. The Linux kernel, the Tor service and the ISO are built inside
throwaway QEMU VMs, with WHPX acceleration on Windows or KVM on Linux.

Iterate on the desktop in a window, on any OS:

```bash
cargo run -p ferro-preview
```

Build the kernel (about 7 minutes) and boot FerroOS in QEMU with 64 MB:

```bash
cargo xtask kernel-build
cargo xtask run
```

Then, optionally:

```bash
cargo xtask tor-build
cargo xtask iso
```

`tor-build` builds the Tor service (about 7 minutes), and the next `run` or
`iso` includes it. `iso` makes the installation image. To try the VPN without
a provider, `cargo xtask wg-test-server` starts a throwaway WireGuard server and
writes its profile to `build/import/`, which FerroOS shows as `D:`
(`VPN IMPORT D:\wg-test.conf`). Don't build an ISO while that test profile is
there.

Under `cargo xtask run`, `build/vault.img` is the encrypted disk and only ever
holds ciphertext. The rescue console (`DIR`, `MEM`, `PS`, `SHUTDOWN`, ...) runs
over serial in your terminal.

Keyboard shortcuts: Ctrl+Esc opens Start, Ctrl+Shift+Esc or Ctrl+Alt+Del opens
Task Manager, and Alt+F4 closes the active window.

## License

FerroOS is dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. The Linux kernel (GPL-2.0), the
Tor service (Arti, MIT/Apache-2.0) and GRUB on the installation media
(GPL-3.0) are separate programs under their own licenses.

