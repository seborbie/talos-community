# talos_worker

RMM agent: connects to the server, reports endpoint inventory/health, runs approved commands, and handles supported interactive transports.

Windows remains the full remote desktop platform. Desktop capture uses DXGI and a Session 1 helper process, and the agent is intended to run as a Windows service.

Linux is supported for enrollment, inventory, health telemetry, approved shell command execution, interactive PTY-backed system shell, and file transfer. Remote desktop, remote registry, and chat are reported as unavailable on Linux.

## Building

The Windows agent includes VP8 encoding for the capture/stream transport layer. Building Windows remote desktop support requires **libvpx** and **libyuv**. On Windows, set the env vars below before building (or use pkg-config for libvpx).

```bash
cargo build -p talos_worker
```

Linux managed-endpoint build:

```bash
cargo build -p talos_worker --release --target x86_64-unknown-linux-gnu
```

The Linux target does not build the Windows capture stack, libvpx, libyuv, ConPTY, or Windows service dependencies. See [`linux/README.md`](linux/README.md) for systemd install and uninstall steps.

### libyuv

**libyuv** is used for BGRA→I420 conversion and scaling (SIMD-optimized). The `yuv-sys` crate builds libyuv from source and generates Rust bindings at build time.

- **Windows:** The bindings step requires **LLVM/clang** (for bindgen). Install LLVM from [releases.llvm.org](https://releases.llvm.org/) and set `LIBCLANG_PATH` to the `bin` directory that contains `libclang.dll` (e.g. `C:\Program Files\LLVM\bin`). Alternatively, use the VS Developer Command Prompt if it provides clang.
- **Linux/macOS:** A system clang is usually enough; set `LIBCLANG_PATH` if the build cannot find libclang.

No separate vcpkg install is required for libyuv; the crate compiles and links it automatically.

### libvpx on Windows (vcpkg)

Run the repository bootstrap from PowerShell:

```powershell
.\scripts\Setup-DevEnviroment.ps1
```

The script checks out the exact commit from Microsoft's signed
[vcpkg 2026.05.25 release](https://github.com/microsoft/vcpkg/releases/tag/2026.05.25), builds the
reviewed libvpx overlay for x64 and x86, records that provenance under `C:\vcpkg\installed`, and
sets the following user environment variables. Release builds reject native inputs without the
matching provenance marker.

```powershell
$env:VPX_LIB_DIR     = "C:\vcpkg\installed\x64-windows\lib"
$env:VPX_INCLUDE_DIR = "C:\vcpkg\installed\x64-windows\include"
$env:VPX_VERSION     = "1.13.0"
```

`VPX_VERSION=1.13.0` selects the newest bindings shipped by `env-libvpx-sys`; the repository
overlay deliberately builds ABI-compatible libvpx 1.13.1.

## Usage

- Normal agent: set `RMM_SERVER_URL` (and optionally `RMM_AGENT_TOKEN`), then run `talos_worker`.
- Direct public-UDP discovery is disabled unless `RMM_STUN_SERVER` is set to an
  operator-approved `hostname-or-IPv4:port`. Leave it unset to avoid an external STUN request and
  use the relay fallback. In an `auto` Viewer session, both endpoint processes must receive the
  same STUN setting for a direct public-UDP attempt.
- Encoding (fps, quality presets) is available in code for the transport layer. Presets: `grayscale`, `low`, `medium`, `high`, `maximum` (16 Mbps, minimal encoding artifacts).

## Running as a Windows service

The agent is designed to run as a Windows service (Session 0). It reports **SERVICE_RUNNING** to the Service Control Manager as soon as the service is ready, so the service does not get stuck in "Starting"—this happens even if the agent has not yet connected to the server.

### Build installer artifacts

1. Open PowerShell **as Administrator**.
2. From the repository root, bootstrap the manifest key as described in the
   [installer guide](../installer/README.md), then make the unsigned Community choice explicit:
   ```powershell
   $manifestPassword = Read-Host "Manifest PFX password" -AsSecureString
   .\scripts\build-installers.ps1 `
     -BuildProfile release `
     -SkipAuthenticodeSigning `
     -ManifestCertificatePath "D:\Protected\Talos\community-manifest-signing.pfx" `
     -ManifestCertificatePassword $manifestPassword
   ```
3. The script builds `talos_worker` and `talos_worker_helper` for x64 + x86, optionally Authenticode-signs agent and Viewer binaries, always signs published updater manifests, stages payloads in `apps/installer/payload`, builds both MSI packages, builds the agent Burn bootstrapper EXE, and publishes the Viewer MSI installer. See [`apps/installer/README.md`](../installer/README.md) for the independent Authenticode and manifest-key inputs.
4. Select compiler profile with `-BuildProfile`:
   - `dev`
   - `release`
5. The build acquires and verifies the exact LZMA SDK 26.00 SFX stub and 7-Zip Extra 26.00
   `7za.exe`; do not commit those generated binaries. A custom `-SfxStubPath` is accepted only when
   it matches the reviewed SFX SHA-256.
6. The acquisition bootstrap prefers the host `tar`. If it is unavailable, ensure 7-Zip is on
   `PATH` or pass `-SevenZipPath`; subsequent archive creation always uses the acquired pinned tool.
7. Installer artifacts are copied to `apps/installer/artifacts/<profile>/` (for example `apps/installer/artifacts/release/`) and include:
   - `Talos.Agent.Setup.exe`
   - `Talos.Agent.Setup.7z`
   - `Talos.Viewer.x64.msi`
   - `7zSD.sfx`
   - `manifest.json`
   - `UNSIGNED-BINARIES.txt`
   - `SHA256SUMS`
   - `build-provenance.json`
8. At the end, the script prints the full paths to the published bundle, `.7z` payload archive,
   SFX stub, manifest, checksums, and build metadata. Initial official Community binaries are
   intentionally unsigned; do not hide the accompanying notice or describe them as publisher-signed.
9. Logs are written to **`C:\ProgramData\Talos\logs\talos_worker.log`** for both the agent and helper. To watch logs:
   ```powershell
   Get-Content -Path C:\ProgramData\Talos\logs\talos_worker.log -Wait
   ```

### Stop and remove

Run as Administrator:

```powershell
.\apps\talos_worker\scripts\uninstall-service.ps1
```

This stops and deletes the **RmmAgent** service.
