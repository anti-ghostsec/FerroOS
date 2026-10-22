# FerroOS

A hardened, privacy-first operating system pairing an authentic Windows 95 desktop experience with the full power of modern Linux. Idling at **under 100 MB of RAM**, it runs native 3D games through Steam and Proton, NVIDIA/CUDA computing, and the complete Debian software ecosystem while enforcing uncompromising privacy by default: hardware kill switches in the system tray, per-app RAM limits, Landlock sandboxing, full-disk LUKS2 Argon2id encryption, duress self-destruct, and runtime physical memory scrubbing.

![FerroOS Heavy Desktop](docs/screenshots/heavy-desktop.png)

> **Heads up:** FerroOS is an independent privacy and systems engineering project. While built with defense-in-depth principles, it has not undergone formal third-party cryptographic auditing. For life-critical threat models, consider established systems such as Tails or Whonix.

---

## Heavy vs. Lite: Two Editions, One Vision

FerroOS is built from a unified codebase in two distinct editions tailored for different hardware targets:

| Feature / Metric | FerroOS Heavy *(Primary Edition)* | FerroOS Lite *(Embedded Companion)* |
|---|---|---|
| **Primary Focus** | Daily workstation, gaming, privacy, full Linux apps | Minimal footprint, air-gapped / embedded devices |
| **Base System** | Hardened Debian stable minbase | Custom Linux kernel + static Rust userspace (`ferro`) |
| **Idle RAM Footprint** | **Under 100 MB** | **~29 MB** (target < 50 MB) |
| **Display Server** | Xorg with lean `ferro-wm` (no Composite overhead) | Direct Linux DRM/KMS software framebuffer (`ferro-shell`) |
| **Hardware Graphics** | Intel Gen1–14, AMD Radeon, NVIDIA nouveau/proprietary, UEFI GOP | bochs, virtio-gpu, standard KMS modesetting |
| **3D & Gaming** | Native Vulkan, OpenGL, Steam/Proton, Mesa 3D | Isolated Debian glibc container |
| **Compute & AI** | On-demand NVIDIA CUDA, PyTorch, llama.cpp, Ollama | None |
| **Storage Encryption** | Full-disk LUKS2 (Argon2id PBKDF), no swap | Custom Argon2id + XChaCha20-Poly1305 encrypted vault |
| **Boot Architecture** | Microsoft-signed shim + Debian signed GRUB & kernel | Microsoft-signed shim + unified kernel image |
| **Secure Boot** | Out-of-the-box (UEFI Secure Boot enabled) | MOK enrollment (`ferro-mok.der`) |
| **Anti-Forensics** | Never sleeps (keys never left in a sleeping machine), runtime memory scrubbing | Amnesic RAM-only mode, cold-boot page zeroing |
| **Duress Self-Destruct** | Anti-coercion PIN nukes LUKS2 headers & keyslots | Duress PIN nukes vault header & crypto keyslots |

---

## Download & Installation

Prebuilt hybrid ISO images are generated automatically via GitHub Actions:

