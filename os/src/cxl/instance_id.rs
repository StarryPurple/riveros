/// Instance ID set at build time via `RINGS_INSTANCE_ID` env var.
/// 0 = default; 1,2,3 for multi-node.
pub const INSTANCE_ID: usize = {
    // option_env! returns Option<&'static str>
    // We need const-compatible operations only.
    let id: usize;
    // Work around non-const Option methods by matching
    let raw = option_env!("RINGS_INSTANCE_ID");
    // Use a simple approach: match on the whole string option
    match raw {
        None => { id = 0; }
        Some(s) => {
            let b = s.as_bytes();
            if b.len() == 1 && b[0] >= b'0' && b[0] <= b'3' {
                id = (b[0] - b'0') as usize;
            } else {
                id = 0;
            }
        }
    }
    id
};
