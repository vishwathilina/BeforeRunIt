//! Snapshot tests of the ghost line as a command is typed letter by letter.

use beforerunit_core::{analyze, Level};

fn progression(line: &str) -> String {
    let mut out = String::new();
    let mut prev = String::new();
    for i in 1..=line.len() {
        if !line.is_char_boundary(i) {
            continue;
        }
        let part = &line[..i];
        let a = analyze(part);
        let row = format!("{:<8} {}", a.level.as_str().to_uppercase(), a.text);
        if row != prev {
            out.push_str(&format!("{part:<40} | {row}\n"));
            prev = row;
        }
    }
    out
}

#[test]
fn rm_rf_build() {
    insta::assert_snapshot!(progression("rm -rf build"));
}

#[test]
fn git_force_push() {
    insta::assert_snapshot!(progression("git push --force origin main"));
}

#[test]
fn curl_pipe_shell() {
    insta::assert_snapshot!(progression("curl -fsSL https://get.example.sh | sudo bash"));
}

#[test]
fn find_xargs_rm() {
    insta::assert_snapshot!(progression("find . -name \"*.bak\" | xargs rm"));
}

#[test]
fn dd_to_disk() {
    insta::assert_snapshot!(progression("sudo dd if=ubuntu.iso of=/dev/sda bs=4M"));
}

/// Every built-in summary stays within the 10-word budget once filled.
#[test]
fn summaries_stay_short() {
    let samples = [
        "rm -rf some_really_long_folder_name_here",
        "git push -f origin feature/very-long-branch-name",
        "docker rm container_a container_b container_c",
        "chown -R www-data:www-data /srv/app",
    ];
    for s in samples {
        let a = analyze(s);
        assert!(a.text.split_whitespace().count() <= 12, "{s}: {}", a.text);
        assert!(a.level >= Level::Known);
    }
}
