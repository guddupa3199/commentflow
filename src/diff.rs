use similar::{ChangeTag, TextDiff};
use std::path::Path;

pub fn print_unified(original: &str, modified: &str, path: &Path) {
    let diff = TextDiff::from_lines(original, modified);
    let name = path.display().to_string();
    println!("--- {name}");
    println!("+++ {name}");
    for group in diff.grouped_ops(3) {
        // An empty group would mean no hunk to print. "similar" does not emit
        // one, but this runs over arbitrary user files, so skip rather than
        // panic in a tool whose whole job is not to damage the run.
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_start = first.as_tag_tuple().1.start + 1;
        let old_len = last.as_tag_tuple().1.end - first.as_tag_tuple().1.start;
        let new_start = first.as_tag_tuple().2.start + 1;
        let new_len = last.as_tag_tuple().2.end - first.as_tag_tuple().2.start;
        println!("@@ -{old_start},{old_len} +{new_start},{new_len} @@");
        for op in group {
            for change in diff.iter_changes(&op) {
                let sigil = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                print!("{sigil}{change}");
            }
        }
    }
}
