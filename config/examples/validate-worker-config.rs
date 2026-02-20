/// Validation example for worker.toml template
///
/// This binary validates that automation/templates/worker.toml parses correctly.
/// Used by CI to catch config errors early.
use automutate_config::WorkerConfig;

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
