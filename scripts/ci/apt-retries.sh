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
#    security). Full mirrors on other domains are added to that list. They
#    go last while the Ubuntu hosts resolve, so a healthy job never uses
#    them, and first when they do not, so the job does not wait out three
#    dead hosts for every file.
#
#    More than one, because a single mirror can have a bad moment of its
#    own: the same day one smoke job read the package index from the first
#    mirror and then could not connect to it for the packages, with the
#    Ubuntu hosts still unreachable behind it.
#
#    apt checks every package index against the Ubuntu archive key whatever
#    host served it, so a mirror cannot change what gets installed. Each one
#    listed here was checked to carry the runner's release with its
#    `-updates` and `-security` pockets and the `amd64v3` variant.
set -euo pipefail

echo 'Acquire::Retries "5";' | sudo tee /etc/apt/apt.conf.d/80-retries >/dev/null

mirrors=/etc/apt/apt-mirrors.txt
fallbacks=(
  http://mirrors.edge.kernel.org/ubuntu/
  http://mirrors.mit.edu/ubuntu/
  http://ftp.osuosl.org/pub/ubuntu/
)

if [ -f "$mirrors" ] && ! grep -qF "${fallbacks[0]}" "$mirrors"; then
  if getent hosts azure.archive.ubuntu.com >/dev/null 2>&1; then
    count=$(grep -c . "$mirrors" || true)
    for url in "${fallbacks[@]}"; do
      count=$((count + 1))
      printf '%s\tpriority:%s\n' "$url" "$count"
    done | sudo tee -a "$mirrors" >/dev/null
  else
    echo "::warning title=apt fallback mirrors::Ubuntu's package hosts do not resolve on this runner. apt uses ${fallbacks[*]} first for this job."
    {
      n=0
      for url in "${fallbacks[@]}"; do
        n=$((n + 1))
        printf '%s\tpriority:%s\n' "$url" "$n"
      done
      awk -F'\t' -v base="${#fallbacks[@]}" 'NF { print $1 "\tpriority:" NR + base }' "$mirrors"
    } | sudo tee "$mirrors.new" >/dev/null
    sudo mv "$mirrors.new" "$mirrors"
  fi
  echo "apt mirror list:"
  cat "$mirrors"
fi
