# Folio host deploy

Files for the Docker Compose host (the `folio` LXC). The release workflow's
gated `deploy` job records a GitHub Deployment; these poll for it and apply
it. See [docs/dev/deploy.md](../../docs/dev/deploy.md).

Install (as root on the host):

```sh
install -d /opt/folio/bin /var/lib/folio-deploy
install -m 0755 folio-deploy.sh folio-deploy-poll.sh /opt/folio/bin/
install -m 0644 folio-deploy.service folio-deploy.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now folio-deploy.timer
```

Check: `systemctl list-timers folio-deploy.timer`, `journalctl -u folio-deploy`.
Deploy by hand: `/opt/folio/bin/folio-deploy.sh v1.2.3`.
