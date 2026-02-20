/// Validation example for controller.toml template
///
/// This binary validates that automation/templates/controller.toml parses correctly.
/// Used by CI to catch config errors early.
use automutate_config::ControllerConfig;

fn main() {
    println!("Validating automation/templates/controller.toml...");

    match ControllerConfig::from_file("automation/templates/controller.toml") {
        Ok(config) => {
            println!("[+] Controller config is valid\n");

            println!("Configuration Summary:");
            println!("  Server:");
            println!("    - Bind address: {}", config.server.bind_address);

            println!("\n  Elasticsearch:");
            println!("    - URL: {}", config.elasticsearch.url);
            //println!("    - RedEDR index: {}", config.elasticsearch.rededr_index);

            //println!("\n  Triage:");
            //println!("    - Model type: {}", config.triage.model_type);
            //println!(
            //    "    - Confidence threshold: {}",
            //    config.triage.confidence_threshold
            //);
            //
            //println!("\n  Mutator:");
            //println!(
            //    "    - Max mutations per artifact: {}",
            //    config.mutator.max_mutations_per_artifact
            //);
            //println!(
            //    "    - Selector weights: {:?}",
            //    config.mutator.selector_weights
            //);
            //
            //println!("\n  Telemetry:");
            //println!("    - RedEDR enabled: {}", config.telemetry.rededr_enabled);
            //println!(
            //    "    - API tracing enabled: {}",
            //    config.telemetry.api_tracing_enabled
            //);
            //println!(
            //    "    - BB coverage enabled: {}",
            //    config.telemetry.bb_coverage_enabled
            //);

            println!("\n[OK] All checks passed!");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[x] Controller config parse error:");
            eprintln!("  {}", e);
            eprintln!("\nPlease check automation/templates/controller.toml for syntax errors.");
            std::process::exit(1);
        }
    }
}
