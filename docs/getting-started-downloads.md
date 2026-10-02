# Run Talos from a download

Talos Community is alpha software for a disposable test environment. Releases are prepared by
the protected release workflows; this guide does not imply that a release has been approved or
published. Check [GitHub Releases](https://github.com/seborbie/talos-community/releases) for an
approved prerelease and its known limitations.

You need **Docker Engine with Compose v2 or newer** on Linux x86-64, or **Docker Desktop running
Linux containers** on Windows x64. Start Docker first. Review Docker Desktop's licensing with your
organisation: Talos does not install Docker, accept its licence, or alter Docker permissions.
Bun, Rust, Git, a compiler, and a separately installed database are unnecessary on the target host.

## Windows

1. Download `talos-community-<version>-windows-x86_64-UNSIGNED.zip` and `SHA256SUMS` from the same
   release. Compare `(Get-FileHash .\talos-community-<version>-windows-x86_64-UNSIGNED.zip).Hash`
   with the matching checksum. Review the release's provenance and unsigned-binary notice.
2. Extract the entire ZIP to a local folder. Double-click **Start-Talos.cmd**. Keep the console
   open until Talos reports that it is running and opens the browser.
3. Follow the local certificate instructions below, then open **https://talos.localhost:8443**
   and create your first account. There is no default password; registration closes after the
   first account.

The launcher uses `%LOCALAPPDATA%\Talos\Server` for this account. Run subsequent commands from the
same Windows account. Closing the console does not stop Talos.

```bat
Start-Talos.cmd status
Start-Talos.cmd stop
Start-Talos.cmd start
Start-Talos.cmd backup --name before-testing
Start-Talos.cmd diagnostics
```

Windows downloads are intentionally unsigned. SmartScreen or organisation policy may block them.
Verify the source/checksum and use your organisation's software approval process. Do not disable
security controls. See the bundled `notices-and-guides/docs/release-signing.md` for the separate
updater-manifest signing policy.

## Linux

The release workflow targets **Ubuntu 24.04 x86-64** for the launcher build and startup smoke.
That is the download baseline; older glibc distributions and musl/Alpine hosts are not verified.
The separately published application containers carry their own runtime dependencies.

Download `talos-community-<version>-linux-x86_64.tar.gz` and `SHA256SUMS`. Compare its
`sha256sum` with the corresponding line, extract it, and enter the extracted folder:

```sh
tar -xzf talos-community-<version>-linux-x86_64.tar.gz
cd talos-community-<version>
sudo ./start-talos.sh
```

Open **https://talos.localhost:8443**, follow the certificate instructions, and create the first
account. Repeat `sudo ./start-talos.sh` to start the saved installation. No image build or registry
login is required for approved public images.

```sh
sudo ./start-talos.sh status
sudo ./start-talos.sh stop
sudo ./start-talos.sh start
sudo ./start-talos.sh backup --name before-testing
sudo ./start-talos.sh diagnostics
```

The Linux state directory is `/var/lib/talos-server`. Docker administration is a privileged host
operation; use your existing authorised Docker access. The launcher does not add users to groups.
Linux images are built for `linux/amd64` and `linux/arm64`; the downloadable launcher and this
quickstart support **Linux x86-64** and **Windows x64**. ARM64 container builds alone are not evidence
of an ARM64 launcher or a verified ARM64 installation.

## Local certificate and networking

The launcher generates one private self-signed certificate for all four `.localhost` names. It
binds only IPv4 loopback ports **8080 and 8443**, makes no ACME requests, and does not expose
PostgreSQL or application ports directly. The API uses `api.talos.localhost:8443`; accepting a
browser exception only for the frontend can leave API requests blocked.

Approve this installation's generated public certificate in your browser/organisation trust
process before using the UI. Windows stores it at
`%LOCALAPPDATA%\Talos\Server\local-tls\certificate.pem`; Linux stores it at
`/var/lib/talos-server/local-tls/certificate.pem`. Verify it came from your local installation and
approve only that certificate. Never share or import `private-key.pem`. Talos does not modify
certificate stores automatically. The certificate lasts 30 days and is renewed on start near
expiry; your trust process may need to approve the replacement.

On Windows, if your organisation permits current-user certificate trust, open a terminal under
the same account, inspect the certificate, and then explicitly approve it:

```bat
certutil -dump "%LOCALAPPDATA%\Talos\Server\local-tls\certificate.pem"
certutil -user -addstore Root "%LOCALAPPDATA%\Talos\Server\local-tls\certificate.pem"
```

The second command is a deliberate **trust change for your user**, not a prerequisite silently
performed by Talos. Record the displayed certificate thumbprint so your administrator can remove
this test certificate after evaluation. Reopen the browser. Browsers with their own certificate
store need the equivalent explicit import through their certificate settings.

On Linux, inspect the certificate with
`sudo openssl x509 -in /var/lib/talos-server/local-tls/certificate.pem -noout -subject -dates -fingerprint -sha256`.
Use your browser's certificate settings or your administrator's trust process to import **only
the public certificate**. Browser trust varies across Linux distributions; a system-wide CA-store
change is not required by this guide. If certificate approval is unavailable, use the documented
public/custom-certificate deployment with a certificate your organisation already trusts.

If your browser or resolver does not map `.localhost` names to loopback, add these **local** names
through your organisation's host-resolution process:

```text
127.0.0.1 talos.localhost api.talos.localhost control.talos.localhost relay.talos.localhost
```

This default is for testing on the controller machine. Testing remote agents requires real DNS
names and a publicly trusted or organisation-approved certificate; follow the bundled
`notices-and-guides/docs/community-deployment.md` and `community-edge.md`. Use
`community-install.example.json` for that separate install. Do not broaden the local listener to
the LAN without configuring the documented authentication, TLS, and edge boundary.

## Storage, updates, and recovery

Talos generates independent JWT, encryption, control-service, and PostgreSQL credentials. They are
stored with owner-only Unix permissions or the Windows owner/Administrators/SYSTEM DACL.
PostgreSQL data lives in the Docker named volume `talos-community_talos_postgres_data`.
There is no Redis dependency; telemetry/AI infrastructure is omitted from the Community stack.

Keep both the state directory and Docker volume. Stopping containers, closing the launcher,
rebooting, or extracting another copy of the **same** bundle preserves data. Containers use
`unless-stopped`; start Docker and rerun the launcher after a reboot if needed. OS service
registration and unattended Docker startup are not provided.

A different release/configuration is deliberately rejected by quickstart. For an upgrade, retain
the old bundle, take an off-host verified backup, review the release notes, and use the platform's
wrapper with `update --config <new-bundle>/community-install.local.json`. Protect that JSON with
`chmod 600` on Linux. Updating never silently replaces the installed credentials. Database
migration failures can require restoring the recorded backup; reversing an image is not a schema
rollback. Follow `notices-and-guides/apps/talos_appliance/README.md`.

`uninstall` preserves state and volumes. `uninstall --remove-data --confirm <installation-id>`
irreversibly removes them; `status` reports the required ID. Back up before deleting anything.
Only one Talos Community Compose project is supported per Docker daemon, even with a different
state directory or Windows account.

## If startup fails

- **Docker missing/stopped or Windows containers selected:** install/start your approved Docker
  runtime, select Linux containers, then rerun. A prerequisite failure creates no installation.
- **Port already allocated:** release 8080/8443 or deliberately edit both ports in the local JSON
  before the first install; use the URL printed by the launcher. Windows automatic browser opening
  uses the shipped 8443 default.
- **Subnet collision:** choose a free private `/24` and matching `proxy_ipv4` together in the local
  JSON before installing. Never relax the proxy allowlist.
- **Image pull denied:** an approved public GHCR release must pull without authentication. Keep
  the error, check network/proxy access, and contact the maintainer; no PAT should be necessary.
- **Interrupted installation:** rerun the same bundle. The launcher retries a matching install
  only before migrations may have started. If it requests recovery, run `status`, retain the
  diagnostic output, and follow the recovery guide. It never deletes state to make a retry succeed.

The archives include AGPL-3.0-only licence text, third-party notices, source revision, SBOMs,
immutable image references, and inner checksums. The release also supplies the corresponding
reviewed source archive. Retain those materials when redistributing or operating a modified copy.
