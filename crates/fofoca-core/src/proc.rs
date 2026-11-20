//! Child processes that can't outlive fo-foca (spec 013).

use std::process::Command;

/// Make the kernel kill this child (SIGKILL) when its parent goes away — crash, `kill`, a closed
/// dev terminal — so a model server never keeps holding VRAM and `pw-record` never keeps the mic.
///
/// Linux delivers the signal when the **thread** that spawned the child exits, so spawn from a
/// thread that outlives the child (the session worker, Tauri's main thread, a command thread that
/// stops its server before returning).
pub fn die_with_parent(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: only async-signal-safe calls (prctl, getppid, _exit) between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // the parent may have died before prctl took effect
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit(") ").next()?.chars().next()
    }

    #[test]
    fn child_dies_when_its_spawning_thread_ends() {
        let pid = std::thread::spawn(|| {
            let mut c = Command::new("sleep");
            c.arg("30");
            die_with_parent(&mut c);
            c.spawn().unwrap().id()
        })
        .join()
        .unwrap();
        let mut gone = false;
        for _ in 0..50 {
            // killed: a zombie until reaped (we never wait), or already gone
            if matches!(state(pid), None | Some('Z')) {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone, "sleep {pid} still running: {:?}", state(pid));
        unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) };
    }
}
