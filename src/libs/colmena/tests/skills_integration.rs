//! Integration test: exercise the skills subsystem end-to-end (builtin + filesystem +
//! composite), without touching any real LLM provider.

use colmena::skills::domain::SkillRepository;
use colmena::skills::infrastructure::{
    BuiltinSkillRepository, CompositeSkillRepository, FilesystemSkillRepository,
};
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn builtin_python_expert_loads_via_composite() {
    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> =
        Arc::new(FilesystemSkillRepository::from_paths(&[], &PathBuf::from("."), &[]).unwrap());
    let composite = CompositeSkillRepository::new(builtin, filesystem).unwrap();

    let entries = composite.list_available();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "python-expert");

    let skill = composite.load_skill("python-expert").await.unwrap();
    assert!(skill.body.contains("Typing"));
    assert_eq!(skill.references.len(), 1);
    let reference = composite
        .load_reference("python-expert", "frameworks")
        .await
        .unwrap();
    assert!(reference.body.contains("Django"));
}

#[tokio::test]
async fn mixing_builtin_and_path_skills_works() {
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = tmp.path().join("company-context");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: company-context\ndescription: Internal patterns at our company\n---\nWe use kebab-case for URLs.\n",
    )
    .unwrap();

    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> = Arc::new(
        FilesystemSkillRepository::from_paths(&["./company-context".to_string()], tmp.path(), &[])
            .unwrap(),
    );
    let composite = CompositeSkillRepository::new(builtin, filesystem).unwrap();

    let names: Vec<String> = composite
        .list_available()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"python-expert".to_string()));
    assert!(names.contains(&"company-context".to_string()));

    let company = composite.load_skill("company-context").await.unwrap();
    assert!(company.body.contains("kebab-case"));
}

#[tokio::test]
async fn colliding_names_in_builtin_and_path_fails_at_construction() {
    let tmp = tempfile::TempDir::new().unwrap();
    let skill_dir = tmp.path().join("python-expert");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: python-expert\ndescription: Override attempt\n---\nmy version\n",
    )
    .unwrap();

    let builtin: Arc<dyn SkillRepository> =
        Arc::new(BuiltinSkillRepository::new(&["python-expert".to_string()]).unwrap());
    let filesystem: Arc<dyn SkillRepository> = Arc::new(
        FilesystemSkillRepository::from_paths(&["./python-expert".to_string()], tmp.path(), &[])
            .unwrap(),
    );

    let err = CompositeSkillRepository::new(builtin, filesystem).unwrap_err();
    assert!(matches!(
        err,
        colmena::skills::domain::SkillError::SkillNameCollision { .. }
    ));
}
