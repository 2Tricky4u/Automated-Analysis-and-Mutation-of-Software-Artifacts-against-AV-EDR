//! Catalog of benign Windows API call sequences for N-gram dilution.
//!
//! EDR behavioral engines detect malicious API call sequences (e.g.,
//! `VirtualAlloc → memcpy → VirtualProtect`). This module provides a catalog
//! of real, benign Windows API calls that can be inserted between existing
//! statements to dilute those N-gram signatures.
//!
//! Each behavior entry has dependencies (e.g., `ReadFile` depends on
//! `CreateFileA`), and the [`BehaviorGraph`] ensures correct topological
//! ordering when selecting which calls to insert.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

/// A single benign API call entry in the catalog.
#[derive(Debug, Clone)]
pub struct BehaviorEntry {
    pub id: u32,
    pub group: BehaviorGroup,
    /// IDs that must execute before this entry.
    pub deps: Vec<u32>,
    /// C declarations needed at function scope (e.g., variable declarations).
    pub declarations: Vec<&'static str>,
    /// C statement(s) to insert.
    pub code: &'static str,
}

/// Groups of related benign behaviors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BehaviorGroup {
    /// GetTickCount, GetEnvironmentVariable, GetComputerName
    SystemQuery,
    /// CreateFileA → ReadFile → CloseHandle
    FileIo,
    /// RegOpenKeyExA → RegQueryValueExA → RegCloseKey
    RegistryIo,
}

impl FromStr for BehaviorGroup {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system_query" => Ok(BehaviorGroup::SystemQuery),
            "file_io" => Ok(BehaviorGroup::FileIo),
            "registry_io" => Ok(BehaviorGroup::RegistryIo),
            _ => Err(()),
        }
    }
}

/// Build the default behavior catalog.
///
/// IDs are namespaced by group:
/// - SystemQuery: 0-9
/// - FileIo: 10-19
/// - RegistryIo: 20-29
pub fn default_catalog() -> Vec<BehaviorEntry> {
    vec![
        // ── SystemQuery (IDs 0-2, no deps, all repeatable) ──────────────
        BehaviorEntry {
            id: 0,
            group: BehaviorGroup::SystemQuery,
            deps: vec![],
            declarations: vec!["char __be_env_buf[256];"],
            code: "GetEnvironmentVariableA(\"COMPUTERNAME\", __be_env_buf, sizeof(__be_env_buf));",
        },
        BehaviorEntry {
            id: 1,
            group: BehaviorGroup::SystemQuery,
            deps: vec![],
            declarations: vec![
                "char __be_comp_name[256];",
                "DWORD __be_comp_size = sizeof(__be_comp_name);",
            ],
            code: "GetComputerNameA(__be_comp_name, &__be_comp_size);",
        },
        BehaviorEntry {
            id: 2,
            group: BehaviorGroup::SystemQuery,
            deps: vec![],
            declarations: vec!["volatile DWORD __be_tick;"],
            code: "__be_tick = GetTickCount();",
        },
        // ── FileIo (IDs 10-12, chained deps) ───────────────────────────
        BehaviorEntry {
            id: 10,
            group: BehaviorGroup::FileIo,
            deps: vec![],
            declarations: vec!["HANDLE __be_hFile = INVALID_HANDLE_VALUE;"],
            code: "__be_hFile = CreateFileA(\"C:\\\\Windows\\\\System32\\\\ntdll.dll\", GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);",
        },
        BehaviorEntry {
            id: 11,
            group: BehaviorGroup::FileIo,
            deps: vec![10],
            declarations: vec!["char __be_read_buf[64];", "DWORD __be_bytes_read = 0;"],
            code: "if (__be_hFile != INVALID_HANDLE_VALUE) { ReadFile(__be_hFile, __be_read_buf, sizeof(__be_read_buf), &__be_bytes_read, NULL); }",
        },
        BehaviorEntry {
            id: 12,
            group: BehaviorGroup::FileIo,
            deps: vec![11],
            declarations: vec![],
            code: "if (__be_hFile != INVALID_HANDLE_VALUE) { CloseHandle(__be_hFile); }",
        },
        // ── RegistryIo (IDs 20-22, chained deps) ───────────────────────
        BehaviorEntry {
            id: 20,
            group: BehaviorGroup::RegistryIo,
            deps: vec![],
            declarations: vec!["HKEY __be_hKey = NULL;", "LONG __be_reg_status;"],
            code: "__be_reg_status = RegOpenKeyExA(HKEY_LOCAL_MACHINE, \"SOFTWARE\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\", 0, KEY_READ, &__be_hKey);",
        },
        BehaviorEntry {
            id: 21,
            group: BehaviorGroup::RegistryIo,
            deps: vec![20],
            declarations: vec![
                "char __be_reg_val[128];",
                "DWORD __be_reg_val_size = sizeof(__be_reg_val);",
            ],
            code: "if (__be_reg_status == ERROR_SUCCESS) { RegQueryValueExA(__be_hKey, \"ProductName\", NULL, NULL, (LPBYTE)__be_reg_val, &__be_reg_val_size); }",
        },
        BehaviorEntry {
            id: 22,
            group: BehaviorGroup::RegistryIo,
            deps: vec![21],
            declarations: vec![],
            code: "if (__be_reg_status == ERROR_SUCCESS) { RegCloseKey(__be_hKey); }",
        },
    ]
}

