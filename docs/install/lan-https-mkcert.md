# LAN HTTPS for homelab deployments (mkcert)

Folio's session cookies use the `__Host-` / `__Secure-` prefixes, which
browsers enforce as Secure-only. **Plain HTTP doesn't work end-to-end**
— the API returns 200 with `Set-Cookie` headers, the browser silently
drops them, and the user appears to "fail" sign-in with no error.
You'll see the same behaviour on a phone hitting `http://192.168.x.x`
on your LAN.

Public deployments handle this with Let's Encrypt. **For LAN-only
homelabs without a real domain**, [mkcert](https://github.com/FiloSottile/mkcert)
is the cleanest path: a one-time local CA that your browser, phone, and
tablet trust, plus a short-lived cert for whatever local hostname you
pick.

## What you'll do

1. Install mkcert on the host running Folio.
2. Generate a local CA + a cert for `folio.lan` (or whatever you call
   it).
3. Bring up Caddy as a TLS-terminating sidecar using those certs.
4. Install the mkcert root CA on each device that needs to reach Folio
   (laptop, phone, tablet).
5. Add `folio.lan` to your home network's DNS (Pi-hole, AdGuard Home,
   `dnsmasq`) or to each device's hosts file.

End state: `https://folio.lan` works for sign-in from every device on
your LAN.

## 1. Install mkcert

```bash
# Debian / Ubuntu
sudo apt install libnss3-tools
curl -L https://github.com/FiloSottile/mkcert/releases/latest/download/mkcert-v1.4.4-linux-amd64 -o /usr/local/bin/mkcert
sudo chmod +x /usr/local/bin/mkcert

# Arch
sudo pacman -S mkcert nss

# macOS
brew install mkcert nss
```

## 2. Generate the CA + cert

```bash
mkdir -p /opt/folio/certs && cd /opt/folio/certs
mkcert -install
mkcert -cert-file cert.pem -key-file key.pem folio.lan
```

`-install` installs the mkcert root into the system trust store of the
host that ran it. **That's not enough for other devices** — see step 4.

## 3. A Caddy sidecar in compose

Create `compose.lan.yml` next to your `compose.prod.yml`:

```yaml
# compose.lan.yml — layer onto compose.prod.yml for LAN HTTPS.
# Usage:
#   docker compose -f compose.prod.yml -f compose.lan.yml up -d

services:
  caddy:
    image: caddy:2-alpine
    restart: unless-stopped
    depends_on:
      app: { condition: service_healthy }
      web: { condition: service_healthy }
    ports:
      - "443:443"
      - "80:80"
    volumes:
      - ./Caddyfile.lan:/etc/caddy/Caddyfile:ro
      - ./certs:/certs:ro
      - caddy_data:/data
      - caddy_config:/config

volumes:
  caddy_data:
  caddy_config:
```

And the Caddyfile:

```caddyfile
# /opt/folio/Caddyfile.lan
folio.lan {
    tls /certs/cert.pem /certs/key.pem

    # Single upstream — the Rust binary handles everything, including
    # reverse-proxying HTML to its internal Next.js SSR upstream over
    # the compose bridge. See `caddy.md` for the production analog.
    reverse_proxy app:8080 {
        flush_interval -1
        transport http {
            read_timeout  10m
            write_timeout 10m
            keepalive 60s
        }
    }
}
```

Bring up the stack:

```bash
cd /opt/folio
docker compose -f compose.prod.yml -f compose.lan.yml up -d
```

## 4. Trust the mkcert root on other devices

`mkcert -CAROOT` prints the path to the root CA file:

```bash
$ mkcert -CAROOT
/home/you/.local/share/mkcert
```

Copy `rootCA.pem` from that directory to each device:

- **macOS:** double-click `rootCA.pem` → Keychain Access → System →
  set the cert to "Always Trust".
- **iOS / iPadOS:** AirDrop or email `rootCA.pem` to the device →
  Settings → General → VPN & Device Management → install profile →
  Settings → General → About → Certificate Trust Settings → enable for
  the mkcert root.
- **Android:** Settings → Security → Encryption & credentials → Install
  a certificate → CA certificate. (Android 11+ separates user-installed
  CAs from system ones; some apps won't honour user CAs. For a homelab
  this is usually fine.)
- **Windows:** double-click `rootCA.pem` → Install → Local Machine →
  Trusted Root Certification Authorities.
- **Linux desktop:** `sudo cp rootCA.pem /usr/local/share/ca-certificates/mkcert-root.crt && sudo update-ca-certificates`.

## 5. Make `folio.lan` resolve

Pick whichever fits your network:

- **Pi-hole / AdGuard Home:** add a local DNS record
  `folio.lan → 192.168.x.x` (the Folio host IP).
- **OPNsense / pfSense:** Services → Unbound DNS → Host Overrides.
- **No local DNS server:** add to each client's hosts file. On a phone
  this isn't possible without root, so go the local-DNS route.

## Verifying

```bash
# From the Folio host:
curl --cacert "$(mkcert -CAROOT)/rootCA.pem" https://folio.lan/readyz

# From a device after installing the root CA:
# open https://folio.lan in the browser — no certificate warning.
```

## When to consider Let's Encrypt instead

If you can put Folio behind a free dynamic-DNS domain (DuckDNS,
no-ip.com) **and** open port 80/443 inbound to your router, Caddy +
Let's Encrypt is less work than mkcert and doesn't need you to install
a CA on every device. The mkcert path is for fully air-gapped homelabs
or "I refuse to expose port 80" setups.

## Why not just disable Secure-cookie enforcement?

It would be a one-line patch. It would also weaken the production
posture (we'd then have to ship two cookie schemes and decide between
them at boot, which is its own footgun). mkcert is two minutes per
device once and zero ongoing maintenance.
