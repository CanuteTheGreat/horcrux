# image-name.sh -- sourced by build.sh, ci-run.sh, publish.sh, publish-github.sh (one place defines the image naming scheme).
#
#   <product>-<desktop-env | server>-<init>-<channel>-<arch>.iso
#   horcrux-server-openrc-daily-x86_64.iso   horcrux-gnome-light-openrc-daily-x86_64.iso   patronus-server-openrc-daily-aarch64.iso
#   later: horcrux-xfce-openrc-..., horcrux-kde-openrc-..., any of them -systemd- if ever built.
#
# IN : VARIANT   comma list of image variants (project/variants/<name>): a desktop-environment variant (gnome-light, xfce, kde, ...)
#                and/or a hardware variant (t2). The old id `desktop` is an alias of `gnome-light`. empty / `server` = server image.
#      INIT      openrc (default) | systemd
# OUT: VARIANT   canonical (alias resolved)
#      IMG_DE    server | gnome-light | gnome | xfce | kde
#      IMG_INIT  openrc | systemd
#      IMG_HW    hardware variant tokens, each with a trailing dash ("" | "t2-"); appear in private/local ISO names only
#      IMG       <DE>-<init>                 (public asset names:  <product>-$IMG-<channel>-<arch>.iso)
#      IMG_OLD_VPFX  "" | "desktop-": the pre-scheme asset prefix (to delete the old asset after the new one is up)
#      IMG_ID    <DE>-<init>-<arch>          (status marker suffix, workflow `only=` value; ARCH must be set)
IMG_KNOWN_DES="gnome-light gnome xfce kde"
IMG_KNOWN_INITS="openrc systemd"
IMG_DE=server; IMG_HW=""; _v=""
for _t in $(echo "${VARIANT:-}" | tr ',' ' '); do
  [ "$_t" = desktop ] && _t=gnome-light            # compatibility alias (old variant id)
  [ "$_t" = server ] && continue
  case " $IMG_KNOWN_DES " in *" $_t "*) IMG_DE=$_t;; *) IMG_HW="$IMG_HW$_t-";; esac
  _v="$_v${_v:+,}$_t"
done
VARIANT=$_v; export VARIANT
IMG_INIT=${INIT:-openrc}
case " $IMG_KNOWN_INITS " in *" $IMG_INIT "*) ;; *) echo "image-name.sh: unknown INIT=$IMG_INIT (openrc|systemd)" >&2; return 2 2>/dev/null || exit 2;; esac
IMG=$IMG_DE-$IMG_INIT
IMG_ID=$IMG-${ARCH:-x86_64}
IMG_OLD_VPFX=""; [ "$IMG_DE" = gnome-light ] && IMG_OLD_VPFX=desktop-      # asset prefix of the pre-scheme names (<p>-desktop-daily-<arch>.iso)
unset _t _v
