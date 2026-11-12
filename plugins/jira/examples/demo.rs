//! Writes the Jira plugin's demo screens as JSON, one file each, for the TaskProxy.com screenshots.
//! Usage: cargo run -p taskproxy-jira --example demo -- <folder>
use std::path::PathBuf;

fn main() {
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Pass the folder to write the screens to."),
    );
    std::fs::create_dir_all(&folder).expect("could not create the folder");
    for (name, node) in taskproxy_jira::demo::screens().expect("a demo screen failed") {
        let file = folder.join(format!("{}.json", name));
        std::fs::write(&file, serde_json::to_string_pretty(&node).unwrap() + "\n")
            .expect("could not write a screen");
        println!("{}", file.display());
    }
}
