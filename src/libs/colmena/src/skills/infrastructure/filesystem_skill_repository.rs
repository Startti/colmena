use crate::skills::domain::{
    Skill, SkillCatalogEntry, SkillError, SkillReference, SkillReferenceMeta, SkillRepository,
    SkillSource,
};
use crate::skills::infrastructure::frontmatter_parser::parse_skill_md;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Maximum size for SKILL.md and each reference file.
pub const MAX_FILE_SIZE_BYTES: u64 = 64 * 1024;

/// Skills loaded from filesystem paths. Each path is validated against
/// an allowed-directories whitelist at construction time.
#[derive(Debug)]
pub struct FilesystemSkillRepository {
    /// Map from skill name (from frontmatter) to canonical skill directory.
    skills: HashMap<String, SkillEntry>,
}

#[derive(Debug)]
struct SkillEntry {
    canonical_dir: PathBuf,
    description: String,
    references: Vec<SkillReferenceMeta>,
    node_type: Option<String>,
}

impl FilesystemSkillRepository {
    /// Build from a list of user-supplied paths plus the directory of the graph JSON.
    ///
    /// Each path is auto-detected as one of:
    /// - **single skill directory** if it contains `SKILL.md`,
    /// - **root of skills** if it does not — every immediate child that contains
    ///   `SKILL.md` is loaded; non-skill children are silently ignored. A root
    ///   with zero skill children returns `SkillError::EmptyRoot`.
    pub fn from_paths(
        paths: &[String],
        graph_dir: &Path,
        extra_allowed_dirs: &[PathBuf],
    ) -> Result<Self, SkillError> {
        let mut allowed: Vec<PathBuf> = Vec::new();
        let graph_canonical = graph_dir.canonicalize().map_err(|e| SkillError::Io {
            path: graph_dir.display().to_string(),
            source: e,
        })?;
        allowed.push(graph_canonical.clone());
        for p in extra_allowed_dirs {
            if let Ok(canon) = p.canonicalize() {
                allowed.push(canon);
            }
        }

        let mut skills: HashMap<String, SkillEntry> = HashMap::new();

        for raw_path in paths {
            let path_buf = if Path::new(raw_path).is_absolute() {
                PathBuf::from(raw_path)
            } else {
                graph_dir.join(raw_path)
            };

            let canonical = path_buf.canonicalize().map_err(|e| SkillError::Io {
                path: path_buf.display().to_string(),
                source: e,
            })?;

            // Check it's inside at least one allowed dir.
            let is_allowed = allowed.iter().any(|root| canonical.starts_with(root));
            if !is_allowed {
                return Err(SkillError::PathNotAllowed(canonical.display().to_string()));
            }

            if !canonical.is_dir() {
                return Err(SkillError::NotADirectory(canonical.display().to_string()));
            }

            if canonical.join("SKILL.md").exists() {
                Self::load_skill_dir(&canonical, &mut skills)?;
            } else {
                let mut found = 0usize;
                let read = std::fs::read_dir(&canonical).map_err(|e| SkillError::Io {
                    path: canonical.display().to_string(),
                    source: e,
                })?;
                for entry in read {
                    let entry = entry.map_err(|e| SkillError::Io {
                        path: canonical.display().to_string(),
                        source: e,
                    })?;
                    let sub = entry.path();
                    // Resolve symlinks and confirm the child is still inside an allowed root.
                    // Children that fail to canonicalize, escape the allowed set, or lack a
                    // SKILL.md are silently skipped — only present, in-tree skill directories
                    // count toward `found`.
                    let Ok(sub_canonical) = sub.canonicalize() else {
                        continue;
                    };
                    if !allowed.iter().any(|root| sub_canonical.starts_with(root)) {
                        continue;
                    }
                    if sub_canonical.is_dir() && sub_canonical.join("SKILL.md").exists() {
                        Self::load_skill_dir(&sub_canonical, &mut skills)?;
                        found += 1;
                    }
                }
                if found == 0 {
                    return Err(SkillError::EmptyRoot(canonical.display().to_string()));
                }
            }
        }

        Ok(Self { skills })
    }

