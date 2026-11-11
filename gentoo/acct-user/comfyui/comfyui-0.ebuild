# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2
EAPI=8
inherit acct-user
DESCRIPTION="ComfyUI service user"
ACCT_USER_ID=-1
ACCT_USER_GROUPS=( comfyui video render )
ACCT_USER_HOME=/var/lib/comfyui
ACCT_USER_HOME_OWNER=comfyui:comfyui
ACCT_USER_HOME_PERMS=0750
acct-user_add_deps