/// Dependency-aware behavior graph for topological ordering.
///
/// Maintains a frontier of nodes whose dependencies are satisfied.
/// `pop()` selects a random frontier node using a seed-controlled PRNG,
/// consumes it, and unlocks its dependents.
pub struct BehaviorGraph {
    entries: HashMap<u32, BehaviorEntry>,
    /// For each entry, the set of unsatisfied parent IDs.
    remaining_deps: HashMap<u32, HashSet<u32>>,
    /// Reverse mapping: parent_id → set of child IDs that depend on it.
    children: HashMap<u32, Vec<u32>>,
    /// Nodes with all deps satisfied, ready to be consumed.
    frontier: Vec<u32>,
    /// Already-consumed node IDs.
    consumed: HashSet<u32>,
    /// PRNG state (xorshift64).
    rng_state: u64,
}

impl BehaviorGraph {
    /// Build a new graph from the given entries.
    ///
    /// Only includes entries whose groups are in `allowed_groups`.
    pub fn new(catalog: &[BehaviorEntry], allowed_groups: &[BehaviorGroup], seed: u64) -> Self {
        let entries: HashMap<u32, BehaviorEntry> = catalog
            .iter()
            .filter(|e| allowed_groups.contains(&e.group))
            .cloned()
            .map(|e| (e.id, e))
            .collect();

        let valid_ids: HashSet<u32> = entries.keys().copied().collect();

        let mut remaining_deps: HashMap<u32, HashSet<u32>> = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut frontier = Vec::new();

        for (id, entry) in &entries {
            // Only count deps that are actually in the filtered set
            let valid_deps: HashSet<u32> = entry
                .deps
                .iter()
                .filter(|d| valid_ids.contains(d))
                .copied()
                .collect();

            if valid_deps.is_empty() {
                frontier.push(*id);
            }

            for &dep in &valid_deps {
                children.entry(dep).or_default().push(*id);
            }

            remaining_deps.insert(*id, valid_deps);
        }

        // Sort frontier for determinism before PRNG selection
        frontier.sort();

        BehaviorGraph {
            entries,
            remaining_deps,
            children,
            frontier,
            consumed: HashSet::new(),
            rng_state: if seed == 0 { 1 } else { seed },
        }
    }