    /// Load a single skill from a canonical directory containing `SKILL.md`.
    /// Caller must have already verified that the path is allowed and is a directory.
    fn load_skill_dir(
        canonical: &Path,
        skills: &mut HashMap<String, SkillEntry>,
    ) -> Result<(), SkillError> {
        let skill_md_path = canonical.join("SKILL.md");
        let md_size = std::fs::metadata(&skill_md_path)
            .map_err(|e| SkillError::Io {
                path: skill_md_path.display().to_string(),
                source: e,
            })?
            .len();
        if md_size > MAX_FILE_SIZE_BYTES {
            return Err(SkillError::FileTooLarge {
                path: skill_md_path.display().to_string(),
                size: md_size,
                limit: MAX_FILE_SIZE_BYTES,
            });
        }

        let md_content = std::fs::read_to_string(&skill_md_path).map_err(|e| SkillError::Io {
            path: skill_md_path.display().to_string(),
            source: e,
        })?;
        let parsed = parse_skill_md(&md_content, &skill_md_path.display().to_string())?;

        let dir_name = canonical
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        if parsed.name != dir_name {
            return Err(SkillError::NameMismatch {
                name: parsed.name,
                dir: dir_name,
                path: canonical.display().to_string(),
            });
        }

        for r in &parsed.references {
            let ref_path = canonical.join("references").join(format!("{}.md", r.name));
            if !ref_path.exists() {
                return Err(SkillError::ReferenceFileMissing {
                    skill: parsed.name.clone(),
                    path: ref_path.display().to_string(),
                });
            }
            let ref_size = std::fs::metadata(&ref_path)
                .map_err(|e| SkillError::Io {
                    path: ref_path.display().to_string(),
                    source: e,
                })?
                .len();
            if ref_size > MAX_FILE_SIZE_BYTES {
                return Err(SkillError::FileTooLarge {
                    path: ref_path.display().to_string(),
                    size: ref_size,
                    limit: MAX_FILE_SIZE_BYTES,
                });
            }
        }

        if skills.contains_key(&parsed.name) {
            return Err(SkillError::SkillNameCollision { name: parsed.name });
        }

        skills.insert(
            parsed.name.clone(),
            SkillEntry {
                canonical_dir: canonical.to_path_buf(),
                description: parsed.description,
                references: parsed.references,
                node_type: parsed.node_type,
            },
        );
        Ok(())
    }
}

#[async_trait]
impl SkillRepository for FilesystemSkillRepository {
    fn list_available(&self) -> Vec<SkillCatalogEntry> {
        self.skills
            .iter()
            .map(|(name, entry)| SkillCatalogEntry {
                name: name.clone(),
                description: entry.description.clone(),
                source: SkillSource::Path,
                node_type: entry.node_type.clone(),
            })
            .collect()
    }

    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
        let entry = self
            .skills
            .get(name)
            .ok_or_else(|| SkillError::SkillNotFound(name.to_string()))?;

