use std::fs;
use std::path::PathBuf;

pub(crate) fn fs_read_benchmark_artifact(path: PathBuf) -> Vec<u8> {
    if !path.exists() {
        panic!(
            "Cannot find one of the benchmark artifacts, re-run the command with `--prepare` first"
        );
    }
    fs::read(path).unwrap()
}
