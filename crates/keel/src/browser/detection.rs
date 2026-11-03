//! Installation and listener identity checks never launch Oya or trust an MCP server's display name.
#[cfg(any(target_os = "macos", windows))]
use camino::Utf8Path;
#[cfg(target_os = "macos")]
use camino::Utf8PathBuf;
#[cfg(any(target_os = "macos", windows))]
use keel_workspace::Real;
use serde::Serialize;
#[cfg(any(target_os = "macos", windows))]
use std::{process::Command, time::Duration};
#[cfg(any(target_os = "macos", windows))]
const DETECT_WAIT: Duration = Duration::from_secs(3);
#[cfg(any(target_os = "macos", windows))]
const OUTPUT_BYTES: usize = 64 * 1024;
#[derive(Clone, Serialize)]
pub(super) struct Installation {
    pub application: String,
    pub executable: String,
}
#[cfg(any(target_os = "macos", windows))]
fn output(mut command: Command) -> Result<String, String> {
    command.stdin(std::process::Stdio::null());
    let output = crate::git::output_within(command, DETECT_WAIT)
        .map_err(|_| "Couldn’t inspect the local Oya Browser installation.".to_string())?;
    if !output.status.success() {
        return Err("Local Oya metadata is unavailable.".into());
    }
    if output.stdout.len() > OUTPUT_BYTES {
        return Err("Oya installation metadata exceeds its limit.".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "Oya installation metadata is not text.".into())
}
#[cfg(target_os = "macos")]
pub(super) fn installed() -> Result<Option<Installation>, String> {
    let mut candidates = vec![Utf8PathBuf::from("/Applications/Oya Browser.app")];
    if let Some(home) = keel_workspace::home() {
        candidates.push(home.join("Applications/Oya Browser.app"));
    }
    let mut query = Command::new("/usr/bin/mdfind");
    query.arg("kMDItemCFBundleIdentifier == 'ai.oya.browser'");
    if let Ok(found) = output(query) {
        candidates.extend(found.lines().take(16).map(Utf8PathBuf::from));
    }
    for application in candidates {
        if !application.is_dir() {
            continue;
        }
        let plist = application.join("Contents/Info.plist");
        let read = |key: &str| {
            let mut command = Command::new("/usr/libexec/PlistBuddy");
            command.args(["-c", &format!("Print :{key}"), plist.as_str()]);
            output(command)
        };
        if read("CFBundleIdentifier")?.trim() != "ai.oya.browser" {
            continue;
        }
        let name = read("CFBundleExecutable")?;
        let name = name.trim();
        if name.is_empty() || name.contains(['/', '\\']) {
            continue;
        }
        let executable = application
            .join("Contents/MacOS")
            .join(name)
            .real()
            .map_err(|_| "The selected Oya executable is unavailable.")?;
        return Ok(Some(Installation {
            application: application.real().map_err(|e| e.to_string())?.to_string(),
            executable: executable.to_string(),
        }));
    }
    Ok(None)
}
#[cfg(windows)]
pub(super) fn installed() -> Result<Option<Installation>, String> {
    // Constant PowerShell code inspects NSIS uninstall registrations, including custom install paths.
    let script = r#"$ErrorActionPreference='Stop'; $roots=@('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'); $items=Get-ItemProperty $roots -ErrorAction SilentlyContinue | Where-Object {$_.DisplayName -eq 'Oya Browser'}; foreach($i in $items){$p=Join-Path $i.InstallLocation 'Oya Browser.exe';if(Test-Path -LiteralPath $p){$f=Get-Item -LiteralPath $p;if($f.VersionInfo.ProductName -eq 'Oya Browser'){[pscustomobject]@{application=$i.InstallLocation;executable=$f.FullName}|ConvertTo-Json -Compress;break}}}"#;
    let text = powershell(script)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|_| "Oya installer metadata is malformed.")?;
    let executable = Utf8Path::new(
        value["executable"]
            .as_str()
            .ok_or("Missing Oya executable identity")?,
    )
    .real()
    .map_err(|e| e.to_string())?;
    Ok(Some(Installation {
        application: value["application"].as_str().unwrap_or_default().into(),
        executable: executable.to_string(),
    }))
}
#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    output(command)
}
#[cfg(not(any(target_os = "macos", windows)))]
pub(super) fn installed() -> Result<Option<Installation>, String> {
    Err("Local Oya installation detection is unavailable on this platform.".into())
}

pub(super) fn listener(port: u16, installation: &Installation) -> Result<u32, String> {
    #[cfg(target_os = "macos")]
    let (pid, executable) = {
        let mut command = Command::new("/usr/sbin/lsof");
        command.args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"]);
        let text = output(command)?;
        let pid = text
            .lines()
            .filter_map(|s| s.strip_prefix('p')?.parse::<u32>().ok())
            .next()
            .ok_or("Oya Browser isn’t running or its agent connection is stopped.")?;
        let mut command = Command::new("/bin/ps");
        command.args(["-p", &pid.to_string(), "-o", "comm="]);
        (pid, output(command)?.trim().to_string())
    };
    #[cfg(windows)]
    let (pid, executable) = {
        let text = powershell(&format!(
            "$ErrorActionPreference='Stop';$p=(Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort {port} -State Listen|Select-Object -First 1).OwningProcess;$x=Get-Process -Id $p;[pscustomobject]@{{pid=$x.Id;path=$x.Path}}|ConvertTo-Json -Compress"
        ))?;
        let value: serde_json::Value = serde_json::from_str(text.trim())
            .map_err(|_| "Couldn’t identify Oya’s listening process.")?;
        (
            value["pid"]
                .as_u64()
                .ok_or("Missing Oya listener process")? as u32,
            value["path"]
                .as_str()
                .ok_or("Missing Oya process identity")?
                .to_string(),
        )
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (port, installation);
    #[cfg(not(any(target_os = "macos", windows)))]
    return Err("Oya listener verification is unavailable on this platform.".into());
    #[cfg(any(target_os = "macos", windows))]
    {
        let actual = Utf8Path::new(&executable)
            .real()
            .map_err(|_| "Couldn’t resolve Oya’s listening process.")?;
        let expected = Utf8Path::new(&installation.executable)
            .real()
            .map_err(|_| "The selected Oya Browser installation is unavailable.")?;
        if actual != expected {
            return Err("The configured browser connection belongs to another process; no credentials were sent.".into());
        }
        Ok(pid)
    }
}
