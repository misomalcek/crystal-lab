use std::net::TcpListener;

/// Ports the host machine commonly occupies. Never pick these even if they
/// happen to be free during development — a collision with a local stack
/// is the class of bug you do not notice because "both work".
pub const RESERVED: &[u16] = &[3000, 5173, 5174, 5432, 6333, 6334, 8080, 8083];

pub fn is_reserved(port: u16) -> bool {
    RESERVED.contains(&port)
}

pub fn free_port() -> Result<u16, String> {
    for _ in 0..48 {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        if !is_reserved(port) {
            return Ok(port);
        }
    }
    Err("no free localhost port".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_hands_out_reserved_ports() {
        for _ in 0..8 {
            let p = free_port().unwrap();
            assert!(!is_reserved(p), "{p}");
            assert_ne!(p, 0);
        }
    }
}
