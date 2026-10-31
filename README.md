# FerroOS

A hardened, privacy-first operating system pairing an authentic Windows 95 desktop experience with the full power of modern Linux. Idling at an **ultra-lean memory footprint**, it runs native 3D games through Steam and Proton, NVIDIA/CUDA computing, and the complete Debian software ecosystem while enforcing uncompromising privacy by default: hardware kill switches in the system tray, per-app RAM limits, Landlock sandboxing, full-disk LUKS2 Argon2id encryption, duress self-destruct, and runtime physical memory scrubbing.

![FerroOS Heavy Desktop](docs/screenshots/heavy-desktop.png)

> **Heads up:** FerroOS is an independent privacy and systems engineering project. While built with defense-in-depth principles, it has not undergone formal third-party cryptographic auditing. For life-critical threat models, consider established systems such as Tails or Whonix.

---

## Heavy vs. Lite: Two Editions, One Vision

FerroOS is built from a unified codebase in two distinct editions tailored for different hardware targets:

| Feature / Metric | FerroOS Heavy *(Primary Edition)* | FerroOS Lite *(Embedded Companion)* |
|---|---|---|
| **Primary Focus** | Daily workstation, gaming, privacy, full Linux apps | Minimal footprint, air-gapped / embedded devices |
| **Base System** | Hardened Debian stable minbase | Custom Linux kernel + static Rust userspace (`ferro`) |
| **Idle RAM Footprint** | **Ultra-Lean** | **~29 MB** (target < 50 MB) |
| **Display Server** | Xorg with lean `ferro-wm` (no compositor unless *Smooth drawing* is ticked) | Direct Linux DRM/KMS software framebuffer (`ferro-shell`) |
| **Hardware Graphics** | Intel Gen1–14, AMD Radeon, NVIDIA nouveau/proprietary, UEFI GOP | bochs, virtio-gpu, standard KMS modesetting |
| **3D & Gaming** | Native Vulkan, OpenGL, Steam/Proton, Mesa 3D | Isolated Debian glibc container |
| **Compute & AI** | On-demand NVIDIA driver & CUDA, Python toolchain for PyTorch / llama.cpp | None |
| **Storage Encryption** | Full-disk LUKS2 (Argon2id PBKDF), no swap | Custom Argon2id + XChaCha20-Poly1305 encrypted vault |
| **Boot Architecture** | Microsoft-signed shim + Debian signed GRUB & kernel | Microsoft-signed shim + unified kernel image |
| **Secure Boot** | Out-of-the-box (UEFI Secure Boot enabled) | MOK enrollment (`ferro-mok.der`) |
| **Anti-Forensics** | Never sleeps (keys never left in a sleeping machine), runtime memory scrubbing | Amnesic RAM-only mode, cold-boot page zeroing |
| **Duress Self-Destruct** | Anti-coercion PIN nukes LUKS2 headers & keyslots | Duress PIN nukes vault header & crypto keyslots |

---

## Download & Installation

Prebuilt hybrid ISO images are generated automatically via GitHub Actions:

