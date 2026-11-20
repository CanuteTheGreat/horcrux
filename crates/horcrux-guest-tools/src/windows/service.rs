//! Windows service wrapper (`horcrux-guest-agent --service`).
//!
//! Desktop Duplication, SendInput and the clipboard only work inside the
//! user's session, while a service lives in session 0. The service therefore
//! launches this same executable (without `--service`) in the active console
//! session, with the logged-in user's token (WTSQueryUserToken) or, at the
//! login screen and when `service_login_screen` is set, with its own SYSTEM
//! token moved to that session. It relaunches the agent when it exits and
//! when the console session changes (logon, logoff, fast user switching,
//! console/RDP reconnect).
use crate::config::Config;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::*;
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::Services::*;
use windows::Win32::System::Threading::*;

/// Service name registered by the MSI.
pub const NAME: &str = "HorcruxGuestTools";

static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicIsize = AtomicIsize::new(0);
static ARGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Entry point for `--service`: hands the thread to the service control manager.
pub fn run(args: Vec<String>) {
    *ARGS.lock().unwrap_or_else(|e| e.into_inner()) = args;
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: the table is NUL-terminated and outlives the dispatcher call.
    if let Err(e) = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } {
        eprintln!(
            "horcrux-guest-agent: --service must be started by the service control manager ({e})"
        );
        std::process::exit(1);
    }
}

fn set_status(state: SERVICE_STATUS_CURRENT_STATE, accept: u32) {
    let h = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::SeqCst) as *mut c_void);
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accept,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_STOP_PENDING {
            10_000
        } else {
            0
        },
    };
    // SAFETY: h came from RegisterServiceCtrlHandlerExW; st is a valid struct.
    unsafe {
        let _ = SetServiceStatus(h, &st);
    }
}

unsafe extern "system" fn handler(
    control: u32,
    _ty: u32,
    _data: *mut c_void,
    _ctx: *mut c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true, Ordering::SeqCst);
        set_status(SERVICE_STOP_PENDING, 0);
    }
    NO_ERROR.0
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(NAME);
    // SAFETY: name is NUL-terminated; handler has the required signature.
    let h = match unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None)
    } {
        Ok(h) => h,
        Err(_) => return,
    };
    STATUS.store(h.0 as isize, Ordering::SeqCst);
    set_status(
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
    );
    supervise();
    set_status(SERVICE_STOPPED, 0);
}

struct Child {
    process: HANDLE,
    session: u32,
}

impl Child {
    fn running(&self) -> bool {
        // SAFETY: the handle is a live process handle owned by self.
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
    }
    fn kill(self) {
        // SAFETY: the handle is owned; it is closed once, here.
        unsafe {
            let _ = TerminateProcess(self.process, 0);
            let _ = WaitForSingleObject(self.process, 5000);
            let _ = CloseHandle(self.process);
        }
    }
}

fn supervise() {
    let args = ARGS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let cfg = Config::load(args.clone()).unwrap_or_default();
    let poll = Duration::from_millis(cfg.service_poll_ms);
    let mut child: Option<Child> = None;
    let mut last_fail = String::new();
    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: plain query.
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some(c) = &child {
            if !c.running() || c.session != session {
                if let Some(c) = child.take() {
                    c.kill();
                }
            }
        }
        if child.is_none() && session != 0xFFFF_FFFF {
            match launch(session, &args, cfg.service_login_screen) {
                Ok(c) => {
                    last_fail.clear();
                    child = Some(c);
                }
                Err(e) => {
                    if e != last_fail {
                        eprintln!("horcrux-guest-agent service: {e}");
                        last_fail = e;
                    }
                }
            }
        }
        std::thread::sleep(poll);
    }
    if let Some(c) = child.take() {
        c.kill();
    }
}

/// The token to run the agent with in `session`: the logged-in user's, or a
/// copy of the service's own with the session id changed.
fn session_token(session: u32, login_screen: bool) -> Result<HANDLE, String> {
    let mut tok = HANDLE::default();
    // SAFETY: tok receives a new token handle (owned by the caller).
    if unsafe { WTSQueryUserToken(session, &mut tok) }.is_ok() {
        return Ok(tok);
    }
    if !login_screen {
        return Err(format!("no user logged on in session {session}"));
    }
    let mut own = HANDLE::default();
    // SAFETY: handles are closed below; the duplicate is returned.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)
            .map_err(|e| format!("OpenProcessToken: {e}"))?;
        let mut dup = HANDLE::default();
        let r = DuplicateTokenEx(
            own,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut dup,
        );
        let _ = CloseHandle(own);
        r.map_err(|e| format!("DuplicateTokenEx: {e}"))?;
        if let Err(e) = SetTokenInformation(
            dup,
            TokenSessionId,
            &session as *const u32 as *const c_void,
            4,
        ) {
            let _ = CloseHandle(dup);
            return Err(format!("SetTokenInformation(session {session}): {e}"));
        }
        Ok(dup)
    }
}

fn launch(session: u32, args: &[String], login_screen: bool) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let token = session_token(session, login_screen)?;
    let mut cmd = format!("\"{}\"", exe.display());
    for a in args {
        cmd.push(' ');
        if a.contains(' ') {
            cmd.push_str(&format!("\"{a}\""));
        } else {
            cmd.push_str(a);
        }
    }
    let mut cmdw = wide(&cmd);
    let mut desktop = wide("winsta0\\default");
    let mut env: *mut c_void = std::ptr::null_mut();
    // SAFETY: token is valid; env is destroyed after CreateProcessAsUserW; the
    // command line buffer is mutable and NUL-terminated as the API requires.
    unsafe {
        let have_env = CreateEnvironmentBlock(&mut env, token, false).is_ok();
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let r = CreateProcessAsUserW(
            token,
            PCWSTR::null(),
            PWSTR(cmdw.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            if have_env {
                Some(env as *const c_void)
            } else {
                None
            },
            PCWSTR::null(),
            &si,
            &mut pi,
        );
        if have_env {
            let _ = DestroyEnvironmentBlock(env);
        }
        let _ = CloseHandle(token);
        r.map_err(|e| format!("CreateProcessAsUserW in session {session}: {e}"))?;
        let _ = CloseHandle(pi.hThread);
        Ok(Child {
            process: pi.hProcess,
            session,
        })
    }
}
