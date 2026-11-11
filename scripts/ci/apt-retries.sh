#!/usr/bin/env bash
# Keep a job's apt calls working when the runner cannot reach Ubuntu's
# package hosts.
#
# GitHub-hosted runners sometimes cannot resolve those hosts ("Temporary
# failure resolving 'security.ubuntu.com'"). On 2026-11-11 that failed all
# four test shards of one pull request at once (installing xmllint) and the
# smoke job of another (Playwright's `install --with-deps`, which runs apt
# itself), before a single test ran. It lasted for hours
# (actions/runner-images#15296), while every other download on the same
# runners worked.
#
# Two measures, both written to apt's own configuration so they cover every
# later apt call in the job, including the ones other tools make:
#
# 1. `Acquire::Retries` makes apt try each file again, with its built-in
#    back-off between attempts. That is enough for a short miss.
#
# 2. The runner image gives apt a list of mirrors, all three under
#    ubuntu.com (/etc/apt/apt-mirrors.txt: azure.archive, archive,
#    security). A full mirror on another domain is added to that list. It
#    goes last while the Ubuntu hosts resolve, so a healthy job never uses
#    it, and first when they do not, so the job does not wait out three dead
#    hosts for every file.
#
#    apt checks every package index against the Ubuntu archive key whatever
#    host served it, so the mirror cannot change what gets installed.
set -euo pipefail

echo 'Acquire::Retries "5";' | sudo tee /etc/apt/apt.conf.d/80-retries >/dev/null

mirrors=/etc/apt/apt-mirrors.txt
fallback=http://mirrors.edge.kernel.org/ubuntu/

if [ -f "$mirrors" ] && ! grep -qF "$fallback" "$mirrors"; then
  if getent hosts azure.archive.ubuntu.com >/dev/null 2>&1; then
    count=$(grep -c . "$mirrors" || true)
    printf '%s\tpriority:%s\n' "$fallback" "$((count + 1))" | sudo tee -a "$mirrors" >/dev/null
  else
    echo "::warning title=apt fallback mirror::Ubuntu's package hosts do not resolve on this runner. apt uses ${fallback} first for this job."
    {
      printf '%s\tpriority:1\n' "$fallback"
      awk -F'\t' 'NF { print $1 "\tpriority:" NR + 1 }' "$mirrors"
    } | sudo tee "$mirrors.new" >/dev/null
    sudo mv "$mirrors.new" "$mirrors"
  fi
  echo "apt mirror list:"
  cat "$mirrors"
fi