1. Download **`ferroos-heavy.iso`** (and its `.sha256`) from the project's [GitHub Releases](https://github.com/anti-ghostsec/FerroOS/releases). Lite isn't published there; build it yourself with `cargo xtask iso` (see [Building from Source](#building-from-source)).
2. Flash the ISO to a USB flash drive using:
   - **[Rufus](https://rufus.ie)**: Select **DD Image Mode** when prompted (ensures all installer partition tables are preserved).
   - **[balenaEtcher](https://etcher.balena.io)**: Select image and flash directly.
   - **Linux / macOS**: `sudo dd if=ferroos-heavy.iso of=/dev/sdX bs=4M status=progress conv=fsync`
3. Boot your target PC:
   - **UEFI Secure Boot**: Supported out of the box with standard Microsoft-signed shim keys.
   - **Legacy BIOS**: Fully supported with hybrid MBR/GPT partitioning.
4. The stick starts a **live session**: the system runs from the stick, every change is kept only in RAM, and nothing is written to the PC's own drives.
5. To install, open **Install FerroOS Heavy** from the live session's Start menu. It runs `ferro-setup`, which installs to an internal SSD/HDD with optional full-disk LUKS2 encryption.

---

## Desktop Experience

FerroOS Heavy delivers a responsive, nostalgic Windows 95 user interface designed for modern productivity without desktop environment bloat:

| | |
|---|---|
| ![Start Menu](docs/screenshots/heavy-start-menu.png) | ![Files and Prompt](docs/screenshots/heavy-files-and-prompt.png) |
| *Authentic Windows 95 Start menu with cascading flyouts* | *File Manager in Details view, sorted by size, beside the green Command Prompt* |
| ![Task Manager](docs/screenshots/heavy-taskmgr.png) | ![Display Properties](docs/screenshots/heavy-window-manager.png) |
| *Task Manager's process tree, with each process's owner and RAM budget* | *Display Properties CRT preview and window management* |

### Authentic Windows 95 Start Menu
- Dark embossed vertical banner ("FerroOS") and generous 34px item heights with crisp retro icons.
- **XDG Desktop Categorization**: Automatically categorizes standard desktop applications into classic cascading flyouts (*Accessories*, *Internet*, *Multimedia*, *System Tools*, *Development*, *Games*).
- **Multi-Column Horizontal Overflow**: When a flyout or right-click menu is taller than the screen, its items split into side-by-side columns (authentic Windows 95 multi-column layout).
- **Type to Search**: Start typing with the Start menu open to find a program.
- **Instant Response & Zero-I/O Caching**: Desktop entries are indexed and cached in memory, so the menu opens without reading the disk.
- **Visual Polish**: 3D etched dividers, crisp raster icons (32x32 on the Start menu, 16x16 in flyouts), and classic flyout hover indicators.
- **Application Context Menus & Uninstall**: Right-click any application in the Start Menu or Programs submenu to **Open**, **Pin to Taskbar**, **Create Desktop Shortcut**, **Hide from Start menu** (and *Show Hidden* to bring them back), **Properties**, or **Uninstall...**, which opens a window showing what apt would remove and then runs `sudo apt-get remove` for the program's package (`ferro-uninstall`).
- Quick shortcuts for *Programs*, *Documents*, *My Computer*, *Settings*, *Help*, *Run...* (opens Command Prompt), *Log Off...*, *Restart...*, and *Shut Down...*.

### Clean Desktop & Dynamic Filesystem Sync
- **Arranged Desktop Surface**: By default, **Recycle Bin** sits at the top `(20, 20)`, **My Computer** (system explorer and drive viewer) underneath `(20, 96)`, **Wisp** (FerroOS's own privacy web browser) at `(20, 172)`, and **Command Prompt** (CRT green terminal) at `(20, 248)`.
- **Live `~/Desktop` Sync**: Any files, folders, or `.desktop` shortcuts created via the shell, terminal, or GUI appear on the desktop automatically.
- **Movable Icons & Persistence**: Drag icons freely across the desktop surface; custom positions persist across reboots in `~/.config/ferro/desktop.conf`. Deleting a preinstalled shortcut permanently hides it.
- **Hover Highlights & Selection**: Mousing over desktop shortcuts renders authentic Windows 95 focus outlines with drop-shadowed labels.
- **Desktop Icon Organization & Windows 11 Context Menu**:
  - Right-click desktop context menu:
    - **View**: Toggle **Auto arrange icons** and **Align icons to grid** (persisted across reboots in `~/.config/ferro/desktop.conf`).
    - **Arrange by Name** / **Arrange by Type**.
    - **Refresh**: Instantly syncs and redraws desktop items from disk.
    - **Paste**: When something has been cut or copied.
    - **New**: Create *Folder*, *Text Document*, or *Terminal Shortcut* directly on the desktop.
    - **Open in Terminal**: Opens Command Prompt in `~/Desktop`.
    - **Task Manager**: Direct shortcut to Task Manager.
    - **Restart Desktop**: Restarts the desktop and taskbar without logging off.
    - **Properties**: Opens Display Properties (`ferro-appearance`).
  - Right-click desktop items: *Open*, *Pin to taskbar*, *Cut*, *Copy*, *Rename* (inline), *Delete*, and *Properties*. Delete moves files and folders to the Recycle Bin (`~/.local/share/Trash`); shortcuts are removed outright, and files on removable drives are deleted through the File Manager. The Recycle Bin itself offers *Open*, *Empty Recycle Bin* and *Properties*.
- **Silky Smooth 60+ FPS Motion Pipeline**: Desktop and window movements utilize dirty-rectangle buffering, motion event compression, and deferred client IPC to ensure stutter-free, silky smooth dragging without low-FPS jumps.
- **Desktop Hotkeys**: Select any desktop item and press `Delete` to remove it, `Enter` to open or run it, or `F2` to rename it inline. `Ctrl+A` selects everything, `Ctrl+X` / `Ctrl+C` / `Ctrl+V` cut, copy and paste, and `F5` refreshes.

### "My Computer" Drive Explorer & Universal Storage Automounting
- **My Computer Top-Level View (`ferro-files`)**: Accessible from the desktop "My Computer" icon, the Start Menu, or by navigating "Up" from the root of any drive. Displays every mounted drive with a letter and authentic Windows 95 drive icons: the drive's own label when it has one, otherwise `Local Disk`, `Removable Disk` or `CD-ROM Drive`, with its size (e.g. `Local Disk 500G (C:)`). Open VeraCrypt volumes show as `VeraCrypt Volume`.
- **Side Folder Tree**: My Computer, your Desktop/Documents/Downloads/Pictures/Music/Videos folders, the same drives and the Recycle Bin are listed down the left side; files can be dropped onto any of them (hold `Ctrl` to copy instead of move). Files dropped on a folder in another File Manager window go into that folder.
- **Dynamic Drive Discovery**: Drives come from `lsblk` (internal HDDs, SATA/NVMe SSDs, USB flash drives, SD cards, mounted disk images); network shares come from `/proc/mounts`.
- **Universal Storage Automounting**:
  - Automatically mounts connected and hotplugged drives to `/media/$USER/<Label>` using `ferro-automount` and `udisks2`.
  - Polkit storage rule (`/etc/polkit-1/rules.d/60-ferro-storage.rules`) allows passwordless storage mounting for local users.
  - Kernel and userspace drivers for NTFS (`ntfs-3g`) and exFAT (`exfatprogs`).
- **Hierarchical Drive Navigation**: Seamless two-way folder navigation: double-clicking any drive letter enters its root folder, and clicking "Up" at the drive root brings you back to the "My Computer" drives overview.

### Preinstalled Privacy & Media Applications (Heavy)
- **Wisp**: FerroOS's own privacy web browser ([anti-ghostsec/wisp](https://github.com/anti-ghostsec/wisp)), built on WebKitGTK. A private session by default (cookies, cache and history never touch the disk), HTTPS-only, tracking parameters stripped, third-party cookies blocked, WebGL and WebRTC off, and plugins for ad and tracker blocking (EasyList, EasyPrivacy, ~110k rules), fingerprint shielding, cookie-banner hiding, dark mode, end-to-end-encrypted bookmarks and passwords, and VidGrab (video downloads with yt-dlp; ffmpeg is preinstalled for HD and MP3). Each update also brings Wisp up to its newest build from its own releases (`ferro-wisp-update`), so Wisp is as new as Wisp, not as the last FerroOS release; the installed copy stays when there's no newer build, the download doesn't check out, or the build needs libraries the system hasn't got yet.
- **Video Player (`ferro-video`)**: mpv with FerroOS's settings and controls (starts silent and loops, Left/Right for the folder's other videos, timeline previews, A-B loop, Delete to the Recycle Bin), run inside `ferro-jail`: a booby-trapped video can't reach the internet or start other programs. Plays music too. Hardware video decoding (VA-API/VDPAU/CUDA) with `ferro-heavy drivers`.
- **Image Viewer (`ferro-viewer`)**: FerroOS's own viewer for PNG, JPEG, BMP, WebP and animated GIF/WebP, with zoom, pan, slideshow, the folder's other pictures and Delete. Sandboxed the same way (seccomp, no network, Landlock).
- **Geany**: Fast, lightweight development environment and code editor.
- **ffmpeg**: Convert, cut and record video and audio from the Command Prompt.
- **Desktop URL & OAuth Integration (`xdg-open`)**: Full XDG MIME protocol handling (`x-scheme-handler/http`, `https`) and user D-Bus session bus. External applications, Electron apps, and development tools (e.g., Antigravity, VS Code) open authorization links and OAuth logins ("Continue with Google") directly in Wisp.

### File Associations & System Defaults (`ferro-associations`)
- Accessible via `Start > Settings > File Associations...`.
- Graphical editor for file type associations stored in `~/.config/ferro/associations.conf`.
- Double-clicking documents in `ferro-files` or desktop shortcuts opens the associated application:
  - Text & Code (`.txt`, `.md`, `.rs`, `.c`, `.sh`, `.conf`) -> **Geany**
  - Images (`.png`, `.jpg`, `.jpeg`, `.bmp`, `.webp`, `.gif`) -> **Image Viewer** (`ferro-viewer`)
  - Video & Audio (`.mp4`, `.mkv`, `.webm`, `.avi`, `.mov`, `.mp3`, `.wav`, `.flac`, `.ogg`, `.opus`...) -> **Video Player** (`ferro-video`)
  - PDF (`.pdf`) -> the system's default PDF handler (`xdg-open`)
  - Web (`.html`) -> **Wisp**
  - Programs (`.deb`, `.tar.gz` and the like) -> installed by `ferro-installer` in a terminal window (a `.deb` with apt, a tarball unpacked into `C:\Programs`); `.zip` archives are extracted.
  - Desktop Shortcuts (`.desktop`) -> FreeDesktop application execution with terminal emulation and working directory support.
  - Anything else -> the system's default handler (`xdg-open`).
- **Desktop Shortcut Execution**: Double-clicking `.desktop` application shortcuts or symlinked shortcuts from inside any folder in `ferro-files` parses their FreeDesktop specifications (`Exec=`, `Terminal=`, `Path=`), strips field codes (`%U`, `%F`), sets the working directory, and launches the application or terminal session directly.
  - Only trusted shortcuts run: owned by you or root, not writable by others, marked executable (or in the system's application folders), and not on a removable drive. Any other `.desktop` file opens as text, so a shortcut from a USB stick or a download can't start a program.

### Taskbar, Window Management & Context Menus
- **Windows 11-Sized Elevated Taskbar (40px)**: Modern, spacious 40px taskbar height matching Windows 11 proportions while retaining authentic Windows 95 3D beveled aesthetics, with task buttons left-aligned after the Start button and pinned icons.
- **Enlarged Window Caption & Grabbing Area (28px)**: Window title bar height elevated to 28px (`TITLE_H`) with enlarged 22x20px caption buttons (`X`, `O`, `_`), providing a generous grabbing zone matching Windows 11 ergonomics for effortless dragging and positioning.
- **Hardware-Cached Root Pixmap & Flicker-Free Desktop**: Desktop wallpaper and icons are rendered to a hardware-accelerated root `Pixmap` (`_XROOTPMAP_ID` and `ESETROOT_PMAP_ID`) with multi-rectangle X11 expose handling. Moving windows or opening applications never causes desktop icons or wallpaper tiles to vanish or flicker.
- **Unpinned Clean Default & Flexible Pinning**: Starts with a clean, distraction-free taskbar (zero forced pins); users can pin any app from the Start menu or desktop context menus.
- **Taskbar Right-Click Context Menu**:
  - Right-click empty taskbar space: Access **Task Manager**, **Show the desktop**, **Display Settings...**, **Network Connections...**, **Volume Control...**, and **Date and Time...**.
  - Right-click pinned icons: **Open** or **Unpin from taskbar**.
  - Right-click running task buttons: **Restore**, **Minimize**, **Maximize**, or **Close**.
- **Window Geometry Persistence**: Windows remember their customized size and desktop position across sessions in `~/.config/ferro/windows.conf` (matching Windows 7 window memory).
- **Exact Browser Context Menu & Popup Positioning (ICCCM 4.1.5)**: Continuous root coordinate synchronization during window moves and strict WM position authority during client `ConfigureRequest`s ensure context menus and dropdowns (e.g. in web browsers) spawn exactly at the mouse cursor, even when windows are moved far to the right screen edge.
- **EWMH / NetWM Popup & Dropdown Menu Support**: Full conformance with `_NET_WM_WINDOW_TYPE_POPUP_MENU`, `_DROPDOWN_MENU`, `_TOOLTIP`, `_NOTIFICATION`, `_COMBO`, and `_DND`. Context menus and dropdowns map directly to their exact root screen coordinates without framing or clipping.
- Double-clicking the titlebar toggles window maximize.
- Instant, responsive caption buttons for Close (`X`), Maximize/Restore (`O`), and Minimize (`_`).
- Smooth 6px resize borders with 20px corner drag handles for seamless resizing.
- Aero Snap: Drag window titlebar to top to maximize, or left/right screen edges for 50% split view. `Win+Arrow` keys do the same from the keyboard.
- `Alt+Tab` / `Alt+Shift+Tab` switch between windows (see [Keyboard Shortcuts](#global-keyboard-shortcuts)).

### Dedicated Network & Internet GUI (`ferro-network`)
- Accessible via `Start > Settings > Network & Internet...`, or the **Network & Privacy settings...** button in the network icon's tray panel.
- Authentic Windows 95 tabbed property sheet dialog:
  - **Ethernet (LAN)**: Live wired interface information including link status, speed, MAC address, IPv4 address, subnet mask, default gateway, and active DNS resolvers. Features one-click **Renew IP (DHCP)**, **Disconnect**, and **Diagnose...** connectivity tests.
  - **Wi-Fi Networks**: Real-time wireless network scanning with signal strength indicators, security detection, modal password prompt dialog, and one-click Connect / Disconnect backed by NetworkManager (`nmcli`). **Allow this network's sign-in page** lets a hotel or café Wi-Fi login page through (see [Encrypted DNS](#system-hardening--network-privacy)).
  - **VPN & Tor**: WireGuard VPN on/off with **Import...** for a `.conf` file, with a kill switch: while the VPN is on, firewall rules (`inet ferro_vpn`) let traffic out only through `wg0`, plus the tunnel's own packets to the server and DHCP, so if the tunnel drops nothing goes out from the real address (programs, servers, and what the PC forwards for VMs and containers). They go in before the tunnel comes up, stay until the VPN is switched off, come back with the VPN at startup (before the network) and after a firewall reload. For configs that send everything through the tunnel (`AllowedIPs = 0.0.0.0/0`); `sudo ferro-vpn killswitch off` goes without it. While the kill switch guards the tunnel, lookups go only to the VPN's own DNS servers (the config's `DNS =` line), inside the tunnel: the VPN already sees every connection, so DNS over TLS to Quad9/Cloudflare would only add a second company that sees them. FerroOS's DNS over TLS comes back when the VPN is switched off, and Tor's DNS still wins while Tor is on, and the Tor switch with an option to use Pluggable Transports (obfs4 / Snowflake) where Tor is blocked.
  - **Hardware Switches**: Dedicated toggles for Network, Microphone, and Camera hardware kill switches.
- **Out-of-the-Box Ethernet Auto-Connect**: Wired Ethernet interfaces automatically configure and connect with DHCP on boot or cable plug-in via NetworkManager and firewall rules (`udp sport 67 udp dport 68`).

### Power & Session Management
- Instant, passwordless power actions from the Start menu (*Shut Down...*, *Restart...*, *Log Off...*).
- Polkit and sudoers rules allow user accounts in the `sudo` group to power off and reboot without terminal prompts or permission denials.

### Desktop Appearance & Screensavers (`ferro-appearance`)
- Accessible from `Start > Settings > Appearance & Display...` or **Properties** on the desktop's right-click menu (the window is *Display Properties*).
- Features a retro CRT monitor preview rendering the selected background or screensaver in real time.
- **Background Tab**:
  - **Browse... File Selection**: Native graphical file dialog (`zenity`) to select any `.png`, `.jpg`, `.bmp`, or `.ppm` image for the desktop background.
  - **Logon Screen Wallpaper Selection**: Dedicated **Logon...** browse button and sync toggle to customize the `ferro-logon` greeting background independently or in sync with the desktop.
  - **Six Retro Desktop Colors**: *Teal* (`#008080`), *Slate* (`#182026`), *Black* (`#000000`), *Navy* (`#000080`), *Plum* (`#550055`), and *Forest* (`#005522`).
  - **Dark Mode UI**: Toggle between classic silver/gray 3D bevels and a modern Charcoal Dark Mode for window borders and dialogs.
  - **Smooth drawing (about 60 MB)**: Runs the `picom` compositor: VSync, no tearing, smooth dragging. Off by default to save memory.
  - **System sounds**: Turn the few soft system sounds (startup, errors, questions) on or off. Emptying the Recycle Bin is silent.
- **Screen Saver Tab**:
  - Choose between *Flying Stars*, *Phosphor Matrix*, *Blank Screen*, or custom screensavers.
  - **Browse... Custom Screensaver**: Easily browse and register any executable script or binary as a screensaver.
  - Adjust idle activation timeout (1–60 minutes) and toggle password protection on wake.
  - Full-screen **Preview** button and quick shortcut button to open the custom screensavers folder.
- **Graphics Tab**: On PCs with more than one graphics card, choose which one draws the desktop and which runs programs, and give single programs (Wine, Steam...) a card of their own (**Add...** / **Remove**). A card nothing is set to use is closed to programs (`ferro-gpu close`): Chromium and Electron apps (OpenCode, Claude, VS Code) open every card while starting, and one whose driver can't draw (nouveau on a GTX 16/RTX card without its firmware) left them with a white window. Naming the card for a program opens it again.
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
  - **Network**: Displays link state; clicking opens a small panel with the connection and the Network, VPN, Tor, Microphone and Camera switches, plus **Network & Privacy settings...** for `ferro-network`.
  - **Speaker (Volume)**: Left-click opens a volume slider with Mute and **Mixer...**; right-click toggles mute instantly; the scroll wheel changes the volume; middle-click opens the mixer (`ferro-volume`).
- **Mixer & Sound Output (`ferro-volume`)**: Per-control sliders, plus **Play sound on** to pick the speakers, headphones or a monitor's HDMI speakers, with a **Test** button. There's no sound server: `ferro-audio` points ALSA at the chosen output (`~/.config/ferro/sound.conf`) and switches on HDMI audio, which starts muted on many graphics cards.
- **Other Programs' Icons**: Programs that put an icon in the tray (XEmbed) get one; past five, the rest fold behind a `<` / `>` button.
- **Status Readout & Clock**: Displays live system memory usage and the clock (`XX MB  HH:MM AM`). Clicking the memory number opens Task Manager, clicking the clock opens Date and Time, and right-clicking either offers both.

### Task Manager & Process Context Menus (`ferro-taskmgr`)
- Accessible via `Ctrl+Shift+Esc`, the taskbar's memory number, or the Start menu.
- **Processes Tab**: Real-time PID, CPU%, memory footprint, user account, and active **RAM Budget**.
- **Right-Click Process Context Menu**: Right-click any running process to **End Process**, **Set RAM Budget...**, or **Remove RAM Budget**.
- **Per-Process RAM Quotas**: Set strict kernel cgroup v2 memory limits (e.g., `512M`, `2G`) with dedicated dialog.
- **Performance Tab**: Live 60-second CPU and RAM history graphs.
- **Harmonized Memory Reporting**: Memory usage is calculated using true active process and OS footprint (`footprint_kb`), accounting for Linux kernel low-memory watermark reserves (`totalreserve_pages`) and page-table bookkeeping. Task Manager and the Taskbar display the identical, accurate active RAM metric.
- **Context Menu Isolation**: Process context menus are strictly restricted to row selections on the Processes tab. Right-clicking anywhere on the Performance tab or modal dialogs will never open an unintended process action menu.

### Green Phosphor Command Prompt (`ferro-terminal`)
- A `bash` shell in LXTerminal, styled after phosphor CRT monitors (`#33FF33` bright green text on `#000000` deep black).
- **Full Clipboard & Context Menu Support**: Windows-style **Ctrl+C** (copy) and **Ctrl+V** (paste) shortcut keys, mouse right-click context menu (*Copy*, *Paste*), and terminal text selection.
- **Reliable Backspace & Delete**: Configured with terminal driver initializations (`stty erase '^?'`) and Xresources translations ensuring backspace and delete keys work reliably without control character glitches.
- **Everything Linux Has**: Run any program (`apt`, `sudo`, `python3`, `git`, ...) and `ferro-heavy` to add drivers and software (see [below](#modern-hardware--on-demand-capabilities)). The DOS-style commands (`DIR`, `MEM`, `WIFI`, `TOR`...) belong to Lite's own prompt, `ferro-cmd` (see [Commands](#keyboard-shortcuts--commands)).

---

## Security, Privacy & Anti-Forensics

FerroOS Heavy embeds layered defense-in-depth across the entire operating system stack:

### Optional Full Disk Encryption (LUKS2 + Argon2id)
- The installer (`ferro-setup` / `ferro-install`) configures LUKS2 full-disk encryption with Argon2id key derivation at 1 GiB of memory and 5 seconds of work per attempt, and requires a passphrase of at least 12 characters.
- Users can uncheck encryption during setup for non-sensitive environments or virtual machine testing.
- No swap is set up, so nothing from memory is ever written to the disk outside the encrypted volume.

### Encrypted Extra Drives (`ferro-drives`)
- **Encrypt a Drive** (Start → Programs → System Tools, or type "encrypt" in the Start menu) erases an extra internal disk or USB drive and makes it one LUKS2 (Argon2id) volume holding ext4, owned by you. FerroOS's own disk, the stick it started from, and drives in use are never offered; the root helper (`ferro-drive`) checks the same rules again.
- When a locked encrypted drive is plugged in, or is already there when you log on, a box asks for its passphrase and it opens in My Computer.
- Tick **Unlock automatically on this PC** to keep a random key for the drive on the (encrypted) system disk: it then opens at startup, before anyone logs on, at a fixed place, `/Drives/NAME`, which suits a server's data. Services that need it can be ordered `After=ferro-drives.service`.
- Everything works from the keyboard: Tab moves (the focused control is outlined), Space ticks, Enter goes on and says what's missing.

### Duress Password & Anti-Coercion Self-Destruct
- During setup, an optional secondary **Duress PIN** (at least 10 characters) can be configured alongside your primary password.
- It works at all three prompts: the disk passphrase prompt at startup, the logon screen, and the lock screen. Entered at any of them, the system:
  1. Erases the LUKS2 key slots and overwrites the header area of the FerroOS system disk (and no other disk), so its contents can never be decrypted again, even with the real passphrase.
  2. Powers off at once.
- The PIN is stored only as a salted Argon2id hash. Because the startup check runs before the disk is unlocked, that hash is on the unencrypted boot partition, which is why the PIN must be long: a short one could be guessed from a copy of the disk.
- Limits: it protects against someone who types it on the real machine. It cannot help if the disk was copied first; an examiner working from a copy is not affected by the wipe.

### Cold-Boot Memory Extraction Defense
- The kernel boots with `init_on_alloc=1` and `init_on_free=1`.
- Every physical memory page freed by any process or kernel cache is immediately zeroed by the Linux memory allocator, so no plaintext keys or credentials are left in RAM chips after use.
- Freelist randomization (`page_alloc.shuffle=1`) makes heap exploits harder; the installed system adds slab cache isolation (`slab_nomerge`), `randomize_kstack_offset=on` and `vsyscall=none`.

### No Sleep: Keys Never Left in a Seized Machine
- Sleep keeps the disk's encryption keys in RAM, where a seized machine can be read out (cold-boot and DMA attacks) without the passphrase.
- FerroOS Heavy therefore never sleeps: closing the lid or pressing the sleep key powers off, and suspend and hibernation are disabled.
- **Panic key**: `Ctrl+Alt+End` cuts power instantly, without a clean shutdown, so the keys are gone from memory within moments.
- The kernel starts with IOMMU DMA protection (`intel_iommu=on`, `amd_iommu=force_isolation`, `iommu=force`, `iommu.strict=1`, `efi=disable_early_pci_dma`), so Thunderbolt and other plug-in devices can't read memory directly.

### Tor Routing with Pluggable Transports
- **Heavy**: Debian's `tor`, switched from the network tray panel or `Network > VPN & Tor` (`ferro-tor on [bridges]` / `ferro-tor off`). When on, firewall rules send every program's traffic and DNS through Tor, not only programs set up with a proxy. Pluggable Transports (`obfs4proxy`, `snowflake-client`) get through networks that block Tor. Off by default; the switch is remembered across restarts.
- **Lite**: Built-in Arti (Rust implementation of Tor) with Pluggable Transports (`lyrebird`, `snowflake-client`, `webtunnel-client`). Turned on per session (`TOR ON`) or for good (`TOR ON PERSIST`); bridges via `TOR BRIDGE ADD <line>`, `TOR BRIDGE LIST` and `TOR BRIDGE CLEAR`.

### System Hardening & Network Privacy
- **Encrypted DNS**: Every lookup goes over DNS-over-TLS to Quad9 (`dns.quad9.net`) or Cloudflare (`cloudflare-dns.com`), their certificates checked; never plain DNS, and never the resolver a router hands out. For a Wi-Fi sign-in page that blocks them, tick **Allow this network's sign-in page** on the Wi-Fi tab (`ferro-portal`): it relaxes this for that network only, for up to 10 minutes.
- **What updates touch (Settings > Updates)**: *Update now*, and a tick for each part of FerroOS that updates bring along (`heavy/components.list`; Wisp for now). Unticked, updates leave that part as it is (`ferro-update-hold`, `/etc/ferro/update-hold`). Uninstalling one from the Start menu removes it and unticks it, so it doesn't come back with the next update; ticking it again brings it back. Debian packages FerroOS added and someone uninstalled already stay uninstalled.
- **Security Updates**: Debian's security updates install themselves daily (`unattended-upgrades`, limited to the security archive; never a restart).
- **MAC Address Randomization**: Ephemeral MAC addresses generated for every Ethernet and Wi-Fi connection, including during Wi-Fi probe scanning.
- **Default-Deny Firewall**: `nftables` drops all inbound connections while permitting outbound traffic and essential ICMP/DHCPv6.
- **Restricted Umask**: Files created by users default to private permissions (`umask 077`).
- **Kernel Lockdown**: `lockdown=integrity`, `kptr_restrict=2`, `dmesg_restrict=1`, `yama.ptrace_scope=1`, `unprivileged_bpf_disabled=1`, core dumps disabled (`suid_dumpable=0`), and maximum ASLR entropy (`mmap_rnd_bits=32`).
- **Metadata Removal**: Right-click a file, **Properties**, then the **Info** tab shows its hidden metadata (EXIF, GPS, camera model, author...) with **Strip** / **Strip All** buttons (`exiftool`).

---

## Modern Hardware & On-Demand Capabilities

FerroOS Heavy includes driver support for modern workstations and laptops, with on-demand acceleration tools accessible via `ferro-heavy` or the Start menu:

```bash
ferro-heavy menu            # Choose from a list (Start > Settings > Add Programs & Drivers...)
ferro-heavy status          # View RAM footprint, Secure Boot, LUKS, GPU, and audio status
ferro-heavy update          # Make a restore point, then update everything that's installed
ferro-heavy install <name>  # Make a restore point, then install any Debian package
ferro-heavy drivers         # Install Mesa 3D, Vulkan & VA-API video decode for Intel / AMD
ferro-heavy nvidia          # Build proprietary NVIDIA DKMS drivers + libcuda
ferro-heavy enroll-key      # Let Secure Boot load that driver (MOK enrollment)
ferro-heavy steam           # 32-bit architecture & Vulkan drivers, then Steam (Proton comes with Steam)
ferro-heavy llm             # Python 3, venv & build tools; then shows the pip commands for PyTorch / llama.cpp
ferro-heavy cuda            # Install full NVIDIA CUDA compiler toolkit (nvcc & headers)
ferro-heavy audio           # Set up sound on the output chosen in Volume Control, unmute it, play a test sound
ferro-heavy torbrowser      # Tor Browser
ferro-heavy scummvm         # ScummVM: classic adventure & children's games (Pink Panther, Humongous, LucasArts...)
ferro-heavy wine            # Wine with 32-bit support for Windows programs and older 32-bit/16-bit games
```

- **Wi-Fi Cards**: Firmware included out of the box for Intel (`iwlwifi`), Realtek (`rtw88`/`rtw89`), MediaTek (`mt7921`/`mt7922`), Qualcomm Atheros (`ath10k`/`ath11k`), and Broadcom.
- **Display Drivers**: Modesetting for Intel Gen1–Gen14, AMD Radeon (`amdgpu`), NVIDIA (`nouveau`), with universal UEFI GOP (`fbdev`) and VESA fallbacks.
- **Audio**: No sound server: plain ALSA (`alsa-utils`), set up by `ferro-audio` for the output picked in the mixer, with Sound Open Firmware (`firmware-sof-signed`) for laptop internal speakers and digital microphone arrays.

---

## Ultra-Lean Idle Footprint: How It Works

Standard desktop Linux distributions consume between 800 MB and 2.5 GB of RAM at idle. FerroOS Heavy achieves its ultra-lean memory footprint through targeted systems optimization:

1. **Kernel Hash Table Pinning**: Linux dynamically sizes internal dcache, icache, and page hash tables based on total system RAM—consuming 30–50+ MB on modern machines. Heavy pins these tables at boot (`dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512`), freeing dozens of megabytes immediately.
2. **Transparent Hugepage Suppression**: Booting with `transparent_hugepage=never` prevents the kernel from allocating 2 MB hugepages to small background daemons, avoiding memory fragmentation.
3. **Lean Xorg Display Layer**: RECORD and XTEST extensions are disabled to reduce memory footprint, while Xorg Composite and unprivileged user namespaces (`kernel.unprivileged_userns_clone=1`) remain enabled so Chromium, Electron (e.g. Antigravity), and hardware-accelerated GUI apps render cleanly without blank window artifacts.
4. **Aggressive Cache Reclaim**: `vm.vfs_cache_pressure=200` and `vm.swappiness=10` aggressively prioritize reclaiming unused directory/inode caches.
5. **Capped Journal Buffers**: `systemd-journald` volatile runtime storage is capped at 4 MB (`RuntimeMaxUse=4M`) with compression enabled (`Compress=yes`).
6. **Lean NetworkManager**: Configured with internal C DHCP client (`dhcp=internal`), file-only keyfile plugins, and connectivity polling disabled.
7. **Masked Redundant Daemons**: Virtual consoles `tty3`–`tty6`, the ext4 scrubber (`e2scrub_all`) and firmware-update polling (`fwupd-refresh`) are masked to eliminate background CPU and RAM wakeups. APT's daily timer stays, since it runs the security updates.

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
| `Win + L` | Lock the screen |
| `Win + ←` / `→` / `↑` / `↓` | Snap the window left / right, maximize, restore (or minimize) |
| `Win + V` | Clipboard history |
| `Win + N` | Notifications |
| `Print` or `Win + Shift + S` | Snip part of the screen |
| `Alt + F4` | Close active window |
| `Alt + Tab` / `Alt + Shift + Tab` | Cycle between open windows |
| `Ctrl + Alt + End` | Panic: power off at once (see [No Sleep](#no-sleep-keys-never-left-in-a-seized-machine)) |

### Lite's Command Prompt (`ferro-cmd`)

Heavy's Command Prompt is `bash`. Lite has its own DOS-style prompt, an interactive line editor with VT100 / ANSI escape processing:
- **Arrow Keys & Cursor Movement**: Pressing Left / Right arrow moves the insertion point `|` smoothly across the line without printing raw escape sequences (e.g. `^[[D`).
- **Command History**: Pressing Up / Down arrow recalls and edits previous commands.
- **Navigation & Editing Hotkeys**: Supports `Home` / `Ctrl+A` (start of line), `End` / `Ctrl+E` (end of line), `Delete` (forward delete), `Ctrl+K` (delete to end), `Ctrl+U` (delete to start), and `Ctrl+C` (cancel).
- **Clean Backspace**: Uniformly processes both `0x7F` (DEL) and `0x08` (BS) so backspacing never outputs random characters or artifacts.

```text
DIR / CD / TYPE         Standard file navigation and inspection
MEM                     Display free, used, and total system memory
MEM /DETAIL             Detailed memory breakdown including kernel tables
MEMTEST <MB>            Test that much RAM
RUN /NET <app>          Launch an application with pre-authorized network access
RUN /NONET <app>        ...or with none
RUN /MEM:<size> <app>   ...with a RAM limit
PERMS /FORGET ALL       Clear all remembered application network permissions
WIFI LIST               Scan and list available Wi-Fi networks
WIFI JOIN <ssid>        Connect to a Wi-Fi network (prompts for passphrase)
WIFI OFF                Disconnect from Wi-Fi
BATTERY                 Query laptop battery charge level and AC power status
BRIGHTNESS <0-100>      Adjust display backlight brightness
VPN IMPORT <path.conf>  Import a WireGuard configuration file
VPN ON / VPN OFF        Toggle WireGuard VPN tunnel
TOR ON / TOR OFF        Toggle system-wide Tor routing (TOR ON PERSIST keeps it on)
TOR BRIDGE ADD/LIST/CLEAR  Manage Tor bridges
DEBIAN apt install <pkg> Install any Debian package (with persistent LUKS2 storage)
DEBIAN /RESET           Forget everything installed, at the next start
```

> **Note on Debian Package Availability:**
> - Classic `synapse` (the legacy GTK2 semantic launcher) was deprecated and removed from modern Debian repositories. If you are looking for an application launcher, use `rofi` (`ferro-heavy install rofi`) or `albert`. If you want the Matrix homeserver, use `matrix-synapse` (`ferro-heavy install matrix-synapse`).

---

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain)
- [QEMU](https://www.qemu.org) (`qemu-system-x86_64`)
- Host C compiler (`clang`, `llvm`, and `musl-tools` on Linux; MinGW GCC on Windows)

The Rust userspace is compiled on your machine as static `x86_64-unknown-linux-musl` binaries (hence clang and musl-tools); the kernel, Tor, Debian and Heavy images are built inside isolated, self-contained QEMU builder VMs, so no container engines are required.

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
- Builds Wisp from the commit in `heavy/wisp.commit` for the image (installed systems then follow Wisp's own releases, which its repository publishes on every push to `main`) (`tools/heavy/build-wisp.sh`, in a Debian container), and the update bundle that installed systems fetch with `ferro-update` (`ferroos-heavy-update.tar.gz`). The bundle carries FerroOS's own programs plus the release's package lists: `ferro-update` installs each Debian package newly added to `heavy/packages.txt` and removes each one listed in `heavy/packages-removed.txt`, once, so an installed system gets everything a fresh install has. A package someone uninstalled isn't put back, and one that can't be had yet is tried again at the next update. A newer `ferro-update` in the bundle runs its own steps.
- Computes SHA-256 checksums (`ferroos-heavy.iso.sha256`, `ferroos-heavy-update.tar.gz.sha256`) and a signed build-provenance attestation, so a download can be checked against this repository: `gh attestation verify ferroos-heavy.iso --repo anti-ghostsec/FerroOS`.
- Publishes the ISO, the update bundle, `ferro-update` and the checksums to GitHub Releases, and removes all but the two newest releases.

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
- Tor (Heavy): BSD-3-Clause
- Arti (Tor, Lite): MIT / Apache-2.0
- Debian Base Packages: Respective upstream open-source licenses
