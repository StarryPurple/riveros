fn main() {
    // Tell cargo to re-run if the instance ID env var changes
    println!("cargo:rerun-if-env-changed=RINGS_INSTANCE_ID");
}
