use crate::md5::md5;

pub fn verify_md5(payload: &[u8], expected: &[u8; 16]) -> bool {
    &md5(payload) == expected
}

pub fn append_md5_trailer(frame: &mut [u8], payload_range: std::ops::Range<usize>) {
    let payload = &frame[payload_range];
    let digest = md5(payload);
    let trailer_start = frame.len() - 16;
    frame[trailer_start..].copy_from_slice(&digest);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_round_trip() {
        let payload = b"hello world packet payload!".to_vec();
        let mut frame = vec![0u8; payload.len() + 16];
        frame[..payload.len()].copy_from_slice(&payload);
        append_md5_trailer(&mut frame, 0..payload.len());
        let trailer: [u8; 16] = frame[payload.len()..].try_into().unwrap();
        assert!(verify_md5(&payload, &trailer));

        frame[0] ^= 1;
        assert!(!verify_md5(&frame[..payload.len()], &trailer));
    }
}
