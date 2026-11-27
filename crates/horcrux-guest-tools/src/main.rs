//! `horcrux-guest-agent`: the Horcrux guest tools service.
//!
//! Connects to the node over the guest-tools port (virtio-serial,
//! docs/design/horcrux-console.md section 4.1), announces what this guest
//! can do, then serves the node's requests: capture + encode the display
//! and send the encoded packets (the node forwards them to the viewer
//! without transcoding), inject input, sync the clipboard, size the
//! displays to the client and capture audio.
//!
//! Every value is a setting: command-line flags override the config file
//! (`/etc/horcrux/guest-tools.conf` on Linux,
//! `%ProgramData%\Horcrux\guest-tools.conf` on Windows; `key = value`
//! lines), which overrides the defaults in [`config::Config`].
mod agent;
mod audio;
mod config;
mod inventory;
#[cfg(target_os = "linux")]
mod linux;
mod power;
#[cfg(windows)]
mod windows;

fn main() {
    let cfg = match config::Config::load(std::env::args().skip(1).collect()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("horcrux-guest-agent: {e}");
            std::process::exit(2);
        }
    };
    if let Some(a) = cfg.action {
        std::process::exit(power::run(&cfg, a));
    }
    if cfg.inventory {
        std::process::exit(inventory::run());
    }
    if cfg.print_caps {
        let p = platform(&cfg);
        println!(
            "{}",
            serde_json::to_string_pretty(&p.hello()).unwrap_or_default()
        );
        return;
    }
    #[cfg(windows)]
    if cfg.service {
        windows::service::run(
            std::env::args()
                .skip(1)
                .filter(|a| a != "--service")
                .collect(),
        );
        return;
    }
    #[cfg(windows)]
    windows::port::configure(
        cfg.port_write_max,
        cfg.port_busy_wait_ms,
        cfg.port_busy_tries,
        cfg.port_write_timeout_ms,
    );
    #[cfg(windows)]
    windows::port::use_inherited(cfg.port_handle);
    // Same-host raw frames run beside the port connection, independent of
    // it: they serve a local viewer even while no remote session is open.
    #[cfg(target_os = "linux")]
    linux::shm::spawn(cfg.clone());
    #[cfg(windows)]
    windows::shm::spawn(cfg.clone());
    loop {
        let p = platform(&cfg);
        match agent::run(&cfg, p) {
            Ok(()) => eprintln!("horcrux-guest-agent: node closed the port"),
            Err(e) => eprintln!("horcrux-guest-agent: {e}"),
        }
        if cfg.once {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(cfg.reconnect_ms));
    }
}

/// Read and write halves of the guest-tools port.
pub type Port = (
    Box<dyn std::io::Read + Send>,
    Box<dyn std::io::Write + Send>,
);

#[cfg(unix)]
pub fn open_port(path: &str) -> Result<Port, String> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| {
            format!("{path}: {e} (is the VM's Horcrux Console guest-tools port enabled?)")
        })?;
    let r = f.try_clone().map_err(|e| e.to_string())?;
    Ok((Box::new(r), Box::new(f)))
}

#[cfg(windows)]
pub fn open_port(path: &str) -> Result<Port, String> {
    windows::open_port(path)
}

#[cfg(target_os = "linux")]
fn platform(cfg: &config::Config) -> Box<dyn agent::Platform> {
    Box::new(linux::Linux::new(cfg.clone()))
}

#[cfg(windows)]
fn platform(cfg: &config::Config) -> Box<dyn agent::Platform> {
    Box::new(windows::Windows::new(cfg.clone()))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform(_cfg: &config::Config) -> Box<dyn agent::Platform> {
    Box::new(agent::Unsupported)
}
