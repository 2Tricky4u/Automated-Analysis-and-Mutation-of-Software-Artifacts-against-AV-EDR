//! MSVC-compatible build mode: clang-cl + link.exe instead of clang + lld-link.
//!
//! When enabled, the build pipeline uses:
//! - `clang-cl` as the compiler driver (sets `_MSC_VER`, MSVC-compatible defaults)
//! - `link.exe` via `cmd.exe /c "call vcvarsall.bat x64 && link.exe ..."` for linking
//!
//! This produces PE binaries with genuine MSVC metadata (Rich header, linker version,
//! section layout) instead of LLD signatures — reducing static detection signals.
//!
//! Invoked from WSL2 via Windows interop (same `wslpath` pattern as Defender scan).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// MSVC-compatible build configuration.
///
/// When present in `BuilderConfig`, the build pipeline switches from
/// `clang + lld-link` to `clang-cl + link.exe`.
#[derive(Debug, Clone)]
pub struct MsvcCompat {
    /// WSL path to vcvarsall.bat (sets up MSVC environment).
    /// e.g. "/mnt/c/Program Files/Microsoft Visual Studio/2022/BuildTools/VC/Auxiliary/Build/vcvarsall.bat"
    pub vcvarsall_path: PathBuf,
}

impl MsvcCompat {
    /// Auto-detect vcvarsall.bat from common VS 2022 install locations.
    ///
    /// Checks (in order): BuildTools, Community, Professional, Enterprise.
    pub fn default_vcvarsall() -> PathBuf {
        let editions = ["BuildTools", "Community", "Professional", "Enterprise"];
        for edition in &editions {
            let path = PathBuf::from(format!(
                "/mnt/c/Program Files/Microsoft Visual Studio/2022/{}/VC/Auxiliary/Build/vcvarsall.bat",
                edition
            ));
            if path.exists() {
                return path;
            }
        }
        // Fallback — will fail with a clear error in invoke_msvc_link's pre-flight check
        PathBuf::from(
            "/mnt/c/Program Files/Microsoft Visual Studio/2022/BuildTools/VC/Auxiliary/Build/vcvarsall.bat",
        )
    }
}

