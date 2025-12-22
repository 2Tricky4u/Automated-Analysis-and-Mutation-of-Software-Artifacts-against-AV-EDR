use std::error::Error;
/// Validation example for worker.toml template
///
/// This binary validates that automation/templates/worker.toml parses correctly.
/// Used by CI to catch config errors early.
use edr_config::WorkerConfig;

fn main() {
    println!("Validating automation/templates/worker.toml...");

    match WorkerConfig::from_file("automation/templates/worker.toml") {
        Ok(config) => {
            println!("[+] Worker config is valid\n");

            println!("Configuration Summary:");
            println!("  Worker Identity:");
            println!("    - Worker ID: {}", config.worker.worker_id);
            println!("    - IP address: {}", config.worker.ip_address);
            println!("    - OS version: {}", config.worker.os_version);

            println!("\n  Controller Connection:");
            if let Some(controller) = &config.controller {
                println!("    - Address: {}", controller.controller_address);
                println!("    - TLS enabled: {}", controller.tls_enabled);
                println!("    - Connect timeout: {}s", controller.connect_timeout_secs);
            } else {
                println!("    - Not configured (standalone mode)");
            }
            println!("\n  Harness:");
            println!(
                "    - Working directory: {}",
                config.harness.working_directory
            );
            println!(
                "    - Execution timeout: {}s",
                config.harness.execution_timeout_secs
            );
            println!("    - Sandbox enabled: {}", config.harness.sandbox_enabled);

            println!("\n  Telemetry:");
            println!("    - ETW enabled: {}", config.telemetry.etw.enabled);
            println!(
                "    - Event Log enabled: {}",
                config.telemetry.eventlog.enabled
            );
            println!(
                "    - Defender enabled: {}",
                config.telemetry.defender.enabled
            );
            println!("    - RedEDR enabled: {}", config.telemetry.rededr.enabled);
            println!(
                "    - API tracing enabled: {}",
                config.telemetry.api_tracing.enabled
            );
            println!(
                "    - BB coverage enabled: {}",
                config.telemetry.bb_coverage.enabled
            );

            println!("\n  External Telemetry:");
            println!(
                "    - External enabled: {}",
                config.telemetry.external.enabled
            );
            println!(
                "    - Cortex enabled: {}",
                config.telemetry.external.cortex.enabled
            );
            println!(
                "    - MDE enabled: {}",
                config.telemetry.external.mde.enabled
            );
            println!(
                "    - Custom HTTP enabled: {}",
                config.telemetry.external.custom_http.enabled
            );

            println!("\n  Security:");
            println!("    - Disable network: {}", config.security.disable_network);
            println!("    - Block internet: {}", config.security.block_internet);
            println!(
                "    - Allow controller only: {}",
                config.security.allow_controller_only
            );
            println!(
                "    - Allowed IPs: {} entries",
                config.security.allowed_ips.len()
            );

            println!("\n[OK] All checks passed!");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[x] Worker config parse error:");
            eprintln!("  {}", e);
            eprintln!("\nPlease check automation/templates/worker.toml for syntax errors.");
            std::process::exit(1);
        }
    }
}
