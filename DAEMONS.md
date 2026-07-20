# Running Lazy with launchd or systemd

Lazy can accept a TCP listener opened by the operating system's service
manager. This is the recommended way to use ports below 1024: launchd or
systemd binds the port, then runs `lazy proxy` as a normal user.

The examples below use:

- an activated socket named `HTTPS`
- HTTPS on `127.0.0.1:443`
- the existing `~/.lazy` state directory
- a certificate and key readable by the developer account

Replace usernames, home directories, binary paths, DNS settings, and TLS paths
before installing either example. To listen on a LAN address, replace
`127.0.0.1` and protect the port with an appropriate firewall or network ACL.

The relevant Lazy options are:

```text
--activated-socket NAME  Receive a named listener from launchd or systemd
--public-port PORT       Put this port in generated service URLs
--state-dir PATH         Use this directory for the control socket and state
```

`--activated-socket` and `--listen` cannot be used together. `--state-dir` is a
global option and can also be set with `LAZY_STATE_DIR`. Every `lazy` command
that communicates with this daemon must resolve the same state directory.

## macOS: launchd

A LaunchDaemon can create the privileged listener while `UserName` keeps Lazy
unprivileged. A per-user LaunchAgent is not sufficient for binding port 443.

Create `/Library/LaunchDaemons/com.example.lazy-proxy.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.lazy-proxy</string>

  <key>ProgramArguments</key>
  <array>
    <string>/Users/alice/.local/bin/lazy</string>
    <string>proxy</string>
    <string>--activated-socket</string>
    <string>HTTPS</string>
    <string>--public-port</string>
    <string>443</string>
    <string>--state-dir</string>
    <string>/Users/alice/.lazy</string>
    <string>--suffix</string>
    <string>.localhost</string>
    <string>--cert</string>
    <string>/Users/alice/.config/lazy/localhost.pem</string>
    <string>--key</string>
    <string>/Users/alice/.config/lazy/localhost-key.pem</string>
  </array>

  <key>UserName</key>
  <string>alice</string>

  <key>Sockets</key>
  <dict>
    <key>HTTPS</key>
    <dict>
      <key>SockNodeName</key>
      <string>127.0.0.1</string>
      <key>SockServiceName</key>
      <string>443</string>
      <key>SockType</key>
      <string>stream</string>
    </dict>
  </dict>

  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
```

Validate and install it:

```sh
plutil -lint /Library/LaunchDaemons/com.example.lazy-proxy.plist
sudo chown root:wheel /Library/LaunchDaemons/com.example.lazy-proxy.plist
sudo chmod 644 /Library/LaunchDaemons/com.example.lazy-proxy.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.example.lazy-proxy.plist
```

Inspect the job and its logs:

```sh
sudo launchctl print system/com.example.lazy-proxy
log stream --predicate 'process == "lazy"'
```

Reload after changing the plist:

```sh
sudo launchctl bootout system/com.example.lazy-proxy
sudo launchctl bootstrap system /Library/LaunchDaemons/com.example.lazy-proxy.plist
```

launchd owns the listening socket across Lazy restarts. The state directory and
`lazy.sock` are created by `alice`, not root.

## Linux: systemd

Use a system socket unit to bind port 443 and a service unit with `User=` to run
Lazy as the developer account.

Create `/etc/systemd/system/lazy-proxy.socket`:

```ini
[Unit]
Description=Lazy HTTPS listener

[Socket]
ListenStream=127.0.0.1:443
FileDescriptorName=HTTPS
Service=lazy-proxy.service

[Install]
WantedBy=sockets.target
```

Create `/etc/systemd/system/lazy-proxy.service`:

```ini
[Unit]
Description=Lazy development proxy
Requires=lazy-proxy.socket
After=lazy-proxy.socket

[Service]
Type=simple
User=alice
ExecStart=/home/alice/.local/bin/lazy proxy --activated-socket HTTPS --public-port 443 --state-dir /home/alice/.lazy --suffix .localhost --cert /home/alice/.config/lazy/localhost.pem --key /home/alice/.config/lazy/localhost-key.pem
Restart=on-failure
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

Load and start the units:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now lazy-proxy.socket lazy-proxy.service
```

Inspect their status and logs:

```sh
systemctl status lazy-proxy.socket lazy-proxy.service
journalctl -u lazy-proxy.service -f
```

Reload after editing either unit:

```sh
sudo systemctl stop lazy-proxy.service lazy-proxy.socket
sudo systemctl daemon-reload
sudo systemctl start lazy-proxy.socket lazy-proxy.service
```

The Linux implementation reads systemd's `LISTEN_PID`, `LISTEN_FDS`, and
`LISTEN_FDNAMES` variables directly, so it does not require `libsystemd` and
continues to work with Lazy's static MUSL binaries.

## Using xip-style DNS

Replace `--suffix .localhost` in either service definition with the same xip
configuration used at the command line:

```text
--xip-domain xip.example.com --xip-ip 192.0.2.10
```

The activated listener address and the address encoded in the public hostname
are deliberately configured independently. Keep `--public-port 443` so Lazy
generates URLs without an explicit nonstandard port.

## Verification

After starting the daemon, run commands as the configured developer, without
`sudo`:

```sh
lazy status
lazy http demo --upstream-port 8000 -- python3 -m http.server 8000
```

Then open <https://demo.localhost>. Confirm ownership and permissions:

```sh
ls -ld ~/.lazy
ls -l ~/.lazy/lazy.sock
```

The directory should be owned by the developer with mode `0700`; the socket
should be owned by the developer with mode `0600`. `lazy status`, runners, and
application commands should never need root privileges.

If you selected a different state directory, pass `--state-dir PATH` to client
commands or export it once:

```sh
export LAZY_STATE_DIR=/path/to/lazy-state
```
