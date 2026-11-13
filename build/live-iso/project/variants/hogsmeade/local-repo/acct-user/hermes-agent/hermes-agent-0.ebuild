# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the Hermes agent's own login on the owner's machine (see app-misc/hogsmeade-agent-account). Password locked (acct-user), ssh only
# with a certificate of the owner's personal user CA that carries the principal "hermes-agent" (TrustedUserCAKeys of net-misc/ctg-ssh-ca-fleet,
# principal = login name). Not the owner's account, so an agent ssh session never touches his user's OpenRC user services or his GNOME session.
EAPI=8
inherit acct-user

DESCRIPTION="HOGSMEADE: the Hermes agent account (ssh via the personal SSH CA, principal hermes-agent)"
KEYWORDS="amd64"
ACCT_USER_ID=-1
ACCT_USER_HOME=/var/lib/hermes-agent
ACCT_USER_HOME_PERMS=0750
ACCT_USER_SHELL=/bin/bash
# adm: read the system logs without sudo
ACCT_USER_GROUPS=( hermes-agent adm )

acct-user_add_deps
