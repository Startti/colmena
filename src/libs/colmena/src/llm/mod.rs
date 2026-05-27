pub mod application;
pub mod domain;
pub mod infrastructure;

// `infrastructure::attachments` shares its module name with `domain::attachments`
// (Plan A: infra adapter for the same concept). Both remain reachable via
// their fully-qualified paths; the glob re-exports below would otherwise raise
// `ambiguous_glob_reexports`.
#[allow(ambiguous_glob_reexports)]
pub use application::*;
#[allow(ambiguous_glob_reexports)]
pub use domain::*;
#[allow(ambiguous_glob_reexports)]
pub use infrastructure::*;