        let md_path = entry.canonical_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&md_path).map_err(|e| SkillError::Io {
            path: md_path.display().to_string(),
            source: e,
        })?;
        let parsed = parse_skill_md(&content, &md_path.display().to_string())?;

        Ok(Skill {
            name: parsed.name,
            description: parsed.description,
            body: parsed.body,
            references: parsed.references,
            source: SkillSource::Path,
            node_type: parsed.node_type,
        })
    }

    async fn load_reference(
        &self,
        skill_name: &str,
        reference_name: &str,
    ) -> Result<SkillReference, SkillError> {
        let entry = self
            .skills
            .get(skill_name)
            .ok_or_else(|| SkillError::SkillNotFound(skill_name.to_string()))?;

        // Verify reference is declared.
        let declared = entry.references.iter().any(|r| r.name == reference_name);
        if !declared {
            let available = entry
                .references
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(SkillError::ReferenceNotDeclared {
                skill: skill_name.to_string(),
                reference: reference_name.to_string(),
                available,
            });
        }

        let ref_path = entry
            .canonical_dir
            .join("references")
            .join(format!("{}.md", reference_name));
        let body = std::fs::read_to_string(&ref_path).map_err(|e| SkillError::Io {
            path: ref_path.display().to_string(),
            source: e,
        })?;

        Ok(SkillReference {
            skill_name: skill_name.to_string(),
            reference_name: reference_name.to_string(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn valid_skill_md(name: &str) -> String {
        format!(
            "---\nname: {}\ndescription: A test skill\n---\n# {}\nbody\n",
            name, name
        )
    }

    #[tokio::test]
    async fn loads_valid_skill_from_graph_dir() {
        let tmp = TempDir::new().unwrap();
        let graph_dir = tmp.path();
        let skill_dir = graph_dir.join("my-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("my-skill"));

        let repo =
            FilesystemSkillRepository::from_paths(&["./my-skill".to_string()], graph_dir, &[])
                .unwrap();

        assert_eq!(repo.list_available().len(), 1);
        let skill = repo.load_skill("my-skill").await.unwrap();
        assert_eq!(skill.name, "my-skill");
        assert!(skill.body.contains("body"));
        assert!(matches!(skill.source, SkillSource::Path));
    }

    #[tokio::test]
    async fn rejects_path_outside_allowed_dirs() {
        let tmp_graph = TempDir::new().unwrap();
        let tmp_other = TempDir::new().unwrap();
        let skill_dir = tmp_other.path().join("evil-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("evil-skill"));

        let err = FilesystemSkillRepository::from_paths(
            &[skill_dir.display().to_string()],
            tmp_graph.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::PathNotAllowed(_)));
    }

    #[tokio::test]
    async fn accepts_path_in_extra_allowed_dirs() {
        let tmp_graph = TempDir::new().unwrap();
        let tmp_shared = TempDir::new().unwrap();
        let skill_dir = tmp_shared.path().join("shared-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("shared-skill"));

        let repo = FilesystemSkillRepository::from_paths(
            &[skill_dir.display().to_string()],
            tmp_graph.path(),
            &[tmp_shared.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(repo.list_available().len(), 1);
    }

    #[tokio::test]
    async fn rejects_missing_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("empty-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        let err =
            FilesystemSkillRepository::from_paths(&["./empty-skill".to_string()], tmp.path(), &[])
                .unwrap_err();
        // A directory without SKILL.md is treated as a root-of-skills scan target.
        // An empty root (no skill subdirectories) returns EmptyRoot.
        assert!(matches!(err, SkillError::EmptyRoot(_)));
    }

    #[tokio::test]
    async fn rejects_name_mismatch() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("actual-name");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("wrong-name"));

        let err =
            FilesystemSkillRepository::from_paths(&["./actual-name".to_string()], tmp.path(), &[])
                .unwrap_err();
        assert!(matches!(err, SkillError::NameMismatch { .. }));
    }

    #[tokio::test]
    async fn rejects_declared_reference_with_missing_file() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill-with-ref");
        let md = "---\nname: skill-with-ref\ndescription: test\nreferences:\n  - name: missing\n    description: not there\n---\nbody";
        write(&skill_dir.join("SKILL.md"), md);

        let err = FilesystemSkillRepository::from_paths(
            &["./skill-with-ref".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::ReferenceFileMissing { .. }));
    }

    #[tokio::test]
    async fn loads_declared_reference() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill-with-ref");
        let md = "---\nname: skill-with-ref\ndescription: test\nreferences:\n  - name: extra\n    description: more info\n---\nbody";
        write(&skill_dir.join("SKILL.md"), md);
        write(
            &skill_dir.join("references").join("extra.md"),
            "detailed info here",
        );

        let repo = FilesystemSkillRepository::from_paths(
            &["./skill-with-ref".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap();

        let r = repo
            .load_reference("skill-with-ref", "extra")
            .await
            .unwrap();
        assert_eq!(r.body, "detailed info here");
    }

    #[tokio::test]
    async fn rejects_undeclared_reference_at_load() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("my-skill");
        write(&skill_dir.join("SKILL.md"), &valid_skill_md("my-skill"));

        let repo =
            FilesystemSkillRepository::from_paths(&["./my-skill".to_string()], tmp.path(), &[])
                .unwrap();

        let err = repo
            .load_reference("my-skill", "whatever")
            .await
            .unwrap_err();
        assert!(matches!(err, SkillError::ReferenceNotDeclared { .. }));
    }

    #[tokio::test]
    async fn rejects_file_exceeding_size_limit() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("big-skill");
        let huge_body = "x".repeat((MAX_FILE_SIZE_BYTES + 1) as usize);
        let md = format!(
            "---\nname: big-skill\ndescription: test\n---\n{}",
            huge_body
        );
        write(&skill_dir.join("SKILL.md"), &md);

        let err =
            FilesystemSkillRepository::from_paths(&["./big-skill".to_string()], tmp.path(), &[])
                .unwrap_err();
        assert!(matches!(err, SkillError::FileTooLarge { .. }));
    }

    #[tokio::test]
    async fn rejects_path_to_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("not-a-dir.md");
        fs::write(&file_path, "stuff").unwrap();

        let err =
            FilesystemSkillRepository::from_paths(&["./not-a-dir.md".to_string()], tmp.path(), &[])
                .unwrap_err();
        // A path that is a file (not a directory) must return NotADirectory.
        assert!(matches!(err, SkillError::NotADirectory(_)));
    }

    #[tokio::test]
    async fn detects_duplicate_skill_name_across_paths() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        // Both have frontmatter name "duplicate" but different directory names.
        // We need the directory name to match the frontmatter name for validation to pass,
        // so we set up the scenario where user points to the same dir twice under different paths.
        write(&a.join("SKILL.md"), &valid_skill_md("a"));
        write(&b.join("SKILL.md"), &valid_skill_md("a")); // intentionally wrong to trigger NameMismatch

        // NameMismatch fires first — so instead, simulate collision via a symlink in a realistic way:
        // here we simply check that if both pass, collision is caught. For now, the NameMismatch
        // guards that case. Collision is tested in the composite repository tests.
        let err = FilesystemSkillRepository::from_paths(
            &["./a".to_string(), "./b".to_string()],
            tmp.path(),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, SkillError::NameMismatch { .. }));
    }

    #[test]
    fn from_paths_scans_root_directory_with_multiple_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skill_a = root.join("alpha");
        std::fs::create_dir_all(skill_a.join("references")).unwrap();
        std::fs::write(
            skill_a.join("SKILL.md"),
            "---\nname: alpha\ndescription: a desc\n---\nbody A\n",
        )
        .unwrap();
        let skill_b = root.join("beta");
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(
            skill_b.join("SKILL.md"),
            "---\nname: beta\ndescription: b desc\n---\nbody B\n",
        )
        .unwrap();
        // A non-skill child must be ignored, not error.
        std::fs::write(root.join("notes.txt"), "ignored").unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[root.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect("root with two skill children must load");

        let names: std::collections::HashSet<String> =
            repo.list_available().into_iter().map(|e| e.name).collect();
        assert!(names.contains("alpha"));
        assert!(names.contains("beta"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn from_paths_empty_root_returns_empty_root_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("empty");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("notes.txt"), "no skills here").unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &[root.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect_err("empty root should error");

        match err {
            SkillError::EmptyRoot(p) => {
                assert!(p.contains("empty"));
            }
            other => panic!("expected EmptyRoot, got {other:?}"),
        }
    }

    #[test]
    fn from_paths_single_skill_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("solo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: solo\ndescription: d\n---\nbody\n",
        )
        .unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[dir.to_string_lossy().into_owned()],
            tmp.path(),
            &[],
        )
        .expect("single skill directory must still load");
        let names: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["solo".to_string()]);
    }

    #[test]
    fn from_paths_root_and_single_in_same_array() {
        let tmp = tempfile::tempdir().unwrap();
        // root with one skill
        let root = tmp.path().join("baseline");
        let baseline_skill = root.join("ground");
        std::fs::create_dir_all(&baseline_skill).unwrap();
        std::fs::write(
            baseline_skill.join("SKILL.md"),
            "---\nname: ground\ndescription: g\n---\nx\n",
        )
        .unwrap();
        // separate single skill directory
        let single = tmp.path().join("user-skill");
        std::fs::create_dir_all(&single).unwrap();
        std::fs::write(
            single.join("SKILL.md"),
            "---\nname: user-skill\ndescription: u\n---\nx\n",
        )
        .unwrap();

        let repo = FilesystemSkillRepository::from_paths(
            &[
                root.to_string_lossy().into_owned(),
                single.to_string_lossy().into_owned(),
            ],
            tmp.path(),
            &[],
        )
        .expect("mixed array must load");

        let names: std::collections::HashSet<String> =
            repo.list_available().into_iter().map(|e| e.name).collect();
        assert!(names.contains("ground"));
        assert!(names.contains("user-skill"));
        assert_eq!(names.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn from_paths_root_scan_skips_symlinked_child_outside_allowed() {
        // Build the layout:
        //   <root_outside>/leaked/SKILL.md     (NOT inside the allowed scan root)
        //   <scan_root>/legit/SKILL.md         (a normal child, valid)
        //   <scan_root>/escape -> <root_outside>/leaked   (symlink that escapes)
        let outside = tempfile::tempdir().unwrap();
        let leaked = outside.path().join("leaked");
        std::fs::create_dir_all(&leaked).unwrap();
        std::fs::write(
            leaked.join("SKILL.md"),
            "---\nname: leaked\ndescription: x\n---\nbody\n",
        )
        .unwrap();

        let scan = tempfile::tempdir().unwrap();
        let legit = scan.path().join("legit");
        std::fs::create_dir_all(&legit).unwrap();
        std::fs::write(
            legit.join("SKILL.md"),
            "---\nname: legit\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&leaked, scan.path().join("escape")).unwrap();

        // Allowed roots: only the scan dir. The symlink's target resolves outside.
        let repo = FilesystemSkillRepository::from_paths(
            &[scan.path().to_string_lossy().into_owned()],
            scan.path(),
            &[],
        )
        .expect("scan must succeed and silently skip the symlink that escapes");
        let names: Vec<String> = repo.list_available().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["legit".to_string()]);
    }

    #[test]
    fn from_paths_collision_across_root_and_single_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("baseline");
        let baseline = root.join("dup");
        std::fs::create_dir_all(&baseline).unwrap();
        std::fs::write(
            baseline.join("SKILL.md"),
            "---\nname: dup\ndescription: g\n---\nx\n",
        )
        .unwrap();
        let single = tmp.path().join("dup");
        std::fs::create_dir_all(&single).unwrap();
        std::fs::write(
            single.join("SKILL.md"),
            "---\nname: dup\ndescription: u\n---\nx\n",
        )
        .unwrap();

        let err = FilesystemSkillRepository::from_paths(
            &[
                root.to_string_lossy().into_owned(),
                single.to_string_lossy().into_owned(),
            ],
            tmp.path(),
            &[],
        )
        .expect_err("collision across root expansion + single must error");
        match err {
            SkillError::SkillNameCollision { name } => assert_eq!(name, "dup"),
            other => panic!("expected SkillNameCollision, got {other:?}"),
        }
    }
}
