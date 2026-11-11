import tomllib

DEFAULTS = {
    "thermal": {"default_mode": "balanced", "game_mode": "balanced-performance"},
    "fans": {"curve_enabled": False, "max_boost": 100},
    "lighting": {"profile": "rainbow", "static_color": "0000FF", "zones": ["shelf", "alienhead"]},
    "safety": {"enable_bios_writes": False},
}


def load(path="/etc/awccd.toml"):
    cfg = {k: dict(v) for k, v in DEFAULTS.items()}
    try:
        with open(path, "rb") as f:
            user = tomllib.load(f)
    except FileNotFoundError:
        return cfg
    for sect, vals in user.items():
        if isinstance(vals, dict) and sect in cfg:
            cfg[sect].update(vals)
    return cfg
