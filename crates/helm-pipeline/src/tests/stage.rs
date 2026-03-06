use crate::stage::*;

#[test]
fn stage_name_variants_are_all_distinct() {
    let names = [
        StageName::Fetch,
        StageName::Decode,
        StageName::Rename,
        StageName::Dispatch,
        StageName::Issue,
        StageName::Execute,
        StageName::Complete,
        StageName::Commit,
    ];
    for i in 0..names.len() {
        for j in 0..names.len() {
            if i == j {
                assert_eq!(names[i], names[j]);
            } else {
                assert_ne!(names[i], names[j]);
            }
        }
    }
}

#[test]
fn stage_name_can_be_pattern_matched() {
    let s = StageName::Fetch;
    let label = match s {
        StageName::Fetch => "fetch",
        StageName::Decode => "decode",
        StageName::Rename => "rename",
        StageName::Dispatch => "dispatch",
        StageName::Issue => "issue",
        StageName::Execute => "execute",
        StageName::Complete => "complete",
        StageName::Commit => "commit",
    };
    assert_eq!(label, "fetch");
}
