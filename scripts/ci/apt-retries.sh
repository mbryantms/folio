#!/usr/bin/env bash
# Let apt retry a failed download instead of failing the job on the first
# miss.
#
# GitHub-hosted runners sometimes cannot resolve the Ubuntu mirrors for a
# short while ("Temporary failure resolving 'security.ubuntu.com'"). On
# 2026-11-11 that failed all four test shards of one pull request at once
# (installing xmllint) and the smoke job of another (Playwright's
# `install --with-deps`, which runs apt itself), before a single test ran.
#
# `Acquire::Retries` makes apt try each file again, with its built-in
# back-off between attempts. Writing it to apt.conf.d covers every later
# apt call in the job, including the ones other tools make.
set -euo pipefail
echo 'Acquire::Retries "5";' | sudo tee /etc/apt/apt.conf.d/80-retries >/dev/null