/// Convert a WSL Linux path to an absolute Windows path via `wslpath -wa`.
///
/// Uses `-wa` (absolute + windows) to ensure the output is always an absolute
/// Windows path, even if the input is relative. This is critical because cmd.exe
/// invoked via WSL interop may have a different working directory.
///
/// Reuses the same pattern as `controller/src/dispatch/job_worker.rs` (Defender scan).
pub async fn wsl_to_win_path(wsl_path: &Path) -> Result<String> {
    let output = tokio::process::Command::new("wslpath")
        .arg("-wa")
        .arg(wsl_path.as_os_str())
        .output()
        .await
        .context("Failed to run wslpath")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "wslpath failed for {:?} (status={}): {}",
            wsl_path,
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Invoke MSVC `link.exe` via `cmd.exe` with vcvarsall environment.
///
/// Equivalent to:
/// ```text
/// cmd.exe /c "call vcvarsall.bat x64 && link.exe <args>"
/// ```
///
/// All WSL paths are converted to Windows paths via `wsl_to_win_path()`.
pub async fn invoke_msvc_link(
    vcvarsall: &Path,
    objects: &[PathBuf],
    output_exe: &Path,
    libs: &[&str],
    extra_flags: &[&str],
) -> Result<()> {
    // Pre-flight: check vcvarsall.bat exists at the WSL path
    if !vcvarsall.exists() {
        anyhow::bail!(
            "vcvarsall.bat not found at WSL path {:?}.\n\
             Install MSVC Build Tools or pass --vcvarsall with the correct path.\n\
             Expected: /mnt/c/Program Files/Microsoft Visual Studio/2022/BuildTools/VC/Auxiliary/Build/vcvarsall.bat",
            vcvarsall
        );
    }

    // Convert all paths to Windows format
    let vcvarsall_win = wsl_to_win_path(vcvarsall).await?;
    let output_win = wsl_to_win_path(output_exe).await?;

    let mut obj_wins = Vec::with_capacity(objects.len());
    for obj in objects {
        if !obj.exists() {
            anyhow::bail!("Object file not found: {:?}", obj);
        }
        obj_wins.push(wsl_to_win_path(obj).await?);
    }

    // Build link.exe argument string
    let mut link_args = Vec::new();

    // Object files
    for obj_win in &obj_wins {
        link_args.push(format!("\"{}\"", obj_win));
    }

    // Output
    link_args.push(format!("/OUT:\"{}\"", output_win));

    // Standard flags
    link_args.push("/SUBSYSTEM:CONSOLE".to_string());
    link_args.push("/MACHINE:X64".to_string());
    link_args.push("/DEBUG:NONE".to_string());
    link_args.push("/INCREMENTAL:NO".to_string());
    link_args.push("/DYNAMICBASE".to_string());
    link_args.push("/NXCOMPAT".to_string());
    link_args.push("/Brepro".to_string());

    // Extra flags (e.g. /NODEFAULTLIB:...)
    for flag in extra_flags {
        link_args.push(flag.to_string());
    }

    // NOTE: No /LIBPATH entries needed — vcvarsall.bat sets up the LIB environment
    // variable with native MSVC CRT + Windows SDK library paths. link.exe uses LIB
    // automatically. The xwin SDK paths (used by lld-link in standard mode) are on
    // the WSL filesystem and inaccessible to native link.exe via UNC paths.

    // Libraries
    for lib in libs {
        link_args.push(lib.to_string());
    }

    let link_args_str = link_args.join(" ");

    // Write a temporary .bat file to avoid WSL→Windows argument escaping issues.
    // When cmd.exe is invoked from WSL, embedded quotes in arguments get mangled
    // by the interop layer (\" instead of ""). A .bat file sidesteps this entirely.
    let bat_dir = output_exe.parent().unwrap_or(Path::new("."));
    let bat_path = bat_dir.join("_msvc_link.bat");
    let bat_content = format!(
        "@echo off\r\n\
         call \"{}\" x64\r\n\
         if errorlevel 1 (\r\n\
           echo vcvarsall.bat failed 1>&2\r\n\
           exit /b 1\r\n\
         )\r\n\
         link.exe {}\r\n",
        vcvarsall_win, link_args_str
    );

    std::fs::write(&bat_path, &bat_content)
        .context("Failed to write temporary batch file for MSVC link")?;

    let bat_win = wsl_to_win_path(&bat_path).await?;

    debug!("MSVC link batch:\n{}", bat_content);

    let output = tokio::process::Command::new("cmd.exe")
        .args(["/c", &bat_win])
        .output()
        .await
        .context("Failed to invoke cmd.exe for MSVC link.exe")?;

    // Clean up temp batch file
    let _ = std::fs::remove_file(&bat_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        anyhow::bail!(
            "MSVC link.exe failed (exit={}):\n\
             BATCH SCRIPT:\n{}\n\
             STDOUT:\n{}\n\
             STDERR:\n{}",
            output.status.code().unwrap_or(-1),
            bat_content,
            stdout,
            stderr
        );
    }

    if !stderr.is_empty() {
        debug!("MSVC link.exe stderr: {}", stderr);
    }

    // Verify output was created
    if !output_exe.exists() {
        // link.exe writes to Windows path; check via the WSL path
        warn!(
            "link.exe succeeded but output not found at WSL path {:?} (Windows: {})",
            output_exe, output_win
        );
    }

    Ok(())
}

/// Return the driver-mode argument that turns `clang` into `clang-cl`.
///
/// On Linux/WSL, `clang-cl` is typically not a separate binary — it's `clang`
/// invoked with `--driver-mode=cl`. This flag makes `clang` behave identically
/// to `clang-cl`: sets `_MSC_VER`, enables `-fms-compatibility` by default,
/// accepts MSVC-style flags (`/O2`, `/D`, `/Fo`, etc.).
pub const DRIVER_MODE_CL: &str = "--driver-mode=cl";