    /// Pop a random entry from the frontier (seed-controlled).
    ///
    /// Returns `None` when no more entries are available.
    pub fn pop(&mut self) -> Option<BehaviorEntry> {
        if self.frontier.is_empty() {
            return None;
        }

        // xorshift64 PRNG
        let idx = self.xorshift64() as usize % self.frontier.len();
        let id = self.frontier.remove(idx);

        self.consumed.insert(id);

        // Unlock children whose deps are now fully satisfied
        if let Some(child_ids) = self.children.get(&id).cloned() {
            for child_id in child_ids {
                if let Some(deps) = self.remaining_deps.get_mut(&child_id) {
                    deps.remove(&id);
                    if deps.is_empty() && !self.consumed.contains(&child_id) {
                        // Insert in sorted position for determinism
                        let pos = self.frontier.binary_search(&child_id).unwrap_or_else(|p| p);
                        self.frontier.insert(pos, child_id);
                    }
                }
            }
        }

        self.entries.get(&id).cloned()
    }

    /// Number of entries remaining (frontier + blocked).
    pub fn remaining(&self) -> usize {
        self.entries.len() - self.consumed.len()
    }

    fn xorshift64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }
}

/// Generate benign C code insertions.
///
/// Returns `(declarations, statements)` — deduplicated declarations to place
/// at the top of the function body, and ordered statements to interleave
/// between existing code.
///
/// # Arguments
///
/// * `groups` — Which [`BehaviorGroup`]s to draw from (empty = no output)
/// * `count` — Maximum number of statements to generate (may return fewer
///   if the catalog is exhausted)
/// * `seed` — PRNG seed for deterministic ordering (0 is mapped to 1)
pub fn generate_insertion(
    groups: &[BehaviorGroup],
    count: usize,
    seed: u64,
) -> (Vec<String>, Vec<String>) {
    let catalog = default_catalog();
    let mut graph = BehaviorGraph::new(&catalog, groups, seed);

    let mut declarations: Vec<String> = Vec::new();
    let mut statements: Vec<String> = Vec::new();
    let mut seen_decls: HashSet<String> = HashSet::new();

    let mut popped = 0;
    while popped < count {
        match graph.pop() {
            Some(entry) => {
                for decl in &entry.declarations {
                    if seen_decls.insert(decl.to_string()) {
                        declarations.push(decl.to_string());
                    }
                }
                statements.push(entry.code.to_string());
                popped += 1;
            }
            None => break,
        }
    }

    (declarations, statements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_catalog_valid() {
        let catalog = default_catalog();
        let ids: HashSet<u32> = catalog.iter().map(|e| e.id).collect();

        // All deps reference valid IDs
        for entry in &catalog {
            for dep in &entry.deps {
                assert!(
                    ids.contains(dep),
                    "Entry {} has invalid dep {}",
                    entry.id,
                    dep
                );
            }
        }

        // No self-dependencies
        for entry in &catalog {
            assert!(
                !entry.deps.contains(&entry.id),
                "Entry {} has self-dependency",
                entry.id
            );
        }
    }

    #[test]
    fn test_no_dependency_cycles() {
        let catalog = default_catalog();
        let groups = vec![
            BehaviorGroup::SystemQuery,
            BehaviorGroup::FileIo,
            BehaviorGroup::RegistryIo,
        ];
        let mut graph = BehaviorGraph::new(&catalog, &groups, 42);

        // If we can drain the entire graph, there are no cycles
        let total = graph.remaining();
        let mut popped = 0;
        while graph.pop().is_some() {
            popped += 1;
        }
        assert_eq!(
            popped, total,
            "Should drain all {} entries (cycles would block)",
            total
        );
    }

    #[test]
    fn test_dependency_ordering_file_io() {
        let catalog = default_catalog();
        let groups = vec![BehaviorGroup::FileIo];
        let mut graph = BehaviorGraph::new(&catalog, &groups, 42);

        let mut order = Vec::new();
        while let Some(entry) = graph.pop() {
            order.push(entry.id);
        }

        // CreateFile (10) must come before ReadFile (11), which must come before CloseHandle (12)
        let pos_10 = order.iter().position(|&id| id == 10).unwrap();
        let pos_11 = order.iter().position(|&id| id == 11).unwrap();
        let pos_12 = order.iter().position(|&id| id == 12).unwrap();
        assert!(pos_10 < pos_11, "CreateFile must come before ReadFile");
        assert!(pos_11 < pos_12, "ReadFile must come before CloseHandle");
    }

    #[test]
    fn test_dependency_ordering_registry_io() {
        let catalog = default_catalog();
        let groups = vec![BehaviorGroup::RegistryIo];
        let mut graph = BehaviorGraph::new(&catalog, &groups, 42);

        let mut order = Vec::new();
        while let Some(entry) = graph.pop() {
            order.push(entry.id);
        }

        let pos_20 = order.iter().position(|&id| id == 20).unwrap();
        let pos_21 = order.iter().position(|&id| id == 21).unwrap();
        let pos_22 = order.iter().position(|&id| id == 22).unwrap();
        assert!(pos_20 < pos_21, "RegOpenKey must come before RegQueryValue");
        assert!(
            pos_21 < pos_22,
            "RegQueryValue must come before RegCloseKey"
        );
    }

    #[test]
    fn test_deterministic_with_seed() {
        let groups = vec![
            BehaviorGroup::SystemQuery,
            BehaviorGroup::FileIo,
            BehaviorGroup::RegistryIo,
        ];

        let (decls1, stmts1) = generate_insertion(&groups, 9, 0xBE41);
        let (decls2, stmts2) = generate_insertion(&groups, 9, 0xBE41);

        assert_eq!(
            decls1, decls2,
            "Same seed should produce identical declarations"
        );
        assert_eq!(
            stmts1, stmts2,
            "Same seed should produce identical statements"
        );
    }

    #[test]
    fn test_different_seeds_differ() {
        let groups = vec![
            BehaviorGroup::SystemQuery,
            BehaviorGroup::FileIo,
            BehaviorGroup::RegistryIo,
        ];

        let (_, stmts1) = generate_insertion(&groups, 9, 0xBE41);
        let (_, stmts2) = generate_insertion(&groups, 9, 0xDEAD);

        // Different seeds should (very likely) produce different orderings
        // This is a probabilistic test but with 9 entries the chance of
        // identical ordering is extremely low
        assert_ne!(
            stmts1, stmts2,
            "Different seeds should produce different orderings"
        );
    }

    #[test]
    fn test_count_limits_output() {
        let groups = vec![
            BehaviorGroup::SystemQuery,
            BehaviorGroup::FileIo,
            BehaviorGroup::RegistryIo,
        ];

        let (_, stmts) = generate_insertion(&groups, 3, 42);
        assert_eq!(stmts.len(), 3, "Should return exactly count=3 statements");
    }

    #[test]
    fn test_count_exceeds_catalog() {
        let groups = vec![BehaviorGroup::SystemQuery];
        let catalog = default_catalog();
        let available = catalog
            .iter()
            .filter(|e| e.group == BehaviorGroup::SystemQuery)
            .count();

        let (_, stmts) = generate_insertion(&groups, 100, 42);
        assert_eq!(
            stmts.len(),
            available,
            "Should return all available entries when count exceeds catalog"
        );
    }

    #[test]
    fn test_group_filtering() {
        let groups = vec![BehaviorGroup::FileIo];
        let (_, stmts) = generate_insertion(&groups, 10, 42);

        // Should only have FileIo entries (3 total)
        assert_eq!(stmts.len(), 3);

        // All statements should relate to file operations
        let combined = stmts.join(" ");
        assert!(combined.contains("CreateFileA"));
        assert!(combined.contains("ReadFile"));
        assert!(combined.contains("CloseHandle"));
    }

    #[test]
    fn test_declarations_deduplicated() {
        let groups = vec![BehaviorGroup::SystemQuery];
        let (decls, _) = generate_insertion(&groups, 3, 42);

        // Check no duplicate declarations
        let unique: HashSet<&String> = decls.iter().collect();
        assert_eq!(
            decls.len(),
            unique.len(),
            "Declarations should be deduplicated"
        );
    }

    #[test]
    fn test_generate_insertion_all_groups() {
        let groups = vec![
            BehaviorGroup::SystemQuery,
            BehaviorGroup::FileIo,
            BehaviorGroup::RegistryIo,
        ];
        let (decls, stmts) = generate_insertion(&groups, 20, 0xBE41);

        // We have 9 entries total in the default catalog
        assert_eq!(stmts.len(), 9);
        assert!(!decls.is_empty());

        // All __be_ prefixed
        for decl in &decls {
            assert!(
                decl.contains("__be_"),
                "Declaration should use __be_ prefix: {}",
                decl
            );
        }
    }

    // ── Edge case tests ───────────────────────────────────────────────────

    #[test]
    fn test_count_one_file_io_gets_root_only() {
        let groups = vec![BehaviorGroup::FileIo];
        let (_, stmts) = generate_insertion(&groups, 1, 42);

        // count=1 → only the root node (CreateFileA, id 10), not orphaned ReadFile
        assert_eq!(stmts.len(), 1);
        assert!(
            stmts[0].contains("CreateFileA"),
            "Single FileIo should be CreateFileA (root), got: {}",
            stmts[0]
        );
    }

    #[test]
    fn test_count_two_file_io_chain() {
        let groups = vec![BehaviorGroup::FileIo];
        let (_, stmts) = generate_insertion(&groups, 2, 42);

        // count=2 → CreateFileA + ReadFile, not CloseHandle
        assert_eq!(stmts.len(), 2);
        let combined = stmts.join(" ");
        assert!(combined.contains("CreateFileA"), "Should have CreateFileA");
        assert!(combined.contains("ReadFile"), "Should have ReadFile");
        assert!(
            !combined.contains("CloseHandle"),
            "Should NOT have CloseHandle with count=2"
        );
    }

    #[test]
    fn test_multi_group_no_cross_contamination() {
        let groups = vec![BehaviorGroup::FileIo, BehaviorGroup::RegistryIo];
        let (_, stmts) = generate_insertion(&groups, 20, 42);

        // All 6 entries (3 FileIo + 3 RegistryIo) should be present
        assert_eq!(stmts.len(), 6);

        // FileIo chain deps satisfied: CreateFileA before ReadFile before CloseHandle
        let combined = stmts.join("\n");
        let create_pos = combined.find("CreateFileA").unwrap();
        let read_pos = combined.find("ReadFile").unwrap();
        let close_pos = combined
            .find("CloseHandle(__be_hFile)")
            .unwrap_or(combined.find("CloseHandle").unwrap());
        assert!(create_pos < read_pos, "CreateFileA before ReadFile");
        assert!(read_pos < close_pos, "ReadFile before CloseHandle");

        // RegistryIo chain deps satisfied: RegOpenKey before RegQueryValue before RegCloseKey
        let reg_open_pos = combined.find("RegOpenKeyExA").unwrap();
        let reg_query_pos = combined.find("RegQueryValueExA").unwrap();
        let reg_close_pos = combined.find("RegCloseKey").unwrap();
        assert!(
            reg_open_pos < reg_query_pos,
            "RegOpenKey before RegQueryValue"
        );
        assert!(
            reg_query_pos < reg_close_pos,
            "RegQueryValue before RegCloseKey"
        );
    }

    #[test]
    fn test_empty_groups_returns_nothing() {
        let groups: Vec<BehaviorGroup> = vec![];
        let (decls, stmts) = generate_insertion(&groups, 5, 42);

        assert!(
            decls.is_empty(),
            "Empty groups should produce no declarations"
        );
        assert!(
            stmts.is_empty(),
            "Empty groups should produce no statements"
        );
    }

    #[test]
    fn test_count_zero_returns_nothing() {
        let groups = vec![BehaviorGroup::SystemQuery, BehaviorGroup::FileIo];
        let (decls, stmts) = generate_insertion(&groups, 0, 42);

        assert!(decls.is_empty(), "count=0 should produce no declarations");
        assert!(stmts.is_empty(), "count=0 should produce no statements");
    }
}