1. Download **`ferroos-heavy.iso`** (or `ferroos.iso` for Lite) from the project's [GitHub Releases](https://github.com/anti-ghostsec/FerroOS/releases).
2. Flash the ISO to a USB flash drive using:
   - **[Rufus](https://rufus.ie)**: Select **DD Image Mode** when prompted (ensures all installer partition tables are preserved).
   - **[balenaEtcher](https://etcher.balena.io)**: Select image and flash directly.
   - **Linux / macOS**: `sudo dd if=ferroos-heavy.iso of=/dev/sdX bs=4M status=progress conv=fsync`
3. Boot your target PC:
   - **UEFI Secure Boot**: Supported out of the box with standard Microsoft-signed shim keys.
   - **Legacy BIOS**: Fully supported with hybrid MBR/GPT partitioning.
4. Choose between:
   - **Live Session**: Runs entirely in RAM. Nothing is written to physical drives.
   - **Install FerroOS**: Launches `ferro-setup` to install to an internal SSD/HDD with optional full-disk LUKS2 encryption.

---

## Desktop Experience

FerroOS Heavy delivers a responsive, nostalgic Windows 95 user interface designed for modern productivity without desktop environment bloat:

| | |
|---|---|
| ![Start Menu](docs/screenshots/heavy-start-menu.png) | ![Files and Prompt](docs/screenshots/heavy-files-and-prompt.png) |
| *Authentic Windows 95 Start menu with cascading flyouts* | *File Manager with context menus & phosphor green prompt* |
| ![Task Manager](docs/screenshots/heavy-taskmgr.png) | ![Display Properties](docs/screenshots/heavy-window-manager.png) |
| *Task Manager with per-process RAM quotas and performance graph* | *Display Properties CRT preview and window management* |

### Authentic Windows 95 Start Menu
- Dark embossed vertical banner ("FerroOS") and generous 34px item heights with crisp retro icons.
- **XDG Desktop Categorization**: Automatically categorizes standard desktop applications into classic cascading flyouts (*Accessories*, *Internet*, *Multimedia*, *System Tools*, *Development*, *Games*).
- **Multi-Column Horizontal Overflow**: When any menu or category exceeds available vertical screen height, items seamlessly split into side-by-side adjacent columns (authentic Windows 95 multi-column layout).
- **Instant Response & Zero-I/O Caching**: Desktop entries are indexed and cached in memory for instantaneous sub-millisecond menu response.
- **Visual Polish**: 3D etched dividers, uniform 16x16 raster icons, and classic flyout hover indicators.
- **Application Context Menus & Uninstall**: Right-click any application in the Start Menu or Programs submenu to **Open**, **Pin to Taskbar**, **Create Desktop Shortcut**, or **Uninstall...** (runs `sudo apt remove --purge` with package resolution).
- Quick shortcuts for *Programs*, *Documents*, *Settings*, *Help*, *Run...*, *Log Off...*, *Restart...*, and *Shut Down...*.

### Clean Desktop & Dynamic Filesystem Sync
- **Arranged Desktop Surface**: By default, **Recycle Bin** sits at the top `(20, 20)`, **My Computer** (system explorer and drive viewer) underneath `(20, 96)`, **LibreWolf** (privacy web browser) at `(20, 172)`, and **Command Prompt** (CRT green terminal) at `(20, 248)`.
- **Live `~/Desktop` Sync**: Any files, folders, or `.desktop` shortcuts created via the shell, terminal, or GUI appear on the desktop automatically.
- **Movable Icons & Persistence**: Drag icons freely across the desktop surface; custom positions persist across reboots in `~/.config/ferro/desktop.conf`. Deleting a preinstalled shortcut permanently hides it.
- **Hover Highlights & Selection**: Mousing over desktop shortcuts renders authentic Windows 95 focus outlines with drop-shadowed labels.
- **Desktop Icon Organization & Windows 11 Context Menu**:
  - Right-click desktop context menu:
    - **View**: Toggle **Auto arrange icons** and **Align icons to grid** (persisted across reboots in `~/.config/ferro/desktop.conf`).
    - **Sort by**: Arrange icons by *Name* or *Item type*.
    - **Refresh**: Instantly syncs and redraws desktop items from disk.
    - **New**: Create *Folder*, *Text Document*, or *Terminal Shortcut* directly on the desktop.
    - **Display settings**: Launches Appearance & Display Properties.
    - **Open in Terminal**: Opens Command Prompt in `~/Desktop`.
    - **Task Manager**: Direct shortcut to Task Manager.
  - Right-click desktop items: *Open*, *Pin to Taskbar*, *Create Desktop Shortcut*, *Rename...*, *Delete* (moves files to `~/.local/share/Trash`), *Empty Recycle Bin*.
- **Silky Smooth 60+ FPS Motion Pipeline**: Desktop and window movements utilize dirty-rectangle buffering, motion event compression, and deferred client IPC to ensure stutter-free, silky smooth dragging without low-FPS jumps.
- **Desktop Hotkeys**: Select any desktop item and press `Delete` to remove it, `Enter` to open or run it, or `F2` to rename it inline.

### "My Computer" Drive Explorer & Universal Storage Automounting
- **My Computer Top-Level View (`ferro-files`)**: Accessible from the desktop "My Computer" icon, the Start Menu, or by navigating "Up" from the root of any drive. Displays all mounted and connected storage devices (`Local Disk (C:)`, `Removable Disk (D:)`, `CD-ROM (E:)`, etc.) with authentic Windows 95 drive icons.
- **Dynamic Drive & Partition Discovery**: Automatically discovers active mounts and physical disk partitions from `/proc/mounts` and `/proc/partitions`. Detects all connected internal HDDs, SATA/NVMe SSDs, USB flash drives, and SD cards.
- **Universal Storage Automounting**:
  - Automatically mounts connected and hotplugged drives to `/media/$USER/<Label>` using `udiskie-mount` and `udisks2`.
  - Double-clicking unmounted partitions in "My Computer" automatically mounts the block device via `udisksctl` on demand.
  - Polkit storage rule (`/etc/polkit-1/rules.d/60-ferro-storage.rules`) allows passwordless storage mounting for local users.
  - Kernel and userspace drivers for NTFS (`ntfs-3g`) and exFAT (`exfatprogs`).
- **Hierarchical Drive Navigation**: Seamless two-way folder navigation: double-clicking any drive letter enters its root folder, and clicking "Up" at the drive root brings you back to the "My Computer" drives overview.

### Preinstalled Privacy & Media Applications (Heavy)
- **LibreWolf**: Hardened privacy browser with anti-fingerprinting, uBlock Origin, and zero telemetry out of the box.
- **MPV**: Lean, high-performance video player with hardware video acceleration (VA-API/VDPAU/CUDA).
- **qView**: Minimalist, instantaneous image viewer with retro aesthetic.
- **Geany**: Fast, lightweight development environment and code editor.
- **Desktop URL & OAuth Integration (`xdg-open`)**: Full XDG MIME protocol handling (`x-scheme-handler/http`, `https`) and user D-Bus session bus. External applications, Electron apps, and development tools (e.g., Antigravity, VS Code) open authorization links and OAuth logins ("Continue with Google") directly in LibreWolf.

### File Associations & System Defaults (`ferro-associations`)
- Accessible via `Start > Settings > File Associations...`.
- Graphical editor for file type associations stored in `~/.config/ferro/associations.conf`.
- Double-clicking documents in `ferro-files` or desktop shortcuts opens the associated application:
  - Text & Code (`.txt`, `.md`, `.rs`, `.c`, `.sh`, `.conf`) -> **Geany**
  - Images (`.png`, `.jpg`, `.jpeg`, `.bmp`, `.ppm`) -> **qView**
  - Media & Audio (`.mp4`, `.mkv`, `.avi`, `.mp3`, `.wav`, `.flac`) -> **MPV**
  - Web (`.html`, `.htm`) -> **LibreWolf**
  - Desktop Shortcuts (`.desktop`) -> FreeDesktop application execution with terminal emulation and working directory support.
- **Universal Desktop Shortcut Execution**: Double-clicking `.desktop` application shortcuts or symlinked shortcuts from inside any folder in `ferro-files` automatically parses their FreeDesktop specifications (`Exec=`, `Terminal=`, `Path=`), strips field codes (`%U`, `%F`), sets the working directory, and launches the application or terminal session directly.

### Taskbar, Window Management & Context Menus
- **Windows 11-Sized Elevated Taskbar (40px)**: Modern, spacious 40px taskbar height matching Windows 11 proportions while retaining authentic Windows 95 3D beveled aesthetics and centered buttons.
- **Enlarged Window Caption & Grabbing Area (33px)**: Window title bar height elevated to 33px (`TITLE_H: 28px`) with enlarged 22x20px caption buttons (`X`, `O`, `_`), providing a generous grabbing zone matching Windows 11 ergonomics for effortless dragging and positioning.
- **Hardware-Cached Root Pixmap & Flicker-Free Desktop**: Desktop wallpaper and icons are rendered to a hardware-accelerated root `Pixmap` (`_XROOTPMAP_ID` and `ESETROOT_PMAP_ID`) with multi-rectangle X11 expose handling. Moving windows or opening applications never causes desktop icons or wallpaper tiles to vanish or flicker.
- **Unpinned Clean Default & Flexible Pinning**: Starts with a clean, distraction-free taskbar (zero forced pins); users can pin any app from the Start menu or desktop context menus.
- **Taskbar Right-Click Context Menu**:
  - Right-click empty taskbar space: Access **Task Manager**, **Show the desktop**, **Display Settings...**, **Network Connections...**, and **Date and Time...**.
  - Right-click pinned icons: **Open** or **Unpin from taskbar**.
  - Right-click running task buttons: **Restore**, **Minimize**, **Maximize**, or **Close**.
- **Window Geometry Persistence**: Windows remember their customized size and desktop position across sessions in `~/.config/ferro/windows.conf` (matching Windows 7 window memory).
- **Exact Browser Context Menu & Popup Positioning (ICCCM 4.1.5)**: Continuous root coordinate synchronization during window moves and strict WM position authority during client `ConfigureRequest`s ensure context menus and dropdowns (e.g. in LibreWolf/Firefox) spawn exactly at the mouse cursor, even when windows are moved far to the right screen edge.
- **EWMH / NetWM Popup & Dropdown Menu Support**: Full conformance with `_NET_WM_WINDOW_TYPE_POPUP_MENU`, `_DROPDOWN_MENU`, `_TOOLTIP`, `_NOTIFICATION`, `_COMBO`, and `_DND`. Context menus and dropdowns map directly to their exact root screen coordinates without framing or clipping.
- Double-clicking the titlebar toggles window maximize.
- Instant, responsive caption buttons for Close (`X`), Maximize/Restore (`O`), and Minimize (`_`).
- Smooth 6px resize borders with 16px corner drag handles for seamless resizing.
- Aero Snap: Drag window titlebar to top to maximize, or left/right screen edges for 50% split view.

### Dedicated Network & Internet GUI (`ferro-network`)
- Accessible via `Start > Settings > Network & Internet...` or clicking the network icon in the system tray.
- Authentic Windows 95 tabbed property sheet dialog:
  - **Ethernet (LAN)**: Live wired interface information including link status, speed, MAC address, IPv4 address, subnet mask, default gateway, and active DNS resolvers. Features one-click **Renew IP (DHCP)**, **Disconnect**, and **Diagnose...** connectivity tests.
  - **Wi-Fi Networks**: Real-time wireless network scanning with signal strength indicators, security detection, modal password prompt dialog, and one-click Connect / Disconnect backed by NetworkManager (`nmcli`).
  - **VPN & Tor**: Full GUI toggle for Arti Tor daemon, bridge configuration, and WireGuard VPN tunnel status.
  - **Hardware Switches**: Dedicated toggles for Network, Microphone, and Camera hardware kill switches.
- **Out-of-the-Box Ethernet Auto-Connect**: Wired Ethernet interfaces automatically configure and connect with DHCP on boot or cable plug-in via NetworkManager and firewall rules (`udp sport 67 udp dport 68`).

### Power & Session Management
- Instant, passwordless power actions from the Start menu (*Shut Down...*, *Restart...*, *Log Off...*).
- Polkit and sudoers rules allow user accounts in the `sudo` group to power off and reboot without terminal prompts or permission denials.

### Desktop Appearance & Screensavers (`ferro-appearance`)
- Accessible from `Start > Settings > Appearance...` or right-clicking the desktop.
- Features a retro CRT monitor preview rendering the selected background or screensaver in real time.
- **Background Tab**:
  - **Browse... File Selection**: Native graphical file dialog (`zenity`) to select any `.png`, `.jpg`, `.bmp`, or `.ppm` image for the desktop background.
  - **Logon Screen Wallpaper Selection**: Dedicated **Logon...** browse button and sync toggle to customize the `ferro-logon` greeting background independently or in sync with the desktop.
  - **Six Retro Desktop Colors**: *Classic Teal* (`#008080`), *Dark Slate* (`#182026`), *Obsidian Black* (`#000000`), *Retro Navy* (`#000080`), *Plum* (`#550055`), and *Forest Green* (`#005522`).
  - **Dark Mode UI**: Toggle between classic silver/gray 3D bevels and a modern Charcoal Dark Mode for window borders and dialogs.
- **Screen Saver Tab**:
  - Choose between *Flying Stars*, *Phosphor Matrix*, *Blank Screen*, or custom screensavers.
  - **Browse... Custom Screensaver**: Easily browse and register any executable script or binary as a screensaver.
  - Adjust idle activation timeout (1–60 minutes) and toggle password protection on wake.
  - Full-screen **Preview** button and quick shortcut button to open the custom screensavers folder.
- Changes broadcast instantly to running apps via X11 root window atoms and persist across reboots in `~/.config/ferro/appearance.conf`.

### Retro Screensavers & Screen Lock (`ferro-screensaver` / `Win+L`)
- Authentic retro screensavers:
  - **Flying Stars**: 3D starfield warp animation with depth perspective.
  - **Phosphor Matrix**: Retro green phosphor cascading digital rain.
  - **Blank Screen**: Energy-saving power black display.
- **Custom Screensavers**: Drop any executable or shell script into `~/.local/share/ferro/screensavers/` to automatically register it in the Appearance picker.
- **Instant Lock (`Win+L`)**: Press `Win+L` at any time to instantly lock the screen and require user authentication.

### Streamlined System Tray & Status Display
- Located in the taskbar notification area beside the clock and volume controls.
- **Streamlined Tray Icons**: Focused on essential daily utilities:
  - **Network**: Displays link state; clicking launches `ferro-network` for Ethernet, Wi-Fi, VPN, and kill switch configurations.
  - **Speaker (Volume)**: Left-click opens `ferro-volume` slider; right-click toggles audio mute instantly.
- **Status Readout & Task Manager**: Displays live system memory usage and clock (`XX MB  HH:MM AM`); clicking opens Task Manager, double-clicking opens Date and Time settings.

### Task Manager & Process Context Menus (`ferro-taskmgr`)
- Accessible via `Ctrl+Shift+Esc`, the tray clock, or the Start menu.
- **Processes Tab**: Real-time PID, CPU%, memory footprint, user account, and active **RAM Budget**.
- **Right-Click Process Context Menu**: Right-click any running process to **End Process**, **Set Budget...**, or **Remove Budget**.
- **Per-Process RAM Quotas**: Set strict kernel cgroup v2 memory limits (e.g., `512M`, `2G`) with dedicated dialog; button properly padded without text overflow.
- **Performance Tab**: Live 60-second CPU and RAM history graphs featuring a visual threshold line referencing Heavy's memory target.
- **Harmonized Memory Reporting**: Memory usage is calculated using true active process and OS footprint (`footprint_kb`), accounting for Linux kernel low-memory watermark reserves (`totalreserve_pages`) and page-table bookkeeping. Task Manager and the Taskbar display the identical, accurate active RAM metric.
- **Context Menu Isolation**: Process context menus are strictly restricted to row selections on the Processes tab. Right-clicking anywhere on the Performance tab or modal dialogs will never open an unintended process action menu.

### Green Phosphor Command Prompt (`ferro-cmd` / `lxterminal`)
- Launches in a retro terminal styled after phosphor CRT monitors (`#33FF33` bright green text on `#000000` deep black).
- **Full Clipboard & Context Menu Support**: Windows-style **Ctrl+C** (copy) and **Ctrl+V** (paste) shortcut keys, mouse right-click context menu (*Copy*, *Paste*), and terminal text selection.
- **Reliable Backspace & Delete**: Configured with terminal driver initializations (`stty erase '^?'`) and Xresources translations ensuring backspace and delete keys work reliably without control character glitches.
- **Universal Command Execution**: Executes all internal DOS-compatible commands (`DIR`, `CD`, `TYPE`, `MEM`, `PS`, `RUN`, `CLS`, `HELP`) and seamlessly passes through to system binaries in `$PATH` (`apt`, `sudo`, `bash`, `python3`, `git`, `synapse`, etc.).
- **Reliable DNS & Repository Access**: Configured with per-network DHCP DNS and resilient fallback resolvers (`9.9.9.9`, `1.1.1.1`, `8.8.8.8`) allowing instant, error-free package updates (`apt update`, `apt install`).

---

## Security, Privacy & Anti-Forensics

FerroOS Heavy embeds layered defense-in-depth across the entire operating system stack:

### Optional Full Disk Encryption (LUKS2 + Argon2id)
- The installer (`ferro-setup` / `ferro-install`) configures LUKS2 full-disk encryption with Argon2id key derivation at 1 GiB of memory and 5 seconds of work per attempt, and requires a passphrase of at least 12 characters.
- Users can uncheck encryption during setup for non-sensitive environments or virtual machine testing.
- No swap is set up, so nothing from memory is ever written to the disk outside the encrypted volume.

### Duress Password & Anti-Coercion Self-Destruct
- During setup, an optional secondary **Duress PIN** (at least 10 characters) can be configured alongside your primary password.
- It works at all three prompts: the disk passphrase prompt at startup, the logon screen, and the lock screen. Entered at any of them, the system:
  1. Erases the LUKS2 key slots and overwrites the header area of the FerroOS system disk (and no other disk), so its contents can never be decrypted again, even with the real passphrase.
  2. Powers off at once.
- The PIN is stored only as a salted Argon2id hash. Because the startup check runs before the disk is unlocked, that hash is on the unencrypted boot partition, which is why the PIN must be long: a short one could be guessed from a copy of the disk.
- Limits: it protects against someone who types it on the real machine. It cannot help if the disk was copied first; an examiner working from a copy is not affected by the wipe.

### Cold-Boot Memory Extraction Defense
- The kernel boots with `init_on_alloc=1` and `init_on_free=1`.
- Every physical memory page freed by any process or kernel cache is immediately zeroed by the Linux memory allocator.
- Freelist randomization (`page_alloc.shuffle=1`) and slab cache isolation (`slab_nomerge`) prevent heap exploitation and leave no plaintext keys or credentials in RAM chips after use.

### No Sleep: Keys Never Left in a Seized Machine
- Sleep keeps the disk's encryption keys in RAM, where a seized machine can be read out (cold-boot and DMA attacks) without the passphrase.
- FerroOS Heavy therefore never sleeps: closing the lid or pressing the sleep key powers off, and suspend and hibernation are disabled.
- **Panic key**: `Ctrl+Alt+End` cuts power instantly, without a clean shutdown, so the keys are gone from memory within moments.
- The kernel starts with IOMMU DMA protection (`intel_iommu=on`, `amd_iommu=force_isolation`, `iommu=force`, `iommu.strict=1`, `efi=disable_early_pci_dma`), so Thunderbolt and other plug-in devices can't read memory directly.

### Tor Routing with Pluggable Transports
- Built-in Arti (Rust implementation of Tor) with native Pluggable Transport support (`lyrebird`, `snowflake-client`, `webtunnel-client`).
- Disabled by default. Can be enabled per-session (`TOR ON`) or made persistent (`TOR ON PERSIST`).
- Add bridges via `TOR BRIDGE ADD <line>`, list with `TOR BRIDGE LIST`, and clear with `TOR BRIDGE CLEAR`.

### System Hardening & Network Privacy
- **Encrypted DNS**: Every lookup goes over DNS-over-TLS to Quad9 (`dns.quad9.net`) or Cloudflare (`cloudflare-dns.com`), their certificates checked; never plain DNS, and never the resolver a router hands out. A Wi-Fi sign-in page that blocks them needs DoT switched off for that network by hand (`resolvectl dnsovertls <link> no`).
- **Security Updates**: Debian's security updates install themselves daily (`unattended-upgrades`, limited to the security archive; never a restart).
- **MAC Address Randomization**: Ephemeral MAC addresses generated for every Ethernet and Wi-Fi connection, including during Wi-Fi probe scanning.
- **Default-Deny Firewall**: `nftables` drops all inbound connections while permitting outbound traffic and essential ICMP/DHCPv6.
- **Restricted Umask**: Files created by users default to private permissions (`umask 077`).
- **Kernel Lockdown**: `lockdown=integrity`, `kptr_restrict=2`, `dmesg_restrict=1`, `yama.ptrace_scope=1`, `unprivileged_bpf_disabled=1`, core dumps disabled (`suid_dumpable=0`), and maximum ASLR entropy (`mmap_rnd_bits=32`).
- **Metadata Removal**: Right-click images to strip EXIF, GPS, camera model, and author metadata.

---

## Modern Hardware & On-Demand Capabilities

FerroOS Heavy includes driver support for modern workstations and laptops, with on-demand acceleration tools accessible via `ferro-heavy` or the Start menu:

```bash
ferro-heavy status     # View RAM footprint, Secure Boot, LUKS, GPU, and audio status
ferro-heavy steam      # Configure 32-bit architecture, Vulkan loaders & Proton for Steam
ferro-heavy nvidia     # Build proprietary NVIDIA DKMS drivers + libcuda + MOK enrollment
ferro-heavy llm        # Install Python 3, PyTorch, and tools for local LLM inference (llama.cpp, Ollama)
ferro-heavy cuda       # Install full NVIDIA CUDA compiler toolkit (nvcc & headers)
ferro-heavy drivers    # Install Mesa 3D acceleration & VA-API video decode for Intel / AMD
ferro-heavy audio      # Configure ALSA sound utilities & optional PipeWire emulation
```

- **Wi-Fi Cards**: Firmware included out of the box for Intel (`iwlwifi`), Realtek (`rtw88`/`rtw89`), MediaTek (`mt7921`/`mt7922`), Qualcomm Atheros (`ath10k`/`ath11k`), and Broadcom.
- **Display Drivers**: Modesetting for Intel Gen1–Gen14, AMD Radeon (`amdgpu`), NVIDIA (`nouveau`), with universal UEFI GOP (`fbdev`) and VESA fallbacks.
- **Audio**: Zero-daemon ALSA utilities (`alsa-utils`) and Sound Open Firmware (`firmware-sof-signed`) support laptop internal speakers and digital microphone arrays.

---

## Under 100 MB Idle RAM: How It Works

Standard desktop Linux distributions consume between 800 MB and 2.5 GB of RAM at idle. FerroOS Heavy achieves sub-100 MB idle memory through targeted systems optimization:

1. **Kernel Hash Table Pinning**: Linux dynamically sizes internal dcache, icache, and page hash tables based on total system RAM—consuming 30–50+ MB on modern machines. Heavy pins these tables at boot (`dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512`), freeing dozens of megabytes immediately.
2. **Transparent Hugepage Suppression**: Booting with `transparent_hugepage=never` prevents the kernel from allocating 2 MB hugepages to small background daemons, avoiding memory fragmentation.
3. **Lean Xorg Display Layer**: RECORD and XTEST extensions are disabled to reduce memory footprint, while Xorg Composite and unprivileged user namespaces (`kernel.unprivileged_userns_clone=1`) remain enabled so Chromium, Electron (e.g. Antigravity), and hardware-accelerated GUI apps render cleanly without blank window artifacts.
4. **Aggressive Cache Reclaim**: `vm.vfs_cache_pressure=200` and `vm.swappiness=10` aggressively prioritize reclaiming unused directory/inode caches.
5. **Capped Journal Buffers**: `systemd-journald` volatile runtime storage is capped at 4 MB (`RuntimeMaxUse=4M`) with compression enabled (`Compress=yes`).
6. **Lean NetworkManager**: Configured with internal C DHCP client (`dhcp=internal`), file-only keyfile plugins, and connectivity polling disabled.
7. **Masked Redundant Daemons**: Virtual consoles `tty3`–`tty6`, background APT timers, and non-essential disk scrubbers are masked to eliminate background CPU and RAM wakeups.

---

## FerroOS Lite: The 29 MB Companion

For embedded systems, retro machines, or air-gapped environments where every megabyte counts, FerroOS Lite runs the same retro user experience in an ultra-compact package:

- **Monolithic Rust Userspace**: The entire userspace (PID 1 `ferro-init`, `ferro-shell`, `ferro-net`, `ferro-cmd`) is compiled into a single static binary.
- **Direct DRM/KMS Rendering**: Renders straight to the Linux kernel Direct Rendering Manager framebuffer without X11 or Wayland.
- **Encrypted Vault**: User files and settings reside in a custom cryptographic vault encrypted with Argon2id and XChaCha20-Poly1305.
- **Zero Disk Residue**: The root filesystem runs purely in RAM; shutdown wipes all uncommitted data.

---

## Keyboard Shortcuts & Commands

### Global Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Win` or `Ctrl + Esc` | Open / close Start menu |
| `Ctrl + Shift + Esc` | Launch Task Manager (`ferro-taskmgr`) |
| `Win + E` | Open File Manager (`ferro-files`) |
| `Win + D` | Show Desktop (minimize / restore all windows) |
| `Alt + F4` | Close active window |
| `Alt + Tab` | Cycle between open windows |

### Interactive Command Prompt Line Editor (`ferro-cmd`)

The built-in Command Prompt provides a full interactive DOS-style line editor with VT100 / ANSI escape processing:
- **Arrow Keys & Cursor Movement**: Pressing Left / Right arrow moves the insertion point `|` smoothly across the line without printing raw escape sequences (e.g. `^[[D`).
- **Command History**: Pressing Up / Down arrow recalls and edits previous commands.
- **Native Context Menu & Clipboard**: Retains full terminal emulator right-click context menu support (`lxterminal`, `xterm`) for quick Copy, Paste, and font configuration without intercepting terminal events.
- **Navigation & Editing Hotkeys**: Supports `Home` / `Ctrl+A` (start of line), `End` / `Ctrl+E` (end of line), `Delete` (forward delete), `Ctrl+K` (kill to end), `Ctrl+U` (clear line), and `Ctrl+C` (cancel).
- **Clean Backspace**: Uniformly processes both `0x7F` (DEL) and `0x08` (BS) so backspacing never outputs random characters or artifacts.

### Useful Terminal Commands

```text
DIR / CD / TYPE         Standard file navigation and inspection
MEM                     Display free, used, and total system memory
MEM /DETAIL             Detailed memory breakdown including kernel tables
RUN /NET <app>          Launch an application with pre-authorized network access
PERMS /FORGET ALL       Clear all remembered application network permissions
WIFI LIST               Scan and list available Wi-Fi networks
WIFI JOIN <ssid>        Connect to a Wi-Fi network (prompts for passphrase)
BATTERY                 Query laptop battery charge level and AC power status
BRIGHTNESS <0-100>      Adjust display backlight brightness
VPN IMPORT <path.conf>  Import a WireGuard configuration file
VPN ON / VPN OFF        Toggle WireGuard VPN tunnel
TOR ON / TOR OFF        Toggle system-wide Tor routing
DEBIAN apt install <pkg> Install any Debian package (with persistent LUKS2 storage)
```

> **Note on Debian Package Availability:**
> - Classic `synapse` (the legacy GTK2 semantic launcher) was deprecated and removed from modern Debian repositories. If you are looking for an application launcher, use `rofi` (`apt install rofi` / `ferro-heavy install rofi`) or `albert`. If you want the Matrix homeserver, use `matrix-synapse` (`apt install matrix-synapse`).

---

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain)
- [QEMU](https://www.qemu.org) (`qemu-system-x86_64`)
- Host C compiler (`clang`, `llvm`, and `musl-tools` on Linux; MinGW GCC on Windows)

The build toolchain compiles the system inside isolated, self-contained QEMU builder VMs—no cross-compilation toolchains or container engines required.

```bash
# 1. Preview the desktop in a window on your current operating system
cargo run -p ferro-preview

# 2. Build FerroOS Heavy hybrid ISO (build/ferroos-heavy.iso)
cargo xtask heavy-iso

# 3. Run automated virtual window manager test suite with screenshots
cargo xtask wm-test

# 4. Build FerroOS Lite standalone ISO (build/ferroos.iso)
cargo xtask iso

# 5. Boot FerroOS Lite in QEMU with 64 MB RAM
cargo xtask run
```

### Automated GitHub Releases

Automated releases are powered by the `.github/workflows/release.yml` GitHub Actions workflow. When a version tag (e.g. `v1.0.0`) is pushed or manually triggered via `workflow_dispatch`:
- The builder automatically provisions the environment, compiles the static userspace binaries, and generates `ferroos-heavy.iso`.
- Computes SHA-256 checksums (`ferroos-heavy.iso.sha256`) and a signed build-provenance attestation, so a download can be checked against this repository: `gh attestation verify ferroos-heavy.iso --repo anti-ghostsec/FerroOS`.
- Publishes the release assets to GitHub Releases with changelog details.

---

## Screenshots

| Logon Screen | Installation Wizard |
|:---:|:---:|
| ![Logon Screen](docs/screenshots/heavy-logon.png) | ![Installation Wizard](docs/screenshots/heavy-install.png) |
| *Logon screen with session locking & duress protection* | *Setup wizard with optional full-disk LUKS2 encryption* |

| Task Manager Budgets | Network Kill Switches & Tray |
|:---:|:---:|
| ![Budgets](docs/screenshots/budgets.png) | ![Privacy Kill Switches](docs/screenshots/privacy.png) |
| *Per-process RAM budget configuration dialog* | *Hardware kill switches and privacy controls* |

---

## License

FerroOS is dual-licensed under the [MIT License](LICENSE-MIT) or the [Apache License (Version 2.0)](LICENSE-APACHE), at your option.

Third-party components packaged in the installation images:
- Linux Kernel: GPL-2.0
- GRUB Bootloader: GPL-3.0
- Arti (Tor): MIT / Apache-2.0
- Debian Base Packages: Respective upstream open-source licenses
