//! Launches a private, headless Firefox that listens for Marionette on a free port.

use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub struct Firefox {
    child: Child,
    profile: PathBuf,
    pub port: u16,
}

const PREFS: &[(&str, &str)] = &[
    ("browser.shell.checkDefaultBrowser", "false"),
    ("browser.startup.page", "0"),
    ("browser.aboutwelcome.enabled", "false"),
    ("browser.tabs.warnOnClose", "false"),
    ("dom.disable_beforeunload", "true"),
    ("datareporting.policy.dataSubmissionEnabled", "false"),
    ("datareporting.healthreport.uploadEnabled", "false"),
    ("toolkit.telemetry.enabled", "false"),
    ("app.update.auto", "false"),
    ("app.update.enabled", "false"),
    ("network.captive-portal-service.enabled", "false"),
    ("browser.safebrowsing.malware.enabled", "false"),
    ("browser.safebrowsing.phishing.enabled", "false"),
    ("media.autoplay.default", "5"),
    ("permissions.default.desktop-notification", "2"),
    ("permissions.default.geo", "2"),
    ("general.smoothScroll", "false"),
    ("ui.prefersReducedMotion", "1"),
];

fn free_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Preferences file for the throwaway profile.
fn user_js(port: u16) -> String {
    let mut s = format!("user_pref(\"marionette.port\", {port});\n");
    for (k, v) in PREFS {
        s.push_str(&format!("user_pref(\"{k}\", {v});\n"));
    }
    s
}

/// Profiles are named `webtui-<pid>-<nanos>`; one whose process is gone (killed without a
/// chance to clean up) is garbage.
fn is_stale(name: &str, alive: impl Fn(u32) -> bool) -> bool {
    name.strip_prefix("webtui-")
        .and_then(|rest| rest.split('-').next())
        .and_then(|pid| pid.parse::<u32>().ok())
        .is_some_and(|pid| !alive(pid))
}

fn remove_stale_profiles() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_stale(&name, |pid| {
            std::path::Path::new(&format!("/proc/{pid}")).exists()
        }) && e.path().is_dir()
        {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

impl Firefox {
    /// `WEBTUI_FIREFOX` overrides the executable.
    pub fn launch() -> io::Result<Firefox> {
        remove_stale_profiles();
        let bin = std::env::var("WEBTUI_FIREFOX").unwrap_or_else(|_| "firefox".into());
        let port = free_port()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let profile = std::env::temp_dir().join(format!("webtui-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&profile)?;
        fs::write(profile.join("user.js"), user_js(port))?;
        let child = Command::new(bin)
            .args(["--headless", "--marionette", "--no-remote", "-profile"])
            .arg(&profile)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(|_| {
                let _ = fs::remove_dir_all(&profile);
            })?;
        Ok(Firefox {
            child,
            profile,
            port,
        })
    }
}

impl Drop for Firefox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_pins_the_port_and_quiets_firefox() {
        let js = user_js(4242);
        assert!(js.starts_with("user_pref(\"marionette.port\", 4242);"));
        assert!(js.contains("dom.disable_beforeunload\", true"));
        assert!(js.contains("media.autoplay.default\", 5"));
        assert!(
            js.lines()
                .all(|l| l.starts_with("user_pref(") && l.ends_with(");"))
        );
    }

    #[test]
    fn only_profiles_of_dead_processes_are_stale() {
        let alive = |pid: u32| pid == 100;
        assert!(is_stale("webtui-200-5", alive));
        assert!(!is_stale("webtui-100-5", alive));
        assert!(!is_stale("other-200-5", alive));
        assert!(!is_stale("webtui-x-5", alive));
        assert!(!is_stale("webtui-", alive));
    }

    #[test]
    fn a_missing_binary_is_an_error_and_leaves_no_profile() {
        let before = fs::read_dir(std::env::temp_dir()).unwrap().count();
        // SAFETY: tests in this module do not run concurrently with other users of this variable.
        unsafe { std::env::set_var("WEBTUI_FIREFOX", "/no/such/firefox") };
        let r = Firefox::launch();
        unsafe { std::env::remove_var("WEBTUI_FIREFOX") };
        assert!(r.is_err());
        let after = fs::read_dir(std::env::temp_dir()).unwrap().count();
        assert!(after <= before + 1, "profile directory left behind");
    }
}
