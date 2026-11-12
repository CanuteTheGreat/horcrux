# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2
EAPI=8
# Members may administer Horcrux in the web UI/API (they sign in with their system account through PAM; wheel works too).
inherit acct-group
ACCT_GROUP_ID=-1
